---
name: warp-codemap
description: warp-winit OHOS 移植项目的代码架构地图，记录核心模块、执行路径和接口关系
---

# Warp-CodeMap：warp-winit OHOS 架构地图

## 模块：warpui（UI 框架平台层）

**路径**: `crates/warpui/src/`

**职责**: 自定义 UI 框架的平台适配层，提供跨平台渲染抽象，桥接平台特定 API（窗口、字体、事件循环）到 warpui_core。

**关键文件**:
- `windowing/ohos/ohos_window_manager.rs` — OHOS 窗口管理器，改用 StandardWindow，由标准层管理渲染资源
- `windowing/ohos/ohos_trusted_window.rs` — OH_NativeWindow 指针 → wgpu Surface 转换
- `windowing/ohos/ohos_delegate.rs` — 平台委托（剪贴板、通知、主题）
- `platform/ohos/app.rs` — winit 事件循环入口（闭包模式），拦截生命周期事件
- `platform/ohos/fonts.rs` — OhosFontDB 字体系统（fontdb + cosmic_text + owned_ttf_parser）
- `platform/app.rs` — AppBuilder::run()，包装 init_fn 并派发到平台后端
- `rendering/wgpu/mod.rs` — wgpu Instance 初始化、后端选择
- `rendering/wgpu/resources.rs` — wgpu surface/device/adapter/surface_config 管理
- `rendering/wgpu/renderer.rs` — 场景渲染调度入口，Renderer::render()
- `rendering/wgpu/renderer/frame.rs` — 每帧绘制管线：Clear → rect → image → glyph
- `rendering/wgpu/renderer/glyph.rs` — 字形缓存、图集管理、glyph 着色器渲染

**对外接口**:
- `update_surface_size()` — `ohos_window_manager.rs`，Resumed 事件中调用 SET_BUFFER_GEOMETRY
- `OhosTrustedWindow` — `ohos_trusted_window.rs`，wgpu Instance::create_surface() 传入
- `OhosFontDB::new()` — `fonts.rs:126`，被 `OhosApp::new()` 创建字体系统
- `wgpu_backend_options()` — `rendering/wgpu/mod.rs:145`，返回 `VULKAN | GL`

**依赖模块**: openharmony-ability（获取 native_window）、winit OHOS（事件循环）

---

## 模块：warpui_core（UI 框架核心层）

**路径**: `crates/warpui_core/src/`

**职责**: UI 框架核心，Entity-Component-Handle 模式，视图树管理、场景构建、事件派发。定义了 AppContext、ViewHandle、Presenter 等核心抽象。

**关键文件**:
- `platform/app.rs` — AppCallbackDispatcher、AppCallbacks 定义，桥接平台代码到用户应用代码
- `windowing/mod.rs` — WindowCallbacks、WindowCallbackDispatcher、BuildSceneCallback 类型定义
- `core/app.rs` — AppContext 核心实现：add_window()（行 2238）、build_scene()（行 2700）、add_view()
- `core/view/handle.rs` — ViewHandle<T> 定义：强引用句柄，通过 (WindowId, EntityId) 标识视图实例
- `presenter.rs` — Presenter 定义：管理视图树的布局与绘制，build_scene()（行 333）执行 layout → after_layout → paint

**对外接口**:
- `AppCallbackDispatcher::new()` — `platform/app.rs:70`，被每个平台 App::new() 调用
- `AppCallbackDispatcher::initialize_app()` — `platform/app.rs:74`，运行用户 init_fn
- `WindowCallbackDispatcher::build_scene()` — `windowing/mod.rs:91`，调用 build_scene_callback 闭包
- `AppContext::build_scene()` — `core/app.rs:2700`，循环 invalidation → Presenter::build_scene，最多 3 次迭代
- `AppContext::add_window()` — `core/app.rs:2238`，创建窗口、Presenter、WindowCallbacks、root_view
- `Presenter::build_scene()` — `presenter.rs:333`，layout → after_layout → paint，产出 Rc<Scene>
- `ViewHandle<T>` — `core/view/handle.rs:20`，`(window_id, entity_id)` 强引用句柄

**依赖模块**: warpui（平台适配层调用本模块核心逻辑）

---

## 模块：ohos 特性标志配置

**路径**: `app/src/features.rs`

**职责**: 管理 OHOS 平台的特性标志，通过运行时覆盖绕过不需要的桌面端功能。

