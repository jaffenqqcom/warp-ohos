# 关掉所有 tab 后 Warp 不退出反而冻死：OHOS 后端漏掉 should_close_window，且 ability 未结束

关联：[[ohos-debug-lessons]]、[[warp鸿蒙移植分析]]

- 日期：2026-09-26
- 设备/包名：HarmonyOS NEXT 2in1，`com.hiwarp.terminal`
- 影响范围：OHOS 平台的窗口/应用生命周期后端（`crates/warpui/src/platform/ohos/` 与 `crates/entry_ohos/`），只影响"把最后一个窗口关掉"这一条路径
- 状态：已修复；`--check-only` 与完整 bundle 均通过、覆盖装机成功，主人确认问题解决

## 问题描述

OHOS 版 Warp 里，把**所有 tab 都关掉**（也就是关掉了最后一个窗口）之后，应用**既不退出、也没有任何反应**：窗口还留在屏幕上，画面停在最后一帧，点哪里都没用，像死机。只能从系统侧强杀。

这不是"慢"，而是**永久**状态：界面不再重绘，输入也不再有任何效果，等多久都不会自己退。触发条件只有一个——关到**最后一个**窗口。只要还留着一个窗口，或者直接杀进程，都不会走到这条路径。

## 问题表现

- 界面层：最后一个 tab 消失后，窗口**不关、不退出**，内容静止（典型是停在关闭前的画面）。此后鼠标点击、键盘输入均无响应。
- 进程层：应用进程仍在，ability 也没有 finish，所以系统里它还是"活着"的应用，只是不再响应。
- 日志层：关最后一个窗口时会打出 `No windows left, terminating app`（若走到正确路径）——**本次没打**，因为判定根本没被执行；反之在零窗口态下，任何窗口事件都会命中 `ohos::event_loop::process_event: dropping {what} because no window is active`（`crates/warpui/src/platform/ohos/event_loop.rs:1229`）。
- 复现步骤：打开 Warp → 把标签页逐个关闭 → 关到最后一个 → 窗口不消失、应用不退出、界面冻住。
- 频率：**必现**（只要关到最后一个窗口），与标签页内容、是否有命令在跑无关。

## 问题原因

这是**两条独立的缺陷叠在一起**：第一层让 warp 自己的事件循环停在"零窗口"状态（冻住），第二层让应用进程不被结束（不退出）。只修任何一层，现象都不会消失——修第一层会变成"warp 停了但窗口留在屏幕上不动"，修第二层会变成"窗口退出判定没执行"。

### 第一层：OHOS 事件循环把关闭请求处理成"无条件删窗口"

warp 的"关最后一个窗口就等于退应用"这条语义，**只在 `should_close_window` 这一个回调里**判断。`app/src/lib.rs:2757`：

```rust
        on_should_close_window: Some(Box::new(move |window_id, ctx| {
            let general_settings = GeneralSettings::as_ref(ctx);
            // On Linux or Windows, if we're about to close the final window, we should quit the app instead.
            // On Mac, we do this conditionally based on a user setting.
            let quit_on_last_window_closed =
                cfg!(any(target_os = "linux", target_os = "freebsd", windows))
                    || *general_settings.quit_on_last_window_closed;
            if ctx.window_ids().count() == 1 && quit_on_last_window_closed {
                log::info!("No windows left, terminating app");
                ctx.terminate_app(TerminationMode::Cancellable, None);
                return ApproveTerminateResult::Cancel;
            }
            // ...（否则再去看有没有未保存内容、要不要弹退出确认）
```

关 tab 的链路最终落到 `ctx.close_window()`（`app/src/workspace/view.rs:12280` 的 `remove_tab`，调用点在 `:12300`），它走 `WindowManager::close_window_async`，在 OHOS 后端被投递成 `AppEvent::CloseWindow`。

