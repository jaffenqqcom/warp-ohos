# warp 鸿蒙移植分析

本文档是 warp 终端（本仓库 `/storage/Users/currentUser/workspace/warp-ohos`）向 HarmonyOS NEXT（OpenHarmony，下文简称 OHOS）移植的前置设计文档。主体按《HMOS移植向导》第 00 章「移植分析方法论」规定的八个分析动作组织（第一至九章），覆盖可行性、编码规范、日志、特性开关、设计文档、代码架构、入口、进程线程八个方面；此外增补了第十章（HAP 复用与编译脚本）、第十一章（平台适配点全清单与 `openharmony-ability` 结合方案）、第十二章（结论与后续步骤）。

## 文档说明

### 修订记录

- **2026-09-25**：新增 11.11 节（`hitshell` + `hitdaemon`：沙箱内终端的权限桥），并同步 11.9、12.2、12.3 三处与之冲突的旧表述；入口 crate 由 `crates/warp_ohos` 改名为 `crates/entry_ohos`（crate 名同步），全文路径引用与 `script/ohos/bundle` 的 `CRATE` 一并更新。
- **2026-09-24**：同步平台能力补齐的最终裁决与实现——第十二章 12.4 记有**全局快捷键**、**剪贴板图片 / HTML**、**app 级窗口最小化 / 唤起**三项已落地（per-window 的最小化 / 最大化 / 全屏仍不做，与 app 级不是同一层）；同步 `openharmony-ability` 的插件改名（`plugin-files` → `plugin-filepicker`、`plugin-openwith` → `plugin-openbysys`、`plugin-filedrop` → `plugin-filedropin`，新增 `plugin-filelaunch`）；修正第十一章中「全局快捷键判降级」「最小化 / 最大化 / 全屏判降级」两处与实现冲突的旧结论。

### 编写依据

- 本仓库当前真实代码状态（所有路径、函数名、行号均经实际检索核对）。
- 《HMOS移植向导》第 00 章方法论与第 01、02、15、16、17、19、23、24 章能力模块。
- 参考工程 HiCodeer（`/storage/Users/currentUser/workspace/HiCodeer`）的 OHOS 移植实证：`hap/` 工程、`script/bundle-ohos` 构建脚本、`crates/gpui_ohos` 平台后端、`openharmony-ability` 基座用法。

### 事实与计划的分野

本仓库此前**没有任何 OHOS 适配代码**。经全仓库检索确认：`script/` 下不存在 ohos 构建脚本，`crates/` 下不存在任何名称含 `ohos` 的 crate，`app/src/platform/` 下不存在 ohos 分支，根目录不存在任何 OHOS 文档。唯一与 OHOS 相关的内容是 `hap/` 目录——它是从 HiCodeer 逐字拷贝过来的 HAP 工程骨架（git 中处于未跟踪状态），内部仍残留 `libhicodeer.so`、`hicodeerd.hnp`、`moduleName='hicodeer'` 等原工程命名。

因此本文档严格区分两类内容：一是**已核实的当前状态**（可对照文件与行号复查）；二是**建议的目标设计**（标注为计划新增，当前尚不存在）。凡本文档引用《HMOS移植向导》中已有的 warp 目标路径（如 `app/src/platform/ohos/ohos_entry.rs`），均属**建议目标**，不是本仓库既存事实。

### 关键结构性结论（先行）

1. warp 的 UI 完全自绘，是**可移植性最好的一类软件**；移植成本集中在窗口后端与系统能力桥接，不在业务逻辑。
2. warp 与 HiCodeer 的 UI 框架不同：HiCodeer 用 gpui，warp 用自研 warpui。因此 `crates/gpui_ohos` 那一层平台后端**不能直接复用**，可复用者是 `openharmony-ability` 基座、`hap/` 工程、`script/bundle-ohos` 脚本骨架与全部适配经验。
3. HAP 侧改动量确实很小（改名与清理为主），移植的**真实工作量在 warpui 的 OHOS 平台后端**。
4. **窗口后端选型**：OHOS 应走**仿 macOS 的非 winit 路线**，以 `openharmony-ability` 作为窗口系统基座。基座已提供事件循环、原生窗口句柄、输入事件、帧回调（按需渲染）、尺寸、避让区、系统配置与生命周期事件，覆盖 warpui 窗口侧的全部需求。注意：`ohos-rs` 另提供 `winit-ohos` 后端（面向 winit 0.31 拆分线，其本身也是 `openharmony-ability` 的封装），**两条路线都能完整使用 `openharmony-ability`**；选型依据是 winit 版本线与改动爆炸半径（warp 现用 0.30.13 单体 fork，迁到 0.31 会波及全平台），而非 ability 是否可用。详见 6.6 节。
5. 最大风险是**条件编译语义冲突**：OHOS 的 `target_os` 是 `linux`、`target_env` 是 `ohos`，而本仓库现有 `target_os = "linux"` 分支共 239 处，它们在 OHOS 上会**默认被激活**，其中相当一部分（尤其是依赖 glibc 与 fork 语义的）在 OHOS 上非法。这需要按第 19 章「跨平台 Rust 适配模式」成对包裹处理。

---

# 第一章 移植可行性分析（分析动作 0）

分析动作 0 要求：确认目标软件是否拥有独立于操作系统的 UI 框架，即 UI 是否完全自绘、不调用任何操作系统的 UI 组件、不需要对 UI 做任何适配。

## 1.1 warp 的 UI 框架归属

warp 使用自研 UI 框架 **warpui**，其分层如下：

- `crates/warpui_core/`：框架核心。包含实体-句柄模型（`Entity` / `ViewHandle` / `AppContext`）、动作系统、场景图、文本布局、字体抽象、平台抽象 trait 定义。
- `crates/warpui/`：框架的高阶部分。包含元素系统、渲染管线、窗口管理、平台后端。
- `crates/warp_tui/`：无头 TUI 前端，复用同一套核心但换成单元格网格渲染。

warpui 的 UI 不是系统控件，而是由 `Element`（Flutter 风格布局描述）构成的场景图，最终经 GPU 自绘。这与 HiCodeer（gpui）属于同一类别：**像素级自绘、不依赖系统 UI 组件**。

判定：**满足 UI 独立性前提**。OHOS 上不需要重建 UI 组件体系，只需让 warpui 拿到一个可用于渲染的原生表面（surface）、一套输入事件、一套字体与一套文本度量能力。

## 1.2 窗口与事件循环依赖

warpui 的窗口层位于 `crates/warpui/src/windowing/`，该目录下目前只有一个后端 `winit`（`crates/warpui/src/windowing/winit/`）。

关键机制在 `crates/warpui/build.rs` 中定义：

```
winit: { not(any(macos, ohos)) },
wgpu: { any(winit, ohos, feature = "experimental-wgpu-renderer") },
```

这段话确立了三条事实：

1. macOS **不使用** winit，而是用自研的 Cocoa 后端（`crates/warpui/src/platform/mac/`）。
2. Linux、FreeBSD、Windows **使用** winit；OHOS 走自研 B 路线，**不使用** winit（移植已在 `build.rs` 的 `winit` 条件中把 `ohos` 与 `macos` 并列排除）。
3. wgpu 渲染在 winit 平台上启用，OHOS 上亦经 `any(winit, ohos, ...)` 启用 GLES 后端。

这说明 warpui 的架构**本就支持替换窗口后端**——macOS 就是活生生的证明。对 OHOS 而言，这提供了一条比 fork winit 更干净的路线：仿照 macOS 模式，新增一个 OHOS 专用窗口后端，而不是把 OHOS 的 platform_impl 塞进 winit。

## 1.3 渲染依赖

warpui 使用 **wgpu** 作为图形抽象层，实现位于 `crates/warpui/src/rendering/wgpu/`，包含 `renderer.rs`（渲染器）、`resources.rs`（资源与 surface 配置）、`shader_types.rs`（着色器类型）。

平台能力枚举 `GraphicsBackend`（`crates/warpui_core/src/platform/mod.rs`）已包含 `Vulkan` 与 `Gl` 两个变体，与第 07 章「Vulkan 优选、GL 保底」的结论直接对应，无需为 OHOS 新增枚举变体。

标注：**需适配**。wgpu 无官方 OHOS 后端，需要为 OHOS 提供 surface 创建路径（OHOS 侧对应 `OH_NativeWindow` 的包装），以及按第 07 章选择 Vulkan 或 GLES 后端。

## 1.4 系统字体依赖

warpui 的字体抽象在 `crates/warpui_core/src/platform/mod.rs` 中以 trait 形式定义（`LoadedSystemFonts`、`TextLayoutSystem`、`FontDB`）。桌面平台依赖 fontconfig 与 FreeType 做字体枚举与字形光栅化。

OHOS 无 fontconfig、无 FreeType。标注：**不可用需绕行**。正确落地方案见 11.8：warp 自带的 cosmic-text 实现（`crates/warpui/src/windowing/winit/fonts.rs`）已覆盖字体枚举、布局与字形光栅化，只需把系统字体加载器换成 OHOS 版本；**不采用** FontParser 枚举 + `libnative_drawing` 光栅化路线（那是旧 warp-ohos 的做法）。

## 1.5 可行性结论

- UI 层：**完全可复用**。warpui 是自绘框架，与系统 UI 组件零耦合。
- 窗口层：**需适配**，但有 macOS 非 winit 后端作为现成范式，工作量可控。
- 渲染层：**需适配**，wgpu 需补 OHOS surface 路径，`GraphicsBackend` 枚举已就绪。
- 字体层：**不可用需绕行**，是已知难点，但 HiCodeer 已在 gpui_ohos 中走通同一路径，经验可移植。
- 业务逻辑（终端仿真、AI、编辑器、Drive 等）：**完全复用**。
- 系统能力（进程创建、剪贴板、通知、文件选择器）：**需绕行或桥接**，属于 OHOS 约束的固有成本。

综合判定：**可行**。这是一次「UI 自绘 + 平台抽象层完备」的移植，改动面集中于平台后端与系统能力桥接，不涉及业务逻辑重写。

关键量化预期：由于 OHOS 的 `target_os` 是 `linux`，本仓库现存的 239 处 `target_os = "linux"` 分支构成最大的隐性回归面，必须逐个甄别「OHOS 上是否真的可用」。这是本项目风险最高的地方，也是全流程最需要纪律约束的地方。

---

# 第二章 编码规范（分析动作 1）

分析动作 1 要求：提取原代码的编码规范，总结成必须遵循的规则清单，新增代码必须与原代码规范一致，不能形成两套风格。

## 2.1 权威来源

warp 的编码规范主要记载在仓库根目录 `AGENTS.md`（该文件同时是项目对 Agent 的指令文件），辅以 `CONTRIBUTING.md`、`.clippy.toml`、`.rustfmt.toml`、`rust-toolchain.toml`。

## 2.2 必须遵循的规则清单

### 2.2.1 语言与格式

- 版次：`edition = "2024"`（见 `app/Cargo.toml`）。
- Rust 工具链：由 `rust-toolchain.toml` 固定。
- 格式化：`./script/format`，`max_width` 为 100。注释换行要填满 100 列，不要提前换行。
- 测试文件命名：`${filename}_tests.rs` 或 `mod_test.rs`，在对应模块末尾以 `#[cfg(test)]` + `#[path = "..."]` 引入。

### 2.2.2 导入与路径

- 避免不必要的类型标注，尤其是闭包参数。
- 避免过多 Rust 路径限定符，用 import 收敛；导入语句统一放文件顶部。
- 例外：`cfg` 守卫分支内可内联导入或用绝对路径一次性引用。

### 2.2.3 函数签名约定

- 若函数接收上下文参数（`AppContext` / `ViewContext` / `ModelContext`），参数必须命名为 `ctx` 且**放在最后**。
- 唯一例外：函数接收闭包参数时，闭包放最后。
- 未使用的参数**整体删除**，不要用下划线前缀保留；同步更新函数签名与全部调用点。

### 2.2.4 宏与格式细节

- `println!` / `eprintln!` / `format!` 优先使用内联格式参数（`eprintln!("{message}")`），满足 Clippy 的 `uninlined_format_args`。
- **禁止**把 `Itertools::format` 的结果直接传给日志宏（`log::*`、`safe_*`）。`Itertools::format` 产出单次使用的格式化器，而日志实现可能多次格式化同一条消息。日志参数应改用可复用的 `String`（如 `iter.join(", ")`）；直接用于 `format!` / `write!` 则可以。

### 2.2.5 注释规范（对移植文档写作同样适用）

- 假设读者是资深工程师；能由自解释命名表达的内容，不写注释解释「是什么」「怎么做」。
- 行内注释只写「为什么」：非显然的业务理由、第三方缺陷的绕行、复杂算法、反直觉写法、意外边界。
- 不写逐行叙述（如 `# Initialize array`）。
- 文档注释保持简洁：公开 API 说明参数、类型、返回值，不叙述内部实现步骤。
- 单一信息源：某成员已有文档注释解释其用途时，引用处不再重复。
- 容器级文档注释描述整体，成员级文档注释描述该成员；两者不互相复述。
- 函数文档注释不列举调用方（不写「被某某调用」「在某某时使用」）。
- **不写「变更说明」类注释**：注释只解释代码的当前状态，不解释你的编辑（如「这里以前是……」）。变更说明属于 PR 描述。
- 不得在无关改动中删除既有注释；仅当注释描述的逻辑已变时才更新或删除。

### 2.2.6 平台与特性开关

- 优先运行时检查（`FeatureFlag::YourFlag.is_enabled()`）而非编译期 `#[cfg(...)]`，以便不重编译即可开关、也便于日后清理。
- 仅当代码在缺少该条件时**无法编译**（平台专属代码、平台专属依赖）才用 `#[cfg(...)]`。
- 新增可切换设置时，同步补上命令面板（Command Palette）的启用/禁用条目与必要的 context flag，使设置不只在 Settings 页可见。

### 2.2.7 匹配穷尽性

- 编写或修改 `match` 语句时，尽量避免通配符 `_`。穷尽匹配有助于在枚举新增变体时发现遗漏。

### 2.2.8 终端模型加锁纪律（高风险）

- 对终端模型（`TerminalModel`）调用 `model.lock()` 时必须极端小心。从不同调用点对同一模型多次加锁会导致死锁与 UI 冻结。
- 新增 `model.lock()` 前，确认当前调用栈上无调用方已持有该锁。
- 优先把已加锁的模型引用向下传递，而不是重新加锁。
- 必须加锁时，锁的作用域尽量短，避免调用可能再次加锁的函数。

## 2.3 移植新增代码的额外约束（本项目硬性规则）

在 warp 原有规范之上，本项目移植还须遵守：

- **OHOS 专属文件**：文件全路径（自仓库根目录起算）中任意一级含 `ohos` 子串者，可自由新增与修改。
- **受保护文件**：全路径不含 `ohos` 子串者，移植改动**只允许新增独立的 `#[cfg(target_env = "ohos")]` 分支**，不得改写既有行、不得改缩进与格式、不得重构、不得删除暂时未被调用的函数。
- **只加不改**：不拆分既有表达式，不在函数参数列表中间插入语句，不为插入代码而重排格式。
- **异常路径必有日志**，正常路径的临时 `info` / `debug` 日志调试完成后删除。
- 不得以屏蔽代码、删减代码、空函数、桩函数、`Result::ok()` 静默吞错等方式解决编译或运行问题。

---

# 第三章 日志系统（分析动作 2）

分析动作 2 要求：分析原代码日志系统的开关形态与输出去向；新增代码必须使用原日志系统不得自建；必须把原日志重定向到 hilog。本项输出三样：日志系统说明、日志开关说明、重定向方法。

## 3.1 日志系统说明

warp 的日志 crate 是 `crates/warp_logging/`，结构如下：

- `crates/warp_logging/src/lib.rs`：定义对外类型与入口。
- `crates/warp_logging/src/native.rs`：桌面/原生实现。
- `crates/warp_logging/src/wasm.rs`：WASM 实现。
- `crates/warp_logging/src/rotation.rs`：日志轮转。

对外类型（`crates/warp_logging/src/lib.rs`）：

- `LogDestination`：`File` 或 `Stderr`，指定输出目的地。
- `LogFrontend`：`Gui` / `Tui` / `Cli`，标识日志会话归属的前端，决定目录与轮转策略。
- `LogConfig`：含 `frontend`、`log_destination`（`Option`，为 `None` 时由环境推断）、`max_file_size_bytes`（会话内大小阈值）。

底层实现用 **`env_logger`**：`crates/warp_logging/src/native.rs` 中的 `init_internal` 构建 `env_logger::builder()`，通过 `env_logger::Target::Pipe` 把输出重定向到文件或 stderr；格式化由 `format_for_terminal_output` 与 `format_for_file_output` 分别负责。

对外入口函数：`init(config: LogConfig)`、`init_for_crash_recovery_process`、`init_logging_for_unit_tests`、`on_crash_recovery_process_killed`、`on_parent_process_crash` 等。

上层调用点：`app/src/lib.rs` 的 `run_internal` 中调用 `warp_logging::init(warp_logging::LogConfig { frontend, log_destination, ..Default::default() })`；（RemoteServerProxy 分支）同样调用 `warp_logging::init`。

## 3.2 日志开关说明

- **级别**：由 `env_logger` 的标准机制控制，即环境变量 `RUST_LOG`（`env_logger::builder()` 默认读取）。
- **目的地**：由 `LogConfig.log_destination` 决定；为 `None` 时由环境推断。
- **前端差异**：由 `LogConfig.frontend`（`LogFrontend`）决定文件目录与轮转策略。
- **工程级开关**：`crates/warp_logging/src/lib.rs` 使用 `#[cfg_attr(...)]` 按平台切换实现文件：

```
#[cfg_attr(not(target_family = "wasm"), path = "native.rs")]
#[cfg_attr(target_family = "wasm", path = "wasm.rs")]
mod imp;
```

这是**本平台适配最关键的机制**。初版设想是再加一个 `#[cfg_attr(target_env = "ohos", path = "ohos.rs")]` 分支替换 `imp`；**实际未走这条路**——`imp` 的导出面有 8 个函数（`init`、`log_directory`、`create_log_bundle_zip`、`rotate_log_files`、`init_for_crash_recovery_process` 等），换 `path` 就得整套重实现。改为在 `native.rs` 内挂子模块、把 `env_logger::Logger` 包一层，详见 3.3。

## 3.3 重定向到 hilog 的方法

按第 06 章「日志系统」与 HiCodeer 的 `crates/zlog/src/ohos.rs` 实证，重定向路径如下。**本节已落地**，与初版设想的差异逐条标注。

1. 在 `crates/warp_logging/src/` 新增 `ohos.rs`（路径含 `ohos`，属 OHOS 专属文件，可自由新增）。
2. 挂载方式**不替换 `imp`**，而是在 `crates/warp_logging/src/native.rs` 内以子模块挂入（`native.rs` 是受保护文件，只加独立行）：
   - 新增 `#[cfg(target_env = "ohos")] #[path = "ohos.rs"] pub(crate) mod ohos;`。
   - 在 `init_internal` 装配完 `env_logger` 后，用 two-line cfg 双行门控把结果交给 hilog。原 `base_logger.init()` 行逐字保留，只在其下加一行 `#[cfg(not(target_env = "ohos"))]`：

```
#[cfg(not(feature = "crash_reporting"))]
#[cfg(not(target_env = "ohos"))]
base_logger.init();

#[cfg(not(feature = "crash_reporting"))]
#[cfg(target_env = "ohos")]
ohos::init_hilog_logger(base_logger.build());
```

   - 选择包装而非替换的理由：包一层 `env_logger::Logger` 让既有级别过滤与文件 sink **原样留用**，只把同一批记录镜像到 hilog，两个 sink 不会漂移；`crash_reporting` 构建被排除，是因为 `sentry_log::SentryLogger` 已占用唯一的全局 logger 槽。
3. `ohos.rs` 中实现 `HilogLogger`（`impl log::Log`），持有内层 `env_logger::Logger`：
   - `enabled` 转调内层；`log` 先 `submit_to_hilog(record)` 再转调内层；`flush` 转调内层。
   - 经 FFI 直调 `OH_LOG_Print`，即 `#[link(name = "hilog_ndk.z")] unsafe extern "C" { fn OH_LOG_Print(...); }`（与 HiCodeer 一致，不引入额外的 binding crate）。
   - 级别映射：DEBUG=3 / INFO=4 / WARN=5 / ERROR=6；`Trace` 并入 `DEBUG`，因为 hilog 没有更细级别。
   - tag 取字面量 `diag`。`log::Log::log` 拿不到 target 字符串，模块路径改折进消息前缀，行格式为 `[module::path] message`。
   - 日志域（domain）用 `0x0001`。
4. 级别策略**沿用既有 `env_logger` 配置**（`filter_level(Info)` + `parse_default_env()` + 对 `naga`/`wgpu_core`/`tantivy`/`wgpu_hal` 的单独压制），未按初版设想下沉为「OHOS debug 用 `Debug` / release 用 `Warn`」：`parse_default_env()` 已让 `RUST_LOG` 在设备上可临时放开，把平台判断硬编进日志 crate 反而是多一层要维护的东西。
5. 全局 logger 安装**之前**的启动期日志走 `warp_logging::direct_hilog(&str)`（`pub use imp::ohos::direct_hilog`）。此刻 `log::` 宏会因 max_level 仍为 `Off` 被直接丢弃，`direct_hilog` 绕过 `log` facade 直投 hilog。`launch_app` 入口与 `prepare_process_environment` 收尾各有一条，用于区分两类故障：只出现 direct 行说明重定向尚未生效（尚未走到 `warp::run()`），两条都不出现则说明 hilog 链接或 FFI 本身有问题。
6. 抓取日志按 tag 过滤（设备系统日志每秒数百行）：

```
timeout 5 hdc hilog 2>&1 | grep "diag"
```

`diag` 是 hilog tag（由主人指定）；`com.hiwarp.terminal` 是 bundleName（进程名），`hdc hilog` 的输出里不带它，按 tag 过滤更快。

## 3.4 日志系统适配的注意事项

- 新增 OHOS 代码必须走 `log::info!` / `log::error!` 等既有宏，不得自建日志通道；不得使用 `println!` 作为日志。**唯一例外**是 `warp_logging::direct_hilog`，它只服务全局 logger 安装之前的启动窗口（`launch_app` 入口、`prepare_process_environment` 收尾）——该窗口内 `log::` 宏必然被丢弃，不直投 hilog 就完全看不见启动过程；窗口一过一律回到 `log::` 宏。
- ArkTS 侧统一用 `hilog`，禁用 `console.log`。
- 正常路径的临时日志在调试完成后必须清理，只保留 `error` / `warn`。
- 崩溃信息的来源边界（第 15 章实测）：OHOS 上桌面 Zed/warp 式的 `crash-handler` + `minidumper` 自建崩溃处理链路不可行，因为 `env::current_exe()` 返回的是 `/system/bin/appspawn` 而非应用自身，且沙箱 `execve` 白名单只允许 `/bin/sh`。崩溃信息只能来自系统 `faultloggerd` 的 cppcrash 文本与应用侧 `log::error!` 钩子。

---

# 第四章 特性开关（分析动作 3）

分析动作 3 要求：分析原代码的特性开放方式，新代码按已有方式进行特性开关，不得另起机制。

## 4.1 warp 的特性开关体系

warp 有两套并行的开关机制，各有明确适用面。

### 4.1.1 编译期 feature（Cargo feature）

- 定义在根 `Cargo.toml` 与各 crate 的 `[features]` 段。例如 `crates/warpui/Cargo.toml` 的 `[features]` 定义了 `enable-metal-frame-capture`、`experimental-wgpu-renderer`、`integration_tests`、`tui` 等。
- 使用方式：`#[cfg(feature = "...")]`。
- 适用：源码级开关，编译后不可变。

### 4.1.2 运行时特性开关（FeatureFlag）

这是 warp 的主力机制，定义在 **`crates/warp_features/src/lib.rs`** 的 `pub enum FeatureFlag`。

- 该枚举当前含大量变体（`Changelog`、`CrashReporting`、`Autoupdate`、`AgentMode`、`ContextChips`、`Ligatures`、`SettingsFile` 等，密集排列于文件前部）。
- 配套常量列表（如 `DOGFOOD_FLAGS`、`PREVIEW_FLAGS`、`RELEASE_FLAGS`、`DEBUG_FLAGS`、`LOCAL_FLAGS`）按构建渠道决定默认开关集合。
- 使用方式：`FeatureFlag::YourFlag.is_enabled()`。
- 渠道装配：`app/src/bin/local.rs` 通过 `ChannelState::new(Channel::Local, config).with_additional_features(features::DEBUG_FLAGS)...` 逐层叠加，并有 `WITH_SANDBOX_TELEMETRY` 环境变量追加 `FeatureFlag::WithSandboxTelemetry` 的实例。
- 运行时可变：`crates/warp_features` 提供 `flag.set_enabled(bool)`，并有运行时开关菜单（`app/src/features.rs` 的 `runtime_flags_menu_items`，以 `FeatureFlag::RuntimeFeatureFlags` 门控显示）。

**重要事实**：经检索确认，`crates/warp_features/src/lib.rs` 中**当前不存在任何 OHOS 相关变体或常量列表**（`Ohos`、`OHOS_FLAGS` 均无命中）。

## 4.2 移植必须遵循的开关规则

