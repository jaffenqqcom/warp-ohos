// §2.5.2 — NAPI 入口：init_ability + start_warp_main
//
// 编译入 libwarp.so，提供两个 NAPI 导出函数：
// - init_ability（由 #[ability] 宏生成，NativeAbility.onCreate 自动调用）
// - start_warp_main（由 #[napi] 宏生成，XComponent.onLoad 后显式调用）
//
// 事件通道架构：
//   UI 线程（ArkTS NAPI 回调）→ run_loop 闭包 → mpsc::Sender
//   warp 线程（start_warp_main 创建）→ mpsc::Receiver → winit EventLoop::run

use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use openharmony_ability::*;
use openharmony_ability_derive::ability;
use napi_derive_ohos::napi;
use napi_ohos::{Env, JsValue, bindgen_prelude::Object};
use napi_ohos::sys;

use winit::platform::ohos::{
    expose_event_channel, ability_event_to_ohos_event, OhosEvent, KeyAction, TouchPhase,
    MouseAction, MouseButton,
};

/// 全局事件通道发送端，由 init_ability 初始化，供 NAPI 触摸/按键函数使用。
static EVENT_TX: OnceLock<Sender<OhosEvent>> = OnceLock::new();

/// 由 ArkTS 在 aboutToAppear 中调用，初始化 libentry.so 的系统服务桥。
/// 通过 dlsym(RTLD_DEFAULT) 查找 libentry.so 中的注册函数，避免编译期 NEEDED 依赖。
#[napi]
pub fn register_service_bridge(env: Env) {
    let raw_env: napi_ohos::sys::napi_env = unsafe { std::mem::transmute(env) };
    if raw_env.is_null() {
        log::error!("register_service_bridge: env is NULL");
        return;
    }
    type InitBridgeFn = unsafe extern "C" fn(napi_ohos::sys::napi_env, napi_ohos::sys::napi_value);
    let func: Option<InitBridgeFn> = unsafe {
        // 先尝试 dlopen libentry.so（如果尚未加载会被加载，已加载则返回缓存句柄）
        let lib = libc::dlopen(b"libentry.so\0".as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
        if lib.is_null() {
            log::error!("register_service_bridge: dlopen libentry.so failed");
            None
        } else {
            let cname = std::ffi::CString::new("ohos_init_service_bridge").unwrap();
            let ptr = libc::dlsym(lib, cname.as_ptr().cast());
            if ptr.is_null() {
                log::error!("register_service_bridge: dlsym ohos_init_service_bridge failed");
                None
            } else {
                Some(std::mem::transmute::<*mut std::ffi::c_void, InitBridgeFn>(ptr))
            }
        }
    };
    if let Some(f) = func {
        let mut exports: napi_ohos::sys::napi_value = std::ptr::null_mut();
        unsafe {
            napi_ohos::sys::napi_create_object(raw_env, &mut exports);
            if exports.is_null() {
                log::error!("register_service_bridge: napi_create_object returned NULL");
                return;
            }
            f(raw_env, exports);
        }
    }
}

/// 双路日志：同时走 log 宏和直接 HiLog 调用。
/// 用于区分日志重定向失败和 hilog FFI 链接/调用失败。
macro_rules! dual_log {
    ($tag:expr, $msg:expr) => {
        log::info!($msg);
        warp_logging::ohos::direct_hilog_info($tag, $msg);
    };
}

/// 入口函数，由 #[ability] 宏在 NativeAbility.onCreate 时自动调用。
///
/// 执行步骤：
/// 1. 将 OpenHarmonyApp 存入 warpui 全局 WARP_APP
/// 2. 创建 mpsc 事件通道
/// 3. 调用 OpenHarmonyApp::run_loop 注册事件处理器闭包
///
/// 事件处理器闭包的行为：
/// - Resume(SaveLoader) 和 SaveState(SaveSaver) 在闭包内直接处理（它们带生命周期参数）
/// - 其余 20 个变体转换为 OhosEvent 后通过 mpsc 通道发送
#[allow(private_interfaces)]
#[ability]
fn init_ability(app: OpenHarmonyApp) {
    // init_ability 在 NativeAbility.onCreate 时自动调用，此时 log_redirect_2_hilog
    // 尚未执行（它在 start_warp_main 中调用），因此 log::info!() 不会输出。

    warpui::windowing::ohos::set_warp_app(app.clone());

    // 通过 winit 公开的 expose_event_channel 获取事件通道发送端。
    // winit 的 EventLoop 从同一通道的接收端读取事件。
    let tx = expose_event_channel();
    let _ = EVENT_TX.set(tx.clone());

    app.run_loop(move |event| {
        let event_name = event.as_str();
        match event {
            Event::SaveState(_saver) => {
            }
            // WindowRedraw 不经过 mpsc 事件通道——vsync 回调已直接发送。
            Event::WindowRedraw(_) => {}
            // AxisEvent（滚轮/触控板双指滑动）
            Event::AxisEvent(vertical, horizontal) => {
                if let Some(tx) = EVENT_TX.get() {
                    send_axis_wheel(vertical, horizontal, tx);
                }
            }
            other => {
                warp_logging::ohos::direct_hilog_info("ability-event", &format!("received: {}", event_name));
                let ohos_event = ability_event_to_ohos_event(&other);
                let _ = tx.send(ohos_event);
            }
        }
    });
}

/// 启动 Warp 主程序的 NAPI 函数，由 ArkTS XComponent.onLoad 显式调用。
///
/// 创建一个新线程，在线程内：
/// 1. 初始化日志（调用 log_redirect_2_hilog，内部静默忽略 Err）
/// 2. 切换工作目录（std::env::set_current_dir）
/// 3. 设置环境变量（set_ohos_environment）
/// 4. 调用 main()
///
/// 本函数立即返回，不阻塞 ArkTS 调用线程。
#[allow(private_interfaces)]
#[napi]
pub fn start_warp_main(
    files_dir: String,
    cache_dir: String,
    temp_dir: String,
    current_dir: String,
) {
    std::thread::Builder::new()
        .name("warp-main".into())
        .spawn(move || {
            // 初始化日志（静默忽略已注册的错误）
            let _ = warp_logging::ohos::log_redirect_2_hilog();
            // 过滤 debug/trace 级别的日志（wgpu 着色器编译日志等），只保留 info/warn/error
            log::set_max_level(log::LevelFilter::Info);

            // 切换工作目录
            let _ = std::env::set_current_dir(&current_dir);

            // 设置环境变量
            // 复用现有 set_ohos_environment 函数（在 ohos 模块中实现）
            set_ohos_environment(&files_dir, &cache_dir, &temp_dir);

            // 调用主入口
            let result = crate::run();
            if let Err(e) = &result {
                warp_logging::ohos::direct_hilog_info("warp-main", &format!("crate::run() failed: {}", e));
            }
            let _ = result;
        })
        .expect("start_warp_main: failed to spawn warp main thread");
}

/// 由 ArkTS aboutToAppear 调用，传入 UIAbilityContext 供 C++ 侧 openUrl 使用。
/// 通过 dlsym 调用 libentry.so 的 ohos_set_ability_context 存储 napi_ref。
#[napi]
pub fn set_ability_context(env: Env, context: Object<'_>) {
    let raw_env: sys::napi_env = unsafe { std::mem::transmute(env) };
    let raw_val = context.raw();
    unsafe {
        let lib = libc::dlopen(b"libentry.so\0".as_ptr().cast(), libc::RTLD_LAZY | libc::RTLD_GLOBAL);
        if lib.is_null() {
            log::error!("set_ability_context: dlopen libentry.so failed");
            return;
        }
        let cname = std::ffi::CString::new("ohos_set_ability_context").unwrap();
        let ptr = libc::dlsym(lib, cname.as_ptr().cast());
        if ptr.is_null() {
            log::error!("set_ability_context: dlsym ohos_set_ability_context failed");
            return;
        }
        type FnType = unsafe extern "C" fn(sys::napi_env, sys::napi_value);
        let f: FnType = std::mem::transmute(ptr);
        f(raw_env, raw_val);
    }
}

/// 由 ArkTS onTouch 回调调用，将触摸事件送入 winit 事件循环。
/// touch_type: Down=0, Up=1, Move=2, TwoFingerDown=-1（自定义值）
#[napi]
pub fn on_touch_event(touch_type: i32, x: f64, y: f64) {
    log::info!(
        "TOUCH: type={} x={} y={}",
        touch_type, x, y
    );
    if touch_type == TOUCH_TYPE_TWO_FINGER_DOWN {
        // 双指 Down → 直接发右键点击
        let scale = warpui::windowing::ohos::get_warp_app()
            .map(|app| app.scale() as f64)
            .unwrap_or(1.0);
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(OhosEvent::MouseEvent(
                MouseAction::Press,
                MouseButton::Right,
                x * scale,
                y * scale,
            ));
        }
        return;
    }
    let phase = match touch_type {
        TOUCH_TYPE_DOWN => TouchPhase::Started,
        TOUCH_TYPE_UP => TouchPhase::Ended,
        TOUCH_TYPE_MOVE => TouchPhase::Moved,
        _ => TouchPhase::Canceled,
    };
    let scale = warpui::windowing::ohos::get_warp_app()
        .map(|app| app.scale() as f64)
        .unwrap_or(1.0);
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(OhosEvent::TouchEvent(phase, x * scale, y * scale));
    }
}