winit 后端（基准实现）收到这个事件后走 `close_window_requested`（`crates/warpui/src/windowing/winit/event_loop/mod.rs:1465`），**先问应用**：

```rust
    fn close_window_requested(
        &mut self,
        window_id: crate::WindowId,
        winit_window_id: winit::window::WindowId,
        termination_mode: TerminationMode,
        window_target: &ActiveEventLoop,
    ) {
        if matches!(
            termination_mode,
            TerminationMode::ForceTerminate | TerminationMode::ContentTransferred
        ) {
            self.close_window(window_id, winit_window_id, window_target);
        } else if let ApproveTerminateResult::Terminate =
            self.callbacks.should_close_window(window_id)
        {
            self.close_window(window_id, winit_window_id, window_target);
        }
    }
```

而 OHOS 后端把同一个事件直接处理掉了，**跳过了 `should_close_window`**：

```rust
        AppEvent::CloseWindow(window_id) => callbacks.window_will_close(window_id),
```

后果是：窗口被删了，但"这是最后一个窗口 → 该退应用"的判断从没执行。warp 于是进入**零窗口态**——事件循环还在转，但：

- `active_window_id()`（`crates/warpui/src/platform/ohos/event_loop.rs:1245`）返回 `None`（`ctx.windows().active_window()` 已无窗口）；
- 于是所有要派发给窗口的事件都走向 `dispatch_to_active_window` 的早退分支（`event_loop.rs:1222`，判定在 `:1228`），每一条都只打一句 warn 就丢掉；
- 渲染帧同样因为没有活动窗口而被丢弃。

**界面冻在最后一帧、输入无处可去、且永不退出**，就是这一层的全部现象。

### 第二层：warp 的事件循环停下来 ≠ 应用退出；ability 才是进程持有者

即使第一层修好、warp 的事件循环正常 `Break`（`AppEvent::Terminate` → `ControlFlow::Break(())`，`event_loop.rs:1083`、`:1094`），**应用仍然不会退出**。

原因是 OHOS 的进程归属：native 侧的 warp 循环只是这个 ability 里跑的一段逻辑，**持有进程并决定窗口去留的是 ArkTS 侧的 `UIAbility`**。warp 的 `delegate.rs` 里 `terminate_app` 只做到"给事件循环发 `AppEvent::Terminate`"，循环退出后没有任何人告诉 ArkTS"这个 ability 可以结束了"。于是在 ArkTS 看来 ability 依然存活，窗口留在屏幕上、停在最后一帧——正是主人最初描述的那种"卡死"。

结束 ability 的唯一途径是 ArkTS 的 `UIAbilityContext.terminateSelf()`（SDK：`ets/api/application/UIAbilityContext.d.ts:1686` 的 `terminateSelf(): Promise<void>`，`:1646` 的 callback 重载）。**NDK（C/Rust 原生层）没有等价接口**，所以这一步必须由 ArkTS 侧发起。

### 因果链

```
关最后一个 tab
   └─ workspace/view.rs:12300  ctx.close_window()
        └─ WindowManager::close_window_async
             └─ AppEvent::CloseWindow { window_id, termination_mode }   ← 投递到 warp-main 事件循环

【修前·第一层】                     【修后·第一层】
AppEvent::CloseWindow(window_id)     先问 should_close_window(window_id)
  => window_will_close(window_id)      ├─ 批准 → window_will_close(window_id)
     窗口被删，零窗口态               └─ 拒绝(停住/弹确认) → 什么都不做
     active_window_id() == None
     此后每帧/每事件都被丢弃
     => 冻在最后一帧、永不退出        最后一个窗口：app/src/lib.rs:2764
                                        ctx.window_ids().count()==1
                                        => terminate_app(Cancellable)
                                           => AppEvent::Terminate
                                              => ControlFlow::Break(())
                                                 => warp::run() 返回

【修前·到此为止】                     【修后·第二层】
warp 停了，能力(ability)还活着        entry_ohos/src/launch_app.rs:104
窗口留在屏幕上不动                     openharmony_ability::terminate_ability()
                                        => threadsafe function → ArkTS
                                           => UIAbilityContext.terminateSelf()
                                              => ability finish ⇒ 应用退出
```