1. **优先运行时开关**。凡是「OHOS 上行为不同但代码可编译」的场景，优先用 `FeatureFlag` 而不是 `#[cfg]`。
2. **按既有方式新增**。若需为 OHOS 增加特性开关，应在 `crates/warp_features/src/lib.rs` 新增变体（如 `Ohos` 语义的 flag）与配套常量列表 `OHOS_FLAGS`，接入 `ChannelState` 装配链，不得另造一套机制。
3. **仅编译期无法绕过时才用 `#[cfg]`**。典型场景：平台专属依赖、平台专属代码在缺少该条件下根本无法编译。
4. **降级必须显式且有日志**。功能被禁用时不得静默，须有对应 `warn` / `error` 日志（第 19 章、`appendix-a` 要求）。
5. **平台的编译隔离成对包裹**。凡是要把 Linux / macOS / Windows 的实现挡在 OHOS 之外的点，用成对 cfg：

```
#[cfg(all(any(target_os = "linux", target_os = "freebsd"), not(target_env = "ohos")))]
```

这条纪律对本项目尤其关键，因为 OHOS 的 `target_os = "linux"`，任何未加 `not(target_env = "ohos")` 的 linux 分支都会在 OHOS 上被激活。

---

# 第五章 设计文档（分析动作 4）

分析动作 4 要求：阅读原代码目录里的文档，提取关键信息、注意事项与约束，总结为新增代码必须遵循的规则；遇到「代码行为与文档不符」时，以文档记载的设计意图为准。

## 5.1 仓库文档清单

- `AGENTS.md`（19535 字节）：项目对 Agent 的开发指令，含构建命令、架构总览、开发指南、编码风格偏好、注释规范、终端模型加锁纪律、测试规范、PR 流程、数据库、GraphQL、特性开关、穷尽匹配。
- `CONTRIBUTING.md`（17313 字节）：人类贡献者流程。
- `README.md`、`FAQ.md`、`SECURITY.md`、`CODE_OF_CONDUCT.md`。
- `WARP.md` 不在根目录；目录级文档分布在 `.github/`、各 crate 的 README。
- `.clippy.toml`、`.rustfmt.toml`、`rust-toolchain.toml`：工具链与 lint 约束。

## 5.2 从文档中提取的核心约束

### 5.2.1 双前端架构（必须尊重的既有事实）

warp 有两个前端，共享 `warp_core` / `warpui` 的实体模型核心，但 UI 框架、渲染、输入、验证方式不同：

- **GUI 桌面端**：`app/` crate，基于 warpui 的像素/GPU 框架，含 `Element` / `View` 布局、GPU/WGSL 渲染、鼠标输入、`.app` 包。
- **无头 TUI**：`crates/warp_tui` crate，控制台应用，用并列的单元格网格元素库（`crates/warpui_core/src/elements/tui`，`TuiElement` trait），由 `tui` cargo feature 门控。

移植含义：OHOS 适配面向 **GUI 前端**；TUI 前端不受影响。文档强调「某前端专属的 skill 会在名称或描述中注明」，移植时不应把 GUI 与 TUI 的验证方式混用。

### 5.2.2 实现验证顺序（AGENTS.md 明确规定）

1. 编辑期间只跑最小范围的 `cargo check` 或 `cargo nextest` 获取有用反馈。
2. 代码与自审完成后，跑相关测试并修到通过。
3. 跑相关 Clippy 与其他 lint / 类型检查 / 构建检查并修正。
4. 所有代码改动完成后，跑一次适用的**格式化**（`./script/format`）。
5. 格式化后不再重跑测试与 lint 即可开 PR。

仅当用户、任务或已批准规格明确要求时才跑完整 `./script/presubmit`。

移植含义：OHOS 分支的新增不应触发全量 presubmit；但「其他平台编译通过」这一回归验证是硬要求（第 00 章 C.1）。

### 5.2.3 测试规范

- 用 `cargo nextest` 并行执行；单元测试文件用 `${filename}_tests.rs` 或 `mod_test.rs`，在对应模块末尾以 `#[cfg(test)]` + `#[path]` 引入。
- 集成测试框架在 `crates/integration/`，**仅适用于 GUI**，基于真实显示器。TUI 元素/屏幕由「渲染为行」的单元测试覆盖。
- 移植含义：`crates/integration/` 这套 GUI 集成测试在 OHOS 上不可用（依赖真实显示器与桌面输入），需另设验证手段（`hdc` 输入模拟 + hilog 观测）。

### 5.2.4 特性开关与穷尽匹配

见第四章。文档明确要求：新增可切换设置时补命令面板条目；`match` 尽量不用 `_`。

### 5.2.5 平台相关文档

`AGENTS.md` 的平台段落写明「Native implementations for macOS, Windows, Linux, plus WASM target」——**当前不存在 OHOS 目标**，与代码状态一致。

## 5.3 文档与代码的一致性核对

对本项目的关键核对项：

- 文档称支持平台为 macOS / Windows / Linux / WASM，核对代码 `crates/warpui/build.rs` 的 `winit: { not(any(macos, ohos)) }` 与 `crates/warpui/src/platform/mod.rs` 的 `current` 分发，二者一致：mac 走自研后端，其余（含 OHOS，但 OHOS 实际不走 winit）走 winit，wasm 单独一支。
- 文档称测试文件命名规范为 `${filename}_tests.rs`，核对 `crates/warpui/src/platform/mac/clipboard_tests.rs` 等确为 `_tests.rs` 后缀。

未发现文档与代码冲突之处。

## 5.4 移植新增文档必须遵守的规则（汇总）

1. 新增代码注释全部用英文，注释只写「为什么」。
2. 新增代码遵循 `ctx` 参数放最后的约定，以及导入收敛、内联格式参数等风格。
3. 新增可开关行为优先用 `FeatureFlag` 并接入 `ChannelState` 装配链。
4. 移植验证采用「OHOS 交叉编译 + 设备运行 + 其他平台 `cargo check` 回归」三段式。
5. 不用 `crates/integration/` 这套 GUI 集成测试框架验证 OHOS。

---

# 第六章 代码架构分析（分析动作 5）

分析动作 5 要求回答三个问题：多系统框架是什么样的？对操作系统的适配有没有总入口？对操作系统的适配有没有总的接口要求？并分析 Windows、Linux、macOS 如何适配，OHOS 应在同一框架内如何适配。以下按分析动作 5 的 5.1 至 5.5 五个阶段展开（对应本文档 6.1 至 6.5 节）。

## 6.1 多操作系统框架设计

warp 的多平台框架分两层，各司其职。

### 6.1.1 框架层（warpui / warpui_core）

- 抽象定义在 `crates/warpui_core/src/platform/mod.rs`：以 trait 形式声明平台能力，以枚举形式声明平台标识。
- 平台实现位于 `crates/warpui/src/platform/`，目录与单文件混用：
  - `crates/warpui/src/platform/mac/`：目录形式（含 `app.rs`、`clipboard.rs`、`delegate.rs`、`event.rs`、`fonts.rs`、`geometry.rs`、`keycode.rs`、`menus.rs`、`notification.rs`、`text_layout.rs`、`utils.rs`、`window.rs`、`mod.rs` 等），自研 Cocoa 后端。
  - `crates/warpui/src/platform/linux/`：**单文件** `mod.rs`，薄转发到 winit 后端。
  - `crates/warpui/src/platform/windows/`：单文件 `mod.rs`，薄转发到 winit 后端。
  - `crates/warpui/src/platform/wasm/`：目录形式（`hidden_input.rs`、`soft_keyboard.rs`、`mod.rs`）。
  - `crates/warpui/src/platform/headless/`：目录形式（`app.rs`、`delegate.rs`、`event_loop.rs`、`windowing.rs`、`mod.rs`），供测试使用。
- 窗口后端位于 `crates/warpui/src/windowing/`：
  - `crates/warpui/src/windowing/mod.rs`：`#[cfg(winit)] pub mod winit;` 门控。
  - `crates/warpui/src/windowing/winit/`：含 `app.rs`、`delegate.rs`、`fonts.rs`、`window.rs`、`mod.rs`、`wasm.rs`、`text_layout_tests.rs`。
- 渲染后端位于 `crates/warpui/src/rendering/`：`wgpu/`（`renderer.rs`、`resources.rs`、`shader_types.rs`、`texture_with_bind_group.rs`）、`atlas/`、`glyph_cache.rs`。

### 6.1.2 应用层（app）

- 入口与平台组装目录为 `app/src/platform/`，当前含：
  - `app/src/platform/mod.rs`：声明 `mac`、`wasm`、`windows` 三个模块，并提供 `pub fn init()`。
  - `app/src/platform/mac.rs`、`app/src/platform/mac/`（目录与同名文件并存）。
  - `app/src/platform/windows.rs`、`app/src/platform/wasm.rs`。
  - **注意：`app/src/platform/` 下没有 `linux.rs`**（linux 不需要 app 层平台模块，直接用 warpui 的 winit 后端）。
- `app/src/lib.rs`：应用主逻辑与 `run()` / `run_internal()` 入口。
- `app/src/bin/`：各渠道二进制入口（`local.rs`、`stable.rs`、`preview.rs`、`dev.rs`、`oss.rs`、`integration.rs`）。

### 6.1.3 平台标识枚举

`crates/warpui_core/src/platform/mod.rs` 定义 `pub enum OperatingSystem`，当前变体为 `Linux`、`Mac`、`Windows`、`Other(Option<&'static str>)`。**当前不含 `Ohos` 变体**。

`OperatingSystem::get()` 用 `cfg_if!` 分发：

- `target_family = "wasm"` → `wasm::current_platform()`
- `target_os = "linux"` 或 `target_os = "freebsd"` → `OperatingSystem::Linux`
- `target_os = "macos"` → `OperatingSystem::Mac`
- `windows` → `OperatingSystem::Windows`
- 其它 → `OperatingSystem::Other(None)`

并提供 `is_mac()` / `is_linux()` / `is_windows()` 判断方法与 `default_shell_family()`（linux/mac/其它 → `ShellFamily::Posix`，windows → `ShellFamily::PowerShell`）。

**关键风险**：因 OHOS 的 `target_os = "linux"`，若不新增分支，`OperatingSystem::get()` 在 OHOS 上会返回 `OperatingSystem::Linux`。任何 `OperatingSystem::get().is_linux()` 的运行时分叉都会把 OHOS 当 Linux 处理。`default_shell_family()` 返回 `Posix` 这一点在 OHOS 上恰好正确（设备 shell 是 `/bin/sh`），但其它 linux 分叉需要逐个甄别。

## 6.2 适配总入口

warp 的适配总入口是分层的两个：

### 6.2.1 warpui 层总入口：`crates/warpui/src/platform/mod.rs`

该文件的 `current` 子模块是框架层的总分发点：

```
pub mod current {
    cfg_if::cfg_if! {
        if #[cfg(target_family = "wasm")] {
            pub use super::wasm::*;
        } else if #[cfg(any(target_os = "linux", target_os = "freebsd"))] {
            pub use super::linux::*;
        } else if #[cfg(target_os = "macos")] {
            pub use super::mac::*;
        } else if #[cfg(target_os = "windows")] {
            pub use super::windows::*;
        } else {
            pub use warpui_core::platform::test::*;
        }
    }
}
```

同文件还定义 `create_system_clipboard()`，按平台返回剪贴板实现，是另一个平台分派点。

### 6.2.2 app 层总入口：`app/src/lib.rs` 的 `run_internal`

应用的平台定制入口在 `app/src/lib.rs` 的 `run_internal`。它构建 `AppBuilder` 后，按平台用扩展 trait 定制：

- macOS：使用 `warpui::platform::mac::AppExt`（设置 dock 图标、菜单栏、dock 菜单等）。
- Linux/FreeBSD：使用 `warpui::platform::linux::AppBuilderExt`（`set_window_class`、`force_x11`）。
- Windows：使用 `warpui::platform::windows::AppBuilderExt`（`set_app_user_model_id`、DXC 着色器编译）。

事件循环由 `app/src/lib.rs` 的 `app_builder.run(move |ctx| { ... })` 启动。

**OHOS 的适配总入口落点**：在 `crates/warpui/src/platform/mod.rs` 的 `current` 中新增 OHOS 分支，在 `create_system_clipboard()` 中新增 OHOS 分支，在 `app/src/lib.rs` 的 `run_internal` 中新增 `#[cfg(target_env = "ohos")]` 定制分支。这三处都是受保护文件，改动方式为「只新增独立分支」。

## 6.3 适配总接口定义

对操作系统的适配总接口定义在 `crates/warpui_core/src/platform/mod.rs`，以 trait 形式给出。核心 trait 清单如下：

- `Delegate`：平台应用委托，处理应用级事件（生命周期、打开 URL、终止等）。
- `DispatchDelegate`：事件派发相关的委托接口。
- `LoadedSystemFonts`：已加载的系统字体集合，供字体系统查询。
- `TextLayoutSystem`：文本布局系统接口（度量、断行、整形）。
- `FontDB`：字体数据库接口（枚举、匹配、加载）。
- `Window`：窗口能力接口。
- `WindowContext`：窗口上下文接口。
- `WindowManager`：窗口管理器接口。

配套数据类型/枚举：

- `WindowBackdrop`、`NotificationInfo`、`LineStyle`、`WindowOptions`、`WindowStyle`、`WindowBounds`、`MicrophoneAccessState`、`TerminationMode`、`FullscreenState`、`CapturedFrameFormat`、`CapturedFrame`、`WindowFocusBehavior`、`SystemTheme`、`Cursor`、`OperatingSystem`、`GraphicsBackend`。

抽象覆盖的能力面：窗口创建与配置、窗口管理、事件派发、生命周期、剪贴板、字体（枚举/加载/回退）、文本布局、光标样式、全屏状态、屏幕捕获、通知、麦克风授权、系统主题。

**这是 OHOS 后端必须完整兑现的契约集合**，也是判断「OHOS 后端要写多少代码」的直接依据。

此外，`crates/warpui/src/platform/app.rs` 定义 `AppBuilder` 与 `AppBackend`，提供 `new`、`new_windowless`、`run` 等方法；各平台通过扩展 trait 追加配置能力（如 `linux::AppBuilderExt`、`windows::AppBuilderExt`、`mac::AppExt`）。

## 6.4 编译隔离与运行分支

### 6.4.1 条件编译分布（实测统计）

全仓 `app/` 与 `crates/` 的 `.rs` 文件里，`target_os = "..."` 条件编译大致分布为 `macos` 最多、`linux` 次之、`freebsd` 再次、`windows` 最少。其中 `target_os = "linux"` 分支数量最大，而 OHOS 的 `target_os` 恰好也是 `linux`，因此这一组是 OHOS 上最危险的隐性回归面，必须逐个甄别「OHOS 上是否真的可用」。

其它统计（仅作量级参考，不随代码逐行更新）：

- 含 `target_env` 的文件：原始分析阶段仅 2 个，且均为判断 `target_env = "gnu"`（与 OHOS 无关），当时印证**本仓库未为 OHOS 做任何条件编译准备**；但 OHOS 移植已在 `crates/` 下大量文件新增 `target_env = "ohos"` 守卫，该前提在移植后已不成立。
- 含 `feature = "local_tty"` 的文件较多（终端子进程相关，是进程创建约束的重灾区）。
- 含 `cfg(unix)` 的文件也较多。

### 6.4.2 每种隔离代码的功能归类

- `target_os = "macos"`：菜单栏、Dock、Cocoa 窗口、Objective-C 桥、keychain、Metal、plist、Sentry 的 macOS 后端等。
- `target_os = "linux"` / `"freebsd"`：X11/Wayland 窗口、`WM_CLASS`、单实例转发、XDG 目录、`inotify` 文件监视、桌面通知、包管理检测等。**这一组是 OHOS 上最危险的一组**，因为 OHOS 满足 `target_os = "linux"`。
- `target_os = "windows"`：`AppUserModelID`、DXC、Job Object、控制台附着、注册表等。

### 6.4.3 运行时操作系统分支

典型形态是 `OperatingSystem::get().is_mac()` / `.is_linux()` / `.is_windows()`。举例（均为真实命中）：

- `crates/warpui_core/src/keymap.rs`：`if OperatingSystem::get() == OperatingSystem::Mac`，用于 mac 专属按键处理。
- `crates/warpui_core/src/keymap.rs`：判断是否为 `Linux | Windows`。
- `crates/warpui_core/src/integration/step.rs`、`crates/warpui_core/src/keymap.rs` 等多处：`OperatingSystem::get().is_mac()` 决定按键语义。

**风险**：这些运行时判断在 OHOS 上若返回 `Linux`，会把 OHOS 当 Linux 处理按键语义。新增 `OperatingSystem::Ohos` 变体后，需要重新审视所有 `is_linux()` 分支，明确 OHOS 应归入哪一侧（多数场景 OHOS 更接近 Linux 的按键行为，但必须逐个判定，不能整批默认）。

### 6.4.4 移植的隔离纪律

按第 19 章与附录 A：

- 所有希望在 OHOS 上**排除**的 linux/freebsd 分支，必须写成 `#[cfg(all(any(target_os = "linux", target_os = "freebsd"), not(target_env = "ohos")))]`。仅写 `target_os = "linux"` 会在 OHOS 上误命中。
- 新增的 OHOS 分支统一用 `#[cfg(target_env = "ohos")]`。
- 受保护文件内只新增独立分支，不改既有行。

## 6.5 各平台适配对照

### 6.5.1 各平台是否严格按总接口适配

- **macOS**：目录 `crates/warpui/src/platform/mac/` 是**最完整**的实现，逐项实现 `Delegate`、`Window`、`WindowManager`、`FontDB`、`TextLayoutSystem`、`LoadedSystemFonts` 等，并大量超出总接口（自研 Cocoa 事件模型、菜单栏、Dock、Objective-C 桥）。
- **Linux / FreeBSD**：`crates/warpui/src/platform/linux/mod.rs` 代码量很小，绝大部分能力**转发给 winit 后端**（`pub use crate::windowing::winit::app::App;`），自身只补 `AppBuilderExt`（`set_window_class`、`force_x11`）、`user_windowing_system()`、`is_wsl()`、`is_wayland_env_var_set()` 等少量平台特有逻辑。
- **Windows**：`crates/warpui/src/platform/windows/mod.rs` 同样是薄转发 winit + 少量平台特有（`AppUserModelID`、DXC）。

### 6.5.2 超出总接口的特殊适配（移植最需警惕的侵入点）

这些「超出总接口」的自定义部分是移植时最需要小心的地方：

- **macOS**：`crates/warpui/src/platform/mac/` 下的 `menus.rs`（菜单栏）、`notification.rs`（通知）、`clipboard.rs`（剪贴板）、`event.rs`（事件模型）、`fonts.rs` + `text_layout.rs`（字体与文本布局，走系统 CoreText）全部是超出总接口的 macOS 专属实现。
- **Linux/FreeBSD**：`crates/warpui/src/platform/linux/mod.rs` 中的 X11/Wayland 判定、`WM_CLASS`、`force_x11` 均为平台特有；`crates/warpui/src/windowing/winit/linux/` 下的 `LinuxClipboard`（被 `crates/warpui/src/platform/mod.rs` 的 `create_system_clipboard()` 引用）是平台专有剪贴板实现。
- **Windows**：`crates/warpui/src/platform/windows/mod.rs` 的 DXC 配置；`app/src/lib.rs` 的 `command::windows::init()`（Job Object）；`app/src/lib.rs` 的 `dynamic_libraries::configure_library_loading()`；`app/src/lib.rs` 的 `platform::windows::check_redirection_guard()`。
- **共性侵入点**：`app/src/lib.rs` 的 `run_internal` 是各平台分支最集中的受保护文件，OHOS 分支也将落在这里。

### 6.5.3 OHOS 在同一框架内的适配方案

综合 6.1 至 6.5，OHOS 适配建议采用**仿 macOS 模式**，理由如下：

- macOS 已证明 warpui 支持非 winit 的自研窗口后端（`crates/warpui/build.rs` 的 `winit: { not(any(macos, ohos)) }`）。
- 若照 Linux 模式走 winit，则涉及 winit 版本线迁移或 fork 维护：winit 的 OHOS 后端由 `ohos-rs` 的 `winit-ohos` 提供，但它面向 winit 0.31 拆分线（`winit-core`），与 warp 现用的 0.30.13 单体 fork 不同线，直接引入即要求全平台升级。两条路线的完整论证见 6.6 节。
- 因此选择「新增 OHOS 专属窗口后端 + 平台后端」，与 macOS 同构。

具体落点（**已按本节目标落地，逐项标注状态；实测结论见 11.10.6**）：

1. **窗口后端**（已落地，但落点与原计划不同）：原计划新增 `crates/warpui/src/windowing/ohos/`；实际**未新增该目录**——OHOS 的窗口后端与平台后端同处 `crates/warpui/src/platform/ohos/`，由 `windowing.rs` 实现 `platform::WindowManager` 与 `platform::Window`，窗口句柄直接取 `openharmony-ability` 的 `RawWindow`，不需要直接对接 OH_NativeWindow 的 NDK 调用。`crates/warpui/build.rs` 的门控已按计划调整：新增 `ohos: { target_env = "ohos" }`，`winit` 改为 `not(any(macos, ohos))`，`wgpu` 改为 `any(winit, ohos, feature = "experimental-wgpu-renderer")`。
2. **平台后端**（已落地）：`crates/warpui/src/platform/ohos/` 已实现 `Delegate`、`Window`、`WindowManager`、`WindowContext`、`FontDB`（复用 cosmic-text 实现）与剪贴板；事件模型与按键映射在 `event_loop.rs`、`keycodes.rs`。IME 与鼠标/触摸仍为未接通道（运行时打 `warn` 留痕，见 11.10.6）。
3. **框架分发**（已落地）：`crates/warpui/src/platform/mod.rs` 的 `current` 与 `create_system_clipboard()` 已新增 OHOS 分支；`crates/warpui/src/windowing/mod.rs` 把 `windowing::winit` 的编译门控放宽为 `any(winit, ohos)`（其中字体子系统 `fonts` 对 OHOS 开放，其余项仍守 `cfg(winit)`），`windowing/winit/mod.rs` 内逐项补 `#[cfg(winit)]` 以保持其它平台不变。
4. **应用层**（未落地，遗留）：`app/src/platform/` 尚未新增 `ohos.rs`，`app/src/lib.rs` 的 `run_internal` 也尚无 `#[cfg(target_env = "ohos")]` 定制分支。当前 OHOS 入口完全走 `crates/entry_ohos` 的 `#[ability] launch_app`，未经过 `run_internal`，故编译虽通过但该分支仍待补。
5. **入口 crate**（已落地）：`crates/entry_ohos/`，以 `cdylib` 形式产出 `libcore.so`（由 `Cargo.toml` 的 `[lib] name = "core"` 决定，必须与 `script/ohos/bundle` 的 `OHOS_LIB_NAME`、ArkTS 侧 `moduleName` 三方一致），内部用 `#[ability]` 宏定义 Rust 入口并转调 warp 的 `run()`；另经 `warpui::platform::ohos::spawn` 把阻塞的事件循环移到独立 `warp-main` 线程（见 11.10.6）。

## 6.6 窗口后端选型专论：winit 路线与仿 macOS 路线

本节回答一个必须显式决策的问题：OHOS 的窗口系统是「走 winit（给 winit 补一个 OHOS 后端）」好，还是「仿 macOS 新增自研后端」好。

**本节为修订版**。上一版论证建立在「复用 `openharmony-ability`」与「走 winit」二者互斥的前提之上，该前提经实测**不成立**——winit 的 OHOS 后端 `winit-ohos` 本身就是 `openharmony-ability` 的封装并再导出其 API（见 6.6.2）。既然两条路线都能完整使用 `openharmony-ability`，选型就必须改从 **winit 版本线与改动爆炸半径**来论证；本节据此重做全部对比。

### 6.6.1 两条路线的定义

- **路线 A′（winit 路线）**：让 OHOS 继续走 warpui 现有的 `crates/warpui/src/windowing/winit/` 抽象层，OHOS 的具体实现由 winit 的 OHOS 后端提供。该后端有两种落法（引入现成 crate 或移植），代价差别很大，见 6.6.2 与 6.6.4。
- **路线 B（仿 macOS 路线）**：仿照 macOS，新增 `crates/warpui/src/platform/ohos/`，不走 winit，直接实现 warpui 的窗口与应用抽象，底层对接 `openharmony-ability`。

### 6.6.2 关键实证：winit-ohos crate 的存在

上一版论证曾断言「winit 的 OHOS 支持不存在，等同自研」。此断言**不成立**，必须修正：winit 官方线之外，`ohos-rs` 组织已提供一个可用的 winit OHOS 后端。

