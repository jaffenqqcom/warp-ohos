# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> ⚠️ **⚠️ 最高优先级规则 ⚠️**
>
> **文件名和路径名都不含 "ohos" 关键字的文件，不可以自行修改，必须询问征求意见！代码里禁止使用魔鬼数字**
>
> **文件名和路径名都不含 "ohos" 关键字的文件，不可以自行修改，必须询问征求意见！代码里禁止使用魔鬼数字**
>
> **文件名和路径名都不含 "ohos" 关键字的文件，不可以自行修改，必须询问征求意见！代码里禁止使用魔鬼数字**
>
> 例外：路径中任意一级包含 "ohos"（如 `ohos_entry.rs`、`app/src/platform/ohos/` 下的文件）可直接修改。ROM 目录（`../winit/`、`../wgpu/`、`../openharmony-ability/`）视为外部依赖，不在本规则范围内。
>
> **禁止执行删除目录的指令。删除目录必须获得明确的授权。** 包括但不限于 `rm -rf`、`cargo clean`、构建脚本的 `--clean` 参数等。

## 快速开始

### 构建（HAP 打包）

```bash
# 完整构建：Rust 交叉编译 + HAP 打包（debug 版，保留调试符号）
./script/ohos/build-vm.sh

# Release 构建（会 strip 符号）
./script/ohos/build-vm.sh --release

# 安装鸿蒙构建依赖
./script/ohos/install_build_deps

**构建流程说明**（`build-vm.sh` 自动完成）：

1. **Rust 交叉编译**：`cargo build --target aarch64-unknown-linux-ohos` → 输出 `libwarp.so` 到 `/tmp/warp-target/aarch64-unknown-linux-ohos/{debug,release}/`
2. **准备 HAP 临时目录**：复制 `app/src/platform/ohos/hap/` 到 `/tmp/hap-$$/`（共享文件系统不支持 CMake/Ninja 写操作）
3. **部署 libwarp.so**：复制到临时目录的 `entry/libs/arm64-v8a/`（HAP 打包用）和 `target/aarch64-unknown-linux-ohos/{debug,release}/`（CMake 链接用）
4. **CMake 编译 libentry.so**：编译 NAPI C++ 源码并链接 `libwarp.so`，生成 `libentry.so`
5. **hvigor 打包 HAP**：`hvigorw assembleHap` 将 `libentry.so` + `libwarp.so` 打包进 HAP
6. **产物复制**：`*.hap` 复制回 `app/src/platform/ohos/hap/entry/build/default/outputs/default/`

> 注意：`./script/ohos/build-vm.sh` 是 openEuler aarch64 虚拟机 版脚本，自动配置 NDK/SDK 路径、将 target 放 `/tmp` 避免共享文件系统硬链接问题。`./script/ohos/build.sh` 是鸿蒙PC（openEuler aarch64 就是上面跑的虚拟机）版，在 VM 上请用 `build-vm.sh`。

### 调试

hdc（HarmonyOS Device Connector，`/usr/bin/hdc`，版本 3.2.0b）用于远程连接和调试鸿蒙设备。

```bash
# 远程连接设备（使用 tconn，非标准 connect）
hdc tconn <IP>:<端口>
hdc tconn 192.168.3.34:44205

# 查看连接状态
hdc list targets -v

# 查看日志（按 tag 和级别过滤）
hdc hilog -t Warp -l I       # 只看 Warp 的 Info 日志
hdc hilog -t Warp -l E       # 只看 Warp 的 Error 日志
hdc hilog -l I | grep "关键词"  # 多关键词过滤
timeout 5 hdc hilog          # 带超时获取（非阻塞）
```

### 测试

```bash
# 运行所有测试（nextest 并行）
cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2

# 运行单个 crate 测试
cargo nextest run -p <crate_name>

# 运行 doc 测试
cargo test --doc

# Warp completer 测试（含 v2 features）
cargo nextest run -p warp_completer --features v2

# 标准 cargo test（单包）
cargo test -p <crate_name>
```

### Lint 与格式化

```bash
# Clippy（全局，-D warnings）
cargo clippy --workspace --all-targets --all-features --tests -- -D warnings

# Clippy（warp_completer 使用默认 features）
cargo clippy -p warp_completer --all-targets --tests -- -D warnings

# WGSL shader 格式化
find . -name "*.wgsl" -exec wgslfmt --check {} +

# C/C++/Obj-C 格式化
./script/run-clang-format.py -r --extensions 'c,h,cpp,m' ./crates/warpui/src/ ./app/src/
```

### 环境与平台设置

```bash
# 平台设置 + 安装 agents skills
./script/bootstrap