**OHOS 专有覆盖**:
```rust
FeatureFlag::OpenWarpNewSettingsModes.set_enabled(false);
```

---

## 模块：winit OHOS 后端

**路径**: `../winit/src/platform_impl/ohos/`

**职责**: 窗口系统事件循环、输入事件转换、vsync 驱动。CustomEvent 通过 `OhosEvent::UserPayload` 变体跨线程传输。

**关键文件**:
- `mod.rs` — EventLoop/Window 实现，OhosEvent 枚举（含 `UserPayload` 变体），`ohos_event_to_winit_event` 转换函数
- `keycodes.rs` — OHOS keycode → winit PhysicalKey/Key 映射表

**事件类型**（OhosEvent 枚举）:
- `SurfaceCreate` — XComponent 表面创建 → `[NewEvents(Init), Resumed]`
- `KeyEvent(KeyCode, KeyAction)` — ArkTS 按键事件
- `ImeEvent(ImePayload)` — 软键盘 IME 输入事件
- `TouchEvent(TouchPhase, x, y)` — 触摸屏事件
- `MouseEvent(MouseAction, MouseButton, x, y)` — 鼠标/触控板事件
- `MouseWheel(dx, dy)` — 滚轮事件
- `UserEvent` — vsync 唤醒信号
- `UserPayload(Box<dyn Any + Send>)` — 跨线程 CustomEvent（OpenWindow 等）

**Window 结构体**:
```rust
struct Window {
    id: WindowId,
    native_window: OpenHarmonyApp,      // 实时获取 XComponent 表面指针和尺寸
    cached_inner_size: AtomicU64,       // 缓存表面尺寸，避免每帧查询
}
```

**对外接口**:
- `EventLoop::run()` — 闭包模式事件循环
- `ohos_vsync_callback()` — OH_NativeVSync 系统回调
- `expose_event_channel()` — 注册事件通道
- `ability_event_to_ohos_event()` — 事件转换

**依赖模块**: openharmony-ability（OpenHarmonyApp）

---

## 模块：openharmony-ability（NAPI 桥接）

**路径**: `../openharmony-ability/crates/ability/src/`

**职责**: ArkTS ↔ Rust NAPI 桥接、XComponent 表面管理、Ability 生命周期。

**关键文件**:
- `app.rs` — OpenHarmonyApp（全局状态，含 native_window/surface_size 方法）

**对外接口**:
- `OpenHarmonyApp::surface_size()` — 查询 XComponent 表面尺寸
- `OpenHarmonyApp::native_window()` — 获取 OH_NativeWindow 指针
- `OpenHarmonyApp::scale()` — 获取屏幕密度

---

## 模块：应用入口与业务逻辑

**路径**: `app/src/`

**职责**: 应用主入口、业务逻辑模块。

**关键文件**:
- `lib.rs` — main()/run()/launch()
- `root_view.rs` — RootView，auth_onboarding_state 状态机
- `platform/ohos/ohos_entry.rs` — init_ability + start_warp_main NAPI 导出，on_touch_event/on_mouse_event/on_key_event NAPI 入口
- `platform/ohos/hap/` — HAP 工程目录

---

## 执行路径

### 路径 1：应用启动与初始化

**功能**: 从 ArkTS 入口到 warp 主循环运行的完整链路（新标准 EventLoop 路径）。

**触发条件**: 用户点击应用图标（OHOS 启动 Ability）。

**线路**:
```
ArkTS EntryAbility.ets
  → DefaultXComponent aboutToAppear()
    → registerServiceBridge()                     [napi_init.cpp]
    → startWarpMain(filesDir, cacheDir, ...)      [ohos_entry.rs]
      → crate::main()                             [lib.rs:117]
        → AppBuilder::run()                       [warpui platform/app.rs:104]
          → OhosApp::new()                        [platform/ohos/app.rs:18]
            → OhosWindowManager::new()
            → OhosDelegate::new()
            → OhosFontDB::new()
            → warpui::App::new()
          → OhosApp::run()                        [platform/ohos/app.rs:40]
            → EventLoop::<CustomEvent>::with_user_event()  + with_openharmony_app()
            → init_wgpu_instance(display_handle)
            → WinitEventLoop::new(ui_app, callbacks, init_fn, proxy)
            → event_loop.run(闭包)
              → EventLoop::run()                  [winit mod.rs]
                → init_vsync()                    [winit mod.rs]
                → rx.recv_timeout(100ms) 循环
```