### 为什么以前没暴露

- 这条路径**只在"窗口数从 1 变成 0"时才产生可观察后果**。开着至少一个窗口时，`should_close_window` 漏调最多让"关某个非最后窗口"少做一次未保存内容检查，界面上看不出来。
- OHOS 后端是照着 winit 后端重写的，重写时把 `CloseWindow` 简化成了"删窗口"这一个动作，丢掉了它前面的"先征求应用同意"语义——属于**后端对齐时的遗漏**，不是原生缺陷。
- 上一版的手工验证只覆盖到"能开关窗口"，没有覆盖"关到零窗口"。

## 排查过程中的误判（诚实记录）

- **误判 `target_os`**：起初按"OHOS 的 `target_os` 不是 linux"去推理，进而认为 `on_should_close_window`（`cfg!(target_os = "linux")`）和退出确认框（`app/src/quit_warning/mod.rs:577` 的同一判断）在 OHOS 上"两支都不进、`shown` 恒为 false"，并据此写过"关窗口不会弹退出确认"的结论。**这是错的**：OHOS 目标是 `aarch64-unknown-linux-ohos`，`rustc --print cfg` 实测 `target_os="linux"` / `target_env="ohos"` / `target_family="unix"`（构建是自举的，`rustc -vV` 的 host 即该三元组）。所以 linux 分支**会进**：关窗口走 warp 自绘 modal（`ctx.windows().show_window_and_focus_app(...)` + `workspace.show_native_modal(...)`），既不需要改该文件，也不需要另配"对话框插件"。相关结论已在 [[warp鸿蒙移植分析]] 的 12.4.3 更正。
- **误判"异步终止只能另开插件"**：因为现成的 `ohos.app-control` 走 `MainThreadSyncBridge`、`terminate` 必须**持 napi `Env` 同步调用**，而 warp 是从 `warp-main` 线程发起的（手里没有 `Env`），且一个插件只能有一种 `Mode`。这个论证只对"用那条现成 route"成立，**"只能另开插件"不成立**：正解与窗口最小化同路——框架 core 存一个 ArkTS threadsafe function，由 Rust 侧非阻塞调用，ArkTS 闭包落在 UI 线程执行。已按此落地。
- **编译期死结（浪费了一轮）**：第一次跑 `./script/ohos/bundle --check-only` 报 `error[E0425]: cannot find function terminate_ability in crate openharmony_ability`。真因是提交 `25815e7` 把根 `Cargo.toml` 的本地开发 `[patch."https://github.com/jaffenqqcom/openharmony-ability-zed"]` 段删掉、改钉 `branch = "main"`（锁定 `d02c7f8`），于是**本地 clone 里的框架改动根本没参与编译**，写多少都没用。经授权加回 patch 段后才编译到本地改动。教训见下。
- **`Cargo.lock` 被试验性命令改掉**：一次带 `--config 'patch...'` 的 `cargo metadata` 试验把 `openharmony-ability*` 的 `source` 行去掉了；后用 `cargo metadata --offline` 复原（复原本轮受 patch 影响的最终形态见"修改文件"）。
- **排查打点已清理**：定位过程中先后在 `event_loop.rs` 的 `CloseWindow` 分支和 `windowing.rs` 的 `close_window_async` 各加过一条 `log::info!` 打点。前者与同函数其它分支、以及 winit 对应实现（全静默）都不一致，属纯打点；后者与同文件其它 `WindowManager` 方法的入口日志同构，但同样按主人要求移除。**两条都已删除**，只保留规则要求的异常分支日志。

### 走过的死路（供后人省时间）

