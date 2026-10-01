# OHOS 命令执行统一路由：沙箱内解析本地跑，解析不到经 hitshell→hitdaemon 跑

关联：[[warp鸿蒙移植分析]]、[[project_ohos_command_routing]]、hitshell/hitdaemon 桥接

- 日期：2026-10-01
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：`crates/command`（warp 唯一的进程启动入口）、`cmdbridge/hitshell`（新增 `--pipe-exec` 模式）、`crates/entry_ohos`（启用 `local_fs`）、`crates/lsp`（Rust LSP 候选探测改用 `command`）、`rust-toolchain.toml`
- 状态：已落地，装机验证通过（rust-analyzer 经桥接启动、常驻，LSP 请求/应答往返正常）

## 问题描述

OHOS 应用运行在沙箱里：沙箱内既没有系统命令（`node`、`gh`、`rust-analyzer` 等），也不能 exec 沙箱外的程序。而 warp 到处都在 fork/exec 外部程序——LSP 服务、git、ripgrep、外部编辑器、MCP 命令、shell 等，全部经由 `crates/command`（`async_process::Command` / `std::process::Command` 的薄包装）。沙箱内直接 exec 这些程序只会拿到 ENOENT。

设备上已有的跨沙箱通道是 **hitshell→hitdaemon**：hitdaemon 是 public HNP，在沙箱外以系统身份常驻；hitshell 是 private HNP，落在应用自己的 `/data/app/bin`，沙箱内可直接执行；hitshell 通过 loopback SSH（管理口 40230 / 命令池 40220）把命令交给 hitdaemon 执行。但既有 hitshell 只服务交互式 shell 场景（在 pty 上跑 `/usr/bin/zsh` 并把终端接回来），**没有"按请求把一个程序跑起来、把双向标准流接回来"的通用模式**。

需求即用户原话：warp 必须通过 hitshell 跳到 hitdaemon 找并运行 LSP，不走这个通道既找不到、也跑不起来；且希望所有 fork/exec 都统一走一套机制。约束是**不能破坏 warp 的代码架构，不能裁剪功能，不能改其他平台的行为**。

## 问题表现

- **LSP 检测不到 / 启动即退**：rust-analyzer 在沙箱内解析不到；即便找到，执行也失败。
- **任意系统命令 ENOENT**：`node`、`gh` 等经 `crates/command` 直接 exec 一律失败。
- 现象上像是"LSP 功能没实现"，实则卡在"沙箱不能 exec 系统程序"这一根本约束上。

## 问题原因

分三层。

**直接原因——沙箱不能 exec 沙箱外的程序。** 这是 OHOS 应用沙箱的硬约束，不是配置问题。

**架构原因——`crates/command` 的返回类型泄漏，没有后端接缝。** `crates/command` 的公开方法直接返回 `async_process::Child` / `std::process::{ExitStatus, Output}`，这些类型深植于所有调用点的调用链中。若为了换执行后端而引入 trait 或枚举来抽象 `Child`，等于大改 public API，会牵连整个代码库——这正是"破坏架构"的红线。

**能力原因——hitshell 既有模式不通用。** 既有 hitshell 是"开一个交互 shell 并接管终端"，它的输入输出是**面向终端**的（raw mode、窗口大小、resize 轮询）。LSP 需要的是"一个任意程序 + 长时间的双向管道 stdio"，两者不是一回事。

## 解决方案

路线 B：**保持 `crates/command` 的 API 不变**，在 OHOS 上把"沙箱解析不到的程序的启动方式"改写成"启动本地 hitshell 并让它去远端跑"。远端性被藏在一个普通的本地子进程后面，返回类型仍是 `async_process::Child`，调用点无感。

### 1. 路由判据（用户最终裁定）

按**程序能否在沙箱内解析**决定落点，而不是维护一张程序名单：

- 沙箱内解析得到 → 本地直接 exec；
- 沙箱内解析不到 → 经 hitshell→hitdaemon 执行。

"解析得到"的判定（`crates/command/src/ohos.rs::is_local`）：

- 含路径分隔符 → 对 `path` 做 `libc::access(path, X_OK)`；
- 裸名字 → 切分本进程 `PATH`，逐个 `access(X_OK)`；
- 空程序名 → 判为本地，让 spawn 如实报错而不是甩给桥。

这条规则天然覆盖了"必须在本地跑"的程序（hitshell 自身、捆绑 shell、应用自重执行），无需点名任何一个。运行时策略（用户指定）：hitdaemon 通 → 都路由过去；不通 → hitshell 回退到本地执行（`exec_locally`）。

### 2. hitshell 新增 `--pipe-exec` 模式（`cmdbridge/hitshell/src/pipe_exec.rs`）

```
hitshell --pipe-exec <program> [arg...]
```