- **存在性**：crates.io 上的 `winit-ohos 0.31.0-beta.3`，仓库 `https://github.com/ohos-rs/winit-ohos.git`，README 首句为「Winit's OpenHarmony backend, driven by `openharmony-ability` lifecycle callbacks」。
- **规模**：共 2219 行。
- **依赖**：`openharmony-ability 1.0.0-beta.2`、`openharmony-ability-derive 1.0.0-beta.1`、`winit-core =0.31.0-beta.3`、`dpi =0.1.2`、`rwh_06 0.6`。
- **它本身就是 openharmony-ability 的封装**：`src/lib.rs` 的 `pub mod ability` 直接 `pub use openharmony_ability::*;` 与 `pub use openharmony_ability_derive::*;`，并对外暴露 `EventLoopExtOpenHarmony`、`ActiveEventLoopExtOpenHarmony`、`WindowExtOpenHarmony`、`EventLoopBuilderExtOpenHarmony`（`with_openharmony_app`）。这是「两条路线都能完整使用 openharmony-ability」的源码级证据。
- **能力覆盖**：把 Ability 1.0 的生命周期、surface、content-rect、avoid-area、配置、原始 XComponent 触摸/鼠标/键盘输入、ArkUI 轴与手势输入、IME 回调，翻译为 winit-core 事件。
- **明确限制**（README 原文要点）：`run_app` 注册回调后立即返回、**不得阻塞** Ability 主线程；`RedrawRequested` 仅在系统 `WindowRedraw` 回调时发出，`request_redraw` **无法主动排帧**；`EventLoopProxy::wake_up` 只是请 Ability 排任务，`proxy_wake_up` 要等 OHOS 在主线程执行该任务后才投递；`ControlFlow` 只影响下一次系统回调的 `StartCause`，不能独立起轮询或截止定时器；`run_app_on_demand` 与 `pump_app_events` **被有意设计为不支持**，因为 Ability 生命周期独占主循环。
- **集成前提**：官方用法要求应用依赖 `richerfu/winit` 的 facade（`winit = { git = "https://github.com/richerfu/winit.git", branch = "master" }`），由 OHOS target dep 选中 `winit-ohos`；且入口 crate 仍须直接依赖 `napi-ohos 1.2` / `napi-derive-ohos 1.2` / `openharmony-ability 1.0.0-beta.2`。注意这是 **winit 0.31 拆分架构下的 facade**，与 warp 现用的 0.30.13 单体 fork 不是同一条线。
- **插件边界**：winit-ohos 只自动接线 window 插件（`plugin-window`，默认关闭），并声明 files / permissions / resources / URLs / webviews / app control 等应用级插件由应用自行负责。

这条实证改变了两条路线的可比性：路线 A′ 不再等于「自研 winit 后端」，而是「引入一个已有的 winit 后端 crate」；但其版本线与 warp 现状不一致，代价转移到了版本迁移上（见 6.6.4）。

### 6.6.3 openharmony-ability 提供的能力（实测清单）

`openharmony-ability` 的 `OpenHarmonyApp`（`crates/ability/src/app.rs`）公开接口，按 warpui 的需求归类如下：

- **事件循环**：`run_loop<'a, F: FnMut(Event) + 'a>(&self, event_handle: F)`。这是进程内唯一的事件入口，由 `#[ability]` 宏生成的 `init` 间接驱动。
- **原生窗口句柄**：`native_window(&self) -> Option<RawWindow>`，可直接交给 wgpu 创建 surface。
- **尺寸与缩放**：`content_rect()`、`window_rect()`、`scale()`。
- **避让区**：`avoid_area(AvoidAreaType)`、`avoid_areas()`。
- **按需渲染**：`create_waker()`、`enable_frame_callback()`、`disable_frame_callback()`、`set_frame_rate(min, max, expected)`。
- **输入**：`dispatch_input_event(InputEvent)`；输入事件类型见 `crates/ability/src/input/mod.rs` 的 `InputEvent` 枚举，并已提供 `ui_input_event_to_input_event` 把 ArkUI 输入事件翻译为 Rust 事件；IME 事件见同文件 `ImeEvent`。
- **系统配置**：`config() -> Configuration`，覆盖深浅色、语言、屏幕密度等（`crates/ability/src/configuration/`）。
- **生命周期与状态**：`save` 与 `load`、`set_init_context` 与 `init_context`。
- **沙箱与包信息**：`module_name()`、`base_path()`、`home_directory()`、`pref_path()`、`preferred_locales()`；模块资源目录经 `application_resource_dir(module_name)` 获取。
- **桥与主线程调度**：`bridge() -> Result<BridgeRuntime>`、`main_thread() -> Result<MainThreadScheduler>`、`with_main_thread_bridge`。
- **插件体系**：`register_plugin`、`registered_plugin`、`bridge_plugin_declarations`、`dispatch_bridge_main_thread_event`、`dispatch_plugin_lifecycle`。
- **节点挂载**：`crates/ability/src/node.rs` 的 `NodeSurface`、`NodeExt`、`NodeSurfaceBridgePlugin`，用于把渲染根挂入 ArkUI 组件树。

事件类型覆盖（`crates/ability/src/event.rs` 的 `Event` 枚举）：`WindowCreate`、`WindowDestroy`、`WindowRedraw(IntervalInfo)`、`WindowResize(Size)`、`ContentRectChange`、`AvoidAreaChange`、`ConfigChanged`、`LowMemory`、`Start`、`GainedFocus`、`LostFocus`、`VisibilityChanged(bool)`、`Resume`、`Pause`、`Stop`、`SaveState`、`Create`、`Destroy`、`SurfaceCreate`、`SurfaceDestroy`、`Input(InputEvent)`、`KeyboardEvent(i32)`、`UserEvent`。

这份清单与 warpui 平台 trait 的需求（6.3 节）几乎一一对应：窗口句柄、尺寸、可见性、配置变更、输入、帧回调、生命周期全部已有。

### 6.6.4 路线 A′ 的两个子方案及其真实代价

「走 winit」有两种做法，代价差别很大，必须分开看。

- **A′-1 直接采用 winit-ohos（升到 winit 0.31 线）**
  - warp 当前 pin 的是 warpdotdev fork（`Cargo.toml`，`rev 14db95a686387211d57cf2afd1d908b2d82a20fe`），版本 **0.30.13**，且是**单体 crate**——`src/platform_impl/` 是 crate 内部模块，实测其根目录无 `winit-core` 拆分。
  - winit-ohos 实现的却是 **winit 0.31 的拆分 trait API**：它 `use winit_core::event_loop::EventLoopProvider`、`winit_core::event_loop::ActiveEventLoop`、`winit_core::window::Window`、`winit_core::application::ApplicationHandler`，并把自身作为 0.31 的后端 crate 插入。
  - warpui 的 `crates/warpui/src/windowing/winit/` 用的是 **0.30 单体 API**：`super::EventLoop::new(...)`（`app.rs`）、`winit::event_loop::EventLoop`、`winit::window::Window`、`WindowAttributes::default()`（`window.rs`）、`winit::platform::x11::EventLoopBuilderExtX11::with_x11(...)`（`app.rs`）。全仓库检索 `ApplicationHandler` / `EventLoopProvider` / `run_app` 均**无命中**，反证其停留在 0.30 风格。
  - 于是 A′-1 的代价 = 把 warp 的 winit 依赖从 0.30.13（warpdotdev fork）迁到 0.31（beta）＋把 `windowing/winit/` 11978 行迁到 0.31 API。**这不是 OHOS 独有改动，而是全平台改动**（Linux / Windows / Web / WASM 一起动），直接触碰本项目硬规则「不能改变软件在其他操作系统的编译和功能」，且要把整个 GUI 押到 `0.31.0-beta` 这条尚未稳定的线上。
  - 附加成本：warpdotdev fork 自带的自定义模块——`src/platform/modifier_supplement.rs`、`run_on_demand.rs`、`startup_notify.rs`——需在 0.31 上重新落位；而 winit-ohos 的 README 明确写「`run_app_on_demand` 与 `pump_app_events` 有意不支持，因为 Ability 生命周期独占主循环」，与 fork 中 `run_on_demand` 的用途存在方向性冲突，须逐项确认。
  - 收益：一旦 A′-1 完成，OHOS 的窗口、事件循环、输入、IME 翻译层**零自研**，直接吃 winit-ohos 的 2219 行。

- **A′-2 把 winit-ohos 的逻辑移植进 warp 现有 0.30.13 fork 的 `src/platform_impl/ohos/`**
  - 保持 warp 的 winit 版本不动，OHOS 成为纯附加的 `platform_impl`，形态与 winit 上游给 linux/macos/android 加后端一致。
  - 代价一：winit-ohos 是按 0.31 的 `winit_core` trait 契约写的；0.30 的 `platform_impl` 用的是另一套内部契约（直接用 EventLoop/Window 结构体实现 0.30 固有 API，没有 `EventLoopProvider` / `ApplicationHandler`）。移植 = 把 2219 行集成层按 0.30 内部契约重写，其中输入翻译与事件映射的**逻辑**大体可搬，**类型与注册方式**必须改。
  - 代价二：warp 从此背上一个 winit fork 分叉——既要跟 warpdotdev 的 0.30 上游，又要跟 ohos-rs 的 OHOS 逻辑，三向同步。

- **A′ 的公共代价**：winit 的多窗口、跨平台显示器、窗口装饰、全屏等抽象与 OHOS「单 Ability、单 XComponent」模型不匹配。实测 winit-ohos 的 `Window` 实现里，`set_title`、`set_transparent`、`set_blur`、`set_visible`、`set_resizable`、`set_minimized`、`set_maximized`、`set_fullscreen`、`set_decorations`、`set_window_level`、`set_ime_purpose`、`focus_window` 等大量 setter 均为空实现（no-op），`available_monitors` 返回空、`primary_monitor` 返回 `None`（`event_loop.rs` 中的相关区间）。warpui 若依赖这些语义，需自行降级。

### 6.6.5 路线 B 的代价

- **纯附加、不动 winit**：新增 `crates/warpui/src/platform/ohos/`（与 `crates/warpui/src/platform/mac/` 同构）＋ `#[cfg(target_env = "ohos")]` 分支；`crates/warpui/build.rs` 的 `winit: { not(macos) }` 一行改为 `not(any(macos, ohos))`（受保护文件，须单独授权）。warp 的 winit 0.30.13 fork 与 Linux/Windows/Web/macOS **零改动**，「不改变其他 OS 编译与功能」天然满足。
- **有树内同构先例**：macOS 走的正是这条形态——`crates/warpui/src/windowing/mod.rs` 的 `#[cfg(winit)] pub mod winit;` 与 `crates/warpui/src/platform/mod.rs` 的 `#[cfg(target_os = "macos")] pub mod mac;` 配对；新增 OHOS 与 mac 平行，范式可直接抄。
- **HAP 与插件对齐更顺**：HiCodeer 的 `hap/` 与整套插件（cursor / files / pinch / filedrop / openwith / ime / url ＋ worker-pool）是对着 **openharmony-ability-zed fork**（`jaffenqqcom/openharmony-ability-zed`，tag `v1.0.0-beta.1-zed.1`）接的；路线 B 可沿用同一 fork 与同一插件集，`hap/` 复用度最高。相比之下 winit-ohos 只自动接线 window 一个插件，并声明 files / permissions / resources / URLs / webviews / app control 等应用级插件由应用自己负责——在插件这件事上，A′ 并不省事。
- **代价一：要自写 OHOS 平台后端**。参照 HiCodeer 的 `crates/gpui_ohos`（7246 行），warpui 侧估 3000 至 5000 行；其中渲染器部分（gpui_ohos 的 `wgpu_renderer.rs` ＋ `wgpu_atlas.rs` = 2034 行）可由 warpui 既有 `crates/warpui/src/rendering/wgpu/` 复用而省去。
- **代价二：窗口侧难点要自己踩**——事件三路（XComponent / ArkUI / IME）消重、帧回调持续触发导致的空转 CPU、避让区、可见性、IME、按需 vsync。但这些坑 gpui_ohos 与基座已趟过，且本仓库第 08、12、13、14 章留有实测记录可循。

### 6.6.6 逐项对比与结论

按维度对比（「是否完整使用 `openharmony-ability`」两条路线都是「是」，故不再是区分点）：

- **是否完整使用 `openharmony-ability`**：A′ 与 B 都是。winit-ohos 的 `lib.rs` 直接 `pub use openharmony_ability::*` 与 `openharmony_ability_derive::*`，其事件循环就是 `app.run_loop(...)`；B 则直接调 ability。
- **是否改动 winit 版本**：A′-1 要全 workspace 从 0.30.13 迁到 0.31（beta）；A′-2 要保持 winit 版本但重写集成层并长期背 fork；B 完全不动 winit。
- **对其他操作系统的爆炸半径**：A′-1 覆盖 Linux/Windows/Web/WASM（触碰本项目硬规则）；A′-2 虽只新增 `platform_impl/ohos`，但仍改 winit 本体；B 为零。
- **需自研行数**：B 约 3000 至 5000（渲染器可复用）；A′-1 的窗口层接近零，但要付全平台迁移；A′-2 需按 0.30 契约重写约 2219 行逻辑并背 fork。
- **维护面**：A′ 需维护 winit fork（上游 0.30/0.31 与 ohos-rs 三向同步）；B 无需。
- **HAP 与插件复用度**：B 最高（`hap/` 本就对着 zed fork 接）；A′ 只自动接 window 插件，应用级插件仍需自行注册。
- **成熟度**：winit-ohos 仅 2219 行、版本 `0.31.0-beta.3`，成熟度低于 macOS 后端先例；但其事件与输入翻译已随 winit 发布，具备参考价值。

**结论：采用路线 B——仿 macOS 新增 OHOS 专用窗口后端，以 `openharmony-ability`（可沿用 zed fork）为窗口系统基座，不走 winit。**

理由按权重排序：

1. **前提修正**：两条路线都能完整使用 `openharmony-ability`（winit-ohos 即 ability 驱动并再导出 ability），因此旧版「ability 互斥」的论证作废。真正的区分点是 **winit 版本线与改动爆炸半径**。
2. **版本线不匹配是决定性因素**：warp 在 0.30.13（warpdotdev fork、单体），winit-ohos 在 0.31（拆分 `winit-core`、beta）。A′-1 要全平台迁 0.31，触碰「不改其他 OS」硬规则并把 GUI 押到 beta；A′-2 要重写集成层并长期背三方 fork。B 只需新增一个平台目录。
3. **爆炸半径**：B 对既有平台零影响，与 macOS 同构、先例在树内；A′ 的改动面横跨全 workspace。
4. **工作量数量级**：B 自研 3000 至 5000 行且渲染器可复用；A′ 自研未必更少，还要额外付迁移或重写＋fork 维护。
5. **HAP 与插件**：`hap/` 本就是对着 zed fork 接的，B 复用度最高；winit-ohos 只自动接 window 插件，省不了应用级插件的事。
6. **唯一会翻转结论的情形**：若 warp 出于自身原因（如 warpdotdev 上游决定）**本就要**把 winit 升到 0.31，则 A′-1 的边际成本骤降，届时应重新评估并优先选 winit-ohos。该条件当前不成立。
7. **风险与保留**：winit-ohos 版本为 beta 且规模小（2219 行），成熟度不及 macOS 后端先例；若未来 warp 升级 winit，可重开此议题。

---

# 第七章 main 函数入口分析（分析动作 6）

分析动作 6 要求：分析 main 的执行顺序与功能，说明初始化阶段对操作系统有哪些适配要求、如何与鸿蒙 HAP 软件框架融合，并制定 main 被 HAP 拉起的方案。

## 7.1 原 main 的执行顺序

入口链：`app/src/bin/local.rs` 的 `fn main()` → 设置 `ChannelState` → `warp::run()`。

`app/src/bin/local.rs` 的 `main` 做三件事：加载渠道配置（`warp_channel_config::load_config!("local")`）、构造 `ChannelState` 并叠加特性开关集合（`DEBUG_FLAGS`、`DOGFOOD_FLAGS`、`PREVIEW_FLAGS`、`LOCAL_FLAGS`）、调用 `warp::run()`。

`app/src/lib.rs` 的 `pub fn run()`：

1. `platform::init()`——**第一步即平台初始化**。
2. `features::init_feature_flags()`。
3. 处理控制模式环境变量（`warp_cli::local_control::ControlArgs::from_control_mode_env()`）。
4. 解析命令行参数（`warp_cli::Args::from_env()`）。
5. 渠道相关的 server URL 覆盖处理。
6. 分发 worker / CLI 子命令。
7. 判定是否为独立 CLI 二进制。
8. 调用 `run_internal(LaunchMode::App { ... })`。

`app/src/lib.rs` 的 `run_internal`：

1. `dynamic_libraries::configure_library_loading()`（仅 windows）。
2. profiling 初始化（按 `launch_mode.needs_profiling()`）。
3. 特性开关初始化。
4. Sentry 主 Hub 初始化（按 `feature = "crash_reporting"`）。
5. tracing 初始化。
6. **`warp_logging::init(...)`**——日志初始化。
7. macOS 后台进程类型标记。
8. windows 重定向守卫。
9. `resource_limits::adjust_resource_limits()`——调整资源限制，需在任何子进程创建之前。
10. 单实例转发（linux、windows）。
11. windows Job Object 初始化。
12. `settings::set_settings_mode(...)` 与用户偏好初始化。
13. `PtySpawner::new()`（按 `feature = "local_tty"`）——**pty 生成器，需在进程最干净状态下创建**。
14. 构造 `AppCallbacks`。
15. 构造 `AppBuilder`（GUI 用 `new`，非 GUI 用 `new_windowless`）。
16. 各平台 `AppBuilderExt` 定制（mac、linux、windows）。
17. `app_builder.run(move |ctx| { ... })`——进入事件循环。

## 7.2 与鸿蒙 HAP 框架融合的方案

OHOS 的 HAP 框架中，主线程被 ArkUI 的事件循环占据，应用入口是 EntryAbility。warp 的 `run()` 是一个「一次性进入事件循环的阻塞调用」，不能直接在 ArkUI 主线程上调用，否则会阻塞 UI。融合方案要点：

### 7.2.1 以 cdylib 形式被 HAP 加载

按第 16 章与 HiCodeer 实证，标准形态是**把 warp 主程序编译成 cdylib（`libwarp.so`），在 HAP 的 EntryAbility 中 `import 'libwarp.so'` 加载**，再由 ArkTS 侧在 Ability 生命周期内触发 native 入口。

需要建立的三方一致约定（缺一不可）：

- so 文件名：`libwarp.so`
- 编译期 `NAPI_BUILD_TARGET_NAME`：`warp`
- ArkTS 侧 `EntryAbility.moduleName`：`'warp'`
- `oh-package.json5` 中 so 依赖名：`libwarp.so`

### 7.2.2 双入口设计（Rust 侧）

按第 16 章「启动链」与 00 章 5.2 的说明，OHOS 侧入口建议设计为两个：

- `start_warp_main`：由 NAPI 导出、被 ArkTS 调用，内部创建 warp 业务线程并启动 `warp::run()`（或其等价入口）。这是「原 main」在 OHOS 上的对应物。
- `init_ability`：由 `#[ability]` 宏生成、由 Ability 生命周期回调调用，负责会话初始化。

由于 warp 的 `run()` 内部会构造 `AppBuilder` 并调用 `run()` 阻塞进入事件循环，OHOS 侧需要把这一步改为「进入 OHOS 事件循环」而非桌面事件循环。

### 7.2.3 生命周期回调接入层

warp 的应用生命周期（如 `app/src/lib.rs` 的「系统从睡眠返回」「系统将进入睡眠」「无窗口则终止应用」）需与 OHOS 的 `onForeground` / `onBackground` / `onDestroy` 建立映射。建议集中在 OHOS 专属文件（计划新增 `app/src/platform/ohos/hap_lifecycle.rs`）中管理，不在受保护文件里散落。

### 7.2.4 加载时机与首页渲染的竞态

第 16 章与 HiCodeer 经验给出两条已付代价的细节：

1. **native 模块必须以 `loadMode='sync'` 同步加载**，否则默认页动态 `import` 会黑屏且没有任何错误日志。
2. **native 初始化要延后到数据目录解析完成之后**，因为 native 入口依赖沙箱目录已就绪。

warp 对应的问题更突出：`run()` 的早期步骤（`platform::init()`、`resource_limits::adjust_resource_limits()`）与 `run_internal` 的偏好初始化（`settings::init_private_user_preferences()`）都依赖可写的用户目录。因此 OHOS 侧必须在触发 native 入口**之前**完成沙箱目录（`filesDir` / `cacheDir` / `tempDir`）的解析，并把结果通过环境变量或入口参数传给 Rust。

### 7.2.5 链接期补符号

OHOS libc 缺少 robust mutex（`pthread_mutexattr_setrobust` / `pthread_mutex_consistent`），而 Rust 标准库会引用它们。HiCodeer 的做法是在 cdylib 中导出 no-op 符号补齐（见 `crates/gpui_ohos/depend/launch-zed/src/lib.rs`）。warp 的 OHOS 入口 crate 需要同样处理，否则最终 cdylib 会带未定义符号。

此外还有第 19 章记载的编译期问题：OHOS 的 `PTHREAD_KEYS_MAX` 是 128（而非 glibc 常见的 1024），且 `aarch64-unknown-linux-ohos` target spec 的 `tls-model: emulated` 使 Rust std 的 thread-local 退化为 pthread key 实现，导致大型 workspace 编译时 rustc 自己 abort（`fatal runtime error: out of TLS keys`）。解法是用 `LD_PRELOAD` 垫片拦截 4 个 pthread TLS 入口（`pthread_key_create` / `pthread_getspecific` / `pthread_setspecific` / `pthread_key_delete`），换用虚拟键空间。这是**构建期**问题，只在编译工具链上生效，不影响产物。垫片源码按本仓脚本框架入库于 `script/ohos/ohos-tls-shim.c`，由 `script/ohos/bundle` 用 SDK clang 编译后注入 `LD_PRELOAD`。

## 7.3 main 融合方案的落地步骤（建议）

1. 新增入口 crate（如 `crates/entry_ohos/`，路径含 `ohos`），`crate-type = ["cdylib"]`，依赖 warp 主 crate 与 `openharmony-ability`。
2. 在入口 crate 中用 `#[ability]` 宏定义入口函数，内部转调 warp 的启动逻辑。
3. 编译产出 `libwarp.so`，由构建脚本拷入 `hap/entry/libs/arm64-v8a/`。
4. HAP 侧 `EntryAbility.ets` 的 `moduleName` 设为 `'warp'`，`import 'libwarp.so'`。
5. 验证启动链：Ability 初始化 → `init`（Rust 入口）→ 数据目录解析 → warp 事件循环 → 首帧渲染。

---

# 第八章 main 对操作系统的依赖分析（分析动作 7）

分析动作 7 要求：看 main 初始化路径里哪些地方使用操作系统能力、受到文件系统与权限约束，逐一列出依赖点、对应的鸿蒙约束与绕行方案。

## 8.1 路径与文件系统依赖

### 8.1.1 路径解析

`crates/warp_core/src/paths.rs` 是路径解析中心，依赖：

- `directories::BaseDirs`。
- `dirs::home_dir()`（如 `warp_home_config_dir`、`data_dir` 的 mac 分支、`gui_config_local_dir`）。
- `project_dirs()` / `project_dirs_for_app_id(...)`。
- 平台分支：`data_dir()`、`config_local_dir()`、`gui_config_local_dir()`、`tui_config_local_dir()` 均以 `cfg_if!` 区分 macOS 与其它平台。

**OHOS 约束**：应用沙箱只能读写 `filesDir` / `cacheDir` / `tempDir`；且应用 uid 不在 `/etc/passwd` 中，`dirs::home_dir()` 与 `directories` 依赖的环境变量（`HOME`）在沙箱内可能为空或指向不可用路径。

**绕行方案**（第 05 章）：新增 `ohos_files_dir` / `ohos_cache_dir` / `ohos_temp_dir` 三个函数（受保护文件只新增函数，不改既有函数），返回沙箱映射路径；在 `data_dir` / `config_local_dir` 等既有函数的 `cfg_if!` 中新增 OHOS 分支指向上述函数。同时在入口 crate 中把 `HOME`、`TMPDIR`、`TEMP`、`TMP` 显式设为沙箱可写目录（第 01 章 `set_ohos_environment` 经验）。

### 8.1.2 日志目录

`app/src/lib.rs` 调用 `warp_logging::init`，其 `native.rs` 实现会创建日志目录与文件。OHOS 上需改为走 hilog（见第三章），并避免在文件系统上落盘（或落盘到 `cacheDir`）。

### 8.1.3 资源目录

warp 使用 `rust-embed` 内嵌资源（`app/src/lib.rs` 中的 `ASSETS`，传入 `Box::new(ASSETS)`）。第 02 章记载：OHOS 上 `rust-embed` 的 debug 构建读不到资源，需启用 `debug-embed`。这是 main 初始化的直接依赖点。

## 8.2 环境与权限依赖

- **HOME / TMPDIR / TEMP / TMP**：`paths.rs` 与日志、设置都依赖。OHOS 无有效 HOME，须显式注入。
- **当前工作目录**：warp 的终端与文件功能依赖可读的 cwd。OHOS 启动时 cwd 是沙箱根 `/`（不可读），需 `chdir` 到应用可读目录（第 18 章实测）。
- **文件访问授权**：访问用户目录（如「打开文件夹」）必须先经选择器授权并持久化（`FILE_ACCESS_PERSIST`），每次访问前激活（第 05 章三件套）。
- **单实例机制**：`app/src/lib.rs`（linux 转发与 windows 转发）依赖本地 socket / 命名对象。OHOS 上 `target_os = "linux"` 会使 linux 分支被编译进来，但它依赖 `app_services::linux::pass_startup_args_to_existing_instance`，其底层（unix socket 路径）在沙箱内不可用，须排除或替换。
- **资源限制调整**：`resource_limits::adjust_resource_limits()` 在 OHOS 上可能无对应能力或权限，须确认其行为（若无害可保留，若报错须 OHOS 分支降级并记日志）。

## 8.3 main 初始化依赖点汇总

按执行顺序列出 OHOS 需要处理的依赖点：

1. `platform::init()`（`app/src/lib.rs`）：warpui 平台初始化，OHOS 需在 `app/src/platform/ohos` 中实现。
2. 特性开关初始化：无 OS 依赖，可复用。
3. 命令行参数解析（`warp_cli::Args::from_env()`）：HAP 拉起时无命令行参数，需提供合理的默认值（`LaunchMode::App`），并处理「无 argv」场景。
4. 日志初始化：改走 hilog。
5. `resource_limits::adjust_resource_limits()`：确认 OHOS 行为。
6. 单实例转发：排除或替换。
7. 设置与偏好初始化：依赖沙箱可写目录，须先注入路径。
8. `PtySpawner::new()`（`feature = "local_tty"`）：这是最重的依赖点，见第九章。
9. `AppBuilder::new(...)` 与资源（`rust-embed`）：确认 `debug-embed`。
10. 事件循环入口：接 OHOS 事件循环。