- 不要从"渲染/GPU/输入子系统坏了"方向查：界面冻住是**零窗口态的副产物**，不是渲染管线故障。判别方法：看日志有没有 `No windows left, terminating app`（没有 = 判定没执行）以及 `dropping ... because no window is active`。
- 不要以为"warp 循环退了应用就该退"：OHOS 上 ability 才是进程持有者，桥接层必须显式结束它。
- 不要为"结束 ability"去找 NDK 接口：**没有**。`terminateSelf` 是 ArkTS-only。
- 改框架 clone 之前，**先确认根 `Cargo.toml` 的 `[patch."…/openharmony-ability-zed"]` 在不在**；不在的话所有改动都不生效，会在"我的改动为什么不编译"上白绕。

## 解决方案

分两层，缺一不可。

### 第一层：让 OHOS 的 `CloseWindow` 与 winit 对齐（先征求应用同意）

`crates/warpui/src/platform/ohos/event_loop.rs`。事件变体从"只带窗口 id"扩为"带窗口 id + 终止模式"，因为**要不要征求同意取决于终止模式**：

```rust
    /// Close a window. The termination mode decides whether the app is offered
    /// the close first, matching the winit back-end's `close_window_requested`.
    CloseWindow {
        window_id: WindowId,
        termination_mode: TerminationMode,
    },
```

处理分支不再是"无条件删窗口"，而是**照抄 winit 的判定次序**：强制/内容已转移的关闭直接关；可取消的关闭先问 `should_close_window`，批准才关。

```rust
        AppEvent::CloseWindow {
            window_id,
            termination_mode,
        } => {
            // Offer a cancellable close to the app first, exactly as the winit
            // back-end does. That callback is what converts "the last window is
            // closing" into an app termination; without it warp is left running
            // with no window, which renders and accepts no input. A forced close
            // skips the offer.
            if matches!(
                termination_mode,
                TerminationMode::ForceTerminate | TerminationMode::ContentTransferred
            ) || matches!(
                callbacks.should_close_window(window_id),
                ApproveTerminateResult::Terminate
            ) {
                callbacks.window_will_close(window_id);
            }
        }
```

`crates/warpui/src/platform/ohos/windowing.rs:218` 的 `close_window_async` 原本把终止模式丢进 `_termination_mode`，现在原样转发：

```rust
    fn close_window_async(
        &self,
        window_id: WindowId,
        termination_mode: platform::TerminationMode,
    ) {
        if self
            .event_sender
            .send(AppEvent::CloseWindow {
                window_id,
                termination_mode,
            })
            .is_err()
        {
            log::warn!(
                "ohos::windowing::WindowManager::close_window_async: the event loop is no longer \
                 running"
            );
        }
    }
```

`||` 的短路语义与 winit 的 `if / else if` 等价：强制关闭时**不会**调用 `should_close_window`，可取消关闭时**恰好调用一次**。

### 第二层：为"结束 ability"在框架 core 里开一个正式端口

因为 `terminateSelf` 是 ArkTS-only，采用与**窗口最小化**完全相同的通路（框架 core 存 ArkTS 闭包 + threadsafe function），而**不开插件**。

框架新增 `crates/ability/src/ability_control.rs`（与既有的 `window_control.rs` 同构）：

```rust
struct AbilityActions {
    terminate: ThreadsafeFunction<(), ()>,
}

static ABILITY_ACTIONS: LazyLock<RwLock<Option<Arc<AbilityActions>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Registers the ArkTS ability actions for the current ability session.
#[napi]
pub fn set_ability_actions(_env: &napi_ohos::Env, terminate: Function<'_, (), ()>) -> Result<()> {
    // ...build_threadsafe_function().callee_handled::<true>().build()?，存入 ABILITY_ACTIONS
}

/// Finishes the ability, so the application closes instead of staying alive
/// without a window.
pub fn terminate_ability() -> bool {
    // ...取不到（锁中毒 / ArkTS 还没注册）就返回 false
    actions
        .terminate
        .call(Ok(()), ThreadsafeFunctionCallMode::NonBlocking);
    true
}
```

