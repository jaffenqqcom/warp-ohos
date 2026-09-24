# zsh 交互式命令行回显重复（`pwd` → `pwdpwd`）

关联：[[ohos-debug-lessons]]

- 日期：2026-09-24
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 移植版全部交互式 zsh 会话（Warp 模式与 `WARP_HONOR_PS1=1` 的 PS1 模式**都**受影响）
- 状态：已修复，设备实测通过

## 问题描述

在 OHOS 设备上打开终端，输入任意命令并按回车，**命令行的文本会在屏幕上出现两遍**：输入 `pwd` 回车后该行显示为 `pwdpwd`，输入 `cat .zshrc` 显示为 `cat .zshrccat .zshrc`。命令本身照常执行、输出正常，重复的只是命令行那段文本，不是命令被执行了两遍。

问题与所输入的命令内容无关，也**与 Warp 的提示符模式无关**——换成 PS1 模式（`WARP_HONOR_PS1=1`）同样复现，这一点是本次定位的关键分水岭。

## 问题表现

- 输入任意命令并回车，命令行文本在提交后于同一行显示为两份（`pwd` → `pwdpwd`）。
- 命令的**执行结果正常**（`pwd` 只打印一次工作目录），说明重复发生在"输入行回显"而非"命令执行"。
- PS1 模式（`WARP_HONOR_PS1=1`）与 Warp 模式（默认）**表现完全一致**，两套提示符脚本都复现。
- 会话可继续使用，不卡死、不报错，只是每次提交都留下一行双份文本。
- 该现象在任何命令、任何终端宽度下都稳定复现，属于 100% 必现。

## 问题原因

根因是 **zsh 找不到 terminfo 终端描述数据库**，导致行编辑器（ZLE）的重画退化成"追加"而不是"覆盖"。

### 因果链

1. Warp 在 spawn pty 时给子进程硬设了 `TERM=xterm-256color`（`crates/warp_terminal/src/local_tty/unix.rs:326`，另有 `:868` 一处同样写法），但**从未设置 `TERMINFO`**。修复前全仓 `TERMINFO` 零命中。
2. hnp 提供的 zsh（`zsh.hnp`，`/data/app/bin/zsh` → 实际安装根 `/data/app/zsh.org/zsh_5.9/`）**静态链接了 ncurses**，二进制里带着 `home_terminfo.c` / `_nc_home_terminfo`，只依赖 `libc.so`。
3. 静态链接的含义是：它的 terminfo 搜索路径**只有编译期写死的那一条**——`strings` 该二进制可读出 `/tmp/zshbuild/hnp/zsh/share/terminfo`，那是**构建主机的临时目录**，设备上根本不存在。它**不会**回落到 `/usr/share/terminfo`。
4. zsh 的 ZLE 每次重画命令行文本时的动作是"画一份 → 退格回到起点 → 覆盖重画一份"，这套动作依赖 terminfo 提供的光标定位与清行能力。
5. 拿不到终端描述时，这些能力全部失效，退化为"无光标定位、无清行"，于是第二趟重画**不是覆盖，而是直接追加**到已有文本后面。
6. 结果就是 `pwd` 变成 `pwdpwd`——同一份文本被重画两次且都留在屏幕上。

### 字节层的对照

- 有 terminfo 时，一次提交产生的字节形态是：
  `\x1b[7mpwd\x1b[27m` + `\x08\x08\x08`（三次退格回起点） + `\x1b[27mp\x1b[27mw\x1b[27md`（逐字符覆盖重画）
- 缺 terminfo 时，退格与光标定位段整体消失，输出直接是 `pwdpwd`。

这也解释了为什么命令执行正常：ZLE 只是把同一次输入重画了两遍，提交给 shell 的输入仍然只有一份。

### 本地复现与判定

设备侧无法直接抓 pty 原始字节流，改用本地 zsh 做行为对照，逐步锁定：

- **失败实验**：最初想用 `TERMINFO=<空目录>` 来"屏蔽 terminfo"，但 zsh 会继续回落到 `/etc/terminfo`、`/usr/share/terminfo`，两次输出逐字节相同，实验无效。
- **决定性实验一**：**不定义 `TERM`**（这是唯一能真正剥夺 terminfo 的手段）→ 输出复现 `pwdpwd`，与用户报告的现象逐字节一致。
- **决定性实验二**：把 `TERMINFO` 指向从 `zsh.hnp` 解包出来的 terminfo 目录 → 输出与宿主机正常 zsh 逐字节相同。
- 佐证：`strings <hnp zsh>` 能读出构建机路径 `/tmp/zshbuild/hnp/zsh/share/terminfo`；全仓 grep `TERMINFO` 零命中。