- `--pipe-exec` 必须是**第一个参数**：被跑程序自己的参数里可能含 `--help` / `--log`，而入口会扫描这两个词；放在最前、且不是任何程序会被以之命名的名字，才能把两者分开。
- 构造 `ExecSpec`：`binary = program`、`args`、`cwd = working_directory()`、`env = forwarded_environment()`；三个 `FdMode` 保持默认 `Piped`（本进程标准流即客户端流，双向都要开）。
- `wait_ready` → `spawn` → 取回 `stdin`/`stdout`/`stderr` → 三路 relay：stdin 用一个独立任务（它在本进程输入关闭时才结束，而那时程序早已退出，直接 await 会等错边），stdout/stderr 用 `smol::future::zip` 并发转发，输出流随程序退出而结束，标志整次运行结束 → `wait_exit_async` 取退出码。
- hitdaemon 侧**无需任何改动**：它的 exec 路径本就为长连接双向管道设计（源码注释点名 clangd），三个 stdio 全部 Piped 并逐一 `forward_stdin`。

### 3. `crates/command` 侧路由（`crates/command/src/ohos.rs` + `async.rs` / `blocking.rs`）

改写后 inner 命令的 program 变成了 hitshell，调用方原想要的 program/args/env 就丢了，因此新增 `Routing` 影子状态承接它们，并在 spawn 前把桥接元数据写进 inner：

- `placement(program)` → `Local` / `Bridged`；
- `invocation(program, placement)` → 要启动的程序前缀参数：`Local` 原样，`Bridged` 变成 `(hitshell_path, ["--pipe-exec", program])`；
- `Routing` 记录 `program` / `args` / `environment`，`arg` / `args` / `env` / `envs` / `env_remove` / `env_clear` 同步维护影子；`get_program` / `get_args`（async）读影子返回调用方视角；
- `spawn` / `status` / `output` 在真正拉起前调用 `apply_to_async` / `apply_to_blocking`，把环境变量名清单（`WARP_HITSHELL_PIPE_ENV`，换行分隔）写进 inner 环境。

因为 `crates/command` 同时有 async 与 blocking 两个包装，两处都要改。

### 4. 环境变量转达

远端程序应看到"调用方显式设置的环境变量"，但**值本身留在本进程环境里**，桥上只传名字：

- 调用方 `.env(k, v)` 时，`k` 的名字进入 `Routing.environment`；spawn 前把名字拼成 `WARP_HITSHELL_PIPE_ENV`（`\n` 分隔，环境变量名不可能含换行）；
- hitshell 按名从自己进程环境读回值，塞进 `ExecSpec.env`；
- `NOT_FORWARDED = ["PATH", "HOME", "TMPDIR", "TMPPREFIX", "TERMINFO"]` 反向排除——它们指向本侧沙箱路径或不含 daemon 程序的搜索路径，转发会指错地方，一律保留 daemon 自己的值。
- **安全性**：键名必须匹配 POSIX 标识符 `[A-Za-z_][A-Za-z0-9_]*`，否则丢弃。原因见下条。

**为什么必须校验键名**：hitdaemon 侧 `command.rs::build_command` 把 `spec.env` 拼进 `sh -c` 行时，**值**做了 `sh_quote`，**键**是裸拼（`format!("{}={}", key, sh_quote(value))`）。一个含 shell 元字符的键名（如 `A;touch /tmp/pwned`）会被 daemon 当作 shell 语法执行。名字非法者本就不是合法环境变量，丢弃即可，也让 daemon 永远看不到危险键。

### 5. `local_fs` 与工具链

- `crates/entry_ohos/Cargo.toml`：`warp` 启用 `local_fs`。LSP 的检测/安装在 `not(local_fs)` 下编译为 false，不开这个 feature，整套 LSP 面就是 no-op。
- `rust-toolchain.toml`：`1.92.0` → `1.97.1`，`components` 只留 `rust-analyzer`。原因：设备上 rust-analyzer 是 rustup 的代理符号链接，它按 **cwd** 读 `rust-toolchain.toml` 决定工具链；仓库 pin 到未安装的 `1.92.0` 时，它一进本工作区就秒退。列表里点名不存在的组件会让 rustup 尝试安装并失败，从而卡住仓库里每条 cargo 命令，因此只保留该工具链实际携带的组件。

### 6. 日志策略

正常路径**不落日志**（入口、会话建立、会话结束三条 `info` 已删除）；仅异常路径保留：hitdaemon 不可达（`error`）、回退本地执行（`info`）、本地也执行不了（`error`）。stdin 中继的写失败是"程序关掉输入"的正常收尾，只在 `debug` 记录，不再静默吞掉。

### 消费路径