---

# 第九章 进程/线程分析与操作系统约束分析（分析动作 8）

分析动作 8 要求：分析 HAP 入口对原程序的影响、原程序的线程/进程结构与 HAP 框架的融合、UI 线程与逻辑线程边界、以及源代码与 OHOS 系统约束的冲突点。

## 9.1 HAP 入口对原程序的影响

- ArkUI 事件循环占据 HAP 主线程。warp 的 `run()` 是阻塞式进入事件循环的调用，**不能在 ArkUI 主线程直接执行**。
- 建议：`start_warp_main` 在 ArkTS 侧被调用后，由 Rust 侧创建一个专用业务线程运行 warp 的事件循环；与 ArkTS/NAPI 的交互一律经 TSFN 投递回创建线程（第 15 章：NAPI 只能在创建线程调用，多线程调用会 SIGABRT）。
- 生命周期事件（前台/后台/销毁）由 ArkTS 侧回调进入 Rust，再转成 warp 业务状态。

## 9.2 线程与进程结构

### 9.2.1 线程边界（四类）

按第 15 章归纳，OHOS 上 warp 的线程边界为：

- **UI 线程（ArkTS / NAPI）**：系统回调大多在此触发，NAPI 只能在此调用。
- **warp 业务线程**：运行 warp 事件循环的线程。
- **TSFN**：后台线程调 NAPI 的桥，把调用投递到 UI 线程执行。
- **子进程 / 原生子进程**：终端 shell 等。

### 9.2.2 进程与 pty

- warp 大量使用 `std::process::Command` 与 pty（`app/src/terminal/local_tty/` 下有 `docker_sandbox.rs`、`mod.rs`、`terminal_manager.rs`、`terminal_view_adaptor.rs` 等；`feature = "local_tty"` 涉及 59 个文件）。
- OHOS 沙箱约束：**手机形态**禁止 `fork` / `exec` / `execl` / `posix_spawn`，`openpty` 因无 `/dev/ptmx` 一并被禁；**PC/2in1 形态**实测 `fork` + `exec /bin/sh` + PTY 可用（第 04、18 章，2026-09-03 更正）。
- 目标形态以 `hap/entry/src/main/module.json5` 与 `hap/entry/build-profile.json5` 为准：当前 HAP 声明的 `deviceTypes` 是 `["tablet", "2in1"]`，即面向**平板与 PC/2in1**，这属于「可 fork/exec」的形态，是重大利好。
- 即便如此，`exec` 的目标受白名单限制：只能 exec 系统 `/bin/sh` 与随包分发的 HNP 可执行文件，不能 exec 任意 ELF。

### 9.2.3 终端子进程的落地路径

warp 的终端要跑 shell（bash/zsh/fish）与工具（git 等），OHOS 上没有这些程序。按第 04、09、12、25 章的选型判据：

- **只调用少数几个程序** → 打成 HNP 内置进 HAP。
- **会调用大量程序** → 走 cmd-agent 桥（本地守护进程或 guest）。

warp 依赖的外部程序较多（shell、git、语言服务器、AI CLI 等），且已有 SSH 客户端能力（第 25 章记载 warp-oh 用 libssh2 + mbedTLS 桥接）。因此建议**混合方案**：少量核心工具（如 zsh、git）打 HNP 本地 fork，其余经 cmd-agent 桥。**（2026-09-25 落地：本地 HNP 为 `zsh.hnp` / `git.hnp`，桥为 `hitshell` + `hitdaemon`，见 11.11。）**

## 9.3 与 OHOS 系统约束的冲突点清单

按附录 A 约束速查表逐条核对，warp 源代码中相关功能与约束的冲突如下：

### 9.3.1 进程与执行

- 冲突：`std::process::Command` 全网使用（终端、工具调用、ripgrep 子进程 `warp_cli::WorkerCommand::RipgrepSearch` 等）。
- 方案：HNP 本地 fork（PC/2in1 可 exec `/bin/sh` 与随包 ELF）+ cmd-agent 桥覆盖其余（第 04、25 章）。

### 9.3.2 Shell 与终端

- 冲突：设备只有 toybox sh 与 `/bin/sh`，无 bash/zsh/fish、无 `stty`。
- 方案：随包打 zsh HNP（第 18 章 warp 交叉编译 zsh 的完整方法），或桥接远端 shell。注意 zsh 的两个坑：`zsh/regex` 模块在静态构建下需把 `link` 改为 `static`；heredoc 临时文件走 `TMPPREFIX`（须注入到可写目录）。

### 9.3.3 文件系统与权限

- 冲突：沙箱只允许 `filesDir` / `cacheDir` / `tempDir`；用户目录需授权；剪贴板读受限。
- 方案：路径三态转换 + 授权持久化 + 剪贴板走系统输入法粘贴或 PasteButton（第 05、17 章）。

### 9.3.4 渲染与图形

- 冲突：wgpu 无官方 OHOS 后端；无 Metal/DX12；无 FreeType / fontconfig。
- 方案：补 OHOS surface（`OH_NativeWindow` 包装），Vulkan 优选、GLES 保底；字体枚举、文本布局与字形光栅化走 warp 自带的 cosmic-text 实现（`windowing/winit/fonts.rs` → `platform/ohos/`，见 11.8），**不用** FontParser + `libnative_drawing`（第 07 章、11.8 节）。

### 9.3.5 输入与交互

- 冲突：同一输入产生三路并行事件流、无 `ModifiersChanged` 事件；IME 无 preedit 回调、API 23 失焦 detach 崩溃。
- 方案：消除重复源 + SourceType 过滤 + 手动构造事件；仅启动 attach（第 12、13 章）。

### 9.3.6 窗口与事件

- 冲突：窗口尺寸来源不一致（标题栏与 surface 差约 84px）、live-resize 每帧重绘、XComponent 帧回调持续触发导致空转 CPU 高。
- 方案：以 surface_size 为单一来源、拖动抑制、按需注册/注销帧回调（第 08、14 章）。

### 9.3.7 运行时桥接

- 冲突：NAPI 非线程安全（多线程调用 SIGABRT）；NAPI 模块不自动触发。
- 方案：TSFN 跨线程桥接；`dlopen` + `dlsym` 手动拉起（第 15 章）。

### 9.3.8 系统能力与后台

- 冲突：后台/锁屏冻结导致连接断；启动存在双线程竞态。
- 方案：`TASK_KEEPING` 持续任务保活（需 `keepBackgroundRunning` 与 `backgroundModes: ["taskKeeping"]`）；启动用事件通道解耦（第 16、17 章）。

## 9.4 线程安全与死锁规避

- **终端模型加锁**：`AGENTS.md` 明确规定 `TerminalModel::lock()` 的死锁风险。OHOS 适配层若在事件回调中额外加锁，极易与业务线程形成环路，必须遵循「优先传引用、锁作用域最短、不在锁内调用可能再锁的函数」三条。
- **NAPI 调用线程**：所有 NAPI 调用必须回到创建线程，经 TSFN 投递。
- **桥接同步与异步分离**：`openharmony-ability` 提供 `AsyncBridge` 与 `MainThreadSyncBridge` 两种模式（第 23 章），需按「调用是否需要同步返回值」选择，避免在主线程阻塞等待。

---

# 第十章 HAP 复用与编译脚本

本章说明 HAP 工程的复用方式与构建工具链，属落地支撑内容。

## 10.1 HAP 工程的复用

`hap/` 目录是从 HiCodeer 逐字拷贝的工程骨架，可复用度高，但**必须清理以下残留**（否则装不起来或行为错乱）：

- `hap/AppScope/app.json5`：`bundleName` 当前为 `com.hiwarp.treminal`（存在拼写错误），而 `hap/build-profile.json5` 中为 `com.hiwarp.terminal`，两者**不一致**，必须统一。
- `hap/oh-package.json5` 与 `hap/entry/oh-package.json5`：`libhicodeer.so` 需改为 `libwarp.so`。
- `hap/entry/src/main/cpp/types/libhicodeer/`：目录与其中的 `Index.d.ts`、`oh-package.json5` 需改名为 `libwarp`，`Index.d.ts` 内容按 warp 实际导出同步。
- `hap/entry/src/main/ets/entryability/EntryAbility.ets`：`moduleName` 当前为 `'hicodeer'`，改为 `'warp'`；`import 'libhicodeer.so'` 改为 `import 'libwarp.so'`；ArkTS 注释中提到的 `crates/zed` 与 `libhicodeer` 需更新为 warp 对应物；插件集合按 warp 需要裁剪。
- `hap/entry/src/main/ets/entryability/Setup.ets`：home 目录选择逻辑（`HomeDirectory.resolveExisting`、`awaitSelection`）按 warp 需要处理；若 warp 不需要「选择 home 目录」这一步，可简化为直接使用沙箱目录（但仍需在 native 加载前完成目录解析）。
- `hap/entry/src/main/module.json5`：`hnpPackages` **已声明** `git.hnp` 与 `zsh.hnp`（均为 private，见 11.9）；`requestPermissions` 按需增删；`skills` 中的 `FileOpen` 类型列表按 warp 支持的文件类型调整。
- `hap/entry/hnp/arm64-v8a/`：**已就位为 `git.hnp` + `zsh.hnp`，`hicodeerd.hnp` 已删除**（warp 不采用 cmd-agent daemon）。`git.hnp` 从 HiCodeer 直接复制（与源仓 md5 一致；HiCodeer 仓内无其构建脚本，属预制二进制载荷）；`zsh.hnp` 亦为预制件，见 11.9。
- `hap/AppScope/syscap.json`：按 warp 需要的能力集调整。
- `hap/build-profile.json5`：`signingConfigs` 中的证书路径当前指向 `/storage/Users/currentUser/Documents/ohos/config/default_hapHD-...`，需确认三件套（`.cer` / `.p7b` / `.p12`）与配套 `material/` 是否在位；`modules` 数组中的 `native_ability` 与各 `plugin_*` 的 `srcPath` 指向 `../target/ohos-arkts/openharmony-ability/...`，该路径由构建脚本在编译时从 cargo 缓存同步生成（当前尚不存在）。
- `hap/local.properties`：SDK 路径按本机实际填写。

## 10.2 openharmony-ability 的引入与用法

`openharmony-ability` 是 HiCodeer fork 的 cargo git 依赖，仓库为 `https://github.com/jaffenqqcom/openharmony-ability-zed`，tag `v1.0.0-beta.1-zed.1`。三组 crate 必须解析到同一 git 源与同一 revision：

- `openharmony-ability`（基座：事件循环、渲染根、生命周期、ArkTS 与 Rust 双向桥、剪贴板 / 文件 URI / 子进程等 NDK 能力）
- `openharmony-ability-derive`（`#[ability]` 宏）
- `openharmony-ability-plugin-*`（cursor / filepicker / filelaunch / filedropin / openbysys / pinch / ime / url 等能力插件）

用法（以 HiCodeer 为样板）：

- Rust 侧入口：

```
#[ability]
pub fn launch_app(app: openharmony_ability::OpenHarmonyApp) {
    openharmony_ability::set_global_app(app.clone());
    // ... 环境准备 ...
    start_warp_main(app.base_path(), app.home_directory());
}
```

- `#[ability]` 宏会生成一个 `openharmony_ability_mod` 模块，导出 NAPI 函数：`init`、`render`、`dispose_render`、`dispose_all_renders`、`dispose_bridge`、`on_back_press_intercept`、`on_bridge_sync_event`、`on_bridge_lifecycle`、`set_window_actions`。其中 `init` 内部以 `APP_CONFIGURED.get_or_init(|| launch_app(APP))` 触发 Rust 入口——**这是 Rust 程序被拉起的唯一入口点**（见 derive crate 的宏展开逻辑）。
- ArkTS 侧：`EntryAbility extends NativeAbility`，设置 `moduleName`、`loadMode='sync'`、`defaultPage`、`bridgePlugins`。
- 插件对应关系：ArkTS 的 `bridgePlugins` 与 Rust 侧注册的插件必须一一对应。

## 10.3 编译工具链与本机位置（实测）

本机工具链已就位。构建脚本按本仓既有框架落位于 `script/ohos/`——与 `script/linux/`、`script/macos/`、`script/wasm/`、`script/windows/` 同级，根级 `script/bundle` 与 `script/run` 按 `uname -s` 分派；OHOS 上 `uname -s` 实测返回 `HarmonyOS`（并非 `Linux`），故需为这两个入口新增 `HarmonyOS` 分支，**不引入根级单文件脚本**。工具链路径假设与 HiCodeer 的 `script/bundle-ohos` 一致：

- brew 前缀：`/storage/Users/currentUser/.harmonybrew`。
- OHOS SDK：`/storage/Users/currentUser/.harmonybrew/opt/ohos-sdk`，编译器 `/storage/Users/currentUser/.harmonybrew/opt/ohos-sdk/native/llvm/bin/clang`（软链到 clang-15）。
- 命令行打包工具链：`/storage/Users/currentUser/workspace/commandline-ohos`，其中：
  - `node.org/node_22.7.0`
  - `hvigor.org/hvigor_1.0.0`
  - `ohpm.org/ohpm_1.1`
  - `sdk.org/sdk_1.0.0`（其 `default/openharmony/toolchains/lib/` 下有 `hap-sign-tool.jar`）
- Rust 工具链：`/storage/Users/currentUser/.rustup/toolchains/` 下有 `1.97.1-aarch64-unknown-linux-ohos` 与 `stable-aarch64-unknown-linux-ohos`（实测分别为 1.97.1 / 1.96.0；均为本机自举，`rustc -vV` 的 host 即 `aarch64-unknown-linux-ohos`）。
- **工具链 pin 冲突（前置阻塞项）**：本仓 `rust-toolchain.toml` pin 的是上游 warp 的 `channel = "1.92.0"`，而本机**未安装**该 toolchain——在仓库内执行任何 `cargo`/`rustc` 都会报 `toolchain '1.92.0-aarch64-unknown-linux-ohos' is not installed`。该文件路径不含 `ohos`，属受保护文件、不得修改。**已定方案**：仿 HiCodeer，在 `script/ohos/bundle` 内以 `export RUSTUP_TOOLCHAIN=1.97.1` 运行时覆盖，不动 pin 文件（1.97.1 本机已装，且 HiCodeer 已在同架构 OHOS 上用它编译过体量相当的大 workspace）。构建脚本必须显式导出该变量，否则会落回未安装的 1.92.0 而直接失败。
- 交叉编译目标三元组：`aarch64-unknown-linux-ohos`。

## 10.4 构建脚本的流程骨架

参照 HiCodeer 的 `script/bundle-ohos`（步骤编排可直接复用，但落位必须改为本仓框架 `script/ohos/bundle`），warp 侧构建脚本应编排如下确定性步骤：

1. 平台探测（brew 在 → 本机 harmonyos；否则 openeuler）。
2. 工具链配置块集中定义所有路径（node / hvigor / ohpm / sdk / rustup）。
3. 编译期 TLS 垫片准备：源码入库于 `script/ohos/ohos-tls-shim.c`，由本步骤用 SDK clang 编成 `.so` 后注入 `LD_PRELOAD`（扩 pthread key，绕 128 上限），并做自检。**此步不可省**：`aarch64-unknown-linux-ohos` 的 std 用 pthread key 实现 `thread_local!`（一变量一键，上限 128），warp 的大 workspace 会让 rustc/cargo 自身 `out of TLS keys` abort。
4. `cargo build --lib -p <入口 crate> --target aarch64-unknown-linux-ohos` 产出 cdylib。
5. 拷贝 `libwarp.so` 到 `hap/entry/libs/arm64-v8a/`。
6. 从 cargo checkout 同步 `native_ability` 与 `plugins` 到 `target/ohos-arkts/openharmony-ability/`。
7. `ohpm install`（顶层与 entry 各一次，并检查关键符号链接齐全）。
8. `hvigorw assembleHap` 产出未签名 HAP。
9. HNP 载荷装配与注入：由 `script/ohos/bundle` 编排 `hnpcli pack`（stage 布局 `<pkg>/bin` + `<pkg>/conf` + `<pkg>/shim` + `hnp.json`），首批载荷含 private `git.hnp`（从 HiCodeer 复制）与 private `zsh.hnp`（仅 zsh 二进制，依赖走系统库，见 11.9），二者均为预制件、已就位，产物落 `hap/entry/hnp/arm64-v8a/`；再注入未签名 HAP（用 python `zipfile`，注意 `allowZip64=True`）。注意 hmdfs 只保留 owner 权限位，hnpcli 依 `stat` 记录权限，需重写 zip 中央目录的权限位，否则可执行文件落地为 0744 不可执行。**2026-09-25 起另有 `script/ohos/cmdbridge` 负责构建并打包 `hitdaemon.hnp`（public）与 `hitshell.hnp`（private），见 11.11——载荷不再全是预制件。**
10. 用 `hap-sign-tool.jar` 的 `sign-app` 整体重签名（**必须用 jar 版，不能用同目录的 ELF 版**，否则签出 merkle-tree 格式导致设备安装报 `code:9568407`）。
11. 安装：`hdc install -r <signed.hap>`。

关键环境变量：`CC_aarch64_unknown_linux_ohos` / `AR_aarch64_unknown_linux_ohos` / `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER` 指向 SDK clang；`RUSTUP_TOOLCHAIN=1.97.1`；`NAPI_BUILD_TARGET_NAME=warp`；`TMPDIR` 钉在项目内临时目录（本机 `/tmp` 只读）。

## 10.5 设备侧动作的固化入口

按项目既有约定，设备侧动作一律走固化入口，不临场手搓：

- 连接设备：`export OHOS_HDC_SERVER_PORT=18710` → `hdc tconn 127.0.0.1:<设备调试端口>`（端口取自 `/bin/param get persist.hdc.port`）→ `hdc list targets` 校验。
- 装包：项目根 `install-local.sh`。
- hilog 落盘控制：`~/.codebuddy/scripts/hilog-persist.sh`。
- 抓日志必须按程序名过滤（如 `grep "com.hiwarp.terminal"`）。

---

# 第十一章 平台适配点全清单与 openharmony-ability 结合方案

warp 的平台适配**不是集中在一个后端目录里**，而是分散在四个层次、三十余个路径上。只改 `platform/ohos/` 一个目录并不足以让应用跑起来。本章把这些适配点逐一列出，说明每一处的**功能是什么**，以及**如何与 `openharmony-ability` 结合**。

## 11.1 适配点的四层分布（实测）

`target_os` / `target_family` / `cfg(unix)` / `cfg(windows)` 在各平台的命中数差异很大：macos 最多、linux 次之、freebsd 再次、windows 最少，另有大量 wasm 与 `cfg(unix)` / `cfg(windows)` 分支。原始分析阶段 `target_env` 仅出现在 2 个 gnu 相关文件、无 ohos；但 OHOS 移植已在 `crates/` 下大量文件新增 `target_env = "ohos"` 守卫，该前提在移植后已不成立。按职责归为四层：

- **第一层 入口层**：`app/src/bin/*.rs`（6 个 channel 入口） → `app/src/lib.rs` 的 `run()` / `run_internal()`。职责：进程启动、参数解析、平台初始化钩子、拉起 UI 事件循环。
- **第二层 框架分发层**：`crates/warpui/build.rs` 的 cfg 别名、`crates/warpui/src/platform/mod.rs`、`crates/warpui/src/platform/app.rs`、`crates/warpui_core/src/platform/mod.rs`。职责：编译期选择后端、运行期分发、操作系统标识。
- **第三层 窗口与平台后端层**：`crates/warpui/src/platform/{mac,headless}/`、`crates/warpui/src/platform/{linux,windows}/`，以及被前者复用的 `crates/warpui/src/windowing/winit/`。职责：实现 `Delegate` / `WindowManager` / `Window` / `WindowContext` / `FontDB` / `TextLayoutSystem` / `DispatchDelegate` 等 trait。
- **第四层 系统能力桥接层**：分散在 `crates/warpui_core`、`crates/warpui`、`crates/warpui_extras`、`crates/warp_core`、`crates/warp_terminal`、`app/src/*`，共 25 项（剪贴板、通知、文件选择器、系统主题、IME、光标、无障碍、URL/文件打开、终止应用、应用资源目录、全局快捷键、菜单栏、安全存储、用户偏好、崩溃上报、自动更新、开机自启、系统信息、麦克风、本地控制、路径、日志、PTY、定时器、线程池）。职责：把 OHOS 的系统能力接到 warp 的抽象上。本层与 ability 的对应关系见 11.1.1 与 11.5。

### 11.1.1 供给侧：openharmony-ability 的两类能力（实测清单）

ability 提供的是一套**多 crate 的能力体系**，不是单个 API。实测其目录（2026-09-24 复测）：`crates/` 下共 16 个 crate——`ability`、`derive`、`worker-pool`，以及 13 个插件（`plugin-app-control`、`plugin-cursor`、`plugin-filedropin`、`plugin-filelaunch`、`plugin-filepicker`、`plugin-ime`、`plugin-openbysys`、`plugin-permission`、`plugin-pinch`、`plugin-resource`、`plugin-url`、`plugin-webview`、`plugin-window`）。按交互方向分为两类。

**A 类：系统窗口消息（推送式，无需桥）**

- 载体：`#[ability]` 宏 + `OpenHarmonyApp::run_loop(FnMut(Event))`（`crates/ability/src/app.rs:757`，非阻塞注册）+ `Event` 枚举（`crates/ability/src/event.rs:9`）。
- 事件源模块：`lifecycle.rs`（381 行，Ability 生命周期）、`input/`（XComponent / ArkUI / IME）、`area/`（避让区，`avoid.rs` / `rect.rs` / `size.rs`）、`configuration/`（`color_mode.rs` / `config.rs` / `direction.rs` / `screen_density.rs`）、`stage/event.rs`、`render/xcomponent.rs`（Surface 创建/销毁）、`waker.rs`（唤醒）、`timer.rs`。
- 特征：**同进程直接回调**，不经过 ArkTS，无序列化、无权限门。

**B 类：系统能力接口（请求式，分两条通道）**

- **B1 内置模块（同进程 NDK 直调，无需桥）**：
  - `clipboard.rs`（404 行）——`write_content` / `read_content`（2026-09-24 由 `write_text` / `read_text` 扩展而来，增加 HTML 与图片口味，见 12.4.2），直接调 `libpasteboard` + `libudmf`，文件注释明确「绕开 ArkTS `@ohos.pasteboard` 权限门」。
  - `hotkey.rs`（322 行）——`register_hotkey` / `unregister_hotkey` / `set_hotkey_triggered_handler`，直连 `libohinput` 的全局热键 C API，头文件无 `@permission` 标注（见 12.4.2）。
  - `window_control.rs`（92 行）——`minimize_main_window` / `show_and_focus_main_window`，把 ArkTS 宿主递进来的两个闭包存成 threadsafe function（见 12.4.2）。
  - `child_process.rs`（112 行）——`spawn_native_child_process` / `kill_native_child_process` / `register_native_child_process_exit_callback`，底层 `OH_Ability_StartNativeChildProcess`。
  - `timer.rs`（86 行）——FFRT 一次性定时器，回调跑在 FFRT worker 线程而非主线程。
  - `file_uri.rs`（180 行）——`OH_FileUri_GetPathFromUri` 把 picker 返回的 `file://docs/...` URI 转成 `std::fs` 可打开的沙箱路径；`OH_FileShare_PersistPermission` 做跨重启持久授权。
  - `memory/`、`configuration/`、`node.rs`（`NodeSurface` 把渲染根挂进 ArkUI 组件树）。
- **B2 插件族（经 `bridge` 异步往返 ArkTS）**：13 个插件，全部以 `XxxExt` 扩展 trait 挂在 `OpenHarmonyApp` 上。实测公开方法与规模：
  - `plugin-filepicker`（255 行）→ `FilePickerExt::show_file_dialog`（返回所选文件 URI）。
  - `plugin-permission`（193 行）→ `PermissionExt::request_permission`。
  - `plugin-url`（114 行）→ `UrlExt::open_url`。
  - `plugin-ime`（335 行）→ `ImeExt::ime() -> ImeClient`。
  - `plugin-cursor`（106 行）→ `CursorExt::set_cursor_style(i32)`。
  - `plugin-app-control`（77 行）→ `AppControlExt::terminate(env, code)`。
  - `plugin-resource`（229 行）→ `ResourceExt::resource_manager()`。
  - `plugin-window`（545 行）→ `WindowExt::window() -> WindowClient`，含 `query_avoid_area` / `create_os_window` / `set_window_decorations`，请求类型覆盖 `WindowBlurRequest` / `WindowColorRequest` / `WindowMoveRequest` / `WindowResizeRequest` / `WindowFocusableRequest` / `WindowStateResponse`。
  - `plugin-openbysys`（199 行）→ `OpenBySysExt`。
  - 其余：`plugin-filelaunch`（99 行）、`plugin-filedropin`（135 行）、`plugin-pinch`（153 行）、`plugin-webview`（1050 行）；独立能力 crate `worker-pool`（562 行，`WorkerPool::dispatch` / `dispatch_with_priority`）。
- 桥机制：`bridge/mod.rs`（1736 行）提供 `BridgePlugin`（`:333`）、`BridgePluginRegistry`（`:469`）、`BridgeRuntime`（`:1168`）、`MainThreadScheduler`（`:1062`）、`BridgeNapiType`（`:66`），以及 `AsyncBridge`（`:139`）与 `MainThreadSyncBridge`（`:142`）两种模式。