### 调试中走过的死路（供后人省时间）

- **怀疑提示符脚本（`zsh_body.sh`）或 DCS 脚本阶段**：一度按"Warp 模式专属问题"的方向排查提示符构造。**用户反馈 PS1 模式同样复现**，直接排除脚本路径——凡是两套提示符脚本都复现的，问题必然在提示符之外的 shell 运行环境。
- **怀疑 fork / 权限**：排除，同上理由。
- **误判为"SELinux / 沙箱把系统 terminfo 挡在外面"**：这曾经是主因假设。后查明 hnp 的 zsh 静态链 ncurses、**根本不会去查 `/usr/share/terminfo`**，所以"沙箱读不到系统库"只是旁证，不是主因——真正的主因是编译期路径指向构建机。这一点纠正很关键，否则修法会跑偏成"给沙箱加系统库读取权限"。
- **`TERMINFO` 指向空的实验设计错误**：见上，zsh 有回落路径，必须用"未定义 `TERM`"才能真正剥夺。

### 同类隐患的通用排查手法

hnp 原生工具的**编译期路径指向构建机**是这一类问题的通病模式，遇到任何"二进制行为诡异但代码看着没问题"的 hnp 工具，先跑：

```sh
strings <binary> | grep -E '/tmp/|/opt/|/home/|/Users/'
```

已知命中与已知安全：

- **已踩**：`/tmp/zsh`（→ 必须设 `TMPPREFIX`）、`/tmp/zshbuild/hnp/zsh/share/terminfo`（→ 就是本 bug，必须设 `TERMINFO`）。
- **已核实无需处理**：`git` 的 exec-path 走 `RUNTIME_PREFIX` 动态推导；`git-remote-https` 的 CA 路径 `/etc/ssl/certs/cacert.pem` 是设备真实路径。
- **已知残留但无害**：`git-remote-https` 里的 `/opt/deps/lib/engines-3`、`/opt/deps/lib/ossl-modules`，仅在加载硬件加速引擎时才用得到。

## 解决方案

在 OHOS 专属的启动路径里，把子进程的 `TERMINFO` 显式指向 shell **自己旁边**的那份 terminfo 数据库。

修改点在 `crates/warp_ohos/src/launch_app.rs`，新增常量与函数：

```rust
const TERMINFO_ENV: &str = "TERMINFO";
const TERMINFO_SUBDIR: &str = "share/terminfo";

fn point_shell_at_bundled_terminfo() {
    let terminfo_dir = std::fs::canonicalize(TERMINAL_SHELL_PATH)
        .ok()
        .and_then(|shell| {
            shell
                .parent()
                .and_then(std::path::Path::parent)
                .map(|root| root.join(TERMINFO_SUBDIR))
        });
    match terminfo_dir {
        Some(dir) if dir.is_dir() => {
            unsafe {
                std::env::set_var(TERMINFO_ENV, &dir);
            }
            warp_logging::direct_hilog(&format!(
                "prepare_process_environment: {TERMINFO_ENV}={}",
                dir.display()
            ));
        }
        Some(dir) => warp_logging::direct_hilog(&format!(
            "prepare_process_environment: no terminfo database at {}, leaving {TERMINFO_ENV} unset",
            dir.display()
        )),
        None => warp_logging::direct_hilog(&format!(
            "prepare_process_environment: could not resolve {TERMINAL_SHELL_PATH} for terminfo"
        )),
    }
}
```

调用位置在 `prepare_process_environment` 里，紧跟 `append_path_entry(HNP_PRIVATE_BIN_DIR)` 之后。

关键实现要点：