# 安装鸿蒙构建依赖
./script/ohos/install_build_deps

# 仅平台设置
./script/bootstrap --skip-common-skills
```

### 运行规则

- 所有 Rust 代码通过 `rustfmt` 格式化，配置见 `.rustfmt.toml`
- Clippy 必须通过 `-D warnings` 级别
- **始终**在开 PR 或推送前运行 `./script/format` 和 `cargo clippy`

## 架构概览

Warp 是一个基于 Rust 的终端仿真器 + 智能开发环境，拥有自定义 UI 框架 **WarpUI**。

### 主二进制入口（鸿蒙版本）

鸿蒙版本采用**两 .so 架构**，运行时同时加载：

**NAPI .so**（`libentry.so`，C/C++）：
- 位于 `app/src/platform/ohos/hap/entry/src/main/cpp/`
- `napi_init.cpp` 的 `Init()` 注册各 NAPI 方法；ArkTS 通过 `import` 加载后直接调用
- `xcomponent_callbacks.cpp` 转发 XComponent NDK 回调到 Rust .so

**Rust .so**（`libwarp.so`，Rust cdylib）：
- 所有 Warp Rust 代码（`crates/warpui/`、`app/` 等）链接为单一 .so
- 导出 C FFI 符号供 NAPI .so 调用
- 入口函数 `hap_start_warp` → `main()` → `run()`，与 OSS 版本走相同代码路径

**ArkTS 入口**：
- `EntryAbility.ets` — 应用生命周期入口，`onWindowStageCreate` 加载 Index.ets
- `Index.ets` — XComponent 渲染表面入口，`.onLoad()` 回调中调用 `startWarpMain` 启动 Warp

**HAP 完整项目目录**：
- `app/src/platform/ohos/hap/` — 鸿蒙 HAP 应用的完整工程根目录（含 `build-profile.json5`、`hvigorfile.ts`、`oh-package.json5` 等）
- `app/src/platform/ohos/hap/entry/` — entry 模块，包含 ArkTS 页面、NAPI C++ 源码和资源文件

**Rust 胶水层**：
- `app/src/platform/ohos/ohos_entry.rs` — 入口函数实现（`hap_start_warp`）
- `app/src/platform/ohos/hap_lifecycle.rs` — HAP 生命周期回调（onForeground/onBackground/onDestroy）

### OHOS 关键架构约束：终端 Shell 与 App 功能的环境分离

**重要约束：** OHOS 的 shell 环境（`sh`）功能有限，无法满足 Warp 对 bash/zsh/fish 的依赖。因此 Warp 的 terminal 功能通过 **SSH 桥接**连接到 PC 端 openEuler VM 上的 bash，所有终端命令实际在 VM 上执行。

**但注意：** Warp 的其他功能（文件列表、git 工具、设置持久化等）看到的是 **OHOS 设备（或 PC 宿主机）的环境**，而非 VM 的环境。存在以下约束：

- **Terminal（终端）**：所有命令在 VM 的 bash 中执行（通过 SSH 桥接 libssh2 + mbedTLS）
- **Warp Drive / 文件浏览**：看到的是 OHOS 应用沙箱的文件系统
- **git 工具**：操作的是 OHOS 设备上的 git 仓库，而非 VM 上的
- **设置持久化**：存储在 OHOS 设备本地（SQLite）

这意味着在 terminal 中 `cd /some/path` 后，Warp 的文件工具不会同步到该路径。两边是独立的环境。

**实现文件**：
- `app/src/platform/ohos/ohos_shell_bridge.rs` — Rust 层 SSH 桥接封装
- `app/src/platform/ohos/hap/entry/src/main/cpp/service_shell_bridge.cpp` — C FFI 入口
- `app/src/platform/ohos/hap/entry/src/main/cpp/shell_bridge_process.cpp` — 子进程主逻辑（SSH 连接 + PTY 转发）

### Crate 组织（Cargo Workspace，60+ crates）

**UI 框架层：**
- `crates/warpui/` — WarpUI 高级 UI 组件和控件
- `crates/warpui_core/` — 核心 UI 框架：Entity-Component-Handle 模式、事件系统、平台抽象
- `crates/warpui_extras/` — 额外 UI 组件

**核心基础设施：**
- `crates/warp_core/` — 核心工具、通道管理、平台抽象
- `crates/warp_features/` — 特性开关系统（`FeatureFlag` 枚举 + 原子状态）
- `crates/warp_logging/` — 日志基础设施
- `crates/warp_util/` — 通用工具函数

**终端相关：**
- `crates/warp_terminal/` — 终端仿真核心（基于 Alacritty）
- `crates/editor/` — 文本编辑功能（warp_editor）
- `crates/vim/` — Vim 模式支持
- `crates/input_classifier/` — 输入分类

**AI 与 MCP：**
- `crates/ai/` — AI 集成
- `crates/mcp/` — MCP 协议实现
- `crates/computer_use/` — Computer Use 工具

**网络与服务：**
- `crates/graphql/` — GraphQL 客户端
- `crates/ipc/` — 进程间通信
- `crates/http_client/` / `crates/http_server/` — HTTP 客户端/服务端
- `crates/websocket/` — WebSocket
- `crates/remote_server/` — 远程服务器

**数据与持久化：**
- `crates/persistence/` — Diesel ORM + SQLite，数据库迁移
- `crates/settings/` / `crates/settings_value/` — 设置系统
- `crates/fuzzy_match/` — 模糊匹配
- `crates/sum_tree/` / `crates/syntax_tree/` — 数据结构与语法树
- `crates/virtual_fs/` — 虚拟文件系统

**其他：**
- `crates/integration/` — 集成测试框架
- `crates/warp_completer/` — 命令补全
- `crates/warp_search_core/` — 搜索核心
- `crates/lsp/` — LSP 支持
- `crates/languages/` — 语言支持
- `crates/markdown_parser/` — Markdown 解析

### 主应用模块（`app/src/`）

主要业务逻辑模块（每个模块通常为独立目录）：

- `terminal/` — 终端仿真、shell 管理、PTY、SSH、会话管理
- `workspace/` / `workspaces/` — 工作区管理
- `ai/` / `ai_assistant/` — AI 集成和 Agent Mode
- `auth/` — 认证和用户管理
- `settings/` / `settings_view/` — 设置 UI
- `drive/` — Warp Drive 同步
- `code_review/` — 代码审查面板
- `notebooks/` — Notebook 功能
- `code/` — 代码编辑器
- `remote_server/` — 远程服务端
- `platform/` — **跨平台适配层**（mac/wasm/windows/ohos）

### 关键架构模式

1. **Entity-Handle 模式**：View 之间通过 `ViewHandle<T>` 引用，非直接所有权；全局 `App` 对象拥有所有实体
2. **跨平台抽象**：`warpui_core::platform` 定义 `OperatingSystem` 枚举；平台特定代码通过 `cfg_if!` 或 `target_env = "ohos"` 选择
3. **特性开关**：`crates/warp_features/` 中的 `FeatureFlag` 枚举 + `AtomicBool` 运行时开关；支持 `DOGFOOD_FLAGS` / `PREVIEW_FLAGS` / `RELEASE_FLAGS` 三级发布通道
4. **通道系统**（Channel）：`crates/warp_core/src/channel/` 定义了 Stable / Preview / Dev / Local / Oss / Integration 通道
5. **插件/Skills 系统**：`.agents/skills/` 目录存放 AI 辅助技能，通过 `skills-lock.json` 版本锁定

## 非显而易见的关键模式

### 终端模型锁

`model.lock()` 必须极其小心使用。多次获取同一模型锁可能导致死锁（macOS 上表现为 Beach Ball）。优先传递已加锁的引用。

### 特性开关优先级

```rust
// 优先运行时检查
if FeatureFlag::YourFlag.is_enabled() { ... }

