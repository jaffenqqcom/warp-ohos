// §3.6 — 模块导出 + 事件循环全局访问点
#![cfg(target_env = "ohos")]

use std::sync::OnceLock;

use openharmony_ability::OpenHarmonyApp;

use crate::platform::ohos::fonts::OhosFontDB;

// ── 全局 WARP_APP（跨 crate 桥接） ──
pub(crate) static WARP_APP: OnceLock<OpenHarmonyApp> = OnceLock::new();

pub fn set_warp_app(app: OpenHarmonyApp) {
    if WARP_APP.set(app).is_err() {
        log::warn!("set_warp_app: WARP_APP already set, ignoring duplicate");
    }
}

pub fn get_warp_app() -> Option<&'static OpenHarmonyApp> {
    WARP_APP.get()
}

// ── 终止等待桥接（跨 crate） ──
static TERMINATION_WAITER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

pub fn set_termination_waiter(waiter: Box<dyn Fn() + Send + Sync>) {
    if TERMINATION_WAITER.set(waiter).is_err() {
        log::warn!("set_termination_waiter: already set, ignoring duplicate");
    }
}

pub fn wait_for_termination() {
    if let Some(waiter) = TERMINATION_WAITER.get() {
        waiter();
    } else {
        log::warn!("wait_for_termination: no waiter registered, skipping");
    }
}

mod ohos_delegate;
mod ohos_trusted_window;
mod ohos_window_manager;

pub use ohos_delegate::{deactivate_ime, activate_ime, OhosDelegate};
pub use ohos_trusted_window::OhosTrustedWindow;
pub use ohos_window_manager::{OhosWindowManager, SharedWindowState, SurfaceState};

// ── 全局访问点：供 ohos_entry 事件循环使用 ──────────────────────────────────
//
// OhosWindowManager 和 OhosDelegate 在 App::new() 中创建后由 crate::App 持有，
// 生命周期覆盖整个应用运行期。事件循环通过 winit EventLoop 驱动。

/// 裸指针的 Send+Sync 包装，用于 OnceLock 存储。
/// 指向的对象由 crate::App 持有，生命周期覆盖整个应用运行期。
///
/// SAFETY: Ptr 始终通过 OnceLock 存储，OnceLock 保证只从单线程初始化后
/// 再被其他线程读取。Ptr 指向的对象由 crate::App 持有，在 App::run() 返回前
/// 始终有效。事件循环只在主线程访问这些指针，不跨线程移动。
struct Ptr<T>(*const T);
unsafe impl<T> Send for Ptr<T> {}
unsafe impl<T> Sync for Ptr<T> {}

static WINDOW_MANAGER_PTR: OnceLock<Ptr<OhosWindowManager>> = OnceLock::new();
static DELEGATE_PTR: OnceLock<Ptr<OhosDelegate>> = OnceLock::new();
static FONT_DB_PTR: OnceLock<Ptr<OhosFontDB>> = OnceLock::new();

/// 存储 OhosWindowManager 的裸指针供事件循环使用。
/// 调用方需保证指针在应用生命周期内有效。
pub(crate) fn set_window_manager_ptr(ptr: *const OhosWindowManager) {
    if WINDOW_MANAGER_PTR.set(Ptr(ptr)).is_err() {
        log::warn!("set_window_manager_ptr: pointer already set, ignoring duplicate");
    }
}

/// 存储 OhosDelegate 的裸指针供事件循环使用。
/// 调用方需保证指针在应用生命周期内有效。
pub(crate) fn set_delegate_ptr(ptr: *const OhosDelegate) {
    if DELEGATE_PTR.set(Ptr(ptr)).is_err() {
        log::warn!("set_delegate_ptr: pointer already set, ignoring duplicate");
    }
}

/// 获取 OhosWindowManager 引用。返回 None 表示尚未初始化。
pub fn get_window_manager() -> Option<&'static OhosWindowManager> {
    WINDOW_MANAGER_PTR.get().map(|ptr| {
        // SAFETY: 指针在 App::run() 返回前始终有效，由 crate::App 持有。
        unsafe { &*ptr.0 }
    })
}

/// 获取 OhosDelegate 引用。返回 None 表示尚未初始化。
pub fn get_delegate() -> Option<&'static OhosDelegate> {
    DELEGATE_PTR.get().map(|ptr| {
        // SAFETY: 指针在 App::run() 返回前始终有效，由 crate::App 持有。
        unsafe { &*ptr.0 }
    })
}

/// 存储 OhosFontDB 的裸指针供窗口管理器字体渲染使用。
/// 调用方需保证指针在应用生命周期内有效。
pub(crate) fn set_font_db_ptr(ptr: *const OhosFontDB) {
    if FONT_DB_PTR.set(Ptr(ptr)).is_err() {
        log::warn!("set_font_db_ptr: pointer already set, ignoring duplicate");
    }
}

/// 获取 OhosFontDB 引用。返回 None 表示尚未初始化。
pub fn get_font_db() -> Option<&'static OhosFontDB> {
    FONT_DB_PTR.get().map(|ptr| {
        // SAFETY: 指针在 App::run() 返回前始终有效，由 crate::App 持有。
        unsafe { &*ptr.0 }
    })
}