### 路径 2：应用初始化（initialize_app 内部流程）

**功能**: 注册所有 singleton model、初始化各子系统。

**触发条件**: EventLoop 收到 `NewEvents(Init)` → `handle_event()` → `initialize_app(init_fn)`。

**线路**:
```
Event::NewEvents(Init)
  → WinitEventLoop::handle_event()               [event_loop/mod.rs:534]
    → self.callbacks.initialize_app(init_fn)
      → app/src/lib.rs::initialize_app()
        → SecureStorage / Settings / Auth 等      [行 1126-1222]
        → chk1-chk17 — 各业务模块 singleton
        → launch(ctx, app_state, launch_mode)     [lib.rs:1105]
          → Path 3: 视图创建
```

### 路径 3：视图创建 + 窗口创建（CustomEvent::OpenWindow 路径）

**功能**: 创建视图树 + 异步创建标准窗口（StandardWindow），走 `CustomEvent::OpenWindow` 事件。

**触发条件**: `initialize_app` → `launch()` → `add_window()`。

**线路**:
```
launch()                                          [lib.rs:2617]
  → ctx.dispatch_global_action("root_view:open_new")
    → open_new()                                  [root_view.rs:1117]
      → open_new_with_workspace_source()          [root_view.rs:832]
        → ctx.add_window(options, build_root_view) [core/app.rs:2238]
          → insert_window_internal()
            → WindowManager::handle(self).update(self, |wm| {
                wm.open_window(window_id, options, callbacks)  ← OhosWindowManager
              })
              → 创建 StandardWindow               [ohos_window_manager.rs]
              → EventLoopProxy::send_event(CustomEvent::OpenWindow)
                → OhosEvent::UserPayload(Box::new(event))    [winit mod.rs:send_event]
                → EVENT_CHANNEL.send(UserPayload)
          → 创建 RootView + Workspace + 视图树

  --- 下一帧，EventLoop 收到 UserPayload ---

  rx.recv_timeout → OhosEvent::UserPayload(data)
    → data.downcast::<CustomEvent>()              [winit mod.rs:EventLoop::run()]
    → event_handler(Event::UserEvent(OpenWindow))
    → 闭包 forwarding → WinitEventLoop::handle_event()
      → Event::UserEvent(OpenWindow{window_id})   [event_loop/mod.rs:588]
        → get platform_window (StandardWindow)     ✓ downcast 成功
        → window.open_window(window_target, ...)
          → create_window() → winit::Window::new()
            → 从 ActiveEventLoop 获取 OpenHarmonyApp  [winit mod.rs:Window::new]
            → native_window: 持有 OpenHarmonyApp
          → Resources::new(window.clone(), ...)    [windowing/winit/window.rs:757]
            → get_wgpu_instance()
            → instance.create_surface(window_handle)  ← OHOS NDK WindowHandle
            → select_adapter → Vulkan(Maleoon 916B)
          → Inner { window, rendering_resources }
        → WindowState::new(window_id)
        → self.state.windows.insert(winit_window_id, WindowState)
        → "no state" 警告消失，后续事件能正常处理
```

### 路径 4：场景构建

**功能**: 视图树 → Rc<Scene>（rects/images/glyphs）。

**线路**:
```
WindowCallbackDispatcher::build_scene()           [windowing/mod.rs:91]
  → AppContext::build_scene(window_id, window)     [core/app.rs:2700]
    → for iter in 1..=3:
        → presenter.build_scene(size, scale, ...)  [presenter.rs:333]
          → layout → after_layout → paint
          → 返回 (Scene, repaint_at, pending_assets)
```

### 路径 5：每帧渲染循环（标准 Window::render 路径）

**功能**: `request_redraw()` → `PENDING_REDRAW` → vsync 触发 → `RedrawRequested` → 标准渲染。

**触发条件**: 视图 invalidate 或 input 事件 → `request_redraw()`。