// 仅编译时必需时使用 cfg
#[cfg(target_os = "macos")]
```

### 穷举匹配

match 表达式中避免使用 `_` 通配符，确保添加枚举变体时编译器提示所有分支。

### 测试文件组织

```rust
#[cfg(test)]
#[path = "filename_tests.rs"]
mod tests;
```

### 外部依赖仓库

鸿蒙移植依赖三个同级目录下的外部仓库，在 `Cargo.toml` 中以 `path = "../"` 引用：

- `../winit/` — 鸿蒙化 fork 的 winit 窗口系统库（v0.30.13），提供 `WindowEvent` 和窗口管理
- `../wgpu/` — 鸿蒙化 fork 的 wgpu 图形库，提供 GPU 渲染管线适配
- `../openharmony-ability/` — 鸿蒙 Ability 生命周期 NAPI 绑定（含 `openharmony-ability` 和 `openharmony-ability-derive` 两个 crate）

### 配置文件

- `.rustfmt.toml` — Rust 格式化配置
- `.clippy.toml` — Clippy lint 配置
- `.cargo/config.toml` — Cargo 配置，包含 OHOS 交叉编译目标配置
- `rust-toolchain.toml` — Rust 工具链版本（通道：1.92.0，组件：rustfmt + clippy）
- `app/Cargo.toml` — 主二进制 crate 的 Cargo.toml，包含 feature gates
- `crates/persistence/migrations/` — SQLite 数据库迁移

---

以下为 HarmonyOS NEXT 移植专项规则，必须严格遵守。

## 1.0 最高优先级规则：文件修改权限

**文件名和路径名都不含 "ohos" 关键字的文件，不允许自行修改，必须询问征求意见后方可进行。**

- 若文件路径中任意一级目录名或文件名包含 "ohos"（如 `ohos_entry.rs`、`ohos_window_manager.rs`、`app/src/platform/ohos/` 下的文件），则允许直接修改
- 若文件路径中没有任何一级包含 "ohos"，则必须先向用户说明**原因、影响范围、修改方案**，获得明确授权后才能修改
- ROM 目录（`../winit/`、`../wgpu/`、`../openharmony-ability/`）视为外部依赖，不在本规则范围内

违反此规则将被视为不可接受的错误。

## 1.1 Rust 编码规范

### 1.1.2 命名规范

- 类型/枚举/struct 使用 PascalCase
- 函数/方法/变量使用 snake_case
- 枚举变体使用 PascalCase
- 常量使用 SCREAMING_SNAKE_CASE
- 宏使用 snake_case

### 1.1.3 导入规范

- 优先使用 `use` 导入而非路径限定符
- 导入语句放在文件顶部
- `#[cfg(...)]` 保护的代码块中可以例外，将导入嵌入作用域或使用绝对路径

