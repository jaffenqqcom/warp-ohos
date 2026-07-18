// §11 — OhosTrustedWindow：OH_NativeWindow → wgpu Surface
//
// 包装 OHOS NDK 原生窗口指针，实现 HasWindowHandle + HasDisplayHandle
// 供 wgpu Instance::create_surface() 创建 GPU 渲染表面。
// 参照 crates/warpui/src/platform/mac/rendering/wgpu/mod.rs 的 TrustedWindow 模式。

use std::ffi::c_void;
use std::ptr::NonNull;

use wgpu::rwh::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, OhosDisplayHandle,
    OhosNdkWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

/// 包装 OH_NativeWindow 指针，提供 wgpu 所需的 HasWindowHandle + HasDisplayHandle。
///
/// # 安全性
///
/// 与 mac 平台的 TrustedWindow 不同，OHOS 的 OH_NativeWindow 指针可以在 XComponent
/// Surface 销毁时失效。为消除 'static 生命周期假设，本类型存储原始指针而非预创建
/// 的 WindowHandle，在每次调用 window_handle() 时按需创建。这样 WindowHandle 的
/// 生命周期绑定到 OhosTrustedWindow 的借用期，由调用方保证在 Surface 有效期内使用。
#[derive(Clone, Debug)]
pub struct OhosTrustedWindow {
    native_window: NonNull<c_void>,
}

impl OhosTrustedWindow {
    pub fn new(native_window: NonNull<c_void>) -> Self {
        log::debug!("OhosTrustedWindow::new: native_window={:p}", native_window);
        Self { native_window }
    }

    /// 从 *mut c_void 原生指针创建，指针为 NULL 时返回 None。
    pub fn from_ptr(ptr: *mut c_void) -> Option<Self> {
        if ptr.is_null() {
            log::warn!("OhosTrustedWindow::from_ptr: null pointer");
            return None;
        }
        let result = NonNull::new(ptr).map(Self::new);
        log::debug!("OhosTrustedWindow::from_ptr: ptr={:p} -> {}", ptr, if result.is_some() { "Some" } else { "None" });
        result
    }

    /// 从 usize 透传的指针值创建。
    pub fn from_usize(ptr: usize) -> Option<Self> {
        let result = Self::from_ptr(ptr as *mut c_void);
        log::debug!("OhosTrustedWindow::from_usize: ptr=0x{ptr:x} -> {}", if result.is_some() { "Some" } else { "None" });
        result
    }
}

// SAFETY:
// OhosTrustedWindow 仅存储一个 NonNull<c_void> 指针，该指针本身没有线程限制。
//
// 根据 OHOS NDK 文档（native_window/external_window.h）：
//   OH_NativeWindow 实例是线程安全的，支持跨线程调用。
//   OH_NativeWindow_NativeWindowRequestBuffer / FlushBuffer 等操作
//   没有线程亲和性要求，可在任意线程调用。
// 确认方式：查阅 NDK 头文件 native_window/external_window.h 中关于线程安全的注释。
//
// wgpu OHOS 后端通过 OhosNdkWindowHandle 访问该指针。wgpu 保证在
// 渲染线程上使用该句柄，不会跨线程传递原始指针。
//
// 生命周期：OhosTrustedWindow 的实例必须在使用前保证 OH_NativeWindow
// 指针有效。指针的生命周期由 OhosWindowManager 通过 SharedWindowState 管理。
// 使用者必须在 Surface 有效期内持有 OhosTrustedWindow 引用，SurfaceDestroyed
// 事件发生后不应再访问。
unsafe impl Send for OhosTrustedWindow {}
unsafe impl Sync for OhosTrustedWindow {}

impl HasWindowHandle for OhosTrustedWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let ohos_handle = OhosNdkWindowHandle::new(self.native_window);
        let raw_window = RawWindowHandle::OhosNdk(ohos_handle);
        // SAFETY: WindowHandle 的生命周期绑定于 &self，调用方保证在
        // OhosTrustedWindow 的有效期内使用。原生指针由 OhosWindowManager
        // 的 SharedWindowState 管理其生命周期。
        unsafe { Ok(WindowHandle::borrow_raw(raw_window)) }
    }
}

impl HasDisplayHandle for OhosTrustedWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw_display = RawDisplayHandle::Ohos(OhosDisplayHandle::new());
        // SAFETY: OhosDisplayHandle 无状态，仅作标识，不涉生命周期。
        unsafe { Ok(DisplayHandle::borrow_raw(raw_display)) }
    }
}