**线路**:
```
视图 invalid → window.request_redraw()
  → winit::Window::request_redraw()               [winit mod.rs]
    → PENDING_REDRAW.store(true)
    → EVENT_CHANNEL.send(OhosEvent::UserEvent)     ← 唤醒 EventLoop

vsync 到达 → ohos_vsync_callback()
  → 检查 PENDING_REDRAW                           [winit mod.rs:ohos_vsync_callback]
  → 如果已设置 → EVENT_CHANNEL.send(OhosEvent::UserEvent)

EventLoop::run():
  rx.recv_timeout → UserEvent
    → ohos_event_to_winit_event → Vec::new()      ← 纯唤醒信号
    → PENDING_REDRAW.swap(false, ...)
    → event_handler(WindowEvent::RedrawRequested)  [winit mod.rs]
    → 闭包 → WinitEventLoop::handle_event()
      → redraw_window()                            [event_loop/mod.rs:954]
        → window_state 存在 ✓
        → platform_window → StandardWindow ✓       ← downcast_window 成功
        → update_size_if_needed()                  [windowing/winit/window.rs:790]
          → 检查 surface_size，需要时重配
        → build_scene() / reuse cached scene
        → window.render(new_scene, font_cache)     [windowing/winit/window.rs:824]
          → Renderer::render(scene, resources, ...)
            → Clear → draw rects/images/glyphs
            → submit() + present()
```

### 路径 6：窗口尺寸变化

**线路**:
```
WindowEvent::Resized(size)
  → 闭包 forwarding → WinitEventLoop::handle_event()
    → convert_window_event → ConvertedEvent::Resize
    → window.handle_resize()
    → window_resized() 回调更新 AppContext
```

### 路径 7：IME 输入事件

**功能**: 软键盘输入通过 IME 回调 → winit Ime 事件 → 标准 EventLoop 处理。

**IME 回调注册** (C++ 层):
```
xcomponent.on_surface_created()
  → IME::new() → ime.insert_text / on_enter / on_backspace / on_status_change
```

**IME 事件转换** (winit 后端):
```
on_insert_text
  → InputEvent::ImeEvent(TextInputEvent)
    → OhosEvent::ImeEvent(ImePayload::Commit(text))
      → WindowEvent::Ime(Ime::Commit(text))
        → WinitEventLoop::handle_ime_event()       [event_loop/mod.rs:1542]
          → dispatch(ClearMarkedText)
          → dispatch(TypedCharacters { text })

on_enter
  → ImeEvent::EnterEvent → ImePayload::Commit("\n")
    → WindowEvent::Ime(Ime::Commit("\n"))
      → handle_ime_event → TypedCharacters("\n")   ← 标准行为，在输入框插入换行

on_backspace
  → ImeEvent::BackspaceEvent → ImePayload::Preedit("")
    → WindowEvent::Ime(Ime::Preedit(""))
      → handle_ime_event → SetMarkedText("")       ← 标准行为，退格无效
```

---

## 输入事件处理（标准 EventLoop 路径）

所有输入事件统一走 `WinitEventLoop::handle_event()`，经 `convert_window_event()` 转换后分发。

### 路径 K1：键盘事件

**功能**: OHOS keycode → winit KeyEvent → 标准层 `convert_keyboard_input_event` → warp KeyDown。

**注意**: `winit::Window` 携带 `OH_NativeWindow*` 指针，`HasWindowHandle` 返回真实 handle。修饰键同时更新 `window_state.modifiers`。

**线路**:
```
Index.ets onKeyEvent(event)
  → on_key_event(keyCode, action)                  [ohos_entry.rs]
  → OhosEvent::KeyEvent(code, action)              → EVENT_CHANNEL
  → ohos_event_to_winit_event::<T>()               [winit mod.rs]
    → keycodes::to_physical_key(code)               [keycodes.rs]
    → keycodes::to_logical(code)                    [keycodes.rs] (含 []\;',./ 等符号键)
    → KeyboardInput { text, logical_key, ... }
  → 闭包 → WinitEventLoop::handle_event()
    → convert_window_event → KeyboardInput
      → 修饰键(text=None + physical_key):          [event_loop/mod.rs:1277]
          → window_state.modifiers.set(SHIFT/CTRL/ALT, is_pressed)  ← 更新标准层状态
          → ConvertedEvent::ModifierKeyChanged
      → 普通键:                                    [event_loop/mod.rs:1292]
          → convert_keyboard_input_event()          [key_events.rs:59]
            → get_input_key(logical_key, shift)     ← Shift 时转大写
            → convert_key(input_key) → "tab"/"a"/"enter" 等
            → keystroke = { key, ctrl, alt, shift, cmd }
            → Event::KeyDown { keystroke, chars }
      → handle_window_event → dispatch_event(KeyDown)
        → if !handled → dispatch(TypedCharacters)
```

