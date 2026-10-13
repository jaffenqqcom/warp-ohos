# OHOS 上 Agent Mode 长请求期间屏幕会熄灭（prevent_sleep 落在 noop 分支）

关联：[[ohos-debug-lessons]]

- 日期：2026-10-12
- 设备/包名：HarmonyOS NEXT 2in1，`com.ohwarp.terminal`
- 影响范围：`prevent_sleep` 的非 OHOS 平台实现选择逻辑，以及 `openharmony-ability` 依赖的 rev。macOS / Windows 的防休眠实现不受影响。
- 状态：**代码已完成、构建已通过，未装机验证**（用户要求本轮不安装）。下方「验证」区分了已证与待证。

## 问题描述

Warp 有一条"防止系统进入空闲休眠"的能力：`prevent_sleep` crate 提供一个 guard，持有期间阻止系统休眠。它在 Agent Mode 请求在途时被挂上（`crates/warp_multi_agent_client/src/lib.rs` 里 `.prevent_sleep("Agent Mode request in-progress")`）。

但在 OHOS 上这个 crate 落在了 `noop` 分支，能力**完全缺失**：Agent Mode 发一条消息、AI 长时间跑的过程中，屏幕仍按系统超时正常熄灭乃至锁屏。功能静默失效——既没有编译错误，也没有任何日志。

## 问题表现

- 在 Warp 的 AI / Agent Mode 面板里发一条消息，等待较长的回复期间，屏幕自动熄灭。
- **无报错、无日志**：`noop.rs` 是空实现，连一行日志都不打，所以从日志侧完全看不出"这个能力没生效"。
- 只在 OHOS 上出现；macOS（`NSProcessInfo beginActivity`）与 Windows（`SetThreadExecutionState`）两条路径正常。
- 触发门槛：需要一次真正发出去的 multi-agent 请求。终端 `$` 提示符下敲命令**不会**经过这条路径。

## 问题原因

两层原因叠加：一层是**平台分支落错**，一层是**依赖 rev 落后**。

### 因果链（为什么 OHOS 落在 noop）

1. `crates/prevent_sleep/src/lib.rs` 用 `cfg_attr` 按平台选实现文件：`mac.rs`（macOS）/ `windows.rs`（Windows）/ `noop.rs`（其余）。

2. 选择依据由 `crates/prevent_sleep/build.rs` 的 `cfg_aliases!` 定义：

   ```
   macos: { target_os = "macos" },
   noop: { not(any(macos, windows)) },
   ```

3. **OHOS 的 `target_os` 是 `linux`**，平台身份体现在 `target_env = "ohos"`（本仓库构建目标是 `aarch64-unknown-linux-ohos`）。因此 OHOS 既不匹配 `macos` 也不匹配 `windows`，`not(any(macos, windows))` 恒为真 → **落进 `noop`**。

4. `noop.rs` 的 `prevent_sleep` 直接返回空 `Guard`，什么都不做。防休眠能力在 OHOS 上等同于不存在。

### 因果链（为什么不能"就地实现"）

OHOS 上"保持屏幕常亮"不是 NDK 能直接调的能力，必须经由 ArkTS：窗口级 `Window.setWindowKeepScreenOn`。Warp 的 Rust 代码与 ArkTS 之间隔着 NAPI 边界，需要框架提供一条跨线程通道。

所幸 `openharmony-ability`（ability 模式框架）**已经实现了这条通道**，模块名就叫 `running_lock`：

- Rust 侧 `crates/ability/src/running_lock.rs` 导出 `acquire_running_lock(reason) -> Result<RunningLockGuard>`，内部通过 threadsafe function 把 acquire/release 动作投递到 ArkTS 线程，并用 `AtomicUsize` 做引用计数（首个 guard 打开、末个 guard 关闭）。
- ArkTS 侧 `native_ability/src/main/ets/ability/NativeAbility.ets` 的 `attachRunningLockActions` 在 ability 创建时注册闭包，底层调 `this.observedWindow.setWindowKeepScreenOn(enabled)`。

但这份能力是**新加的**：它出现在 `9a32891`，而 warp-ohos 的 `Cargo.lock` 钉在 `ebba8a3`（用 `git ls-tree ebba8a3 | grep running_lock` 验证为 0 命中）。**rev 落后** 是这层原因。

### 库为何不用 `@ohos.runningLock`

`running_lock.rs` 的模块注释写明：`@ohos.runningLock` 无法服务此需求——`RunningLockType.BACKGROUND_USER_IDLE` 在目标设备上 `isSupported` 返回 false，其余类型要么已废弃、要么绑接近传感器。所以库改用窗口级 `setWindowKeepScreenOn`，但接口命名仍为 running_lock。