```
Command::new(program)
  -> start_async_command / start_blocking_command
    -> placement(program)
       -> Local  : 直接 exec program
       -> Bridged: exec <hitshell> --pipe-exec program [args...]
                   （hitshell 经 hitshell→hitdaemon 在沙箱外跑 program，
                     并把本进程标准流与该程序双向对接）
```

调用方全程只见到一个普通本地子进程，`async_process::Child` 类型不变。

## 修改文件

- `cmdbridge/hitshell/src/pipe_exec.rs` — **新增**：`--pipe-exec` 模式（`run` / `exec_locally` / `forwarded_environment`）。
- `cmdbridge/hitshell/src/main.rs` — 入口最先分发 `--pipe-exec`；`finish` 统一收尾；`HITDAEMON_NOT_READY_MESSAGE` / `READY_TIMEOUT` / `EXIT_WITHOUT_STATUS` / `relay_to_local` / `relay_to_remote` / `working_directory` 放宽可见性；`print_help` 增补用法。
- `crates/command/src/ohos.rs` — **新增**：`Placement` / `Routing` / `placement` / `invocation` / `start_async_command` / `start_blocking_command` / `is_local` / `search_path` / `executable` / `is_environment_name`。
- `crates/command/src/lib.rs` — `#[cfg(target_env = "ohos")] mod ohos;`。
- `crates/command/src/async.rs` — 增 `routing` 字段；`new` / `new_with_session` / `new_with_process_group` 走 `ohos` 构造；`arg` / `args` / `env` / `envs` / `env_remove` / `env_clear` 维护影子；`get_program` / `get_args` 读影子；`spawn` / `status` / `output` 前 `apply_to_async`。
- `crates/command/src/blocking.rs` — 同上的 blocking 版（`apply_to_blocking`）；`get_args` 与 async 一致改为读影子，返回类型由 `CommandArgs<'_>` 改为 `impl Iterator<Item = &OsStr>`。
- `crates/entry_ohos/Cargo.toml` — `warp` 启用 `local_fs`。
- `crates/lsp/src/servers/rust.rs` — Rust LSP 候选探测由 `tokio::process::Command` 改用 `command::r#async::Command`，使探测也走统一路由。
- `rust-toolchain.toml` — `1.92.0` → `1.97.1`，组件只留 `rust-analyzer`。

## 验证

- **探测**：`hitshell --pipe-exec rust-analyzer --help` → 远端跑通，`exited with 0`。
- **常驻会话**：`hitshell --pipe-exec rust-analyzer`（0 参数）→ 会话建立、进程常驻不退出。
- **设备现场**：daemon 侧可见 `rust-analyzer`（父进程为 hitdaemon）及其 `rust-analyzer-proc-macro-srv`。
- **端到端**：LSP 日志出现 `didOpen` → server 的请求/应答往返。
- **工具链**：本仓库内 `rust-analyzer --version` = `rust-analyzer 1.97.1`，`cargo --version` = `cargo 1.97.1`。

## 排查过程中的误判（诚实记录）

- **想给 `crates/command` 加执行后端抽象**：会动到 public API（`Child` 类型泄漏到调用链），属破坏架构，否决；改为"程序改写 + 本地子进程"。
- **以为 hitshell 既有 exec 不支持长连接 stdio**：其实 hitdaemon 的 exec 路径本就三路 Piped、为 clangd 设计，缺的只是 warp 侧的一个通用入口模式。
- **以为 `crates/command` 只有一套包装**：它 async 与 blocking 各一套，两套都要改，否则 blocking 路径漏路由。
- **把 rust-analyzer "秒退"当成桥接没接好**：真因是 `rust-toolchain.toml` pin 到未安装的 `1.92.0`，rustup 代理按 cwd 读到后直接退出。`--help` 探测（cwd 不含该 pin）反而通过，才让人误判为"能检测、不能启动"。
- **`ExecSpec.stdin` 是死字段**：从不被读取，输入实际走 live 的 `RemoteChild.stdin`；不要指望往这个字段塞字节。

## 已知缺口与后续

- **跨进程契约在两处各写一份**：`--pipe-exec` / `WARP_HITSHELL_PIPE_ENV` / 分隔符在 hitshell 与 `crates/command` 各有一份常量，无单一来源，改动须同步。
- **`is_local` 每次 spawn 重新切分 `PATH` 并逐个 `access`**：`PATH` 进程内固定，可缓存；当前为可读性保留无缓存实现。
- **非 UTF-8 路径**：判为"远端"、值经 lossy 转换；正常配置不会产生，未专门处理。
- **提权执行面**：把"沙箱内解析不到的程序"一律路由到 daemon 身份执行，包括用户可改配置里的外部编辑器 `Exec=`、MCP 命令等，等于把这些配置的执行权限提升到 daemon。当前按"用户明确要求统一走该通道"接受，需确认这一面是有意为之。