### 路径 K2：修饰键组合

**功能**: Ctrl+C、Alt+D、Shift+方向键等组合键。

**线路** (以 Alt+D 为例):
```
按下 Alt (单独事件)
  → KeyboardInput { text:None, physical:AltLeft }
  → modifier check → window_state.modifiers.alt = true
  → ModifierKeyChanged dispatched

按下 D (后续事件)
  → KeyboardInput { text:Some("d"), logical: Character("d") }
  → convert_keyboard_input_event()
    → window_state.modifiers.alt_key() = true
    → get_input_key("d", shift) → "d"
    → keystroke = { key: "d", alt: true }
    → Event::KeyDown { keystroke }
  → 终端识别 Alt+D 绑定 → 触发对应操作
```

### 路径 M1：鼠标/触控板事件

**功能**: ArkTS onMouse → warp 鼠标事件。

**注意**: 按钮映射：ArkTS Button(Left=0, Middle=1, Right=2) → winit MouseButton(Left=0, Middle=1, Right=2)。

**线路**:
```
Index.ets onMouse(event)
  → on_mouse_event(action, button, x, y)           [ohos_entry.rs]
    → 按钮映射: 0→Left, 1→Middle, 2→Right
  → OhosEvent::MouseEvent(action, button, x, y)    → EVENT_CHANNEL
  → ohos_event_to_winit_event
    → Press/Release → WindowEvent::MouseInput
    → Move         → WindowEvent::CursorMoved
  → WinitEventLoop::handle_event()
    → convert_window_event
      → MouseInput → LeftMouseDown/Up, RightMouseDown
      → CursorMoved → MouseMoved / LeftMouseDragged
        (根据 window_state.current_mouse_button_pressed 判断)
```

### 路径 T1：触摸屏事件

**功能**: ArkTS onTouch → winit Touch → 标准层转为模拟鼠标事件。

**线路**:
```
onTouch(Down, 单指)
  → on_touch_event(0, x, y)                       [ohos_entry.rs]
  → OhosEvent::TouchEvent(Started, x, y)
  → ohos_event_to_winit_event
    → WindowEvent::Touch(Started, location)
  → convert_window_event → convert_touch_started
    → dispatch(MouseDown) / dispatch(MouseMoved)

onTouch(Move, 单指)
  → onWheelEvent(dx, dy)
  → OhosEvent::MouseWheel(dx, dy)
  → WindowEvent::MouseWheel(LineDelta)
  → convert → ScrollWheel

onTouch(Down, 双指)
  → on_touch_event(3, x, y)
  → OhosEvent::MouseEvent(Press, Right, x, y)
  → WindowEvent::MouseInput(Press, Right)
  → convert → RightMouseDown
```

---

## 窗口管理事件处理

现在所有窗口事件由标准层通过 `WinitEventLoop::handle_event()` 处理：

| 事件 | 标准层处理 | OHOS 特有操作 |
|------|-----------|--------------|
| `NewEvents(Init)` | `initialize_app()` → 窗口创建 | 无 |
| `UserEvent(OpenWindow)` | `StandardWindow::open_window()` → 创建 winit::Window + wgpu | 无（winit::Window 自带 native_window） |
| `RedrawRequested` | `redraw_window()` → `StandardWindow::render()` | 无 |
| `Resized` | `ConvertedEvent::Resize` → `window.handle_resize()` | 无 |
| `CloseRequested` | `close_window_requested()` | 无 |
| `Focused` | `ActiveWindowChanged` (CustomEvent) | 无 |
| `KeyboardInput` | `convert_keyboard_input_event()` → KeyDown | 修饰键更新 `window_state.modifiers` |
| `MouseInput` | 转为 LeftMouseDown/Up、RightMouseDown | 无 |
| `CursorMoved` | 根据按钮状态→ MouseMoved / LeftMouseDragged | 无 |
| `Touch` | 转为模拟鼠标事件 | 无 |
| `Ime` | `handle_ime_event()` → TypedCharacters | 无 |
| `MouseWheel` | → ScrollWheel | 无 |

### OHOS 拦截的 3 个生命周期事件

在 `platform/ohos/app.rs` 的闭包中拦截，不交给标准层：

| 事件 | 处理内容 |
|------|---------|
| `Resumed` | SET_BUFFER_GEOMETRY（初始化 native window 缓冲区队列尺寸） |
| `Suspended` | 空（仅日志） |
| `AboutToWait` | 排空 `OhosDelegate::drain_pending_dispatch_tasks()` |