### 走过的弯路（记录以备后查）

- **误判一：以为必须"替换依赖源"。** 最初的方案是把 `crates/warpui/Cargo.toml` 与 `crates/entry_ohos/Cargo.toml` 里 9+7 个 `openharmony-ability*` 依赖从 git 改成本地 path。实测后发现本地 `/storage/Users/currentUser/workspace/openharmony-ability-zed` 的 HEAD（`9a32891`）**与 github `main` 是同一个 commit**，且该 rev 的 cargo git checkout（`~/.cargo/git/checkouts/openharmony-ability-zed-4e98660a3ff9703d/9a32891`）本机已存在。结论：**只需升 rev，不必换源**，`Cargo.toml` 依赖声明与 path 均不动。
- **误判二：以为要实现 runningLock API。** 实际上库已实现，且底层是 `setWindowKeepScreenOn`。直接复用即可，不新增任何 ArkTS 代码。

## 解决方案

关键洞察两条：

1. **升 rev 而非换源。** `cargo update -p openharmony-ability --offline` 一条命令把 10 个同源包从 `ebba8a3` 升到 `9a328912`，离线完成（checkout 已在本机）。`Cargo.toml` 的 `branch = "main"` 声明保持不变。
2. **复用库的 ability 模式通道。** 在 `crates/prevent_sleep` 里加一个 OHOS 实现，把库的 `RunningLockGuard` 包成 `prevent_sleep` 期望的 `Guard`。调用方（`http_client` / `warp_multi_agent_client`）**零改动**。

### 改动

平台选择：给 `build.rs` 增加 `ohos` 别名，并把 `noop` 的判据排除 `ohos`（否则两者会同时命中）：

```
macos: { target_os = "macos" },
ohos: { target_env = "ohos" },
noop: { not(any(macos, windows, ohos)) },
```

`lib.rs` 的实现选择链插入一行：

```rust
#[cfg_attr(ohos, path = "ohos.rs")]
```

新增 `crates/prevent_sleep/src/ohos.rs`：

```rust
pub struct Guard {
    inner: Option<RunningLockGuard>,
}

pub fn prevent_sleep(reason: &'static str) -> Guard {
    match acquire_running_lock(reason) {
        Ok(inner) => {
            log::info!("Preventing sleep with reason: {reason}");
            Guard { inner: Some(inner) }
        }
        Err(error) => {
            // ArkTS 尚未注册 running-lock actions 时才会走到这里；如实降级并记录。
            log::warn!("Not preventing sleep: the OHOS running lock is unavailable: {error}");
            Guard { inner: None }
        }
    }
}
```

`Guard` 用 `Option` 而非裸持有，是因为 `prevent_sleep` 的签名不允许返回 `Result`；失败路径是**真实的能力不可用降级**（带 warn 日志），不是空桩。`Guard` 需要 `Send + Sync`（调用点跨 await 持有，且会被 `prevent_sleep::Stream::wrap` 包进流里），库的 `RunningLockGuard` 是 unit struct，自动满足。

依赖声明加在 `crates/prevent_sleep/Cargo.toml`，限定 OHOS target，非 OHOS 平台不参与解析：

```toml
[target.'cfg(target_env = "ohos")'.dependencies]
openharmony-ability = { git = "https://github.com/jaffenqqcom/openharmony-ability-zed", branch = "main" }
```

### 为什么不全用 `#[cfg(target_env = "ohos")]` 包住

`crates/prevent_sleep` 是**非 OHOS 路径的共享 crate**。`ohos.rs` 这个文件名本身含 `ohos`，是 OHOS 专属文件；但 `build.rs` / `lib.rs` / `Cargo.toml` 三处是共享文件，改动只能是"插入独立行 + 用 cfg 别名门控"，不得改变 macOS / Windows 的行为——这三点都遵守了。

### 权限结论（本次一并查证）

- `Window.setWindowKeepScreenOn` 在 SDK 定义（`@ohos.window.d.ts:6738-6758`）里**没有 `@permission` 标注**，`@throws` 只有 `401` / `1300002` / `1300003`。**不需要任何权限声明**，`hap/entry/src/main/module.json5` 保持原样（只声明 `FILE_ACCESS_PERSIST` / `INTERNET` / `ALLOW_EXTERNAL_NATIVE_CODE`）是正确的。
- 对照 `@ohos.runningLock.create`（`@ohos.runningLock.d.ts:200/214/229`）明确要求 `@permission ohos.permission.RUNNING_LOCK`。**只有改走 runningLock API 才需要这条权限**，当前代码零引用。