- **用 `canonicalize` 而不是直接拼字符串**：`/data/app/bin/zsh` 是软链，hnp 私有包的真实布局是 `/data/app/<org>/<name>_<version>/`（实测 `zsh.org/zsh_5.9`），只有 `canonicalize` 才能解出真实安装根，再取它的 `share/terminfo`。
- **`is_dir()` 通过才设**：宁可留空也不设一个不存在的路径，避免把 zsh 引向死路径后连 `$TERMINFO` 之外的回落机会都没了。
- **三条出口都有日志**：命中、目录不存在、shell 路径解析失败各自打一条 `direct_hilog`。这段代码跑在 `warp::run()` 安装 logger **之前**，只能用 `direct_hilog` 直达 hilog。

为什么选这个方案：

- 改动完全落在 OHOS 专属 crate（`crates/warp_ohos/`）里，不碰 `crates/warp_terminal/src/local_tty/unix.rs` 这个各平台共用的文件，其它平台行为零变化。
- 不需要重新打包或重建 `zsh.hnp`，也不需要往包外拷文件。
- 幂等：每次启动按当前安装位置重算，hnp 升级换了版本目录也能自动跟上。

被否决的替代方案：

- **把 terminfo 拷到 zsh 编死的那条路径**（`/tmp/zshbuild/hnp/zsh/share/terminfo`）：设备上 `/tmp` 不存在且不可写。
- **改 `unix.rs` 让所有平台统一设 `TERMINFO`**：影响面远超本问题，且会在 macOS/Linux 上覆盖系统 terminfo 的正常解析。
- **重建 `zsh.hnp`，把 terminfo 路径编成设备路径**：需要重做 hnp 构建，成本高且引入不必要的外部依赖。
- **打包时把 terminfo 放到别处再设环境变量**：与"就在 shell 旁边"相比多一次路径约定，没有收益。

### 验证

- `./script/ohos/bundle` 构建通过，签名校验全绿。
- `./install-local.sh` 装包成功。
- 设备 hilog 实测出现 `prepare_process_environment: TERMINFO=/data/app/zsh.org/zsh_5.9/share/terminfo`（同时证实了软链解析出的真实安装根）。
- 设备上输入 `pwd` / `cat .zshrc` 不再出现双份文本。

## 附带验证与遗留

修复 terminfo 后顺带核实了"颜色等能力是否也受影响"，结论是**用户的担心成立，且已被同一次修复一并治好**。

缺失 terminfo 时的实际损伤（本地对照实验所得）：

- `terminfo[colors]` / `setaf` / `setab` / `sgr0` / `cup` / `civis` 全部为空（有 terminfo 时 `colors=[256]`）。
- **数字颜色损坏**：`%F{123}` 输出 `\x1b[3123m`，正确形态应为 `\x1b[38;5;123m`。
- **文本属性全丢**：`%B` / `%U` / `%S` 无任何输出（正常时分别为 `\x1b[1m` / `\x1b[4m` / `\x1b[7m`），粗体、下划线、反显都会失效。
- **不受影响的部分**：`%F{red}` 这类颜色名与裸 ANSI `\x1b[31m` 正常——zsh 对它们走内置 ANSI 映射，不查 terminfo。这也是为什么此前只表现为"文本重复"而没有立刻暴露颜色问题。

遗留项（**尚未定论，与本次修复无关**）：

- hnp zsh 的 `fpath` 同样指向构建机路径（`/devcloud/workspace/j_U6KRACXY/.../share/zsh/5.9/functions`），而 `zsh.hnp` 只打包了 `terminfo`、**没有打包 `functions` 目录**；设备 `/usr/share/zsh/functions` 也只有 3 个文件。
- 实验现象：`zsh: colors: function definition file not found`（`autoload -U colors` 时）。
- Warp 的 bootstrap 脚本（`app/assets/bundled/bootstrap/zsh_body.sh`）并未设置 `fpath`。
- 影响面判断：只会影响依赖 autoload 取 zsh 自带函数库的用法；Warp 提示符自身的颜色构造不依赖它。是否需要修（在 bootstrap 里设 `fpath`，或打包 `functions`）待单独评估。

## 修改文件

- `crates/warp_ohos/src/launch_app.rs` — 新增 `TERMINFO_ENV`、`TERMINFO_SUBDIR` 两个常量与 `point_shell_at_bundled_terminfo()` 函数，并在 `prepare_process_environment` 中调用；解决 zsh 找不到 terminfo 导致的命令行回显重复。

未改动其它任何文件：本修复不涉及 `zsh_body.sh`、`zsh.hnp` 内容、`crates/warp_terminal/**` 或任何非 OHOS 平台代码。