### 11.1.2 供给与需求的对应关系

两类能力与四层**不是重叠关系，而是分工关系**：

- **A 类（窗口消息）喂第一层与第三层**：`run_loop` 的注册点是**第一层**入口（替代 macOS 的阻塞 `app.run()`，`platform/mac/app.rs`）；`Event` 的翻译与分发落在**第三层**的 `platform/ohos/event_loop.rs`，并直接驱动 `WindowContext`（`Event::WindowRedraw` → 重绘、`WindowResize` → `size()`、`SurfaceCreate/Destroy` → surface 生命周期）与 `WindowManager`（焦点、可见性、避让区）。
- **B1 内置模块喂第四层，且属「直连」而非「走桥」**：剪贴板、子进程、定时器、文件 URI 转换，全部可直接调用，零桥接成本。
- **B2 插件族喂第四层，且属「现成 facade」而非「自行设计桥」**：文件选择器、权限、URL 打开、光标、IME、终止应用、资源——ability 已把桥与 ArkTS 侧 HAR 一并做好，warp 只需注册插件并调 trait 方法。
- **B2 的 `plugin-window` 回填第三层**：窗口装饰、模糊、颜色、移动、缩放、可聚焦等能力可用，缩小了第三层需要「降级」的面。
- **第二层不受 ability 影响**：它是纯编译期/分发层的机械改动，与 ability 无耦合。

一句话概括：**ability 是「供给侧」，四层是「需求侧」；A 类覆盖第一、三层的窗口与事件需求，B 类覆盖第四层的系统能力需求，其中 B2 的 `plugin-window` 部分回填第三层。**

## 11.2 第一层：入口层

**现状（实测）**：`app/src/bin/` 下有 6 个入口——`dev.rs`、`integration.rs`、`local.rs`、`oss.rs`、`preview.rs`、`stable.rs`。它们汇聚到 `app/src/lib.rs`：`run()`、`run_internal()`、`app_builder.run()`；平台分支位于 `lib.rs` 的 mac / linux / windows 各扩展段。

**功能**：解析命令行与 channel、创建 `AppBuilder`、注册平台专属初始化（如 `lib.rs` 的 `dynamic_libraries::configure_library_loading()`、`platform::windows::check_redirection_guard()`、`command::windows::init()`）、调用 `AppBuilder::run(init_fn)` 进入事件循环。

**与 ability 结合**：

- HAP 侧由 `EntryAbility.ets` 继承 `NativeAbility`（`hap/entry/src/main/ets/entryability/EntryAbility.ets`），`moduleName`/`loadMode='sync'`/`bridgePlugins` 已在位。so 加载后由 NAPI `init` 触发 Rust 入口，即 `#[ability] fn ...(app: OpenHarmonyApp)`。
- 入口 crate（角色等同 HiCodeer 的 `depend/launch-zed`）在 `#[ability]` 函数内：先做 `ensure_shell_env` 等环境准备，再调用 warp 的 `run_internal()` 等价路径。
- **关键语义差异（已实测）**：macOS 的 `platform/mac/app.rs` 用 `app.run()` **阻塞**持有主循环；而 ability 的 `run_loop`（`crates/ability/src/app.rs:757`）是**非阻塞**的——它把回调存入 `self.event_loop`、置 `HAS_EVENT=true` 后立即返回，主循环由 ArkTS `NativeAbility` 生命周期驱动。因此 OHOS 分支的 `App::run` 必须实现为「注册回调后返回」，不能照抄 macOS 的阻塞模型。
- `app/src/lib.rs` 的 `run_internal` 是整个入口层最集中的受保护文件，OHOS 分支将落在这里，属**须单独授权的只加分支**改动。

## 11.3 第二层：框架分发层

### 11.3.1 编译期门控：`crates/warpui/build.rs`

现状别名（实测）：`macos: { target_os = "macos" }`、`winit: { not(any(macos, ohos)) }`、`wgpu: { any(winit, ohos, feature = "experimental-wgpu-renderer") }`、`native: { not(target_family = "wasm") }`。

**功能**：决定哪些平台走 winit、哪些平台编译 wgpu 渲染器。`crates/warpui/src/windowing/mod.rs` 的 `#[cfg(winit)] pub mod winit;` 与 `crates/warpui/src/rendering/mod.rs` 的 `#[cfg(wgpu)] pub mod wgpu;` 都消费这两个别名。

**与 ability 结合**：路线 B 下把门控调为 `not(any(macos, ohos))`、并让 `wgpu` 在 OHOS 下为真（新增 `ohos` 别名或把 `wgpu` 改为 `any(winit, ohos, feature = ...)`）。这是**一行级分支调整**——移植已在 `build.rs` 落地，文件属受保护（路径不含 ohos），后续改动仍须单独授权。

### 11.3.2 分发入口：`crates/warpui/src/platform/mod.rs`

现状：模块声明 `app`/`linux`/`mac`/`wasm`/`windows`/`headless`；`pub mod current` 用 `cfg_if!` 按 wasm → linux/freebsd → macos → windows 顺序分发，兜底 `warpui_core::platform::test::*`；`create_system_clipboard()` 按 macos / linux-freebsd / windows 三路构造剪贴板；`is_mobile_device()` 在非 wasm 下恒为 `false`。

**功能**：把「当前平台后端」这一概念收敛为 `platform::current::*`，是全仓最核心的分发点。

**与 ability 结合**：新增 `#[cfg(target_env = "ohos")] pub mod ohos;`，在 `current` 的 `cfg_if!` 中插入 ohos 分支（**必须置于 linux 分支之前**，因为 OHOS 的 `target_os` 就是 `linux`），并在 `create_system_clipboard()` 增加 ohos 分支。`is_mobile_device()` 在 OHOS（tablet/2in1）需重新评估。均为受保护文件，只加不改。

### 11.3.3 后端选择：`crates/warpui/src/platform/app.rs`

现状：`AppBackend` 只有两个变体 `CurrentPlatform(Box<super::current::App>)` 与 `Windowless(Box<super::headless::App>)`；`AppBuilder::new` 走 `current`，`new_windowless` 走 headless；`AppBuilder::run` 汇聚到 `AppBackend::run`。

**功能**：GUI 与无窗口两种后端的选择与统一启动入口。

**与 ability 结合**：OHOS 走 `CurrentPlatform` 变体即可——`ohos::App` 只需满足 `current::App` 的同一接口契约（`new(callbacks, assets, test_driver)` + `run(init_fn)`），**无需改动 `app.rs`**。这是分层设计的红利。

### 11.3.4 操作系统标识：`crates/warpui_core/src/platform/mod.rs`

现状：`OperatingSystem` 枚举有 `Linux`/`Mac`/`Windows`/`Other(Option<&'static str>)`；`OperatingSystem::get()` 按 wasm → linux/freebsd → macos → windows 分发；`default_shell_family()` 把 Linux/Mac/Other 归为 `ShellFamily::Posix`。

**功能**：运行期回答「我是什么系统」，影响 shell 家族、路径、行为分支。

**与 ability 结合**：两种做法——(a) 新增 `OperatingSystem::Ohos` 变体并插入 `get()` 分支（更精确，但要排查所有 `match` 点以维持穷尽性）；(b) 沿用 `Other(Some("ohos"))`（零改动，但失去类型区分）。推荐 (a)，代价是需在全仓核对 `OperatingSystem` 的匹配点。受保护文件，只加分支。

## 11.4 第三层：窗口与平台后端层

### 11.4.1 三套现成模板（实测规模）

- **macOS 模板**：`crates/warpui/src/platform/mac/`（含 `window.rs`、`rendering/metal/renderer.rs`、`text_layout.rs`、`fonts.rs`、`app.rs`、`delegate.rs`、`menus.rs`、`event.rs`、`keycode.rs`、`clipboard.rs` 等，另含若干 Objective-C 文件 `mac/objc/*.m/.h`）。**含大量 Cocoa 绑定，不能直接搬。**
- **winit 模板**：`crates/warpui/src/windowing/winit/`，其中 `platform/{linux,windows}` 只是薄转发。**属路线 A′，路线 B 不使用。**
- **headless 模板**：`crates/warpui/src/platform/headless/`（`windowing.rs`、`event_loop.rs`、`delegate.rs`、`app.rs`、`mod.rs` 等），**纯 Rust、无 ObjC、无原生窗口**。它自带 mpsc 事件循环 + `AppEvent`、内存态 `WindowManager`、独立 `AppDelegate`，字体直接复用 `warpui_core::platform::test::FontDB`（`headless/app.rs`）。

**结论**：headless 是路线 B 的**最佳起点**——它已经把「不用 winit 时如何实现整套平台 trait」演示了一遍，规模仅 845 行。OHOS 后端的做法是：以 headless 的骨架为底，把其中三处替换掉——mpsc 事件循环 → ability 的 `run_loop` + `Event` 分发；内存态窗口 → 持有原生窗口句柄与 wgpu surface 的 `Window`；测试字体 → OHOS 真实字体提供者。

### 11.4.2 必须实现的 trait 全集（实测签名）

来自 `crates/warpui_core/src/platform/mod.rs`：

- `Delegate`（约 24 个方法）：`dispatch_delegate`、`request_user_attention`、`clipboard`、`system_theme`、`open_url`、`open_file_path`、`open_file_path_in_explorer`、`open_file_picker`、`open_save_file_picker`、`application_bundle_info`、`show_native_platform_modal`、`request_desktop_notification_permissions`、`send_desktop_notification`、`set_cursor_shape`、`close_ime_async`、`is_ime_open`、`open_character_palette`、`set_accessibility_contents`、`register_global_shortcut`、`unregister_global_shortcut`、`set_dock_icon_visible`（默认空实现）、`terminate_app`、`is_screen_reader_enabled`、`microphone_access_state`、`is_gui`（默认 `true`）。
- `DispatchDelegate`（非 wasm 版要求 `Send + Sync`）：`is_main_thread`、`run_on_main_thread`。
- `FontDB`：`load_from_bytes`、`load_from_system`、`load_all_system_fonts`、`process_loaded_system_fonts`、`family_id_for_name`、`load_family_name_from_id`、`select_font`、`fallback_fonts`、`font_metrics`、`glyph_advance`、`glyph_raster_bounds`、`glyph_typographic_bounds`、`rasterize_glyph`、`glyph_for_char`、`text_layout_system`。
- `TextLayoutSystem`（要求 `Send + Sync`）：`layout_line`、`layout_text`。
- `LoadedSystemFonts`（标记 trait）：`as_any`。
- `Window`：`minimize`、`toggle_maximized`、`toggle_fullscreen`、`fullscreen_state`、`set_background_backdrop`（默认空）、`uses_native_window_decorations`、`set_titlebar_height`、`supports_transparency`、`graphics_backend`、`supported_backends`、`as_ctx`、`callbacks`、`as_any`。
- `WindowContext`：`size`、`origin`、`backing_scale_factor`、`max_texture_dimension_2d`、`render_scene`、`request_redraw`、`request_frame_capture`。
- `WindowManager`（约 22 个方法）：`open_window`、`platform_window`、`remove_window`、`active_window_id`、`key_window_is_modal_panel`、`app_is_active`、`activate_app`、`show_window_and_focus_app`、`hide_app`、`hide_window`、`set_window_bounds`、`set_window_alpha`（默认空）、`set_all_windows_background_blur_radius`、`set_window_title`、`close_window_async`、`active_display_bounds`、`active_display_id`、`display_count`、`bounds_for_display_idx`、`active_cursor_position_updated`、`windowing_system`、`os_window_manager_name`、`is_tiling_window_manager`、`ordered_window_ids`（默认空）、`cancel_synthetic_drag`（默认空）。

### 11.4.3 逐 trait 与 ability 的对应（实测）

- **`DispatchDelegate` → 直接命中**：`is_main_thread` / `run_on_main_thread` 对应 `OpenHarmonyApp::main_thread() -> Result<MainThreadScheduler>`（`crates/ability/src/app.rs`）与 `with_main_thread_bridge`。这是最省事的一项。
- **`WindowContext::size` / `origin` / `backing_scale_factor` → 直接命中**：对应 `content_rect()`、`window_rect()`、`scale()`。
- **`WindowContext::request_redraw` / 帧节流 → 直接命中**：对应 `create_waker()`、`enable_frame_callback()`、`disable_frame_callback()`、`set_frame_rate(min, max, expected)`。ability 已实现「按需渲染 + 空闲不空转」，这正是自研最易踩坑处。
- **`WindowManager` 的显示/焦点/可见性 → 直接命中**：`app_is_active`、`activate_app`、`hide_app` 对应 `Event::GainedFocus` / `LostFocus` / `VisibilityChanged` / `Start` / `Resume` / `Pause`（`crates/ability/src/event.rs`）；避让区对应 `avoid_area(AvoidAreaType)` 与 `avoid_areas()`。
- **`Delegate::system_theme` → 直接命中**：`config()` 返回的 `Configuration` 含 ColorMode（深浅色）、语言、屏幕密度。
- **`Delegate::close_ime_async` / `is_ime_open` → 直接命中**：`crates/ability/src/input/mod.rs` 的 `ImeEvent` 与 `ime::KeyboardStatus`。
- **`WindowManager::active_display_id` / `active_display_bounds` → 需降级**：OHOS 单 XComponent 模型下多显示器语义不成立，参照 winit-ohos 的同类实现（返回空迭代器、`primary_monitor` 返回 `None`）做退化，并记 `warn` 日志。
- **`WindowManager` 的窗口外观类方法 → 部分回填、部分降级**：`set_all_windows_background_blur_radius` 可由 `plugin-window` 的 `WindowBlurRequest` 承担；装饰、颜色、移动、缩放、可聚焦分别对应 `WindowDecorationsRequest` / `WindowColorRequest` / `WindowMoveRequest` / `WindowResizeRequest` / `WindowFocusableRequest`，均经 `WindowClient`（`plugin-window/src/lib.rs`）异步调用——注意 warpui 这些方法是同步签名，需改成「发起请求 + 结果经事件回流」的模式。而 `set_title`、`minimize`、`toggle_maximized`、`toggle_fullscreen` 在 OHOS 由系统窗口管理、应用无对等控制权，仍需明确降级（记 `warn`，非静默空函数）。
- **`Delegate::open_character_palette` / `show_native_platform_modal` / `set_accessibility_contents` / `request_user_attention` → 走桥或降级**：OHOS 无桌面级同名概念，需逐项决定（详见 11.5）。（`register_global_shortcut` 原列此项，2026-09-24 已实现并移出，见 12.4.2（1）。）
- **`FontDB` / `TextLayoutSystem` → 需移植 warp 自带的 cosmic-text 实现**：ability 不提供字体与文本布局，这是第三层里唯一规模较大的落地项。**基座既不是 macOS/CoreText，也不是 hicodeer**：mac 实现（`platform/mac/fonts.rs` 与 `text_layout.rs`）绑定 CoreText；hicodeer 的 `gpui_ohos/src/ohos/text_system.rs`（648 行）实现的是 gpui 的 `PlatformTextSystem`，接口与 warp 不匹配。正确基座是 **warp 自己的 winit 后端 `crates/warpui/src/windowing/winit/fonts.rs`**（cosmic-text + swash + font-kit），它已实现 warp 的 `FontDB`/`TextLayoutSystem`。完整论证、逐 trait 对应、hicodeer 可复用清单与风险项见 **11.8**。
- **`Window::graphics_backend` / `supported_backends` → 取决于 wgpu**：见 11.4.4。

### 11.4.4 渲染接入（依赖红利：上游 crate 支持 OHOS，warp 自身零预埋）

**先明确一件事，避免误读**：这里说的「已支持」指的是 **warp 所依赖的第三方 crate 在上游支持了 OHOS**，不是 warp 代码里存在任何 OHOS 预埋。实测：全仓 `app/`、`crates/`、`script/` 下所有 `.rs` 文件检索 `ohos` / `Ohos` / `OHOS`，**零命中**。

因此这是**依赖红利**，不是现成能力：省掉的是「fork wgpu、自己写 EGL / `OH_NativeWindow` 绑定」，**没省掉**的是「写窗口后端」——warp 侧仍需自己实现 `HasWindowHandle` / `HasDisplayHandle` 并调用 `create_surface`（见本小节末的接入点）。

具体红利来自两个第三方 crate：

- **`raw-window-handle 0.6.2` 原生支持 OHOS**：提供 `OhosNdkWindowHandle { native_window: NonNull<c_void> }` 与 `DisplayHandle::ohos()`（`raw-window-handle-0.6.2/src/ohos.rs`）；枚举侧有 `RawWindowHandle::OhosNdk` 与 `RawDisplayHandle::Ohos`。其中 `DisplayHandle::ohos()` 返回 `DisplayHandle<'static>`（无借用），天然满足 warpui 的 `HasDisplayHandle` 要求。该 crate 同时是 wgpu 的依赖并被其重导出为 `wgpu::rwh`（`wgpu-30.0.0/src/lib.rs`）；warp 另在 `crates/warpui_core/Cargo.toml` 直接依赖它。
- **`wgpu-hal 30.0.0` 原生支持 OHOS**：`src/gles/egl.rs` 有 `(Rwh::OhosNdk(_), _) => {}`，同一文件另有分支有 `(WindowKind::Unknown, Rwh::OhosNdk(handle)) => handle.native_window.as_ptr()`（直接用 `native_window` 指针建 EGL window surface），以及 `src/vulkan/instance.rs` 均有 `#[cfg(target_env = "ohos")]` 分支。warp 使用的正是 `wgpu 30.0.0`（`Cargo.toml`），版本对齐。
- **后端选择天然对上**：warp 给 wgpu 开启的 feature 含 `gles`（`Cargo.toml`），而 wgpu-hal 的 OHOS 支持恰好落在 GLES/EGL 路径上，OHOS 走 GLES 后端无需额外开启任何东西。
- **ability 侧已有句柄**：`OpenHarmonyApp::native_window() -> Option<RawWindow>`；HiCodeer 的 `gpui_ohos` 即通过 `native_window().raw_window_handle()` 取得 `RawWindowHandle`（`gpui_ohos/src/ohos/window.rs`）。

**接入点与一个待解细节**：warpui 的 surface 创建在 `crates/warpui/src/rendering/wgpu/resources.rs`（`instance.create_surface(window_handle)`），其入参签名为 `impl Into<wgpu::SurfaceTarget<'static>> + wgpu::rwh::HasDisplayHandle`。OHOS 后端需要提供一个同时满足这两者的类型。`DisplayHandle::ohos()` 是 `'static`，无障碍；但 `WindowHandle<'a>` 借用于本地 `RawWindowHandle`，与 `SurfaceTarget<'static>` 的 `'static` 约束存在张力。gpui_ohos 的做法是绕开安全路径、直接走 `wgpu::SurfaceTargetUnsafe::RawHandle` + `create_surface_unsafe`（`gpui_ohos/src/ohos/wgpu_renderer.rs`）。

因此需要二选一，**这是全章唯一需要改受保护渲染文件的点**：
- (a) 由 OHOS 窗口类型**按值持有** `RawWindowHandle`（`OhosNdkWindowHandle` 是 `Copy`，且 XComponent 的 `OHNativeWindow` 生命周期长于窗口），并提供一个 `'static` 包装类型，沿用安全路径；
- (b) 在 `resources.rs` 新增一个 `#[cfg(target_env = "ohos")]` 的专用构造路径，内部使用 `create_surface_unsafe`，并附 SAFETY 说明（须单独授权）。

推荐 (a) 优先评估——若可行则完全不改受保护文件。

### 11.4.5 第三层的落地清单（计划新增）

1. `crates/warpui/src/platform/ohos/mod.rs`：模块声明与 re-export。
2. `crates/warpui/src/platform/ohos/app.rs`：`App`，以 headless 骨架为底，`run` 内调用 `app.run_loop(...)` 注册回调后**返回**。
3. `crates/warpui/src/platform/ohos/event_loop.rs`：把 ability 的 `Event` 枚举翻译为 warpui 的 `AppEvent`（对应 headless 的 `event_loop.rs`）。
4. `crates/warpui/src/platform/ohos/delegate.rs`：`AppDelegate` + `DispatchDelegate`（对应 headless 的 `delegate.rs`）。
5. `crates/warpui/src/platform/ohos/windowing.rs`：`WindowManager` + `Window` + `WindowContext`，持有 `RawWindowHandle` 与 wgpu surface（对应 headless 的 `windowing.rs`，加渲染后显著增厚）。
6. `crates/warpui/src/platform/ohos/input.rs`：ability `InputEvent` / `ImeEvent` → warpui 键盘与鼠标事件（对应 gpui_ohos 的 `keycodes.rs` 213 行 + 事件翻译）。
7. `crates/warpui/src/platform/ohos/fonts.rs` 与 `text_layout.rs`：字体与文本布局（规模最大的自研项）。
8. `crates/warpui/src/platform/ohos/clipboard.rs`：剪贴板。

## 11.5 第四层：系统能力桥接层（分散最广）

以下逐项给出「路径 / 功能 / 与 ability 结合方案」。分类标记：**[直连]** 用 ability 内置模块（B1，同进程直调，无需桥）；**[现成 facade]** ability 插件已提供 Rust 接口与配套 ArkTS HAR（B2），只需注册插件并调 trait 方法；**[走桥]** ability 无现成接口，需自行设计桥；**[降级]** OHOS 无对等能力，明确不支持；**[不适用]** 该模块在 OHOS 下不编译或不启用。