/// 由 ArkTS onWheel / onTouch(Move) 回调调用，将滚轮事件送入 winit 事件循环。
#[napi]
pub fn on_wheel_event(delta_x: f64, delta_y: f64) {
    log::info!(
        "WHEEL: dx={} dy={}",
        delta_x, delta_y
    );
    // 触摸屏/触控板 delta 是 vp/帧
    // TOUCH_WHEEL_UNIT vp ≈ 1 行
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(OhosEvent::MouseWheel(delta_x / TOUCH_WHEEL_UNIT, delta_y / TOUCH_WHEEL_UNIT));
    }
}

/// AxisEvent（鼠标滚轮/触控板双指）处理
fn send_axis_wheel(vertical: f64, horizontal: f64, tx: &std::sync::mpsc::Sender<OhosEvent>) {
    log::info!(
        "AXIS: vertical={} horizontal={}",
        vertical, horizontal
    );
    // 鼠标滚轮：AXIS_WHEEL_UNIT 轴单位 → 1 行
    // 触控板双指：走 AxisEvent 但和鼠标同路径，统一缩放
    let _ = tx.send(OhosEvent::MouseWheel(horizontal / AXIS_WHEEL_UNIT, -vertical / AXIS_WHEEL_UNIT));
}

/// 设置 OHOS 应用沙箱环境变量。
/// 为 Warp 主线程设置必要的环境变量（如 HOME、TMPDIR），
/// 确保 Rust 代码中的 std::env::home_dir()、临时文件创建等接口正常工作。
fn set_ohos_environment(files_dir: &str, _cache_dir: &str, temp_dir: &str) {
    // HOME 指向应用沙箱 files 目录，供终端和工具链（如 git、ssh）使用
    if std::env::var("HOME").is_err() {
        std::env::set_var("HOME", files_dir);
    }
    // TMPDIR 指向应用沙箱 temp 目录，供临时文件创建使用
    if std::env::var("TMPDIR").is_err() {
        std::env::set_var("TMPDIR", temp_dir);
    }
    // 兼容性别名
    if std::env::var("TEMP").is_err() {
        std::env::set_var("TEMP", temp_dir);
    }
    if std::env::var("TMP").is_err() {
        std::env::set_var("TMP", temp_dir);
    }
}