---

## 系统服务桥（Rust → C FFI → NAPI → ArkTS）

**关键机制**: 
- ffi_fn! 宏 (ohos_delegate.rs:40-89) — dlsym + OnceLock 惰性查找
- ohos_call_napi() (service_tsfn.cpp) — 统一 TSFN 调用入口
- g_napi_env + g_exports_ref (napi_init.cpp) — 全局 NAPI 环境缓存

**服务桥初始化**:
```
Index.ets aboutToAppear()
  → warpModule.registerServiceBridge()
    → dlopen("libentry.so") + dlsym("ohos_init_service_bridge")
    → RegisterClipboard / FilePicker / Notification / Theme / Url / Display / Ime
```

**S1 剪贴板**: OhosClipboard::read()/write() → dlsym → NAPI → ArkTS pasteboard API
**S2 输入法关闭**: ohos_close_ime() → ArkTS controller.stopInputSession()

---

## 模块：TerminalManager 特征

**路径**: app/src/terminal/terminal_manager.rs:24

实现者: local_tty::TerminalManager / remote_tty::TerminalManager

---

## 模块：TerminalModel

**路径**: app/src/terminal/model/terminal_model.rs:451

核心字段: alt_screen, block_list, alt_screen_active, title, colors, shell_launch_state

特性: 实现 ansi::Handler，接收 PTY ANSI 转义序列

---

## 模块：Block 和 BlockList

### Block - block.rs:274
字段: id, header_grid, rprompt_grid, output_grid, padding, state, exit_code
BlockState: BeforeExecution / Executing / DoneWithExecution / Background / Static

### BlockList - blocks.rs:225
字段: blocks, block_heights(SumTree), size, selection, active_gap
核心: 始终保证至少一个 active block

---

## 模块：TerminalView

**路径**: app/src/terminal/view.rs:2444
字段: model(Arc), size_info, input, colors, scroll_position, context_menu, find_bar

---

## 模块：Input

**路径**: app/src/terminal/input.rs:1503
字段: model, editor, input_suggestions, sessions, focus_handle
职责: 输入编辑器 -> PtyController -> EventLoop -> PTY fd

---

## 模块：local_tty

**路径**: app/src/terminal/local_tty/
子模块: terminal_manager.rs, event_loop.rs, shell.rs, spawner.rs, unix.rs
PtyOptions: { size, window_id, shell_starter, start_dir, env_vars }

**OHOS 分支**: 三个关键入口点改为调用 OHOS 桥接：
- `shell.rs::compute_fallback_shell()` → `ohos_get_passwd()`（VM 上查 passwd）
- `shell.rs::supported_shell_path_and_type()` → `ohos_supported_shell()`（直接返回 VM shell 类型）
- `unix.rs::get_pw_entry()` → `ohos_get_pw_entry()`（VM 上查 user/dir/shell）
- `spawner.rs::spawn_pty_directly()` → `spawn_pty_ohos()`（SSH 桥接代替 fork+exec）

---

## 模块：PTY 系统

### Pty(unix) - unix.rs:156
{ pty_handle, fd, token, signals }
函数: make_pty()(openpty), spawn()(fork+exec)

### PtySpawner - spawner.rs:119
spawn_pty() -> 先尝试 TerminalServer，失败则直接 spawn

**OHOS 分支** - spawner.rs:272:
```
spawn_pty_ohos()
  → OhosPtyBridge::spawn(&env_list)     [ohos_shell_bridge.rs]
    → create_pty_pair()                  ← openpty + cfmakeraw(关闭ECHO)
    → ohos_start_shell_bridge(slave_fd, env_vars, init_script)  ← C FFI
    → 返回 (master_fd, OhosPtyHandle)
```

### OhosPtyHandle - ohos_shell_bridge.rs:132
{ pid }
实现 PtyHandle trait: pid(), has_process_terminated()(waitid WNOHANG), kill()(SIGTERM+waitpid)

### EventLoop - event_loop.rs:40
spawn() -> PTY reader 线程:
3 个 mio 事件源: CHANNEL/PTY/SIGNALS
- PTY 读取 -> ansi::Processor -> TerminalModel
- Message -> Input/Resize/Shutdown

（OHOS 路径使用同样的 EventLoop，PTY fd 来自 OhosPtyBridge::spawn()）

