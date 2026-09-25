# OHOS 终端启动 burst 丢 bootstrap：少数 tab 卡在 Starting zsh

关联：[[ohos-debug-lessons]]、[[2026-09-24-zsh-command-echo-duplicated]]、[[2026-09-25-ohos-enter-tab-escape-no-bytes]]

- 日期：2026-09-25
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：`cmdbridge/hitshell`（终端桥）与 `cmdbridge/hitdaemon`（命令服务）之间的交互式 pty 建链时序；只在"一次开多个终端"的启动 burst 下显形
- 状态：已修复，debug/release 编译与覆盖装机通过；设备侧 3×14 并发回归全绿

## 问题描述

OHOS 版 Warp 一次打开多个终端 tab（典型场景是应用启动时恢复上次的十几个会话）时，**少数 tab 永远停在 "Starting zsh"**：没有提示符、没有报错、也不退出，像被挂住。它不是必然复现——同一批 tab 里绝大多数正常起来，失败占比随时序浮动（实测 14 个 tab 约 3 个失败）。单开一个 tab 几乎从不失败，所以最初很容易被当成偶发。

## 问题表现

- 界面层：失败 tab 停在 "Starting zsh"，此后不动；同一时刻其它 tab 正常出現提示符。
- 远端首块输出：失败 tab 的第一个输出块是 `%` 标记（`ESC[1m ESC[7m%` 加一整行空格，即 zsh 的 `PROMPT_EOL_MARK`），**不是** bootstrap 的回显。**首块是 `%` 等价于"bootstrap 从未进 pty"**——zsh 起在空 pty 上、收不到任何输入时的第一个动作就是打这个标记。
- 应用日志（`/data/app/el2/100/base/com.hiwarp.terminal/haps/entry/files/.local/state/warp-oss/warp-oss.log`）：该 session id 在日志中**只出现 1 次**（成功会话出现 6~19 次），即它的 `InitShell` 应答从未回来；约 7 秒后统一出现 `Bootstrapping failed for shell "unknown"`。
- 设备日志（hilog）：daemon 侧留下 `exec: no live child for stdin channel=<n>` 的 warn（`cmdbridge/hitdaemon/src/exec.rs:103`）。
- 频率：burst 14 个 tab 约 3 个失败；单开一个 tab 几乎必成。与命令内容、与是否加载 shell 集成无关。

## 问题原因

根因是**建链时序竞态**：hitshell 在 daemon 把 pty master 注册进 `MASTERS` 之前就把 bootstrap 字节发进了 channel，这些字节在 daemon 侧找不到目的地，被静默丢弃。

### 三方职责

- **Warp（宿主应用）**：把终端会话的 bootstrap 命令写进终端 stdin，也就是 hitshell 进程的 stdin。它是"第一个写的人"，不等任何回执。
- **hitshell（`cmdbridge/hitshell`，私有 HNP，终端子进程）**：向 daemon 请求 pty + exec，然后把 stdin→channel、channel→stdout 双向转发（`main.rs:439` 的 `bridge`）。
- **hitdaemon（`cmdbridge/hitdaemon`，公共 HNP，沙箱外）**：接受 SSH 请求，为 channel 开一台真 pty 并跑 `zsh`，把 master 登记进 `MASTERS`，之后才把 client 的 `data` 写进 master。

### 因果链

1. `hitshell/src/pty.rs::open_shell_pty` 依次发两个请求，两者都带 `want_reply=true`：`request_pty(true, ...)`（`:113`）与 `exec(true, command)`（`:117`）。
2. **`want_reply=true` 本身不产生等待**。russh 0.55 的 `Channel::exec` / `request_pty` 只是 `send_msg` 把请求发出去就返回（`russh-0.55/src/channels/mod.rs:204`、`:234`），回执只能由调用方自己 `channel.wait()` 去取。修前 `open_shell_pty` 发完就返回，**没有任何一处等回执**。
3. daemon 侧的 `pty_request` 立刻回 Success（`hitdaemon/src/sshd.rs:164`），但它只是把 cols/rows/term 记进 `self.ptys`（`:156`），**并不建 pty**——所以这条 Success 不代表 channel 可用。
4. daemon 侧的 `exec_request` 走 pty 分支：`tokio::spawn(run_pty_shell(...))` 后立即 `return Ok(())`（`hitdaemon/src/sshd.rs:196-213`），**这里不发 Success**。真正的 Success 在 `run_pty_shell` 内部、`MASTERS.insert(...)`（`hitdaemon/src/pty.rs:310`）**之后**才发（`pty.rs:315`）。这个次序本身是对的，问题在于客户端不等它。
5. 于是出现一段窗口：**daemon 已接受 exec、尚未注册 master**。窗口内到达的 `data` 走 `sshd.rs:125` 的 `forward_input`，`master_for` 查不到项（`pty.rs:151`）返回 `false`，落到 `exec::forward_stdin`，那里也没有该 channel 的 exec child（`exec.rs:98-104`），于是打 `exec: no live child for stdin channel=` 并**把字节丢掉**。丢掉的正是 bootstrap。