// ── OHOS ArkUI 官方 MouseAction 值（_ark_u_i___event_module.md）──
const MOUSE_ACTION_HOVER: i32 = 0;
const MOUSE_ACTION_PRESS: i32 = 1;
const MOUSE_ACTION_RELEASE: i32 = 2;
const MOUSE_ACTION_MOVE: i32 = 3;

// ── OHOS ArkUI 官方 MouseButton 值（ts-universal-mouse-key.md）──
const MOUSE_BUTTON_NONE: i32 = 0;
const MOUSE_BUTTON_LEFT: i32 = 1;
const MOUSE_BUTTON_RIGHT: i32 = 2;
const MOUSE_BUTTON_MIDDLE: i32 = 3;
const MOUSE_BUTTON_BACK: i32 = 4;
const MOUSE_BUTTON_FORWARD: i32 = 5;

// ── onTouch 事件类型（与 ArkTS TouchType 枚举一致）──
// ArkTS TouchType: Down=0, Up=1, Move=2, Cancel=3
const TOUCH_TYPE_DOWN: i32 = 0;
const TOUCH_TYPE_UP: i32 = 1;
const TOUCH_TYPE_MOVE: i32 = 2;
// 双指 Down 是自定义值，非标准 TouchType，用负数避免与 Cancel(3) 冲突
const TOUCH_TYPE_TWO_FINGER_DOWN: i32 = -1;

// ── 滚轮缩放常量 ──
// 鼠标滚轮：AxisEvent 轴单位 120 → 1 行（用户要求慢 3 倍）
const AXIS_WHEEL_UNIT: f64 = 120.0;
// 触摸板滚轮：ArkTS onWheel delta 单位 vp，50vp → 1 行（用户要求慢 5 倍）
const TOUCH_WHEEL_UNIT: f64 = 50.0;

// ── PREV_MOUSE_BUTTON 无前值标记 ──
const PREV_BUTTON_NONE: i32 = -1;

/// 前一个鼠标事件的按钮值，用于 Release 事件中 button=0 时推断实际按钮。
static PREV_MOUSE_BUTTON: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(PREV_BUTTON_NONE);