---

## 模块：OHOS Shell SSH 桥接（Rust 封装层）

**路径**: app/src/platform/ohos/ohos_shell_bridge.rs

**职责**: 通过 dlsym 加载 libentry.so 的 C FFI 符号，提供两个高层接口替代 fork+exec：
- OhosPtyBridge — Shell 长连接（PTY ↔ SSH 桥接）
- OhosCommand — 一次性命令（SSH exec）

**条件编译**: 仅 `target_env = "ohos"`，文件头 `#![cfg(target_env = "ohos")]`

### OhosPtyBridge
- `spawn(env_vars)` — 创建 PTY、序列化环境变量、加载 bash_init_shell.sh、调用 C FFI
- `create_pty_pair()` — openpty + cfmakeraw（关闭 ECHO，防止控制序列回显）
- `load_bash_init_script()` — 从 assets 加载 `bash_init_shell.sh`，替换 `@@USING_CON_PTY_BOOLEAN@@` → "false"
- `serialize_env_vars()` — `[(OsString,OsString)]` → `"K=V\n"` 文本

### OhosCommand
- `new(command)` / `spawn()` — 构造 `bash -c '...'`（含 cd 前缀），调用 C FFI ohos_exec_command
- `OhosChild::output_sync()` — 从管道读取 stdout+退出码（最后 4 字节 little-endian i32）

### ohos_supported_shell()
直接返回已知 shell 类型，不做 `which` 验证（VM 上必然存在 bash/zsh/fish）：
- `bash`/`sh` → `/bin/bash`
- `zsh` → `/bin/zsh`
- `fish` → `/usr/bin/fish`

### ohos_get_passwd()
用 `OhosCommand` 在 VM 上执行 `getent passwd warp`，返回 `(user, dir, shell)`

---

## 模块：OHOS SSH 桥接 C++（libentry.so + libohos_shell.so）

**路径**: app/src/platform/ohos/hap/entry/src/main/cpp/

### service_shell_bridge.cpp — C FFI 导出（编译进 libentry.so）

两个 extern "C" 函数，供 Rust 侧通过 dlsym 加载：

| 函数 | 签名 | 用途 |
|------|------|------|
| `ohos_start_shell_bridge` | `(pty_fd, env_vars, init_script, &pid) → i32` | 启动 ShellBridgeMain 子进程 |
| `ohos_exec_command` | `(command, &pipe_fd, &pid) → i32` | 启动 ExecCmdMain 子进程 |

内部机制：拼装 entryParams → `OH_Ability_StartNativeChildProcess("libohos_shell.so:EntryName")`

### shell_bridge_process.cpp — 桥接子进程主逻辑（编译进 libohos_shell.so）

**ssh_connect_to_vm()** — 公共 SSH 连接：
1. TCP socket → 172.16.100.2:22（硬编码）
2. libssh2_session_handshake
3. libssh2_userauth_password → warp/12345678

**ShellBridgeMain()** — Shell 长连接（生命期与终端 Tab 一致）：
1. 提取 PTY fd + 解析 env_vars/init_script
2. SSH 连接 → `libssh2_channel_open_session`
3. `libssh2_channel_request_pty_ex`（终端类型 `xterm-256color`）
4. `libssh2_channel_exec("bash --norc --noprofile -i")`
5. 写 init rcfile：通过 heredoc 写入 `/tmp/warp_init.sh` → `source /tmp/warp_init.sh`
6. **主循环**：`select()` 双向转发（PTY ↔ SSH channel）
   - PTY→SSH: `read(pty_fd)` → `libssh2_channel_write(channel)`
   - SSH→PTY: `libssh2_channel_read(channel)` → `write_all(pty_fd)`
   - 窗口尺寸变化：`ioctl(TIOCGWINSZ)` → `libssh2_channel_request_pty_size_ex`
   - Keepalive: `libssh2_keepalive_send`

**ExecCmdMain()** — 一次性命令（生命期与单条命令一致）：
1. 提取管道 fd + 命令字符串
2. SSH 连接（阻塞模式）
3. `libssh2_channel_exec(command)`
4. `select()` + `libssh2_channel_read` 读取输出到管道
5. 写入退出码（最后 4 字节 i32-le）

---

## 模块：OHOS CMake 构建（libentry.so / libohos_shell.so 编译）

**路径**: app/src/platform/ohos/hap/entry/src/main/cpp/CMakeLists.txt

