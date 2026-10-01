# OHOS 关闭终端后，该终端在 hitdaemon 上启动的程序不随之退出

关联：[[ohos-debug-lessons]]、[[2026-09-25-ohos-shell-bootstrap-race]]、[[ohos-debug-lessons#进程树与信号]]

- 日期：2026-10-01
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：`cmdbridge/hitdaemon`（`peers.rs` 的客户端进程树回收、`pty.rs` 的会话登记与收尾）。只影响**交互式终端**（pty shell）路径；`exec` 路径（LSP 等经 `--pipe-exec` 启的命令）不受影响。
- 状态：已修复。cmdbridge 构建零警告、单测 18/18；覆盖装机后关终端验证——shell 与其独立进程组里的作业均随之消失。

## 问题描述

关掉一个 Warp 终端后，**该终端在 hitdaemon 上启动的程序没有跟着退出**，变成孤儿继续运行（典型是用户在那个终端里跑的长跑命令 / 后台作业）。daemon 的进程树回收只覆盖了 shell 自己那个进程组，覆盖不到作业控制给每条命令新建的进程组。

## 问题表现

- 关掉终端后，`ps` 里仍能看到该终端曾启动的命令/作业。
- 设备侧取证（关键）：作业与 shell **同会话、但不同进程组**。修复前实测（`/proc/<pid>/stat` 的第 5、6 字段为 pgrp、session）：
  - 终端 shell `zsh`：pid=11599，`pgrp=11599`，`session=11599` —— 会话首进程。
  - 终端里的作业 `hermes`：pid=12354，`pgrp=12354`，`session=11599` —— **同会话、独立进程组**。
  - 对照（exec 路径）`rust-analyzer`：pid=12117，`pgrp=12117`，`session=1788` —— 会话是 daemon 继承的那个，不是新会话。
- 只在交互式终端里跑的命令上出现；warp 自身经 `--pipe-exec` 启动的 LSP 等不受影响。

## 问题原因

根因是**回收维度用错**：只按"进程组"回收，而交互式 shell 的作业被作业控制放进了**新的进程组**。

### 因果链

1. 一个 Warp 终端 = 一个本地 `hitshell` 进程（交互模式）。daemon 侧 `pty.rs::run_pty_shell` 用 openpty + `/bin/sh -c "<cd ...; exec /usr/bin/zsh ...>"` 起 shell，并在 `pre_exec` 里 `libc::setsid()`（`pty.rs:267`）——shell 因此是**会话首进程**，`pid == pgid == sid`。
2. daemon 只把 shell 的**进程组**登记进 `peers`：`add_group(client_id, shell_pgid)`（`pty.rs:294`）。
3. 客户端（本终端的 `hitshell` 进程）消失时，管理连接断开（`management.rs:80-86` 的 Drop → `management_closed`），触发 `peers::retire`：对登记的组做 `kill(-pgid, SIGTERM)`，宽限 1 秒后 `SIGKILL`（`peers.rs` 的 `signal_now`）。
4. 交互式 zsh 开启作业控制（job control），**用户运行的每条命令 / 管道被放进一个新的进程组**（仍在同一会话内）。`kill(-shell_pgid)` 只打到 shell 自己那个组，够不到这些作业组 → 作业变孤儿存活。
5. 第二个漏点：`pty.rs` 的 `run_pty_shell` 收尾只 `child.kill()`（对直系子进程 SIGKILL），**对孤儿毫无作用**；该路径还会 `drop_group` 删掉记录，连 `retire` 兜底都没了。

### 为什么 exec 路径没这个问题

`exec.rs` 的命令用 `process_group(0)` 起 `sh -c`，但**不 setsid**，所以它们的 session 字段仍是 daemon 继承的会话（实测 `session=1788`），不等于任何会话 id；按会话扫描天然扫不到 → 也不会被误伤。命令本身不会自建进程组，因此按组杀已足够。

```
终端 shell(setsid)  session=11599 ┌─ 进程组 11599  ← kill(-11599) 能打到
                                   ├─ 作业A 组 12xxx ← kill(-11599) 打不到（原 bug）
                                   └─ 作业B 组 12yyy ← kill(-11599) 打不到（原 bug）
```

## 解决方案

关键洞察：改用**会话（session）**维度回收，而非只按进程组。会话首进程死后，其作业**仍带着原 sid**，因此扫 `/proc/<pid>/stat` 第 6 字段即可把整棵会话收齐——这比按 ppid 递归遍历更稳（leader 一死 ppid 链就断），也不需要 cgroup。

### 1. `peers.rs`：会话登记 + 按会话收树

- `struct Peer` 增加 `sessions: BTreeSet<i32>`；新增 `add_session` / `drop_session`（守卫同 `add_group`）。
- 新增纯函数 `parse_stat_groups(&str) -> Option<(i32, i32)>`：`comm`（第 2 字段）可含空格与右括号，必须取**最后一个 `)` 之后**再按空白切分 → `[state, ppid, pgrp, session]`。
- 新增 `session_pgids(targets) -> BTreeSet<i32>`：扫 `/proc`，凡 `session ∈ targets` 的进程，收集其 **pgrp**（按 pgid 杀而非按 pid：同组中扫描窗口期新 fork 的成员也能被 `kill(-pgid)` 覆盖）；跳过非数字名、`pid<=1`、自身 pid，`ENOENT/EACCES` 容错跳过。
- 新增 `tree_groups` / `signal_tree(groups, sessions)`：目标集 = 直接登记的组 ∪ 会话扫描出的组；TERM 一次，宽限后 KILL。
- `retire` / `retire_all` 改用 `signal_tree`。`retire_all` 做一次全量扫描覆盖所有会话（避免逐会话重扫）。
- `signal_now` 增加"跳过 daemon 自身进程组（`getpgrp()`）"的防御。

### 2. `pty.rs`：登记会话、收尾拆会话

- spawn 后**同时**登记 `add_group` 与 `add_session`（两值相等，都是 shell pid）。**不替换** `add_group`——若 `/proc` 被限制不可读，至少仍按组杀掉 shell，不比现状差。
- 三处 `drop_group` 同步补 `drop_session`。
- 循环收尾把裸 `child.kill()` 之后补上 `kill_session(shell_pgid)`，覆盖"shell 正常 exit 但仍残留后台/nohup 作业"与"通道先断、shell 仍在"两种情形。

关键改动（`retire` 前后）：

```rust
// 修前：只按登记的进程组
let groups = { /* remove(client_id) -> peer.groups */ };
signal_groups(groups);

// 修后：按登记的组 + 会话内所有组
let (groups, sessions) = { /* remove(client_id) -> (peer.groups, peer.sessions) */ };
signal_tree(&groups, &sessions);
```

### 3. 一次设计修正（code-review 后）

`signal_tree` 最初在宽限期到点时**重新扫 `/proc`** 再 KILL。这会在 1 秒窗口内把会话 id 重解析一遍——若期间某个新进程复用了刚释放的 pid 并 `setsid`（**新终端的 shell 正是这种形态**），二次扫描会命中它并误杀一个正在使用的新终端。首次扫描已收齐当前全部作业组，二次扫描只多覆盖"宽限期内新开进程组"这一极小概率情形，收益远小于误杀风险。故改为**一次解析、TERM/KILL 复用同一集合**（同时省掉一次全 `/proc` 扫描）。

### 备选方案与取舍

- **在 pty 路径禁 job control**：改交互体验（fg/bg、Ctrl-C 语义受损），且部分程序仍会自建会话。排除。
- **依赖 SIGHUP 传播**：SIGKILL 下 shell 没有退出路径，后台 nohup 作业本就免疫；不可靠。排除作为唯一手段。
- **关闭 pty master 触发 hangup**：内核只给控制终端的**前台进程组**发 SIGHUP，后台组收不到。只覆盖一小部分。
- **subreaper / 按 ppid 递归**：不提供"该杀谁"的集合，且 leader 死后 ppid 链断。更弱。
- **cgroup v2**：最干净，但 OHOS 上守护进程拿不到 cgroup 委派。不可行。

结论：`/proc` 按会话扫描是 OHOS 上最小且可靠的方案。前提是 daemon 身份能读 `/proc/<pid>/stat`——**动手前已实测**：设备上非 root 身份读 `/proc/1/stat`、`/proc/self/stat` 均成功，会话字段可解析。

## 修改文件

- `cmdbridge/hitdaemon/src/peers.rs` — `Peer` 增 `sessions`；新增 `add_session`/`drop_session`/`session_pgids`/`process_groups`/`parse_stat_groups`/`tree_groups`/`signal_tree`/`kill_session`；`retire`/`retire_all` 由"仅组"改为"组 + 会话"；`signal_now` 跳过自身组；内联 `parse_stat_groups` 的 5 个单测。
- `cmdbridge/hitdaemon/src/pty.rs` — `run_pty_shell` 登记 `add_session`（与 `add_group` 并存）；三处收尾补 `drop_session`；循环收尾补 `kill_session(shell_pgid)` 拆整个会话。

## 验证

- 构建：`./script/ohos/cmdbridge --build-only`（OHOS 目标，`CARGO_INCREMENTAL=0` 全量重编）零警告。
- 单测：`cargo test --bin hitdaemon`（OHOS 目标）18 passed，含新增的 5 个 `/proc` stat 解析用例（comm 含空格 / 含括号 / 空名 / 截断）。
- 装机：`./script/ohos/bundle` + `./install-local.sh`（覆盖安装，非卸载）；从系统命令行**重启 hitdaemon**（公共 HNP 的二进制需重启进程才生效，安装不替换运行中的进程）。
- 功能取证：修复前记录到"作业与 shell 同会话、不同进程组"的 `/proc` 字段；关闭该终端后，shell 与其独立进程组里的作业（`hermes`）均已消失，其它终端不受影响。
- 边界（如实记录）：本次未取到 daemon 拆除日志的 hilog 旁证——daemon 的日志经 `logger::attach_file` 落在**客户端 HOME 下的文件**，`hdc shell` 读不到。理论上前台作业也可能因会话首进程死亡的 SIGHUP 而退出，故本次结论以"代码路径 + `/proc` 字段取证 + 结果消失"三者一致为准。

## 同源修复（HiCodeer 的 cmd-agent）

同一份代码在 HiCodeer 仓的 `crates/gpui_ohos/depend/cmd-agent/hicodeerd`（守护进程，对应本仓 `hitdaemon`）里同样存在——修复前 `peers.rs` 与本仓版**逐字节相同**，`pty.rs` 的进程树回收区段也相同，故同一 bug 同源。已把同一修复移植过去：

- `hicodeerd/src/peers.rs` — 以本仓修复版覆盖（覆盖后与本仓逐字节相同）。
- `hicodeerd/src/pty.rs` — 登记会话、三处补 `drop_session`、收尾补 `kill_session`。

验证：`cargo check --release`（OHOS 目标）通过、`cargo test --release --bin hicodeerd` 18 passed。

另顺手补了 hicodeerd `pty.rs` **缺失的 `O_NOCTTY` 防护**（本仓已有、hicodeerd 没有）：打开 pty slave 时若不加 `O_NOCTTY`，当 daemon 以"无控制终端的会话首进程"方式启动（`setsid`/`nohup` 脱离启动），`open` 会把该 pty 抢成 daemon 自己的控制终端——于是子进程 `TIOCSCTTY` 报 EPERM（**终端全起不来**），且 pty 拆除时内核给 daemon 发 SIGHUP（**daemon 被打死、名下会话全没**）。属独立问题，一并补齐。

## 已知取舍与后续

- **pid/sid 复用**：sid 是可能已死的会话首进程 pid；窗口期内若有无关进程复用该 pid 并 setsid 会被误伤（已通过"一次解析、不再二次扫描"收敛窗口）。只对登记过的 sid 操作以缩小面。
- **daemon 被 SIGKILL**：`retire_all` 不执行，孤儿永久残留；新 daemon 无记录，不做启动期盲扫（会误杀）。
- **主动 setsid 的守护进程**：脱离会话，按设计逃逸（属预期）；`nohup cmd &`（不 setsid）仍在会话内，会随终端一起被杀——这正是本次要的行为。
- **可观测性**：`retire` 日志的组计数取自登记表，不含会话扫描额外得到的组，会略低估实际拆除数。