/// 依据 OpenHarmony ArkUI 官方定义（ts-universal-mouse-key.md）：
///   button: 0=None, 1=Left, 2=Right, 3=Middle, 4=Back, 5=Forward
fn raw_button_to_mouse_button(raw: i32) -> MouseButton {
    match raw {
        MOUSE_BUTTON_LEFT => MouseButton::Left,
        MOUSE_BUTTON_RIGHT => MouseButton::Right,
        MOUSE_BUTTON_MIDDLE => MouseButton::Middle,
        MOUSE_BUTTON_BACK => MouseButton::Back,
        MOUSE_BUTTON_FORWARD => MouseButton::Forward,
        v => MouseButton::Other(v),
    }
}

/// 由 ArkTS onMouse 回调调用，将鼠标事件送入 winit 事件循环。
///
/// 官方 ArkUI 定义（C API _ark_u_i___event_module.md）：
///   action: Hover=0, Press=1, Release=2, Move=3
///
/// 官方 MouseButton 定义：
///   button: None=0, Left=1, Right=2, Middle=3, Back=4, Forward=5
///
/// winit/warpui 协议：
///   - Move → 只发 CursorMoved，warpui 通过 current_mouse_button_pressed
///     决定是 MouseMoved 还是 LeftMouseDragged
///   - Press → CursorMoved + MouseInput(Pressed, btn)，warpui 设按钮状态并派发 Down 事件
///   - Release → CursorMoved + MouseInput(Released, btn)，warpui 清按钮状态
#[napi]
pub fn on_mouse_event(mouse_action: i32, mouse_button: i32, x: f64, y: f64) {
    log::info!(
        "MOUSE: action={} button={} x={} y={}",
        mouse_action, mouse_button, x, y
    );
    let scale = warpui::windowing::ohos::get_warp_app()
        .map(|app: &_| app.scale() as f64)
        .unwrap_or(1.0);
    let scaled_x = x * scale;
    let scaled_y = y * scale;
    let scaled_y = y * scale;

    // 追踪按钮值变化，用于 Release 时推断实际按钮
    let prev = PREV_MOUSE_BUTTON.swap(mouse_button, std::sync::atomic::Ordering::Relaxed);

    // ── Press ──
    if mouse_action == MOUSE_ACTION_PRESS {
        let btn = if mouse_button == MOUSE_BUTTON_NONE {
            // press 不带按钮值时，使用上一次记录的按钮
            raw_button_to_mouse_button(prev)
        } else {
            raw_button_to_mouse_button(mouse_button)
        };
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(OhosEvent::MouseEvent(
                MouseAction::Press,
                btn,
                scaled_x,
                scaled_y,
            ));
        }
        return;
    }

    // ── Release ──
    if mouse_action == MOUSE_ACTION_RELEASE {
        let btn = if mouse_button == MOUSE_BUTTON_NONE {
            // release 不带按钮值时，使用上一次记录的按钮
            raw_button_to_mouse_button(prev)
        } else {
            raw_button_to_mouse_button(mouse_button)
        };
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(OhosEvent::MouseEvent(
                MouseAction::Release,
                btn,
                scaled_x,
                scaled_y,
            ));
        }
        return;
    }

    // ── Move / Hover ──
    if mouse_action == MOUSE_ACTION_MOVE || mouse_action == MOUSE_ACTION_HOVER {
        let btn = raw_button_to_mouse_button(mouse_button);
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(OhosEvent::MouseEvent(
                MouseAction::Move,
                btn,
                scaled_x,
                scaled_y,
            ));
        }
        return;
    }

    // 未识别的 action 值，记录日志但不阻塞
    log::warn!("MOUSE: unknown action={} button={}", mouse_action, mouse_button);
}

/// 由 ArkTS onKeyEvent 回调调用，将按键事件送入 winit 事件循环。
/// key_code: OHOS 原生 keyCode 值（与 NDK OH_NativeXComponent_KeyCode 枚举一致）
/// key_action: 0=Down, 1=Up
/// modifier_state: OHOS modifierKeyState（位掩码：1=Ctrl, 2=Shift, 4=Alt）
#[napi]
pub fn on_key_event(key_code: i32, key_action: i32, modifier_state: i32) {
    let action = match key_action {
        0 => KeyAction::Pressed,
        _ => KeyAction::Released,
    };
    let code = openharmony_ability::xcomponent::KeyCode::from(key_code);
    if let Some(tx) = EVENT_TX.get() {
        // modifierKeyState 随事件一起入队列，出队列时用
        let _ = tx.send(OhosEvent::KeyEvent(code, action, modifier_state as u8));
    }
}