**依赖库**: mbedTLS + libssh2（均以 OBJECT 库形式静态编译）

| 组件 | 编译方式 | 源码来源 |
|------|---------|---------|
| mbedTLS | file(GLOB) + add_library(OBJECT) | FetchContent 下载 tarball |
| libssh2 | add_library(OBJECT) + `-DLIBSSH2_MBEDTLS` | FetchContent 下载 tarball |
| 桥接代码 | add_library(SHARED libentry.so) | `napi_init.cpp` + `service_shell_bridge.cpp` |
| 子进程 | add_library(SHARED libohos_shell.so) | `shell_bridge_process.cpp` |

**构建关键点**：
- mbedTLS 不作为正常 CMake 库 target 编译（OpenHarmony 版的 CMakeLists.txt 不调用 `add_subdirectory(library)`），改用 OBJECT 库手动编译源文件
- libssh2 同样 OBJECT 库，`LIBSSH2_MBEDTLS` 预定义宏
- `target_compile_options` 加 `-include sys/uio.h`（OHOS NDK 缺 `struct iovec`）
- OBJECT 库链接：`target_sources(libentry.so PRIVATE ${mbedtfls_objects} ${libssh2_objects})`

---

## 模块：Writeable PTY

**路径**: app/src/terminal/writeable_pty/
Message: Input, Shutdown, ChildExited, Resize
PtyController: 模型层，转发写入到 event loop

---

## 模块：ShellStarter

**路径**: app/src/terminal/local_tty/shell.rs:51
枚举: Direct / Wsl / MSYS2 / DockerSandbox

---

## 模块：PaneGroup

**路径**: app/src/pane_group/
PanesLayout: SingleTerminal / Snapshot / Template / AmbientAgent
PaneGroup: { panes(PaneData树), focus_state, pane_history, pane_contents }
PaneData: { root: PaneNode, len, hidden_panes }
PaneNode: Branch / Leaf(PaneId)
PaneContent trait: id(), attach(), detach(), snapshot(), focus()
TerminalPane: PaneView<TerminalView>

---

## 模块：Workspace

**路径**: app/src/workspace/view.rs:936
{ window_id, tabs, active_tab_index, tab_groups }
关键方法: add_terminal_tab()(:11609), add_tab_with_pane_layout()(:11859), AddDefaultTab(:22214)

---

## 执行路径

### 路径 8：终端 Tab 创建

Workspace -> AddDefaultTab -> DefaultSessionMode::Terminal
 -> add_terminal_tab() -> add_tab_with_pane_layout(SingleTerminal)
 -> PaneGroup::add_session() -> create_session()
 -> local_tty::TerminalManager::create_model()

create_model() 内部:
1. 创建通信通道(mio channel, broadcast, events)
2. 创建 Sessions + EventDispatcher
3. ShellStarter::init()
4. create_terminal_model()
5. init_pty_controller_model()
6. 创建 TerminalView
7. 设置 subscriptions

shell 确定后: on_shell_determined() -> create_pty() -> Pty::new() -> PtySpawner::spawn_pty() -> fork+exec -> start_pty_event_loop() -> EventLoop::spawn()

### 路径 9：PTY I/O 事件循环

PTY reader 线程: mio::Poll 循环
- PTY: 读 shell -> ansi::Processor -> TerminalModel
- CHANNEL: Input->write(pty), Resize->TIOCSWINSZ, Shutdown->退出
- SIGNALS: SIGCHLD -> child_exit

### 路径 10：终端数据流

D1 用户输入: Input -> PtyController -> Message::Input -> EventLoop -> write(pty_fd) -> shell
D2 Shell输出: shell -> write(ptym) -> EventLoop -> ansi::Processor -> TerminalModel -> request_redraw -> TerminalView::paint -> BlockGridElement
D3 Resize: WindowEvent::Resized -> SizeInfo -> Message::Resize -> TIOCSWINSZ -> TerminalModel::resize()

### 路径 11：多 Tab / 多 Pane 架构

Window -> Workspace -> tabs: Vec<TabData>
各 Tab -> PaneGroup -> PaneData 树 (Branch/Leaf)
多终端隔离: 每终端独立进程/PTY/Model/View/EventLoop 线程
分屏: PaneGroup::split() -> PaneData::split_at()
关闭: PaneGroup::close_pane() -> detach() -> remove_leaf()