### 1.1.4 函数签名规范

- 如果函数接受上下文参数（`AppContext`、`ViewContext`、`ModelContext`），参数名必须为 `ctx` 且放在**最后**
- 唯一的例外：函数接受闭包参数时，闭包放在最后

### 1.1.5 上下文参数命名

```rust
// 正确
fn handle_event(&mut self, event: Event, ctx: &mut ViewContext<Self>) {}

// 错误
fn handle_event(&mut self, ctx: &mut ViewContext<Self>, event: Event) {}
```

### 1.1.6 参数处理

- 完全删除未使用的参数，不要加 `_` 前缀保留。同时更新函数签名和所有调用处

### 1.1.7 格式化宏

- 在 `println!`、`eprintln!`、`format!` 中使用内联格式参数

```rust
// 正确
eprintln!("{message}");
// 错误
eprintln!("{}", message);
```

### 1.1.8 迭代器格式化

- 不要将 `Itertools::format` 的结果直接传给日志宏。使用 `iter.join(", ")` 等生成可复用的 String

### 1.1.9 注释管理

- 不要删除无关变更中的现有注释
- 只有在逻辑变更时才移除或修改注释

### 1.1.10 鸿蒙新增代码规范

- **所有新增的鸿蒙相关代码必须遵循以上所有规范**
- 鸿蒙特定代码放在 `#[cfg(target_env = "ohos")]` 条件编译块中（注：鸿蒙的 target_os = "linux"，通过 target_env = "ohos" 区分）
- 鸿蒙平台模块命名为 `ohos`，遵循 `mac`、`windows`、`wasm` 的命名惯例
- 新增的 NAPI 桥接层代码遵循 Rust FFI 安全最佳实践
- **所有新增鸿蒙特定的特性必须使用 `FeatureFlag` 枚举**，添加到 `crates/warp_features/src/lib.rs`
- **鸿蒙的发布通道**（Channel）配置在 `crates/warp_core/src/channel/` 中，新增 `Channel::Ohos` 变体
- **优先使用运行时检查** `FeatureFlag::YourFlag.is_enabled()`，而不是 `#[cfg(target_env = "ohos")]` 编译时分支
- `#[cfg(target_env = "ohos")]` 仅在代码无法编译（如依赖鸿蒙特有 API）时使用
- 鸿蒙通道的默认特性在 `app/src/features.rs` 中通过 `OHOS_FLAGS` 常量配置

### 1.1.11 日志接口规范

1. **Rust 侧**：统一使用 `log::xxx!()`（如 `log::info!()`、`log::error!()`），**禁止直接调用 HiLog C API**
2. **ArkTS 侧**（HAP 目录）：统一使用 `hilog.xxx()` 从 `@kit.PerformanceAnalysisKit`，**禁止使用 `console.log()`**
3. **格式化参数**：必须使用 `%{public}s` / `%{private}d` 等标准格式，禁止字符串拼接
4. **domain**：固定为 `0x0001`（Warp 应用领域），新增模块需在文档中注册
5. **tag**：使用当前模块的简短英文标识，最长 31 字节