- **剪贴板** — 路径：`crates/warpui_core/src/clipboard_utils.rs`（`arboard`，`#[cfg(not(target_family = "wasm"))]`）、`crates/warpui/src/platform/mac/clipboard.rs`、`crates/warpui/src/windowing/winit/{linux,windows}/clipboard.rs`；分发点 `crates/warpui/src/platform/mod.rs`。功能：系统剪贴板读写（文本/图片）。方案：**[直连]**。ability 的 `clipboard.rs`（155 行）已提供 `write_text(&str) -> bool` 与 `read_text() -> Option<String>`，直接封装 `libpasteboard` + `libudmf`，且文件注释明确「绕开 ArkTS `@ohos.pasteboard` 权限门」。新增 `platform/ohos/clipboard.rs` 包一层 `crate::Clipboard` trait 即可，并在 `create_system_clipboard()` 增加 ohos 分支。注意：ability 只覆盖**纯文本**，warp 的图片/富文本剪贴板需另找路径或明确降级。
- **系统通知** — 路径：`crates/warpui/src/windowing/winit/notifications/{linux,windows,wasm}.rs` + `mod.rs`、`crates/warpui_core/src/notification.rs`、`crates/warpui/src/platform/mac/notification.rs`、`app/src/notification.rs`。功能：发通知、请求通知权限、点击回调。方案：**[走桥]**。ability 的 13 个插件清单（见 11.1.1）**不含通知插件**，需自行经 `bridge` 新增或在 ArkTS 侧实现后回调；权限申请部分可复用 `plugin-permission`。
- **文件选择器** — 路径：`crates/warpui/src/windowing/winit/delegate.rs`（打开与保存），当前用 `native_dialog`。功能：打开/保存文件对话框。方案：**[现成 facade]**，`plugin-filepicker::FilePickerExt::show_file_dialog`（255 行，`FileDialogOptions` 支持 dialog_type / allow_many / default_location / filters）返回文件 URI；再配 `ability::file_uri`（180 行）的 `OH_FileUri_GetPathFromUri` 把 URI 转成 `std::fs` 可打开的沙箱路径，并用 `OH_FileShare_PersistPermission` 做持久授权。**此项由「必须自行设计桥」降为「调用现成接口」，且 `file_uri` 恰好补上了「picker 给 URI、Rust 打不开」这个最易踩的坑。**
- **系统主题** — 路径：`Delegate::system_theme`（`winit/delegate.rs`、`mac/delegate.rs`）。功能：返回 Light/Dark。方案：**[直连]**，`OpenHarmonyApp::config()`（`:730`）的 `Configuration` 直接含 ColorMode（`crates/ability/src/configuration/color_mode.rs`）。
- **IME** — 路径：`Delegate::close_ime_async`（`winit/delegate.rs`）、`is_ime_open`（`winit/delegate.rs`）。功能：关闭/查询输入法。方案：**[直连] + [现成 facade]**。事件侧由 A 类的 `ImeEvent`（`crates/ability/src/input/mod.rs:129`）推送；控制侧由 `plugin-ime::ImeExt::ime() -> ImeClient`（335 行）提供会话控制。
- **光标** — 路径：`Delegate::set_cursor_shape` / `get_cursor_shape`（`winit/delegate.rs`）。功能：设置指针形状。方案：**[现成 facade]**，`plugin-cursor::CursorExt::set_cursor_style(i32)`（106 行）直接设置系统指针样式，需把 warpui 的 `Cursor` 枚举映射到 OHOS 的 `Input_PointerStyle` 数值。
- **无障碍** — 路径：`Delegate::set_accessibility_contents`（`winit/delegate.rs`）。功能：给屏幕阅读器提供内容。方案：**[走桥]**，插件族无无障碍插件，需自行桥接 `@ohos.accessibility`。
- **URL / 文件打开** — 路径：`winit/delegate.rs`（`open_url_in_system`、`open_url`、`open_file_path`、`open_file_path_in_explorer`）。功能：调系统打开 URL、文件、文件管理器。方案：**[现成 facade]**，`plugin-url::UrlExt::open_url(url) -> Future<Result<()>>`（114 行）对应 `Delegate::open_url`；「用其他应用打开」可考虑 `plugin-openbysys`（199 行，`OpenBySysExt`）。`open_file_path_in_explorer` 无直接对应，需 **[走桥]** 或降级。
- **终止应用** — 路径：`Delegate::terminate_app`（`winit/delegate.rs`、`mac/delegate.rs`）。功能：退出应用。方案：**[现成 facade]**，`plugin-app-control::AppControlExt::terminate(env, code)`（77 行）。
- **应用资源目录** — 路径：`Delegate::application_bundle_info`、`crates/warp_core` 的资源读取。功能：读取应用内置资源。方案：**[现成 facade] + [直连]**，`plugin-resource::ResourceExt::resource_manager()`（229 行）给出 `NativeResourceManager`；另有 `OpenHarmonyApp::application_resource_dir(module_name)`（`:77`）。
- **全局快捷键** — 路径：`crates/warpui/src/windowing/winit/delegate/global_hotkey.rs`（依赖 `global_hotkey` crate）。功能：注册系统级热键。方案：**已实现（2026-09-24）**——OHOS 并非无此概念，`libohinput` 的热键 C API 完整可用（见 12.4.2（1）），走框架 core 直连 NDK，不开插件。
- **菜单栏** — 路径：`crates/warpui/src/platform/mac/menus.rs` + `mac/objc/menus.{h,m}`、`crates/warpui_core/src/platform/menu.rs`、`app/src/app_menus.rs`。功能：原生菜单栏。方案：**[不适用]**，OHOS 无桌面菜单栏；warp 已有非 mac 路径（`AppBuilder::convert_custom_triggers_to_keystroke_triggers`，`platform/app.rs`）把菜单项转为快捷键。
- **安全存储** — 路径：`crates/warpui_extras/src/secure_storage/`（`mac.rs`/`linux.rs`/`windows.rs`/`noop.rs`/`unavailable.rs`），选择逻辑在 `mod.rs` 的 `cfg_attr(path = ...)`。功能：存取凭据。**风险**：OHOS 因 `target_os = "linux"` 会落到 `linux.rs`（D-Bus/secret-service），在 OHOS 上不可用。方案：**[走桥]**——ability 未提供安全存储接口（与剪贴板不同），需新增 `ohos.rs`（用 OHOS 的 `@ohos.security.asset` 或 HUKS），或经授权评估后明确降级到 `unavailable.rs`。
- **用户偏好** — 路径：`crates/warpui_extras/src/user_preferences/`（`file_backed`/`toml_backed`/`registry_backed`/`user_defaults`/`in_memory`）。功能：存 UI 偏好。方案：**[直连]**，OHOS 落到 `file_backed` 或 `toml_backed` 即可，需核对 `mod.rs` 的 cfg 选择顺序。
- **崩溃上报** — 路径：`app/src/crash_reporting/{linux,mac,sentry_minidump}.rs`。功能：崩溃捕获与上报。方案：**[走桥]/延后**，插件族无崩溃上报插件；OHOS 可用 `@ohos.app.ability.errorManager` 或 faultLogger，短期可先不接。
- **自动更新** — 路径：`app/src/autoupdate/{linux,mac,windows}.rs`。功能：应用自更新。方案：**[不适用]**，HAP 由系统应用市场更新，应明确停用该模块的 OHOS 分支（停用而非空函数）。
- **开机自启** — 路径：`app/src/login_item/{macos,windows}.rs`。功能：登录时自启。方案：**[不适用]**，OHOS 由系统管理。
- **系统信息** — 路径：`app/src/system/info.rs`。功能：采集 OS 版本、内存等。方案：**[走桥]** 或读 `/proc`；内存类可优先用 `ability::memory`（`crates/ability/src/memory/`），其余 OHOS 用 `@ohos.deviceInfo` / `@ohos.systemParameter`。
- **麦克风 / 语音** — 路径：`app/src/voice/`、`Delegate::microphone_access_state`。功能：语音输入与转写。方案：**[现成 facade]（权限）+ [走桥]（采集）**——权限申请用 `plugin-permission::PermissionExt::request_permission`（193 行），音频采集需 `@ohos.multimedia.audio`，ability 未提供。
- **本地控制服务** — 路径：`app/src/local_control/`（Unix socket + loopback HTTP + 凭据 broker）。功能：CLI 与运行中实例通信。方案：**[直连]**，OHOS 支持 Unix socket 与 loopback；需 `INTERNET` 权限（`hap/entry/src/main/module.json5` 已声明）并调整沙箱路径。
- **路径** — 路径：`crates/warp_core/src/paths.rs`（依赖 `directories::BaseDirs` 与 `dirs::home_dir`）。功能：计算配置/数据/日志目录。方案：**[直连]**，优先改用 ability 的 `home_directory()`（`:449`）与 `base_path()`（`:444`），比 `directories` 猜测沙箱路径更准。
- **日志** — 路径：`crates/warp_logging/src/lib.rs`（`#[cfg_attr(not(target_family="wasm"), path="native.rs")]`）。功能：日志落盘/输出。方案：**[直连]**，新增 hilog 重定向（详见 3.3 节）。
- **PTY** — 路径：`crates/warp_terminal/src/local_tty/unix.rs`（`nix::pty::openpty`，门控为 `target_os = "linux"/"macos"/"freebsd"`）。功能：创建伪终端供 shell 使用。方案：两条路线。**(a) 沿用 `nix::pty::openpty`**——先实测，若在 OHOS 沙箱下被拒，参照 HiCodeer 为 `rustix-openpty` 打的 patch 思路走 `openat` 回退。**(b) 改用 `ability::child_process`（B1 内置）**——`spawn_native_child_process("libxxx.so:Entry", params)` 以同包 `.so` 入口起子进程，`register_native_child_process_exit_callback` 收退出通知；这是 OHOS 原生的「受控子进程」模型（HiCodeer 用它起 cmd-agent 守护进程）。若走 (b)，交互式 PTY 语义（winsize、信号、终端属性）需另行确认。`crates/warp_terminal` 中**无任何 `target_env` 门控**，是需重点验证的模块。
- **定时器** — 新增项。功能：延迟/超时任务（warp 有 `app/src/interval_timer.rs` 与多处超时逻辑）。方案：**[直连]**，`ability::timer::OpenHarmonyTimer::start(timeout, callback)`（86 行）基于 FFRT，回调跑在 worker 线程而非主线程。注意它是**一次性**定时器（fire-and-forget），周期性定时需自行重排。
- **线程池** — 新增项。功能：后台任务并行执行。方案：**[现成 facade]**，`worker-pool::WorkerPool`（562 行，`dispatch` / `dispatch_with_priority` / `for_available_parallelism`）可作 OHOS 侧后台执行器；但 warp 已有自己的线程与 rayon 用法，是否替换需评估。
- **动态库加载** — 路径：`app/src/dynamic_libraries.rs`。功能：Windows 专用 `SetDllDirectoryW`。方案：**[不适用]**，调用点已 `#[cfg(windows)]` 门控，OHOS 不编译。
- **防病毒** — 路径：`app/src/antivirus/windows.rs`。功能：Windows 专用检测。方案：**[不适用]**。

## 11.6 适配点分级与优先级

- **P0 必须做（否则跑不起来）**：第二层全部分发点；第三层的 `App` / `WindowManager` / `Window` / `WindowContext` / `Delegate` / `DispatchDelegate` 与 wgpu surface 接入；第四层的剪贴板、路径、日志、PTY。
- **P1 应做（核心体验）**：字体与文本布局（`FontDB` / `TextLayoutSystem`）；输入与 IME 翻译；文件选择器；URL/文件打开；系统主题；安全存储。
- **P2 可延后**：通知、无障碍、光标、系统信息、麦克风、本地控制、崩溃上报。
- **P3 明确不适用（写清理由，不做空实现）**：自动更新、开机自启、菜单栏、防病毒、Windows 动态库加载。（全局快捷键原列此项，2026-09-24 已实现并移出，见 12.4.2（1）。）

## 11.7 与 openharmony-ability 结合的总原则

1. **能用 ability 的直接用，不要重造**：事件循环、原生窗口句柄、尺寸与缩放、避让区、按需帧回调、系统配置（深浅色/语言/密度）、输入事件与 IME、生命周期、主线程调度、沙箱路径（`home_directory` / `base_path` / `application_resource_dir`），全部已有。
2. **系统能力优先用 ability 的现成接口，只有缺口才自行设计桥**：`clipboard`（同进程直调 NDK）、`child_process`、`timer`、`file_uri` 属 **B1 内置模块**，零桥接；文件选择器（`plugin-filepicker`）、权限（`plugin-permission`）、URL 打开（`plugin-url`）、光标（`plugin-cursor`）、IME（`plugin-ime`）、终止应用（`plugin-app-control`）、资源（`plugin-resource`）、窗口外观（`plugin-window`）属 **B2 插件族**，ability 已把 Rust 接口与 ArkTS 侧 HAR 一并做好，只需在 `hap/` 的 `bridgePlugins` 注册对应插件并调 trait 方法。只有**确无现成接口**的少数项（通知、无障碍、安全存储、崩溃上报、音频采集）才需自行经 `bridge()`（`:598`）/ `register_plugin`（`:648`）实现异步往返。
3. **无对等能力的明确降级，不做静默空函数**：多显示器、窗口标题、per-window 的最小化/最大化/全屏、菜单栏等——按本项目硬性规则「不准使用空函数、桩函数」处理，要么给出真实退化行为，要么明确不支持并记 `warn`/`error` 日志。（全局快捷键与 **app 级**窗口最小化 / 唤起已于 2026-09-24 实现，移出本列，见 12.4.2。）注意窗口**装饰、模糊、颜色、移动、缩放、可聚焦**已由 `plugin-window` 提供，不在降级之列。
4. **分发点只加不改**：`OperatingSystem` 枚举、`platform/current` 分发、`create_system_clipboard()`、`build.rs` cfg 别名、`app/src/platform/mod.rs` 模块声明、`app/src/lib.rs` 的 `run_internal` 分支——全部是受保护文件，按「新增独立分支、不改已有行」的方式落地，逐项单独授权。
5. **受保护文件清单（须逐项授权后才能改）**：`crates/warpui/build.rs`、`crates/warpui/src/platform/mod.rs`、`crates/warpui/src/platform/app.rs`、`crates/warpui_core/src/platform/mod.rs`、`crates/warpui/src/rendering/wgpu/resources.rs`、`crates/warp_core/src/paths.rs`、`crates/warpui_extras/src/secure_storage/mod.rs`、`crates/warp_terminal/src/local_tty/unix.rs`、`app/src/lib.rs`、`app/src/platform/mod.rs`。

## 11.8 第三层专题：字体与文本子系统的 OHOS 落地方案

字体与文本是 11.4.3 里唯一「ability 不提供、必须自己落地」的规模项，也是最容易被两条错误路线带偏的地方——照搬 macOS 的 CoreText，或照搬 hicodeer 的 `gpui_ohos`。本节给出实测证据与正确基座。

### 11.8.1 warp 的接口要求（实测签名）

契约定义在 `crates/warpui_core/src/platform/mod.rs`：

- **`LoadedSystemFonts`**（标记 trait）：`as_any`。
- **`TextLayoutSystem`**（要求 `Send + Sync`）：`layout_line(text, line_style, style_runs, max_width, clip_config) → Line`、`layout_text(text, line_style, style_runs, max_width, max_height, alignment, first_line_head_indent) → TextFrame`。
- **`FontDB`**：`load_from_bytes`、`load_from_system`、`load_all_system_fonts() → BoxFuture<'static, Box<dyn LoadedSystemFonts>>`、`process_loaded_system_fonts`、`family_id_for_name`、`load_family_name_from_id`、`select_font(family_id, Properties)`、`fallback_fonts(char, font_id) → Vec<FontId>`、`font_metrics`、`glyph_advance`、`glyph_raster_bounds`、`glyph_typographic_bounds`、`rasterize_glyph(..., SubpixelAlignment, RasterFormat)`、`glyph_for_char`、`text_layout_system`。

两个要点：**一是布局结果是富结构的 `Line`/`TextFrame`**（不是「一串字形」）；**二是系统字体加载是异步的**（`load_all_system_fonts` 返回 future，`process_loaded_system_fonts` 回填）。

### 11.8.2 「汇报 layout 结果并保存」的确切含义（实测）

这是 warp 的**核心缓存机制**，与后端无关，位于 `crates/warpui_core/src/text_layout.rs`：

- `TextCache<T>`：`prev_frame: Mutex<HashMap<CacheKeyValue, Arc<T>>>` + `curr_frame: RwLock<HashMap<...>>`，`finish_frame()` 做双世代轮转（本帧未命中的上帧条目被淘汰）。
- `LayoutCache`：`line_cache: TextCache<Line>` + `text_frame_cache: TextCache<TextFrame>`。
- `layout_text` / `layout_line`：以 `CacheKeyRef`（text + font_size + line_height_ratio + fixed_width_tab_size + style_runs + max_width/max_height + alignment + indent + clip）查缓存；未命中才调平台的 `TextLayoutSystem`，随后 `insert` 保存。同一文本同一样式**同帧只算一次、跨帧复用**。
- **`Line` 里物化的 layout 产物**：`width`、`trailing_whitespace_width`、`runs: Vec<Run>`、`ascent`/`descent`、**`caret_positions: Vec<CaretPosition>`**（逐字素光标位）、**`chars_with_missing_glyphs: Vec<char>`**。其中 `Run` = `font_id + glyphs + styles + width`，`Glyph` = `id + position_along_baseline + index + width`。
- **缺字汇报回路**：`crates/warpui_core/src/fonts/text_layout_system.rs` 的 `request_fallback_font_for_char(ch, RequestedFallbackFontSource)`，来源分 `UncachedText` 与 **`TextFrame(key)`**；后者把「缺失字符」回指到**已保存的那个 `TextFrame` 的 key**，字体补齐后按 key 精准失效重排。

**结论：平台实现必须交出完整的 `Line`/`TextFrame`（含 `caret_positions` 与 `chars_with_missing_glyphs`）。** 这既是「能不能用」的判据，也是验收标准。

### 11.8.3 hicodeer 的实现（实测）与不匹配清单

`HiCodeer/crates/gpui_ohos/src/ohos/text_system.rs` 实现的是 **gpui 的 `PlatformTextSystem`**，后端为 `cosmic_text` + `swash` + `fontdb` + `font_kit`。逐项比对，**不匹配**：

- **返回类型不同**：`layout_line(text, font_size, runs) → LineLayout { font_size, width, ascent, descent, runs: Vec<ShapedRun>, len }`；`ShapedRun { font_id, glyphs }`、`ShapedGlyph { id, position, index, is_emoji }`（`gpui/src/text_system/line_layout.rs`）。对比 warp 的 `Line`：**缺 `trailing_whitespace_width`、`caret_positions`、`chars_with_missing_glyphs`、`clip_config`、`Run.styles`、`Glyph.width`**。
- **没有 `layout_text`**：gpui 把换行/对齐放在更高层（`line_wrapper.rs`），平台层只做单行；而 warp 的 `layout_text` 是**平台方法**。
- **没有 caret 位**：gpui 的 `LineLayout` 不保存光标位，靠 `index_for_x` / `closest_index_for_x` / `x_for_index` **按需从 run 现算**；warp 要求**物化并被缓存**。
- **没有缺字汇报**：hicodeer 依赖 cosmic_text 在 shaping 内部**自动逐字形回退**，不存在「汇报缺失 → 稍后重排」这条回路。
- **进而是两套契约**：`FontDB::select_font(family_id, Properties)` / `fallback_fonts(char, font_id)` 与 gpui 的 `font_id(&Font)` / 自动回退语义并不对应。

**结论：hicodeer 的字体/文本子系统与 warp 接口不匹配，不能直接使用。** 它的价值是「证明后端栈可在 OHOS 跑」与「给出 OHOS 字体枚举配方」（见 11.8.6）。

**一处需纠正的既有认知**：向导第 09/10/11 章描述的方案（`OH_Drawing_FontParser` 枚举 + `libnative_drawing` 光栅化 + 回退学习缓存）对应的是**旧 warp-ohos** 的实现，**不是 hicodeer**——hicodeer 实际走 cosmic-text + swash。

### 11.8.4 正确基座：warp 自带的 cosmic-text 实现（实测）

`crates/warpui/src/windowing/winit/fonts.rs`：

- **`impl platform::FontDB for FontDB`**；**`impl platform::TextLayoutSystem for TextLayoutSystem`**（`layout_line`、`layout_text`）。
- **构造并写入 `caret_positions`**，`runs` / `Glyph.width` 齐备——即「汇报 layout 结果并物化」**warp 自己已经实现**。
- 后端与 hicodeer 同源：`cosmic_text`（warpdotdev fork，`crates/warpui/Cargo.toml` rev `a7c7b71497542758e08f77390b8efce543b3181f`）+ `fontdb`（经 `resvg::usvg::fontdb`）+ `swash`（`windowing/winit/fonts/swash_rasterizer.rs`）+ `font_kit`（`crates/warpui/src/fonts/font_kit.rs`，子像素光栅化）。
- **不依赖 winit 类型**（检索 `winit::` / `WindowHandle` 仅命中自身路径），平台相关的只有**字体加载器**：`fonts/linux.rs`（fontconfig `FontconfigLoader`）、`fonts/windows.rs`，以及 `cfg(not(any(linux, freebsd, windows)))` 的 stub loader（直接 bail「尚未实现」）。

**结论：字体/文本不是「最大的障碍」。** warp 已有一份接口天然对齐、且已物化 `caret_positions` 的 cosmic-text 实现；要做的是把它从 `#[cfg(winit)]` 迁到 `platform/ohos/`，并只重写字体加载器。

### 11.8.5 落地清单（计划新增）

需迁移或新增到 `crates/warpui/src/platform/ohos/`：

1. `fonts.rs`：`FontDB` + `TextLayoutSystem` 主体（源自 `windowing/winit/fonts.rs`），内含 OHOS 版 `LoadedSystemFonts` 与 `process_loaded_system_fonts`。
2. `fonts/text_layout.rs`：`RunBuilder` + `TextStylesMap`（cosmic_text 样式映射）。
3. `fonts/swash_rasterizer.rs`：swash 光栅化。
4. `fonts/font_handle.rs` + `fonts/str_index_map.rs`：字体句柄与字节↔字符索引映射。
5. `fonts/ohos.rs`（**新写**）：OHOS 系统字体枚举/加载器，替代 `linux.rs` 的 `FontconfigLoader`，对接 `FontDB::load_all_system_fonts` 的异步骨架。
6. 复用既有的 `crates/warpui/src/fonts/font_kit.rs`（`#[cfg(native)]`，OHOS 下会编译）：子像素光栅化器。

逐 trait 的 OHOS 来源：

- `load_from_bytes` / `load_from_system` / `family_id_for_name` / `load_family_name_from_id` / `select_font` / `font_metrics` / `glyph_advance` / `glyph_typographic_bounds` / `glyph_for_char` / `glyph_raster_bounds` / `rasterize_glyph` / `text_layout_system` → **沿用 warp 已有的 cosmic-text 逻辑**，仅字体来源换成 OHOS 加载器。
- `load_all_system_fonts` / `process_loaded_system_fonts` → **重写**：目录扫描 + 元数据解析，产出 `LoadedSystemFonts`。
- `fallback_fonts(char, font_id)` → 沿用 warp 逻辑（按优先级返回候选），候选集合来自 OHOS 枚举结果。

### 11.8.6 hicodeer 可复用清单（OHOS 字体枚举配方）

从 `ohos/text_system.rs` 可直接借用的 OHOS 专属细节：

- **字体目录**：`/system/fonts`、`/system/font`、`/vendor/fonts`、`/system_ext/fonts`、`/data/fonts`（`is_dir()` 过滤后 `load_fonts_dir`）。
- **回退族名**：`HarmonyOS Sans`、`HarmonyOS_Sans`、`sans-serif`、`Noto Sans`。
- **族名归一化**：`normalize_family_name`（trim + 小写 + `_`/`-`→空格 + 折叠空白）。
- **emoji 判定**：postscript name == `NotoColorEmoji`。
- **后端栈已在 OHOS 验证**：`cosmic_text` / `swash` / `fontdb` / `font_kit` 均已在 hicodeer 上跑在 OHOS，说明这组依赖可编译、可运行。

### 11.8.7 风险项（含实测结果）

**已实测排除（2026-09-23）**：以最小 crate 隔离验证——在临时目录下建空 lib crate，仅声明 warpui 在 OHOS（`target_os = "linux"`）会拉入的字体依赖集合，用 `RUSTUP_TOOLCHAIN=1.97.1` 对 `aarch64-unknown-linux-ohos`（本机 host 即 target，属原生编译非交叉）跑 `cargo check` 与 `cargo build`，**两者均 exit=0**（各约 43s / 54s）。

通过清单：`cosmic-text`（warpdotdev fork rev `a7c7b714`）、`font-kit`（warpdotdev fork rev `a04b225e`，features `source` + `source-fontconfig-dlopen`）、`fontconfig` 0.8.0（dlopen）、`fontdb` 0.23、`owned_ttf_parser` 0.25、`resvg` 0.47（连带 `usvg` / `tiny-skia`）、`bimap` 0.6、`dashmap` 6、`memmap2` 0.9；传递依赖 `swash` 0.1.19、`freetype` 0.7.2、`skrifa` / `read-fonts` / `rustybuzz` 亦全过。

**重点**：OHOS 的 `target_os = "linux"` 会同时命中 `cfg(not(target_os = "macos"))` 与 `cfg(any(target_os = "linux", target_os = "freebsd"))`，故 `fontconfig` 与 `source-fontconfig-dlopen` 也是必过项——实测确认无碍（此前是最可疑项）。

实测中的一个环境坑：首轮曾报 `Text file busy (os error 26)`（hmdfs 上 cargo 写完 build script 即 exec，存在写句柄未释放的时序窗口；同一文件手动执行与重跑均正常，属瞬态）。构建脚本应对此有容错（重试或降并发）。

1. ✅ **warp 的 cosmic-text fork**（`warpdotdev/cosmic-text` rev `a7c7b714…`）在 `aarch64-unknown-linux-ohos` 编译通过——此前顾虑 hicodeer 用的是上游 cosmic-text、版本不同，实测证明 fork 本身无障碍。
2. ✅ **其余依赖**在 OHOS target 编译通过：`font_kit`、`owned_ttf_parser`、`resvg`、`fontdb`、`dashmap`、`bimap`、`fontconfig`。
3. ⬜ **OHOS 实际字体目录内容**与 hicodeer 假设的一致性（族数、CJK 覆盖、emoji 字体是否存在）——须设备侧验证。
4. ⬜ **异步加载骨架的落地**：`load_all_system_fonts` 返回 `BoxFuture`，需确认其在 OHOS 下的执行线程，与「NAPI 只在创建线程」「主线程不可阻塞」两条约束协同。
5. ⬜ **光栅化路径选择**：`font_kit` 子像素光栅化在 OHOS 的可用性——warp 有 `fontkit-rasterizer` feature 分支（`swash_rasterizer` 与之互斥），需实测后选定主路径。
6. ⬜ **维护约定**：OHOS 副本源自 `windowing/winit/fonts.rs`（受保护文件，**不改它**），需在方案中写明后续与上游同步的约定，避免两份实现漂移。

### 11.8.8 与 11.4.3 的关系

11.4.3 原表述为「需自研，参照 mac + hicodeer」，**现予修正**为「基座是 warp 自带的 winit cosmic-text 实现，hicodeer 仅提供 OHOS 字体枚举配方」。落地工作量的性质由「从零自研」下调为「移植 warp 自有实现 + 新写一个字体加载器」。

### 11.9 终端 shell 的 OHOS 落地方案：预制 private `zsh.hnp`

> **2026-09-25 更新**：终端的**启动 shell 已改为 `hitshell`**——连上 hitdaemon 就得到系统权限的会话，连不上则 `exec` 本节这个 zsh 兜底。本节描述的 zsh 与 HNP 机制仍是兜底路径的基础，桥的完整设计见 11.11。

**目标**：预制一个 private `zsh.hnp` 随 HAP 分发，让 terminal 模块直接 `fork` + `exec` 本地 zsh——不经 SSH 桥，也不依赖系统预装。

#### 11.9.1 为什么必须是 HNP

- OHOS 系统镜像不预装 bash/zsh/fish，只有 toybox 的精简 `sh`。
- toybox sh 没有 `precmd_functions` / `preexec_functions` 这类 zsh 钩子机制，而 warp 的终端工作流（命令边界、提示符就绪、背景块、行编辑器状态）全部建立在 DCS 钩子之上——用 toybox sh 意味着整条钩子链路要在 Rust 侧重做。
- 换成真正的 zsh，钩子链路（`zsh_body.sh` 走 `precmd_functions` + `preexec_functions`）**完全复用**，差别只是换一个 shell 二进制。
- HNP 允许随包分发原生可执行：`private` 类型装到 `/data/app/bin`，**应用进程可直接 exec**（见第 04 章「方案四：本地 HNP 直 exec」）。

#### 11.9.2 zsh 的取得方式：直接取用现成的 `/usr/bin/zsh`（不自行编译）

本机已有现成产物，**无需自己交叉编译**：

- 路径 `/usr/bin/zsh`（`root:root`，`0755`，约 1.33 MB）。
- 实测形态：`ELF 64-bit LSB arm64, dynamic`，interpreter `/lib/ld-musl-aarch64.so.1`，版本串 `zsh 5.9 (aarch64-unknown-linux-musl)`——与第 18 章记载的那份同源。
- **动态依赖实测为三个（并非"仅 libc"）**：`libncursesw.so.6`、`libtinfo.so.6`、`libc.so`。前两者位于 `/usr/lib`（erofs 只读系统分区），interpreter 经 `/lib` → `/system/lib` 解析。
- zsh 二进制**无 RPATH / RUNPATH**，库解析依赖系统默认搜索路径（musl 的 `/lib` 与 `/usr/lib`）。
- `/bin` 在本机是 `/system/bin` 的符号链接，故 `/bin/zsh` 不存在——这正是 warp 写死的 `ZSH_SHELL_PATH = "/bin/zsh"` 在 OHOS 上命不中的原因。