配套三处接线：

- `crates/ability/src/lib.rs:1` / `:23` — `mod ability_control;` 与 `pub use ability_control::*;`
- `crates/derive/src/lib.rs:194` — `#[ability]` 宏里生成 ArkTS 可调的 `set_ability_actions`，转调框架实现（与既有 `set_window_actions` 同样式）
- `native_ability/src/main/ets/ability/type.ets:129` 与 `NativeAbility.ets:244`、`:254`、`:420` — ArkTS 侧把自己的生命周期动作交出去，并在每个 module 初始化时挂上：

```ts
  private attachAbilityActions(module: Module): void {
    if (typeof module.setAbilityActions !== "function") {
      console.warn("[NativeAbility] the native module exports no setAbilityActions port");
      return;
    }
    module.setAbilityActions!((): void => {
      this.terminateAbility();
    });
  }

  private terminateAbility(): void {
    const context = this.context as common.UIAbilityContext;
    context.terminateSelf().catch((error: BusinessError): void => {
      console.error(`[NativeAbility] unable to terminate the ability: ${String(error)}`);
    });
  }
```

warp 侧在"事件循环已结束"这个唯一的收口处调用它 —— `crates/entry_ohos/src/launch_app.rs:104`：

```rust
        // warp's loop stopping only ends warp: the ability owns the process and
        // outlives it, so an unfinished ability would keep the window on screen
        // frozen on its last frame instead of closing the application.
        if !openharmony_ability::terminate_ability() {
            log::warn!(
                "launch_app: the ArkTS ability actions are not registered, so the ability was not \
                 finished and the window will stay frozen"
            );
        }
```

放在 `warp::run()` 返回之后，覆盖所有让循环退出的来源（用户关最后一个窗口、系统销毁 ability 等），不会重复触发。

### 被否决的替代方案

- **新开一个 `ohos.ability-control` 插件**：能行，但要为一个"一句话的调用"引入插件 ID、ArkTS 注册、`Mode` 选择一整套机制；而框架 core 已有 `window_control.rs` 这套现成通路，复用即可。插件应当是"能力"，不是"绕路"。
- **在 Rust 侧硬找 `terminateSelf` 的 NDK 等价物**：不存在。查过 NDK ability C API 清单，无跨/自身 ability 结束接口。
- **第一层只补 `should_close_window` 不改事件签名**：不行。终止模式是"要不要征求同意"的唯一依据，签名不带它就分不出"强制关闭"和"可取消关闭"，会把强关也卡在确认框上。
- **第一层既然 `target_os` 是 linux、干脆不改 OHOS，靠上层兜底**：不行。判定入口就在后端回调里，后端不调，上层再正确也没人执行。

## 验证

- 编译（warp 侧）：`./script/ohos/bundle --check-only` 多次通过（53s~65s）；输出确认 `openharmony-ability`、`openharmony-ability-derive`、各 `plugin-*` **均从本地 clone `/storage/Users/currentUser/workspace/openharmony-ability-zed` 编译**，证明 dev patch 生效、验的确实是本地改动。
- 打包与签名：完整 `./script/ohos/bundle` 通过（约 6 分 11 秒），产出并验签 `entry-default-signed.hap`；`.synced-rev` 记为 `d02c7f8…+<ArkTS 子树 sha256>`，其中的 `+hash` 证明 ArkTS 侧改动经本地 clone 同步进了 HAP。
- 装机：`./install-local.sh` **覆盖安装**成功（未用 `--reinstall`，沙箱数据保留）。
- 设备侧：主人确认关掉所有 tab 后应用正常退出，不再冻住。
- 代码风格：改动过的 Rust 文件过 `rustfmt --check` 干净；框架仓 `cargo fmt --all --check` 报出的漂移全部是既有问题（`app.rs`、`child_process.rs`、`clipboard.rs`、`file_uri.rs`、`input/mod.rs`、`lib.rs`(既有的 `mod file_uri;` 错序)、6 个 plugin、`worker-pool`、`demo_native`），非本次引入。