## 修改文件

- `Cargo.lock` — 10 个 `openharmony-ability*` 包的 source rev 由 `ebba8a3` 升到 `9a328912`（`cargo update -p openharmony-ability --offline`）。依赖声明未改。
- `crates/prevent_sleep/build.rs` — 新增 `ohos: { target_env = "ohos" }` 别名；`noop` 判据改为 `not(any(macos, windows, ohos))`，防止 OHOS 同时命中两个分支。
- `crates/prevent_sleep/src/lib.rs` — 实现选择链插入 `#[cfg_attr(ohos, path = "ohos.rs")]`。
- `crates/prevent_sleep/Cargo.toml` — 新增 `[target.'cfg(target_env = "ohos")'.dependencies]` 段的 `openharmony-ability` 依赖。
- `crates/prevent_sleep/src/ohos.rs` — **新增文件（OHOS 专属）**。实现 OHOS 版 `Guard` 与 `prevent_sleep`，包 `openharmony_ability::acquire_running_lock`；成功持锁并打 info，失败降级并打 warn；`Drop` 打 info。

## 门控与跨平台约束

- `crates/prevent_sleep/src/ohos.rs` 路径含 `ohos`，是 OHOS 专属文件，可自由创建。
- `build.rs` / `lib.rs` / `Cargo.toml` 属共享文件，改动严格遵守"已有代码不逐行改、只插入独立行 + cfg 门控"：`ohos` 走自己的别名分支，`noop` 的语义被收窄为"既非 macOS/Windows 也非 OHOS"。
- 依赖是 target-specific 的，macOS / Linux / Windows 构建既不编译 `ohos.rs`，也不解析 `openharmony-ability`。非 OHOS 的防休眠行为零变化。

## 验证

**已证（构建与产物层面）**

- `cargo check -p prevent_sleep --target aarch64-unknown-linux-ohos` 通过；日志显示 `openharmony-ability ... #9a328912` 与 `prevent_sleep` 均编译成功。
- `./script/ohos/bundle`（debug）`BUILD SUCCESSFUL`，签名产物 `hap/entry/build/default/outputs/default/entry-default-signed.hap`，HNP 三载荷正常注入。
- ArkTS 同步到新 rev：`target/ohos-arkts/openharmony-ability/.synced-rev` = `9a3289125b...`；`NativeAbility.ets` 含 `attachRunningLockActions` / `setRunningLockActions`，含 `setWindowKeepScreenOn`，**`@ohos.runningLock` 零命中**。
- `libcore.so` 符号落地：`openharmony_ability::running_lock::acquire_running_lock`、NAPI 导出 `set_running_lock_actions`，以及三条日志字符串（`Preventing sleep with reason: ` / `Not preventing sleep: the OHOS running lock is unavailable: ` / `No longer preventing sleep`）。

**待证（设备侧，本轮未做）**

- `./install-local.sh` 覆盖安装（**不卸载**），在 AI / Agent Mode 面板发一条消息。
- `hdc hilog` 按 `com.ohwarp.terminal` 过滤，应看到 `Preventing sleep with reason: Agent Mode request in-progress`，回复结束后看到 `No longer preventing sleep`。
- 目视确认：长回复期间屏幕不熄灭/不锁屏；回复结束后恢复由系统超时控制。

## 已知风险与未决项

- **SDK 对 `setWindowKeepScreenOn` 有非规范使用警告。** 其官方注释（`@ohos.window.d.ts:6741-6744`）要求只在导航/视频/绘图/游戏等场景置 true，并点名"无屏幕交互"场景不要用，否则**系统可能反过来主动熄屏**。Agent Mode 长请求期间用户往往正是不碰屏幕，存在踩中该判定的可能。这是文档警告，未经实测；装机验证时应专门观察长请求期间有无被反杀。
- **guard 粒度是"单次请求"而非"整个回合"。** 一次 Agent Mode 回复会发多条 MAA 请求（每个工具调用往返一次），guard 按请求建/释。请求首尾相接时表现为持续长亮；中途有空档（如需用户确认工具权限）时屏幕可能短暂恢复。这属设计如此，不是缺陷；若要求"整个回合长亮"，需把 guard 提升到 turn 粒度，是另一处改动。
- **触发前提**：需真正发出 multi-agent 请求（登录 + 云端可达）。被动建议路径另受 cargo feature `prompt_suggestions_via_maa` 与服务端实验开关（`app/src/server/experiments/mod.rs`）控制。