#### 11.9.3 HNP 打包要点（三个实测坑，第 04 章）

- **包内 `bin/` 文件必须带执行位**：hnp 是 zip，安装时按条目 UGO 权限原样落盘；丢了 x 位则 ELF 落 `0644`，`exec` 报 `permission denied`（见 10.4 第 9 步的权限位重写）。
- **ncurses 依赖：直接用系统库，不随包（2026-09-23 决定）**：实测系统确有全部依赖——`libc.so` 在 `/system/lib64`、interpreter `ld-musl-aarch64.so.1` 在 `/system/lib`、`libncursesw.so.6` 与 `libtinfo.so.6` 在 `/usr/lib`。故 `zsh.hnp` **只放 zsh 二进制**，三个依赖库全部由系统提供，**不随包、不设 `LD_LIBRARY_PATH`**。
  **依据**：`git.hnp` 内含的 git 亦只依赖 `libc.so`、未自带任何库即可运行，说明系统库对应用进程可用。
  **风险提示（未实测）**：`libncursesw` / `libtinfo` 只存在于 `/usr/lib`（不在 `/system/lib`、也不在 `/lib`），该路径在应用沙箱内的可读性尚未验证。若实测发现沙箱读不到 `/usr/lib`，回退方案是把这两个 `.so`（合计约 390 KB）放进 `<pkg>/lib/` 并设 `LD_LIBRARY_PATH`。
- **不要指望 `chmod`**：staging 若在 hmdfs（`/storage`）上，权限位会被改写（other 位恒为 0），而设备端安装器只看 other 执行位决定 0755/0744。
- **声明改动必须重新打包**：`module.json5` 的 `hnpPackages` 改动需重跑 `hvigorw assembleHap`，手工改旧 HAP 无效。

#### 11.9.4 terminal 侧调用链（原生机制足够，2026-09-25 起归类表加 `hitshell` 一项）

warp 的 shell 解析在 `crates/warp_terminal/src/local_tty/shell.rs`（**受保护文件，不改**）；名字归类表所在的 `crates/warp_terminal/src/shell/mod.rs` 同属受保护文件，2026-09-25 因新增 `hitshell` 归类获授权改过一处（见 11.11）。其既有机制已足够：

- 优先读环境变量 `$WARP_SHELL_PATH`（实现见 `crates/warp_util/src/path.rs`，即 `env::var("WARP_SHELL_PATH")`）；设置后即用之，**但无效会 panic**（`shell.rs`）。
- 否则回退：passwd 的 shell → `ZSH_SHELL_PATH`（`/bin/zsh`）→ `BASH_SHELL_PATH`（`/bin/bash`）→ `FISH_SHELL_PATH`（`/bin/fish`）。注意这三者是**写死的绝对路径常量**，不走 `PATH` 查找。
- `supported_shell_path_and_type` = `resolve_executable` + `parse_shell_type_from_path`（按**文件名**匹配 `ShellType::from_name`）。
- `resolve_executable` 对含分隔符的绝对路径直接做 `file_exists_and_is_executable` 判定——**要求执行位**。

**落地方案**：OHOS 入口在 `run()` 早期设置 `WARP_SHELL_PATH`。原方案指向 `/data/app/bin/zsh`，terminal 侧解析为 `ShellType::Zsh` 后直接 exec；**2026-09-25 起改为指向 `/data/app/bin/hitshell`**（见 11.11），由 hitshell 决定这次会话的实际执行者。

**注意**：因该变量无效即 panic，赋值前须确认 HNP 已就位；若无法保证，应改走「不设置该变量 + 自有可降级分支」，不可盲目设值。

#### 11.9.5 与 native child 的边界

- private HNP（`/data/app/bin`）**仅应用自身 mount 可见**，只能由**主进程** `fork` + `exec`；`OH_Ability_StartNativeChildProcess` 拉起的 native child 与之 mount namespace 不同，看不到该路径（第 04 章实测 `ENOENT`）。
- 这决定终端的本地 zsh 会话必须由主进程起，不能交给 native child 去 exec。

#### 11.9.6 与 SSH 桥的关系

- 本地 zsh HNP 与 cmd-agent/SSH 桥是**分工而非替代**：shell 会话本地化直 exec；需要 exec 系统程序的场景（node、语言服务器、chmod 等）仍走桥。
- **2026-09-25 落地后**这条关系有了实体：桥即 `hitshell` + `hitdaemon`（见 11.11），且 terminal 的启动 shell 就是 `hitshell`——连得上走桥、连不上落回本地 zsh，「分工」变成同一次启动内的自动择路。
- 12.2 第 6 步与 12.3 的相关条目据此收敛。

#### 11.9.7 待实测

> **2026-09-25 状态**：第 1 条已随终端落地跑通（设备上 zsh 会话可起、`TERMINFO` 已锚定、钩子链路正常）；第 2 条按「系统库可用」处理且未再复现问题；第 3 条已达成；第 4 条已被 11.11 取代——不再考虑省掉 HNP。

- private `zsh.hnp` 装入后 `/data/app/bin/zsh` 的执行位与 exec 实测。
- 应用沙箱内能否读到 `/usr/lib` 下的 `libncursesw.so.6` / `libtinfo.so.6`（当前按「系统库可用」处理、不随包；若读不到则按 11.9.3 的回退方案随包 + `LD_LIBRARY_PATH`）。
- zsh 在 OHOS 上加载 `zsh_body.sh` 钩子（precmd/preexec）与 warp DCS 链路的端到端验证。
- （可选：省 HNP 的验证）`/usr/bin/zsh` 能否被应用沙箱**直接 exec**——第 04 章判据是「`/bin` 之外的外部程序一律失败，须走 private HNP」，`/usr` 是否例外未验证；若可，则连 `zsh.hnp` 都可省。

---

## 11.10 起手实施实测：入口 crate 与 `script/ohos`

本节回填 12.2 第 1 步「最小可运行基线」的落地物与首次编译实测结论。

### 11.10.1 已落地物（均为新增，未触碰受保护文件）

- `crates/entry_ohos/Cargo.toml`：OHOS 入口 crate，`[lib] name = "warp"` + `crate-type = ["cdylib"]`，产物即 `libwarp.so`。
- `crates/entry_ohos/build.rs`：`napi_build_ohos::setup()`（仅 `target_env = "ohos"` 时执行）。
- `crates/entry_ohos/src/lib.rs`：crate 级 `#![cfg(target_env = "ohos")]` + 两个 no-op 符号（`pthread_mutexattr_setrobust` / `pthread_mutex_consistent`，OHOS libc 缺而 std 引用）。
- `crates/entry_ohos/src/launch_app.rs`：`#[ability] launch_app` → `set_global_app` → 环境准备（`WARP_SHELL_PATH`、`HOME`、`PATH`；该变量 2026-09-25 起为 `/data/app/bin/hitshell`，见 11.11）→ `ChannelState::set`（复刻 `app/src/bin/oss.rs`）→ `warp::run()`。
- `script/ohos/bundle`：OHOS 构建入口，TLS 垫片 → cargo build → 拷 so → ArkTS 同步 → ohpm → hvigor → HNP 注入 → 重签。
- `script/ohos/ohos-tls-shim.c`：TLS 垫片源码，与 `HiCodeer/script/ohos-tls-shim.c` 逐字一致（md5 `2ff6c83a96733e76c8b24b5dccae7000`）。
- `script/ohos/bootstrap`：工具链存在性检查（不安装任何东西：OHOS 工具链由设备镜像提供）。
- `script/ohos/install_build_deps`：转调 `bootstrap`。
- `script/ohos/read-sign-pwd.js`：从 `build-profile.json5` 的密文解出签名口令（照 HiCodeer）。

**无需改 `app/Cargo.toml`**：根 `Cargo.toml` 的 `members = ["crates/*", "app"]` 是 glob，新 crate 自动入 workspace；入口 crate 作为独立 cdylib 依赖 `warp`（app 的 rlib）即可，与 HiCodeer 的 `launch-zed` 同形。两个 lib 同名 `warp` 因 crate-type 不同（rlib / cdylib）而产物不冲突（`libwarp.rlib` vs `libwarp.so`）。

HNP 载荷为**预制**（`hap/entry/hnp/arm64-v8a/` 下的 `git.hnp` + `zsh.hnp`），故 OHOS 脚本**不需要** `hnpcli pack`，只做「注入 + 重签」。

### 11.10.2 框架接入状态

- ✅ `script/bundle`：新增 `elif HarmonyOS → ./script/ohos/bundle`。
- ✅ `script/run`：在参数解析**之前**拦截 HarmonyOS 并 `exec ./script/ohos/bundle "$@"`（OHOS 无 `cargo run` 语义，对应的「运行」即构建可安装 HAP）。
- ⬜ `script/bootstrap`：分发分支尚未接（待授权），当前 `script/ohos/bootstrap` 只能直接调用。

### 11.10.3 首次编译实测（2026-09-23）

命令：`./script/ohos/bundle --check-only`（等价于 `LD_PRELOAD=<TLS 垫片> RUSTUP_TOOLCHAIN=1.97.1 cargo check -p entry_ohos --target aarch64-unknown-linux-ohos`）

- 结果：**EXIT=101，耗时 5m47s**。
- 前置链路全部通过：工具链断言（`rustc host == aarch64-unknown-linux-ohos`，实测 `rustc 1.97.1`）、TLS 垫片编译、`openharmony-ability` git 依赖解析。
- 中断点：**第三方 crate `nix 0.26.4` 编译失败，9 个错误**。依赖树在它处被切断，因此**报错面尚未展开到 warp 自身代码**。

- 1. E0425 `O_FSYNC` 不存在（`fcntl.rs`）：OHOS libc 未定义该常量。
- 2–5. E0425 `__fsword_t` 类型不存在（4 处，`sys/statfs.rs`）：glibc 内部类型名，OHOS 无。
- 6. E0425 `XFS_SUPER_MAGIC` 不存在（`sys/statfs.rs`）：同上。
- 7. E0425 `ST_RELATIME` 不存在（`sys/statvfs.rs`）：同上。
- 8. E0308 `cmsg_len` 类型不匹配（`u32` vs `usize`，`sys/socket/mod.rs`）：`cmsghdr.cmsg_len` 宽度与 glibc 不同。
- 9. E0004 `SigevNotify::SigevThreadId` 未覆盖（`sys/signal.rs`）：OHOS 定义了 `SIGEV_THREAD_ID`，枚举多一个变体。

**根因归纳**：`nix` 用 `cfg(target_os = "linux")` 圈定 Linux 实现，而 OHOS 的 `target_os` 恰是 `linux`、`target_env` 是 `ohos`，于是 nix 按 glibc 的 libc 定义编译 OHOS，而 `libc` crate 为 OHOS 提供的是 musl 血统的绑定——两边假设不一致。这与第 11.1 节「`target_os = "linux"` 使 linux 分支默认激活」的头号风险同源，只是这次触发它的是上游 crate 而非 warp 自身。

**影响面**：`nix` 在 workspace 中被 **11 个 crate** 直接依赖（`ai`、`app-installation-detection`、`build_cache`、`computer_use`、`integration`、`lsp`、`warp_ripgrep`、`warp_terminal`、`warpui`、`app` 等），版本由根 `Cargo.toml` pin 在 `0.26.4`。`warp_terminal` 用它做 PTY，`warpui` / `app` 用它做进程与信号——**它是 OHOS 适配的结构性门槛，必须先解决才能继续展开报错面**。

### 11.10.4 待决方案（已选定：本地 path patch）

1. **升级 `nix`**：0.28 / 0.29 已在 `Cargo.lock` 中（由其他依赖引入），但新版本仍走 `cfg(target_os = "linux")` 分支，预期同样失败；需实测确认。
2. **为 OHOS 提供 nix 补丁**：把 9 处差异修在一份 fork 上并以 git 依赖引入。注意：项目约束禁止用 `[patch.crates-io]` 替换依赖来绕过编译问题，此路需明确授权。
3. **在 OHOS 上绕开 `nix`**：把 `warp_terminal` 的 PTY、`warpui` / `app` 的进程与信号调用改为直调 `libc`（OHOS 分支），让 nix 仅在非 OHOS 平台生效。改动落在 warp 自己的代码里，最正规但工作量最大。

**最终选定：方案 2 的本地化变体——本地 path patch（已落地）**。根 `Cargo.toml` 新增三条 path 依赖，指向 `patches/` 下随仓库提交的上游 crate 副本：`nix`、`gettext-sys`、`interprocess`（后者两者同样在 OHOS 上按 glibc 假设编译而失败）。采用 path patch 而非 git fork 的原因是这些修复是本地性的、未向上游提交。选择依据是改动最小且**不触碰 warp 自身业务代码**；方案 3 虽最「正规」，但需改写 `warp_terminal` 的 PTY 与 `warpui`/`app` 的进程/信号调用，爆炸半径远大于两处上游 crate 的定点修复。

同批引入的还有 `ohos-*` 系列原生绑定：根 `Cargo.toml` 以 `git = "https://github.com/jaffenqqcom/ohos-native-bindings-zed.git"`（默认分支）引入，而非 crates.io 版本。原因有二：一是 API 面，`openharmony-ability` 是按该 fork 写的（`off_frame_callback`、当前的 `on_ui_input_event` 签名），crates.io 版本没有；二是版本解析，`[patch.crates-io]` 只替换**同版本号**，把 patch 钉在比 crates.io 更旧的 tag 上会让两份副本同时留在依赖图里，它们各自定义的 FFI 类型（`OH_NativeXComponent`、`ArkUI_Node`）随之变成不同类型而报错；默认分支的版本不低于 crates.io，patch 直接生效、每个 crate 只存在一份。

### 11.10.5 本节结论

- 12.2 第 1 步的**构建链路已就位**（入口 crate、TLS 垫片、脚本框架、HNP 载荷、签名材料齐备），`bootstrap` 实测通过。
- 但**报错面调查被 `nix` 阻断**——它先于任何 warp 自身代码失败，属「前置门槛」而非「适配点」。
- 下一步应先解决 `nix`，再重跑 `--check-only` 以取得 warp 依赖树的完整报错清单。

> 上述阻断已于同批解决，报错面已完整展开并清零，见 11.10.6。

### 11.10.6 OHOS 平台后端落地与全仓编译通过（2026-09-23）

本节回填 12.2 第 2 步（补齐 warpui OHOS 平台后端）的落地结果与该阶段首次全仓编译通过实测。

**实测命令与结果**

- 命令：`./script/ohos/bundle --check-only`（等价于 `RUSTUP_TOOLCHAIN=1.97.1 cargo check -p entry_ohos --target aarch64-unknown-linux-ohos`，含 TLS 垫片预载）。
- 首次全量：`Finished dev profile ... in 6m 20s`，**零错误**，覆盖 `warpui`（本次新写的平台后端）→ `warp`（app crate）→ `entry_ohos`（入口 crate）。
- 格式化后增量复跑：`Finished dev profile ... in 53.07s`，同样零错误。
- 续接改动（TLS 垫片改名 `rust-tls-shim.so`、`bundleName` 统一、hilog 重定向）后增量复跑：`Finished dev profile ... in 1m 10s`，仍零错误。
- 至此 12.2 第 1、2 步的**编译面**达成：最小可运行基线的构建链路 + warpui OHOS 平台后端均通过编译。

**新增物（全部落在路径含 `ohos` 的文件内）**

- `crates/warpui/src/platform/ohos/app.rs`：`spawn()` 与 `App`（即 `platform::current::App`）。
- `crates/warpui/src/platform/ohos/event_loop.rs`：跨线程事件队列与 `Translator`。
- `crates/warpui/src/platform/ohos/windowing.rs`：`platform::WindowManager` / `platform::Window` 实现与 wgpu surface 绑定。
- `crates/warpui/src/platform/ohos/delegate.rs`：`platform::Delegate` / `DispatchDelegate` 实现。
- `crates/warpui/src/platform/ohos/mod.rs`：模块声明与 `App` / `spawn` / `open_url_in_system` 再导出，使 `platform::current::App` 得以解析（这是此前最后两个编译错误的根因）。

**关键设计结论（实测确立）**

- **两线程模型**：ArkTS（ability）线程只做事件**生产**，绝不阻塞；warp 的阻塞事件循环跑在 `spawn()` 起的独立 `warp-main` 线程上做**消费**，两者以 `std::sync::mpsc` 队列解耦。`openharmony-ability` 的 `run_loop` 把闭包转成 `'static + Send + Sync` 后存在单线程 cell 里、由 ability 线程驱动，因此 `run_loop` 必须在 ability 线程调用（`event_loop::register` 负责此事）。
- **渲染零补丁**：wgpu 30 的 GL/EGL 后端已接受 `RawWindowHandle::OhosNdk` 并用其 `native_window` 指针建 EGL window surface（`wgpu-hal` 的 `create_surface` 识别该句柄）；因不提供 display 句柄，wgpu 落到 `WindowKind::Unknown` 分支，正好走 EGL 路径。因此 **warp 侧无需对 wgpu 打补丁**，仅需把 ability 的 `RawWindow` 包成实现 `HasWindowHandle` / `HasDisplayHandle` 的 surface target（`OhosSurfaceTarget`）。
- **渲染驱动**：由 XComponent 的帧回调（`on_frame_callback` → `Event::WindowRedraw`）驱动；帧处理与 winit 后端同序——先 `update_size_if_needed` 让 swap chain 尺寸跟上，再 `build_scene`（仅当无缓存 scene），最后 `render(scene, font_cache)`。
- **按键输入**：`ohos-native-bindings-zed` 的 `KeyEventData` 已不带修饰键状态，故修饰键由 `keycodes::Modifiers` 跨事件累计，并在失焦时清空（否则修饰键会卡住）；`key_without_modifiers` 走「不按 shift 的键名」。

**hilog 日志重定向（本批新增，方法与文件清单见 3.3）**

- 新增 `crates/warp_logging/src/ohos.rs`：`HilogLogger`（`impl log::Log`）包住 `env_logger::Logger`，把同一批记录镜像到 hilog，级别过滤与文件 sink 原样留用；FFI 直调 `OH_LOG_Print`（`#[link(name = "hilog_ndk.z")]`），tag `diag`、domain `0x0001`、级别映射 DEBUG/INFO/WARN/ERROR = 3/4/5/6。另导出 `direct_hilog(&str)` 供启动早期直投。
- 接线落在 `crates/warp_logging/src/native.rs`（受保护文件，只加独立行）：新增 `#[cfg(target_env = "ohos")] #[path = "ohos.rs"] pub(crate) mod ohos;`；`init_internal` 末尾用 two-line cfg 把 `base_logger.init()` 换成 `ohos::init_hilog_logger(base_logger.build())`。`crash_reporting` 构建不参与——`sentry_log::SentryLogger` 已占用唯一的全局 logger 槽，且该 feature 不在 app 默认集内，OHOS 构建不会启用。
- 其余接线：`crates/warp_logging/src/lib.rs` 加一行 `pub use imp::ohos::direct_hilog;`；`crates/entry_ohos/Cargo.toml` 加 `warp_logging` 依赖；`crates/entry_ohos/src/launch_app.rs` 在 `launch_app` 入口与 `prepare_process_environment` 收尾各加一条 `direct_hilog`（这两处早于 `warp::run()` 安装 logger，`log::` 宏此刻必然被丢弃）。
- 抓取命令：`timeout 5 hdc hilog 2>&1 | grep "diag"`。**设备侧尚未实测，「hilog 能出日志」这一条仍待设备确认。**

**同时改动（非 OHOS 路径文件，均为「只加一行 cfg」）**

`not(target_env = "ohos")` 的成对包裹按第 19 章模式补到了 app crate：`settings/mod.rs`（`mod linux` 与 `pub use linux::*`）、`settings/init.rs`、`settings_view/features_page.rs`（导入、枚举变体、匹配臂、字段与其初始化、两个 widget 项）、`workspace/view.rs`、`crash_recovery.rs`、`test_util/settings.rs`、`debug_dump.rs`。原因是 OHOS 的 `target_os = "linux"` 会激活这些 Linux 专有项，而 `warpui::platform::linux`（winit 系 Linux 后端）在 OHOS 上已被门控掉。`crash_recovery.rs` 的 `RecoveryMechanism::X11` 匹配臂保持存在（`X11` 变体来自 `warp_cli`，在 OHOS 上仍存在，删臂会破坏穷尽性），只把臂内依赖 Linux 设置的语句块包进门控。

`warp_logging` 内另有两处同类改动（路径不含 `ohos`，同样只加独立行）：`native.rs` 的 `init_internal` 末尾一行 cfg 门控，与 `lib.rs` 的一行 re-export（见 3.3）。

**两条必须留痕的实测事实**

- `script/ohos/bundle` 里导出 `RUSTFLAGS="${RUSTFLAGS:-} -A warnings"`，**OHOS 构建默认屏蔽全部警告**。因此「`--check-only` 无警告输出」不能当作代码干净的证据，警告检查需另行手动执行。
- 本机 host 即 `aarch64-unknown-linux-ohos`，且未安装任何非 OHOS target/toolchain，**无法在本机交叉编译验证其它平台**。「不改其它平台」只能靠 cfg 等价性证明：所有新增门控都是 `not(target_env = "ohos")` 谓词，在非 OHOS 目标上恒真，故展开后的 token 流与原状逐字相同；`build.rs` 的 `winit: { not(any(macos, ohos)) }` 在非 OHOS 上等价于原 `not(macos)`，`wgpu` 的 `any(winit, ohos, ...)` 等价于原 `any(winit, feature = ...)`；`windowing/mod.rs` 的 `any(winit, ohos)` 在 macOS 上仍为假（该模块原本在 macOS 上就未编译）。

**格式化**

仓库的 `script/format` 在本机不可用：`rustfmt` 组件未装于 `1.97.1-aarch64-unknown-linux-ohos`，而 `rust-toolchain.toml` 钉的 `1.92.0` 未安装。改用 `rustup run stable-aarch64-unknown-linux-ohos rustfmt` 并带上项目同款配置（`imports_granularity=Module`、`group_imports=StdExternalCrate`）对本次改动文件就地规整；app 侧文件复检为「本就合规」。

**仍未完成（承接 12.2）**

- 12.2 第 3 步起：设备侧跑通（进事件循环、出一帧、hilog 可观测）、输入与 IME、终端与进程。hilog 重定向已在代码侧接通，但「设备上确实能抓到这行日志」与 `hilog_ndk.z` 的实际链接结果都**只能在设备上确认**。
- 6.5.3 第 4 项的 app 层 `run_internal` OHOS 分支与 `app/src/platform/ohos.rs` 尚未补。
- 启动期 `color_mode` 缺口：OHOS 只在配置**变化**时投递 `ConfigChanged`，启动时拿不到当前明暗模式（ArkTS 侧 `EntryAbility.ets` 调的是 `setColorMode(COLOR_MODE_NOT_SET)`），故 `system_theme()` 在首帧只能保守返回 Light。
- TLS 垫片已改名为 `rust-tls-shim.so`（`script/ohos/bundle` 的 `SHIM_SO` 改为字面量，不再随 `NAPI_MODULE_NAME` 变化），陈旧产物 `warp-tls-shim.so` / `core-tls-shim.so` 已清理。
- `hap/AppScope/app.json5` 的 `bundleName` 拼写已修（`com.hiwarp.treminal` → `com.hiwarp.terminal`），与 `hap/build-profile.json5` 一致；`hap/` 内 HiCodeer 残留仍待清理。

> **2026-09-24 补记**：本节「仍未完成（承接 12.2）」列的前三项均已达成——设备侧已跑通（进事件循环、出帧、hilog 可观测，抓取命令见 3.3），输入与 IME 已接，终端本地 zsh 会话可跑（见 11.9）。平台能力补齐的最终裁决与实现见 12.4。

## 11.11 `hitshell` 与 `hitdaemon`：沙箱内终端的权限桥（2026-09-25 落地）

**要解决的问题**：private HNP 让终端能在沙箱内 exec zsh（11.9），但会话身份仍是应用沙箱（实测 uid `20020231`）。系统命令行那套环境——`/data/service/hnp` 下别的 public 包、系统命令行装上去的工具——沙箱进程既看不到也够不着。而应用无权拉起沙箱外的进程，所以「让终端拿到系统权限」只能靠一个**由用户在系统命令行手动启动**的守护进程。

**两个组件**（`cmdbridge/` 下的两个独立 crate，均不属于仓库 workspace，也不依赖任何 warp crate，因此改动它们不会触发主程序重编）：

- `hitdaemon`：**public** HNP。跑在沙箱外，监听回环端口，是一个只认编译进去的管理密钥的 SSH 服务端；收到会话请求后以它自己的账号 `exec` 一个 shell。它必须由用户从系统命令行启动，这正是高权限的来源。
- `hitshell`：**private** HNP，装到 `/data/app/bin/hitshell`，是 terminal 的**启动 shell**。它连上 hitdaemon、请求一个远端 pty，然后双向中继字节与窗口尺寸；连不上时不再退出，而是**把自己替换成沙箱内的 zsh**。

**为什么启动 shell 是 hitshell 而不是 zsh**：两条路都由「用户开一个终端」触发，不需要用户先判断 daemon 在不在。

- hitdaemon 在跑 → 用户开一个终端就自动拿到系统权限的会话，无需额外操作。
- hitdaemon 没跑 → hitshell 打印原因后 `exec` 掉自己，换成 private `zsh.hnp` 的 zsh，终端照常可用。