## 修改文件

warp 侧（`/storage/Users/currentUser/workspace/warp-ohos`）：

- `crates/warpui/src/platform/ohos/event_loop.rs` — `AppEvent::CloseWindow` 由元组变体改为携带 `TerminationMode` 的结构体变体（`:93`）；处理分支（`:1097`）改为先按 winit 语义询问 `should_close_window`，批准或强制关闭才 `window_will_close`（**第一层修复主体**）。
- `crates/warpui/src/platform/ohos/windowing.rs` — `close_window_async`（`:218`）不再丢弃 `termination_mode`，随事件转发。
- `crates/entry_ohos/src/launch_app.rs` — `warp::run()` 返回后调用 `openharmony_ability::terminate_ability()`（`:104`），失败则 warn（**第二层修复的 warp 侧收口**）。
- `Cargo.toml`（根，**本地开发用，不入库**）— 加回 `[patch."https://github.com/jaffenqqcom/openharmony-ability-zed"]` 与 10 条 path 依赖，使本地框架 clone 参与编译，`script/ohos/bundle` 的 ArkTS 同步也依赖该段。
- `Cargo.lock` — 随上述 patch 重新解析：`openharmony-ability*` 共 10 条 `source = "git+…?branch=main#d02c7f8…"` 行被移除（path 依赖无 source）。**这是本地 patch 的产物，与 patch 段同生共死**。

框架侧（`../openharmony-ability-zed`）：

- `crates/ability/src/ability_control.rs` — **新增**。`set_ability_actions`（`:30`，收 ArkTS 闭包存成 threadsafe function）与 `terminate_ability`（`:50`，Rust 侧非阻塞派发）；静态表在 `:23`。与 `window_control.rs` 同构。
- `crates/ability/src/lib.rs` — `:1` 增 `mod ability_control;`，`:23` 增 `pub use ability_control::*;`。
- `crates/derive/src/lib.rs` — `:194` 在 `#[ability]` 宏中生成 ArkTS 可调的 `set_ability_actions`，转调框架实现。
- `native_ability/src/main/ets/ability/NativeAbility.ets` — `attachAbilityActions`（`:244`）、`terminateAbility`（`:254`），并在 module 初始化时调用（`:420`）。
- `native_ability/src/main/ets/ability/type.ets` — `Module` 接口增 `setAbilityActions`（`:129`）。

文档：

- `移植记录/design/warp鸿蒙移植分析.md` — 更正 12.4.3「对话框与退出确认」「异步终止」两处结论（原结论建立在 `target_os` 误判与"只能另开插件"之上）。

## 遗留

- **框架改动仍只在本地 clone**，要让它成为可共享的状态，必须推到 fork 的 `main`；否则别人按 `branch = "main"`（`d02c7f8`）构建时会 `cannot find function terminate_ability`。推送属对外不可逆动作，由主人执行。
- 根 `Cargo.toml` 的 dev `[patch]` 段与 `Cargo.lock` 里 10 行 source 的删除是**同一条本地状态**，两者必须同时存在或同时不存在；patch 段按注释不该入库，所以这两处都不宜提交。框架改动推上去之后应删掉 patch 段、把 `Cargo.lock` 恢复成 git 源。
- 第二层目前只覆盖"事件循环正常结束"这一种收口。若将来出现"warp 不退出但需要结束 ability"的其它路径（例如系统侧要求 finish），应复用 `terminate_ability()` 而不是另写一条通路。
- 关最后一个窗口时的**退出确认框**（有长命令在跑时弹 warp 自绘 modal）本次未在设备上专门验证——按代码它会弹（linux 分支会进），但属于另一条观察项。