### 时序

```
hitshell                                        hitdaemon
   |                                                |
   |-- request_pty(want_reply=true) --------------->|  pty_request: 记 cols/rows，channel_success
   |<-------------- Success ------------------------|  (sshd.rs:164)   ← 只代表"收到尺寸"，pty 还没建
   |                                                |
   |-- exec(want_reply=true) ---------------------->|  exec_request: spawn(run_pty_shell)，立即 return
   |                                                |  (sshd.rs:196-213) ← 此处不发 Success
   |            ====== 竞态窗口开始 ======            |      ... spawn 出来的任务正在 openpty/setsid/spawn
   |-- data("bootstrap ...") ---------------------->|  data(): forward_input → MASTERS 无此项 → false
   |                                                |          → forward_stdin → 无 child → 丢弃
   |                                                |  MASTERS.insert(...)        (pty.rs:310)
   |<-------------- Success ------------------------|  channel_success            (pty.rs:315)
   |            ====== 竞态窗口结束 ======            |
   |                                                |  zsh 起在空 pty 上 → 首输出即 PROMPT_EOL_MARK `%`
```

### 为什么是概率性的

窗口宽度 = `run_pty_shell` 里 openpty + setsid + TIOCSCTTY + spawn + `AsyncFd::new` 的耗时，与 hitshell 拿到连接后转发首个 stdin 块的时刻赛跑。单开一个 tab 时，Warp 写 bootstrap 的时机通常落在这个窗口之后，所以几乎必成；burst 下十几个 hitshell 同时抢 CPU 与 daemon 的连接，双方时序都被打散，窗口时不时就被命中（约 3/14）。

## 排查过程中的误判（诚实记录）

- **把失败 tab 首块的 `%` 读成"终端宽度不对"**：`%` 是 zsh 的 `PROMPT_EOL_MARK`，宽度不匹配确实会打它，所以这个方向曾被认真追过一轮。判据修正为：**zsh 5.9 在完全无输入时，第一个输出就是 `%`**；而宽度问题不会让 bootstrap 消失。bootstrap 消失这条更硬的证据（首块不是回显、session id 只出现一次）把结论钉死在竞态上。
- **怀疑 hitshell 的 stdout 行缓冲没 flush**：这是同一批工作里**真实存在**的另一个 bug（relay 循环未 flush，表现为"无回显、像卡死"），已单独修复并保留，但它与本问题无关，本轮不再在它上面绕圈。
- **怀疑 zsh 集成 / precmd 劫持了 bootstrap**：`Bootstrapping failed for shell "unknown"` 是 ~7 秒超时之后的**结果**，不是原因；超时的原因是应答根本没来。
- **怀疑 `hitshell.hnp` / `zsh.hnp` 未随覆盖安装刷新**：实测 daemon 软链与数据目录 mtime 与安装时刻一致，新二进制行为当场生效，排除。
- **怀疑 bootstrap 命令内容有误**（引号、路径、`cd` 目标不存在）：把同一段 bootstrap 手工喂给成功 tab 的 shell 完全正常，排除。

### 走过的死路（供后人省时间）

- 不要从"宽度 / stty / terminfo"方向查"卡在 Starting zsh"：`%` 既可能是宽度也可能是无输入，**先看首块到底是不是 bootstrap 的回显**再分方向。
- 不要因为"单开一个 tab 是好的"就认为建链没问题：这个 bug 只在并发下命中。
- 不要把 `Bootstrapping failed` 当根因，它只是超时兜底。
- `hdc hilog` 里 hitshell/hitdaemon 域缓冲很浅（约几百行，1~2 分钟就被 git chip 的周期探测刷掉），现场必须**边复现边抓**；应用自己的 `warp-oss.log` 反而全量，判 session id 出现次数比翻 hilog 更可靠。

## 解决方案

**根因修复：让 hitshell 在返回"pty 可用"之前，等齐 daemon 对两个请求的回执。**

`cmdbridge/hitshell/src/pty.rs` 的 `open_shell_pty`。修前只发不等：

```rust
// BEFORE: 两个请求都带 want_reply=true，但 russh 的 exec/request_pty 只负责发送，
// 回执要调用方自己 wait()。发完即返回，channel 是否已被 daemon 接受无人确认。
channel.request_pty(true, TERM_TYPE, cols, rows, 0, 0, &[]).await?;
channel.exec(true, command).await?;
```

修后逐个等回执，只接受 `Success`：