**参数来源：不写死，全部转发**（用户明确要求）：

- terminal 对 `ShellType::Zsh` 会启动 `<shell_path> -c "exec -a -zsh '<shell_path>' <参数...>"`（见 11.9.4），故 hitshell 收到的是 `-c` 加该命令串。
- hitshell 用 `split_shell_words` 把命令串拆回词表，取出 `-a` 的 argv0 与 `<shell_path>` 之后的**全部**参数，两条路都用这一份：经 hitdaemon 起 zsh 时原样转发，本地兜底时原样交给 zsh。参数集本身不硬编码。
- 只有手工执行（无 `-c`）时才回落到默认 `-g --no-rcs`。默认值存在的理由同 11.9：`--no-rcs` 避开设备 `/etc/zshrc` 里会劫持 ZLE 的系统 shell 插件。
- argv0 只有本地兜底能真正生效（走 `CommandExt::arg0`，实测终端里 zsh 的 argv[0] 就是 `-zsh`）；hitdaemon 那条路经 `sh -c` 起 shell，设不了 argv0——这与 hitshell 改造前的既有行为一致。

**「连不上」的判据与兜底行为**：

- 判据就是 `wait_ready` 失败（回环端口拒绝连接，或握手超时），与旧版打印 `hitdaemon is not ready, start hitdaemon from the system command line first. 请先在系统终端工具启动hitdaemon程序。` 的条件完全相同。
- 兜底时**先原样打印那条消息**（用户要求保留：它是唯一的补救指引），再 `exec /data/app/bin/zsh` 并带上转发的参数。`exec` 是进程替换，pid 与 pty 都不变，所以这条路径与「终端直 exec zsh」完全等价。

**terminal 侧的接线**：

- `crates/entry_ohos/src/launch_app.rs`：`TERMINAL_SHELL_PATH` 由 `/data/app/bin/zsh` 改为 `/data/app/bin/hitshell`，`WARP_SHELL_PATH` 随之指向它。
- **`TERMINFO` 的推导必须继续锚定 zsh**：`point_shell_at_bundled_terminfo()` 原用 `canonicalize(TERMINAL_SHELL_PATH)` 反推 HNP 根再取 `share/terminfo`，而 hitshell 包里没有 terminfo，沿用会把 `TERMINFO` 丢掉、zsh 重绘与回显再次出问题。故新增 `ZSH_SHELL_PATH = /data/app/bin/zsh` 专供该推导。
- `crates/warp_terminal/src/shell/mod.rs`（**非 OHOS 路径、属受保护文件，2026-09-25 已获授权**）：`ShellType::from_name` 只认 bash/zsh/fish/pwsh 的文件名，hitshell 会落到 `None`，而 `WARP_SHELL_PATH` 无效即 panic。故新增一个匹配分支，判据常量按 `#[cfg]` 分派——`target_env = "ohos"` 时为 `["hitshell"]`，其它平台为空切片，对非 OHOS 平台零影响（CODEBUDDY.md 第 10 条要求非 OHOS 文件的改动必须用 `target_env = "ohos"` 包裹）。

**打包与注入**：

- 新脚本 `script/ohos/cmdbridge`：分别构建两个 crate（各自独立 `CARGO_TARGET_DIR`，避免互相驱逐指纹）、stage（`<pkg>/bin` + `hnp.json`）、`hnpcli pack`，并重写 zip 中央目录的权限位（同样的 hmdfs 权限坑，见 11.9.3 与 10.4 第 9 步）。
- `module.json5` 的 `hnpPackages` 增加 `hitdaemon.hnp`（public）与 `hitshell.hnp`（private）；载荷落 `hap/entry/hnp/arm64-v8a/`，由 `script/ohos/bundle` 一并注入。**HNP 至此不再全是预制件**：`zsh.hnp` / `git.hnp` 仍是，这两个由本仓构建。
- 管理密钥两侧各自编译进二进制，故 `script/ohos/cmdbridge` 在构建前做四份密钥文件的配对校验——不配对只会在设备上表现为认证失败，很难定位。

**实测（2026-09-25）**：

- **兜底路径已在真机验证**：设备上没有 hitdaemon 进程时，终端里的 shell 表现为 `-zsh -g --no-rcs`、其父进程就是 `com.hiwarp.terminal`，且 `ps` 里没有 hitshell 残留——旧版 hitshell 连不上只会 `exit(1)`，只有新的 `exec` 兜底能留下这个结果。同时 hilog 有 `Parsed shell version string: Some([Number(5), Number(9)])` 与 `No history file found for shell zsh`，说明 warp 侧确实按 zsh 完成了 bootstrap，`$WARP_SHELL_PATH` 校验未 panic。
- **覆盖安装会刷新 private HNP 载荷**：`install-local.sh`（`hdc install -r`，无 `--reinstall`）装完后 hitshell 的新逻辑立刻生效，`script/ohos/bundle` 末尾「不保证刷新」的提示属保守说法。不要因此动用 `--reinstall`（它卸载应用、清空沙箱，CODEBUDDY.md 第 21 条禁止）。
- **待实测**：hitdaemon 启动时的远端会话路径。代码只把参数来源从常量换成转发，其余沿用已跑通的实现，但设备侧尚未实测。

---

# 第十二章 结论与后续步骤

## 12.1 结论

1. **可行性**：warp 属于可移植性最好的类别（UI 完全自绘、平台抽象层完备）。移植可行。
2. **改动面**：真实工作量集中在三处——warpui 的 OHOS 窗口与平台后端、app 层的平台定制与入口、系统能力桥接（进程/剪贴板/字体/IME）；业务逻辑（终端仿真、AI、编辑器、Drive）完全复用。窗口后端采用**仿 macOS 的非 winit 路线**，以 `openharmony-ability` 为窗口系统基座（论证见 6.6 节）。
3. **HAP 侧**：改动量小（改名与清理为主），但必须彻底清理 HiCodeer 残留并修正 `bundleName` 不一致。
4. **头号风险**：OHOS 的 `target_os = "linux"` 使 warp 中数量可观的 linux 分支在 OHOS 上默认激活，其中依赖 glibc / fork / X11 / Unix socket 的部分在 OHOS 上非法。必须以 `not(target_env = "ohos")` 成对包裹，并逐个甄别 `OperatingSystem::get().is_linux()` 的运行时分支。
5. **形态利好**：目标设备形态为 tablet / 2in1（HAP 已声明），该形态实测允许 `fork` + `exec /bin/sh` + PTY，终端本地会话有落地基础。

## 12.2 后续步骤（建议顺序）

1. **建立最小可运行版本基线**：入口 crate + `libcore.so` + `hap` 改名 + 启动链跑通（能进事件循环、能出一帧、能打印 hilog）。此阶段不碰字体与终端。**（状态：编译面已达成，见 11.10.1 与 11.10.6；hilog 重定向已接（见 3.3），设备侧跑通待做。）**
2. **补齐 warpui OHOS 平台后端**：按 6.3 节的 trait 清单逐项实现，先窗口与事件循环，再剪贴板与字体。**（状态：已落地并通过全仓编译，见 11.10.6；IME 与鼠标/触摸通道尚未接。）**
3. **渲染打通**：补 OHOS surface，先 GLES 保底，再尝试 Vulkan。
4. **字体系统**：把 warp 自带的 cosmic-text 实现（`windowing/winit/fonts.rs`）迁成 `platform/ohos/` 版本，只重写 OHOS 字体目录枚举与加载器；**不采用** FontParser + `libnative_drawing` 路线（论证见 11.8）。
5. **输入与 IME**：三路事件流消重 + 修饰键 + IME attach。
6. **终端与进程**：预制 private `zsh.hnp`，terminal 经 `WARP_SHELL_PATH=/data/app/bin/hitshell` 启动桥 `hitshell`；连上 hitdaemon 得到系统权限会话，连不上则 `exec` 本地 zsh 兜底（见 11.9、11.11）。
7. **路径与设置持久化**：沙箱路径三态映射 + 授权持久化。
8. **每步验证**：OHOS 交叉编译可安装 + 设备运行观测 + 其他平台 `cargo check` 回归 + 日志清理，并留痕（命令、输出、结论）。

## 12.3 待明确事项

以下事项需与相关人员确认后方可推进，本文档不擅自决策：

- 目标设备形态的最终范围（仅 2in1、还是含手机/平板；这直接决定进程与 pty 方案）。
- **（2026-09-25 已落地）** HiCodeer 的 cmd-agent 是否要复刻：已按「本地守护进程 + SSH 桥」实现，即 `hitdaemon` + `hitshell`（见 11.11）；QEMU guest 仍不做。剩余待定的是「除 zsh 之外，其余命令是否一律本地 fork」。
- 签名材料（`.cer` / `.p7b` / `.p12` + `material/`）的提供方与路径。
- warp 的渠道与打包形态（stable / preview / local 中哪一个作为 OHOS 首发渠道）。
- 是否引入 `openharmony-ability` 的第三方 fork（license 与长期维护的评估）。

## 12.4 系统能力补齐清单与最终裁决（2026-09-24）

本节记录对 OHOS 平台后端系统能力缺口的逐项复核结论与最终裁决，供后续开发直接引用，**无需重复分析**。

复核对象：`crates/warpui/src/platform/ohos/delegate.rs` 与 `crates/warpui/src/platform/ohos/windowing.rs`。

### 12.4.1 复核方法

`Delegate` 与 `Window` 两个 trait 的全部方法在 OHOS 后端**都有实现，不存在编译期缺失**；缺口是实现深度，分三类：

1. **`report_gap` 显式打点**：`request_user_attention`、`show_native_platform_modal`、`open_character_palette`。（`register_global_shortcut` / `unregister_global_shortcut` 原本也在此列，已于 2026-09-24 实现，见 12.4.2。）
2. **静默降级**（返回保守值而不报错）：`request_desktop_notification_permissions` 恒 `PermissionsDenied`、`send_desktop_notification` 直接丢弃、`application_bundle_info` 恒 `None`、`is_screen_reader_enabled` 恒 `None`、`microphone_access_state` 返回 `Denied` / `NotDetermined`、`set_accessibility_contents` 不填充、`terminate_app` 只停事件循环而进程留驻。
3. **窗口层空实现**：`Window` 的 `minimize` / `toggle_maximized` / `toggle_fullscreen` 只记日志（注意这是 **per-window** 层；**app 级**的隐藏 / 唤起已于 2026-09-24 实现，见 12.4.2（3）），`set_window_title` 与 `set_all_windows_background_blur_radius` 为空，`display_count` 恒 1、`active_display_id` 恒 0（单显示器假设），`supports_transparency` 恒 `false`。

另有 `event_loop.rs` 三处：`InputEvent::HoverEvent`（指针悬停）、`KeyboardEvent`（软键盘高度）、`SaveState`（ability 保存状态回调）。

### 12.4.2 裁决与实现：以下三项（2026-09-24 均已落地）

**（1）全局快捷键**

- 现状：`register_global_shortcut` / `unregister_global_shortcut`（`delegate.rs`）原本只打 warn 日志。
- 触发入口明确：`app/src/root_view.rs` 注册的 quake mode 快捷键与 activation hotkey，用户在设置里打开开关后完全无效果。
- 实现方向：**在 `openharmony-ability` 的 core 里加模块直连 NDK，不开插件。** `libohinput` 已提供完整的热键 C API 且头文件无 `@permission` 标注：`OH_Input_CreateHotkey` / `OH_Input_SetPreKeys` / `OH_Input_SetFinalKey` / `OH_Input_SetRepeat` / `OH_Input_AddHotkeyMonitor(const Input_Hotkey*, Input_HotkeyCallback)` / `OH_Input_RemoveHotkeyMonitor` / `OH_Input_CreateAllSystemHotkeys` / `OH_Input_GetAllSystemHotkeys`（均 since 14），声明位于 SDK 的 `native/sysroot/usr/include/multimodalinput/oh_input_manager.h`。落地形态与 `crates/ability/src/clipboard.rs` 同构，warp 侧调用即可。
- 实现落位：
  - 框架 core 新增 `crates/ability/src/hotkey.rs`，对外提供 `register_hotkey` / `unregister_hotkey` / `set_hotkey_triggered_handler`，以及 `INPUT_OCCUPIED_BY_SYSTEM` 等错误码常量（`crates/ability/src/lib.rs` 已登记 `mod hotkey` + `pub use`）。
  - 两个必须遵守的约束：**订阅用的 `Input_Hotkey` 对象要保活到注销**（`AddHotkeyMonitor` 之后立刻 `Destroy` 可能给系统留下悬垂指针，注册表因此持有对象、注销时才销毁）；**每个订阅槽位注册一个独立的 `extern "C"` trampoline 入口函数**（`RemoveHotkeyMonitor` 要求传回与订阅时完全相同的回调指针，故不能用闭包；而槽位号只能靠入口点区分——**回调收到的 `Input_Hotkey` 读不出键位**，`OH_Input_GetPreKeys` 对回调里传入的它恒返回 401，订阅表里另存 `pre_keys` / `final_key`）。
  - warp 侧新增 `crates/warpui/src/platform/ohos/hotkey.rs`，做 `Keystroke` ↔ OHOS 键码的双向映射。键名约定必须与 `platform/ohos/keycodes.rs` 一致（shift 下字母大写、数字与符号取 shifted 形式），否则触发时回溯出的 `Keystroke` 与 `App::global_shortcuts` 表里存的键不相等、快捷键静默不响应。修饰键上限 2 个（`OH_Input_SetPreKeys` 的约束），超限或键名不支持时告警并放弃注册。
  - `event_loop.rs` 新增 `AppEvent::GlobalShortcutTriggered(Keystroke)`，在 `process_event` 里交给 `callbacks.global_shortcut_triggered`；`delegate.rs` 在 `AppDelegate::new` 安装一次触发器，把 hotkey 回调线程经 mpsc 送回 warp 事件循环线程（与 `file_drop` 同一条路径），并在 `register_global_shortcut` / `unregister_global_shortcut` 接线，失败时按错误码给出可读原因（系统保留 / 已被其它应用占用 / 键盘不支持）。

**（2）剪贴板图片与 HTML**

- 现状：纯文本**已经可用**，缺的是另外两种口味——图片与 HTML 原本是**直接丢弃**的（图片 `crates/warpui/src/platform/ohos/clipboard.rs` 告警丢弃、HTML 调试级丢弃、读取时 `html` / `images` 写死为 `None`）。
- 场景：截图后粘贴图片、从浏览器复制富文本。
- 实现方向：**扩展框架 core 的 `crates/ability/src/clipboard.rs`**（现有实现直连 `libpasteboard` + `libudmf`），补齐 HTML 与图片的 UDMF 类型读写；warp 侧把"丢弃"改为"写入 / 读出"。**同样不开插件。**
- 实现落位：
  - 框架侧把 `write_text` / `read_text` 扩展为 `write_content` / `read_content`（配 `ClipboardContent` / `ClipboardImage`），新增 `OH_UdsHtml_*` 绑定与 `OH_UdmfRecord_AddGeneralEntry` / `GetGeneralEntry` 绑定、`OH_UdmfData_GetRecords` 绑定。
  - 图片走 UDMF 的 **general entry**（存编码后的文件字节），**不用 `OH_UdsPixelMap`**——后者存的是解码后的像素，写入要编码、读出要解码，两边都得拉上图像编解码链。可承载的是 `general.png` / `general.jpeg` / `general.tiff`；**UDMF 未定义 GIF / WebP / SVG 的 record 类型，这三种口味过不了剪贴板**（macOS 后端能带是因为它的剪贴板接受任意类型标识）。
  - 记录分组：文本与 HTML 共用一个 record（只看首个 record 的消费方也能同时读到两种），每张图片单独一个 record（一个 record 里每种类型只能有一条 entry）。
  - warp 侧 `crates/warpui/src/platform/ohos/clipboard.rs` 把"丢弃"改为写入、把写死的 `None` 改为读出；图片的文件名沿用 macOS 后端做法，从 HTML 口味里提取（`warpui_core::clipboard_utils::extract_filename_from_html`）。
  - 2026-09-24 完整打包（`./script/ohos/bundle`，debug）一次通过：Rust 侧链接 `libohinput` 成功，hvigor 出 `entry-default-signed.hap`。

**（3）窗口最小化 / 唤起（全局快捷键生效的必要配套）**

- 现状：全局快捷键按下后日志链齐全，窗口却**没有任何视觉反应**。根因是 `crates/warpui/src/platform/ohos/windowing.rs` 的 `hide_app()` 是空实现（只打 debug 日志），而 **OHOS NDK 没有窗口最小化 / 恢复 API**：`window_manager/oh_window.h` 只有 `OH_WindowManager_ShowWindow`（since 15）/ `IsWindowShown` / `SetWindowFocusable` / `SetWindowStatusBar*`，AbilityKit 亦无。这两个能力只在 ArkTS 侧存在。
- ArkTS 侧的两个方法：`window.minimize(): Promise<void>`（since 12）与 `window.restore(): Promise<void>`（since 14，**只支持主窗口**），syscap 均为 `SystemCapability.Window.SessionManager`，两者都**没有 `@permission` 标注**（所以不是权限问题）。**别用 `recover()`**：它是「全屏 / 最大化 / 分屏 → 浮动窗口」的恢复，不是最小化恢复。
- 实现方向：**仍然不走插件**——框架里已有「Rust 调 ArkTS」的现成机制（`crates/ability/src/waker.rs` 的 threadsafe function 就是这么传的），故放在 core 里做。
- 实现落位：
  - 框架 core 新增 `crates/ability/src/window_control.rs`：`set_window_actions(minimize, show_and_focus)`（`#[napi]`，把两个 `Function` 经 `build_threadsafe_function().callee_handled::<true>()` 存进全局静态）与 `minimize_main_window()` / `show_and_focus_main_window()`（供 warp 调用，经 `ThreadsafeFunctionCallMode::NonBlocking` 派发）。
  - `crates/derive/src/lib.rs` 在 `openharmony_ability_mod` 里加 `set_window_actions` 的 napi 导出（所有 `#[ability]` 模块因此自动拥有 `setWindowActions`）；框架 ArkTS 侧 `native_ability/src/main/ets/ability/type.ets` 的 `Module` 接口同步加声明，`NativeAbility.ets` 在 `initializeSession` 的 `module.init` 之后调 `module.setWindowActions(...)` 把 `minimize` / `restore` 两个闭包交出去（闭包**延迟读 `this.observedWindow`**：init 早于 `onWindowStageCreate`，注册那一刻主窗口还不存在）。
  - warp 侧只改 `crates/warpui/src/platform/ohos/windowing.rs`：`hide_app()` → `minimize_main_window()`，`activate_app()` 与 `show_window_and_focus_app()` → `show_and_focus_main_window()`。
  - **另一处关键修正**：`active_window_id()` 原本恒返回 `Some`，`show_or_hide_non_quake_mode_windows` 因此每次都判成「该隐藏」、窗口永远切不回来；改为按 `openharmony_ability::window_visibility()` 门控（窗口最小化时返回 `None`，快捷键于是走 `activate_app`），并在 `hide_app()` 里 `set_active_window(None)` 做确定性兜底（`set_active_window` 不动 `active_window_stack`，`frontmost_window_id()` 仍能拿回窗口 id）。`app_is_active()` 也由硬编码 `true` 改为读 `window_visibility()`。
  - 2026-09-24 装机实测（**覆盖安装，未卸载**）：按下 Alt+W 后 hilog 出现 `window_control::minimize_main_window: dispatching to ArkTS`，warp.log 出现 `hide_app: minimizing the main window` / `activate_app`，窗口最小化与唤起均生效。

### 12.4.3 裁决：以下各项不做

**桌面通知**（`send_desktop_notification` + `request_desktop_notification_permissions`）

OHOS 侧 API 其实是齐的——`notificationManager` 的 `publish` / `isNotificationEnabledSync` / `requestEnableNotification(context)` / `setBadgeNumber` 都在。**不做是价值判断，不是可行性问题。**

**对话框与退出确认**（`show_native_platform_modal`）

`app/src/quit_warning/mod.rs` 的 `show()` 用 `cfg!` 分两支：macOS 走 `show_native_platform_modal`，linux / freebsd / windows 走 warp 自绘 modal，**OHOS 两支都不进、`shown` 恒为 `false`**，因此有长命令在跑时关窗口不弹任何确认。要修必须改该文件（非 OHOS 专属文件），且在 OHOS 上还得另配一个对话框插件。

**异步终止**（`terminate_app`）

现状只发 `AppEvent::Terminate` 停掉 warp 事件循环，ability 仍持有进程，用户退出后应用留在后台。现有 `ohos.app-control` 是 `MainThreadSyncBridge`（ArkTS 侧 `SyncPluginBase`），`terminate` 走 `process.ProcessManager().exit()` 且**必须持有 napi `Env` 同步调用**；而 warp 从 warp-main 事件循环线程发起，该线程没有 `Env`。由于**一个插件只能有一种 `Mode`**，无法在同一插件内增加异步 action，只能另开插件。

**终端响铃提示**（`request_user_attention`）

触发点是 `app/src/terminal/view.rs` 的 `ModelEvent::Bell`（命令执行结束等）。OHOS window 层**没有主动「请求注意」的 API**：`@ohos.window.d.ts` 只有被动的 `windowHighlightChange` 事件与 `isWindowHighlighted` 查询，没有闪烁或唤起接口；要做只能用 `notificationManager.setBadgeNumber` 角标，故随通知一并放弃。

**窗口最小化 / 最大化 / 全屏（per-window 三条）**（`windowing.rs`）

**这三条不做，理由是没有触发入口**（**app 级**的隐藏 / 唤起已实现，见 12.4.2（3）——两者不是同一层：前者是 `Window` trait 的 per-window 方法，后者是 `WindowManager` 的 app 级动作）。三条证据：

- `windowing.rs` 的 `uses_native_window_decorations()` 返回 `true`（注释："The ability draws the window frame"），warp 不绘制自己的窗口按钮。
- 自绘的红黄绿窗口按钮只在 macOS / Windows 渲染（`app/src/util/traffic_lights.rs`），`root_view:minimize_window` 只被它的按钮触发。
- `StandardAction::ToggleFullScreen` 绑定的是 `mac_only_keystroke("cmd-ctrl-f")`（`app/src/util/bindings.rs`），只有 macOS 有键。

OHOS 上窗口的最小化 / 最大化本来由**系统窗口装饰**提供，warp 内部这三条路径是死路，接上插件也无人能触发。要使其有意义，必须先在 app 层补菜单项或键盘绑定（属另一个话题，且动的是非 OHOS 专属文件）。另注：`ohos.window` 插件本身能力很全（`minimize_window` / `maximize_window` / `restore_window` / `recover_window` / `show_window` / `focus_window` / `destroy_window` / `is_window_maximized` / `is_window_minimized` / `set_window_blur` / `set_window_focusable` / `set_window_decorations` / `set_window_background_color` / `move_window_to` / `resize_window` / `query_avoid_area`），**唯独没有全屏切换**。

**其余不做项**（理由从简）

- 无障碍（`set_accessibility_contents` + `is_screen_reader_enabled`）：需独立 AccessibilityExtensionAbility 与系统权限，工程量与终端场景下的价值不匹配。
- emoji 字符面板（`open_character_palette`）：OHOS 用户走软键盘自带的 emoji 面板。
- 麦克风权限（`microphone_access_state`）：需给框架 permission 插件补 `check`（现仅有 `request`），且其后还压着音频采集整块工作。
- `application_bundle_info`：唯一消费者是 macOS 专用外部编辑器（`app/src/util/file/external_editor/mac.rs`），OHOS 上属 N/A。
- `event_loop.rs` 三处（hover、软键盘高度、`SaveState` 回调）。

### 12.4.4 本轮复核中的两处操作性发现

**框架仓的改动只能在 warp 侧验证。** `openharmony-ability-zed` 单独编译不过 `openharmony-ability`：`crates/ability/Cargo.toml` 的注释称"由根 Cargo.toml `[patch.crates-io]` 指向本地化副本"，但该 `[patch]` 段在框架仓根 `Cargo.toml` 中并不存在，于是依赖解析到 crates.io 的 `ohos-xcomponent-binding 0.3.1`，与 `crates/ability/src/render/xcomponent.rs` 的调用面不符（`on_ui_input_event` 参数个数不匹配，E0061 / E0277 / E0599）。正确做法是在 warp 仓执行 `./script/ohos/bundle --check-only`（2026-09-24 实测 4 分 07 秒、`EXIT=0`），框架仓的 crate 作为 path 依赖会被一并编译；ArkTS 侧改动则需完整 `./script/ohos/bundle` 交由 hvigor 检查。

**保存对话框的默认值字段本来就存在。** `FileDialogOptions` 一直带有 `default_location`，缺的是两处：warp 侧从未传值，且 ArkTS 的 `pickerLocation()` 只接受**已经是 URI** 的字符串（`location.indexOf("://") > 0`），传入本地 path 会被直接丢弃。2026-09-24 已补齐：ArkTS 侧对非 URI 输入改用 `fileUri.getUriFromPath` 转换（失败则告警并回退到 picker 自身默认值；该 API 文档写明面向**应用沙箱内**路径），Rust 侧新增 `default_file_name` 字段（映射到 `DocumentSaveOptions.newFileNames`），warp 侧 `open_save_file_picker` 传入 `default_directory` 与 `default_filename`。**该项在"只做两项"的裁决之前完成，经用户确认予以保留**；2026-09-24 完整打包（`./script/ohos/bundle`，`EXIT=0`，`hvigor BUILD SUCCESSFUL`）通过，ArkTS 侧已由 hvigor 验证。