```rust
// AFTER: 两个请求都会得到一次回执（pty 一个、exec 一个，顺序即发送顺序）。
// daemon 是在"收下 exec 请求"的过程中才把该 channel 的 pty master 登记进
// MASTERS 的，并且登记完才回这一条 Success，所以在第二条回执到达之前该
// channel 还没有 pty 会话：这个窗口内发出的输入会被路由到一个并不存在的
// exec child 的 stdin 上并丢弃。调用方的第一次写就是终端的 bootstrap 命令，
// 于是抢在回执之前写的那次会话，会停在"命令从未送达"的 shell 上 —— 界面上
// 就是卡在 Starting zsh、没有提示符。
for request in [PTY_REQUEST, EXEC_REQUEST] {
    match channel.wait().await {
        Some(ChannelMsg::Success) => {}
        Some(ChannelMsg::Failure) => { /* log + Err：daemon 拒绝了这次请求 */ }
        Some(other) => { /* log + Err：收到非预期回执 */ }
        None => { /* log + Err：回执到达前 channel 关闭 */ }
    }
}
```

非 `Success` 的三种情形都返回错误，由 `run_interactive` 转成"无法在 hitdaemon 上打开 shell"，进而走既有的 fallback（换成自带 zsh），不再把"半建成的 channel"交给上层。

### 为什么这样修是充分的

- hitshell 的 stdin 是 pty，Warp 写进去的 bootstrap 在 hitshell 读取之前**一直留在内核缓冲里**，不会丢。
- hitshell 读取 stdin 只发生在 `bridge` → `relay_to_remote`（`main.rs:489`）里，而 `bridge` 在 `open_shell_pty` 返回之后才被调用（`main.rs:150-164`）。
- 所以只要把 `open_shell_pty` 的返回推迟到 exec 回执之后，内核里那一段 bootstrap 就会在 `MASTERS` 已登记之后才被读取、转发。修复靠的是"推迟第一次读"，不是靠等一个固定时长。

### 被否决的替代方案

- **在 daemon 侧把窗口内的 `data` 缓存下来、注册完再补写**：要引入"未就绪 channel 的输入队列"这一新语义，且超时、上限、乱序都要定义，改动面和风险都远大于客户端等一个本来就存在的回执。
- **让客户端 `sleep` 一小段再转发**：仍是时序赌博，只是换了个方向；换台机器、换个负载就会复现。
- **daemon 侧提前发 Success 再登记 master**：把窗口从"daemon 未注册"扩大到"客户端以为已注册"，只会更容易丢字节，方向相反。
- **改动 daemon**：不需要。daemon 现在的次序（先 `MASTERS.insert` 再 `channel_success`）正是修复所依赖的前提，一个字未改。

## 验证

- 编译：debug 与 release 两条链（`./script/ohos/cmdbridge --release` → `./script/ohos/bundle --release`）均通过，产出的 HAP 验签成功，`./install-local.sh` 覆盖安装成功（未用 `--reinstall`，沙箱数据保留）。
- 设备侧回归：**3 次 app 启动 × 每次 14 个 bridge，全部拿到 `InitShell`，`Bootstrapping failed` 0 次**；修复前同一 burst 是 3/14 失败。
- 本机直连 daemon 的并发探针（本机即设备，回环共享 40220/40230，无需装机）：按 Warp 的调用方式 fork N 个 pty 并**立刻**写 bootstrap，统计应答是否存在。A/B 实测：**未修复 4/16 有应答，修复后 16/16，另两轮 24/24 全应答**。
- 判据说明：失败 tab 的远端首块是 `%`（`PROMPT_EOL_MARK`）而非 bootstrap 回显；应用日志里 session id 只出现 1 次即 InitShell 未回。

## 修改文件

- `cmdbridge/hitshell/src/pty.rs` — `open_shell_pty` 在两个请求（`request_pty`、`exec`）之后各等一次 `channel.wait()`，只接受 `ChannelMsg::Success`，其余情形记录并返回错误（**本问题的修复主体**）。

daemon 侧本次未修改：`hitdaemon/src/pty.rs` 的 `MASTERS.insert` 先于 `channel_success`、`hitdaemon/src/sshd.rs` 的 pty 分支立刻 return，都是修复所依赖的正确次序，不是缺陷。

同批还有其它 hitshell 改动（无 daemon 时的 fallback shell、`-c` 命令路由到 daemon、终端专用连接池、relay 每块 flush、always-on 的 hilog logger），它们是各自独立的问题，不属于本次根因，未夹带进这个修复。

## 遗留

- `exec: no live child for stdin channel=` 这条 warn 在正常启动 burst 下也可能零星出现（任何早于注册的输入都会命中），目前按"可恢复异常"保留在 warn 级。
- "一次开很多 tab" 仍会同时拉起十几个 hitshell 进程，每个各占一条到 daemon 的连接；这是连接数而非本 bug 的问题，见设备侧连接数核验记录。
