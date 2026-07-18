// §3.4 — 窗口管理：OhosWindowManager
//
// 鸿蒙为单窗口模型（XComponent 提供一个渲染表面）。OhosWindowManager 改为
// 包装标准 windwing::winit::Window，由标准层管理渲染资源和生命周期。
// downcast_window 现在能成功转型，18 处标准调用自动生效。
//
// 保留 SharedWindowState 用于 Resumed 事件的表面参数同步（native_window 指针、
// 表面尺寸、密度），以及 update_surface_size 用于 SET_BUFFER_GEOMETRY。

use std::any::Any;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use pathfinder_geometry::rect::RectF;
use pathfinder_geometry::vector::Vector2F;

use warpui_core::platform::{
    Window, WindowManager, WindowOptions,
};
use warpui_core::windowing::WindowCallbacks;
use warpui_core::{DisplayId, DisplayIdx, WindowId};
use warpui_core::{OptionalPlatformWindow, Scene};
use winit::event_loop::EventLoopProxy;

use crate::windowing::winit::app::CustomEvent;
use crate::windowing::winit::Window as StandardWindow;

// ── OH_NativeWindow 缓冲渲染 FFI ─────────────────────────────────────────────────
//
// 仅保留 SET_BUFFER_GEOMETRY 调用（update_surface_size 需要）。
// 其余缓冲操作（RequestBuffer / FlushBuffer）随 render_native_window 移除。

#[repr(C)]
struct NativeBufferRegion {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[link(name = "native_window")]
extern "C" {
    fn OH_NativeWindow_NativeWindowHandleOpt(
        window: *mut c_void,
        code: i32,
        arg1: i32,
        arg2: i32,
    ) -> i32;
}

// ── 诊断模式 ───────────────────────────────────────────────────────────────────
// 标记保留但不再由 render_current_window 使用。后续可集成到 StandardWindow 渲染流程。

static DIAGNOSTIC_MODE: AtomicBool = AtomicBool::new(false);

pub fn is_diagnostic_mode() -> bool {
    DIAGNOSTIC_MODE.load(Ordering::Relaxed)
}

pub fn set_diagnostic_mode(enabled: bool) {
    DIAGNOSTIC_MODE.store(enabled, Ordering::Relaxed);
    log::info!("OhosWindowManager: diagnostic mode set to {enabled}");
}

// ── Surface 状态枚举 ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceState {
    Uninitialized,
    SurfaceCreated,
    Running,
    Suspended,
    Destroyed,
}

// ── 窗口内部状态 ─────────────────────────────────────────────────────────────

struct WindowInner {
    /// Surface 状态机当前状态。
    surface_state: SurfaceState,
    /// OH_NativeWindow 指针。
    native_window: Option<usize>,
    /// Surface 物理像素宽。
    width: u32,
    /// Surface 物理像素高。
    height: u32,
    /// 屏幕密度缩放比。
    density_scale: f64,
    /// 是否在前台。
    focused: bool,
}

impl WindowInner {
    fn new() -> Self {
        Self {
            surface_state: SurfaceState::Uninitialized,
            native_window: None,
            width: 0,
            height: 0,
            density_scale: 1.0,
            focused: true,
        }
    }
}

/// 共享窗口状态，用于 Resumed 事件时同步表面参数到 StandardWindow。
#[derive(Clone)]
pub struct SharedWindowState {
    inner: Arc<Mutex<WindowInner>>,
}

impl SharedWindowState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(WindowInner::new())),
        }
    }

    pub fn set_surface_params(
        &self,
        native_window: usize,
        width: u32,
        height: u32,
        density_scale: f64,
    ) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.native_window = Some(native_window);
        guard.width = width;
        guard.height = height;
        guard.density_scale = density_scale;
        match guard.surface_state {
            SurfaceState::Uninitialized | SurfaceState::Destroyed => {
                guard.surface_state = SurfaceState::SurfaceCreated;
            }
            _ => {}
        }
    }

    pub fn set_surface_state(&self, state: SurfaceState) {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .surface_state = state;
    }

    pub fn set_focused(&self, focused: bool) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).focused = focused;
    }

    pub fn surface_size(&self) -> (u32, u32) {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (guard.width, guard.height)
    }

    pub fn density_scale(&self) -> f64 {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .density_scale
    }

    pub fn native_window(&self) -> Option<usize> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .native_window
    }

    pub fn is_focused(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).focused
    }
}

// ── OhosWindowManager ────────────────────────────────────────────────────────

struct OhosWindowManagerInner {
    active_window_id: Option<WindowId>,
    /// 标准 windwing::winit::Window，被 downcast_window 转型。
    window: Option<Rc<StandardWindow>>,
    /// 回退的显示信息，仅在 surface_size 返回 0 时使用。
    display_bounds: RectF,
    /// EventLoopProxy，用于发送 CustomEvent::OpenWindow。
    event_loop_proxy: EventLoopProxy<CustomEvent>,
}

pub struct OhosWindowManager {
    shared_state: SharedWindowState,
    inner: Mutex<OhosWindowManagerInner>,
}

impl OhosWindowManager {
    pub fn new(event_loop_proxy: EventLoopProxy<CustomEvent>) -> Self {
        Self {
            shared_state: SharedWindowState::new(),
            inner: Mutex::new(OhosWindowManagerInner {
                active_window_id: None,
                window: None,
                // FIXME: 应使用 ohos_get_display_info() 查询实际显示尺寸
                display_bounds: RectF::new(Vector2F::new(0.0, 0.0), Vector2F::new(1080.0, 1920.0)),
                event_loop_proxy,
            }),
        }
    }

    pub fn shared_state(&self) -> &SharedWindowState {
        &self.shared_state
    }

    /// 更新 OH_NativeWindow 缓冲区几何尺寸（SET_BUFFER_GEOMETRY）。
    /// wgpu surface 尺寸由 StandardWindow 在 Resized 事件中自动更新。
    pub fn update_surface_size(&self, size: Vector2F) {
        if let Some(nw_ptr) = self.shared_state.native_window() {
            if nw_ptr != 0 {
                let ptr = nw_ptr as *mut std::ffi::c_void;
                let (w, h) = (size.x() as i32, size.y() as i32);
                let ret = unsafe { OH_NativeWindow_NativeWindowHandleOpt(ptr, 0, w, h) };
                if ret != 0 {
                    log::warn!(
                        "OhosWindowManager::update_surface_size: SET_BUFFER_GEOMETRY returned {}",
                        ret
                    );
                }
            } else {
                log::warn!("OhosWindowManager::update_surface_size: native_window is null (0)");
            }
        } else {
            log::warn!(
                "OhosWindowManager::update_surface_size: native_window not available"
            );
        }
    }
}

impl WindowManager for OhosWindowManager {
    fn open_window(
        &mut self,
        window_id: WindowId,
        _window_options: WindowOptions,
        callbacks: WindowCallbacks,
    ) -> Result<()> {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if inner.active_window_id.is_some() {
            log::warn!(
                "OhosWindowManager: open_window called while window already open, ignoring"
            );
            return Ok(());
        }
        inner.active_window_id = Some(window_id);

        log::info!("OhosWindowManager::open_window: creating StandardWindow");
        // 创建 StandardWindow（标准层 windwing::winit::Window），
        // 确保 downcast_window 转型成功。
        let win = Rc::new(StandardWindow::new(callbacks));
        inner.window = Some(win);

        log::info!("OhosWindowManager::open_window: sending OpenWindow event");
        // 发送 CustomEvent::OpenWindow，通知 EventLoop 创建 winit::Window 和 wgpu surface。
        // OpenWindow 事件处理时会：
        //   1. 通过 platform_window() 获取 StandardWindow（刚创建的，转型成功）
        //   2. 调 StandardWindow::open_window() → 创建 winit::Window（携带真实 native_window）
        //   3. Resources::new(window) → 创建 wgpu surface
        let _ = inner
            .event_loop_proxy
            .send_event(CustomEvent::OpenWindow {
                window_id,
                window_options: _window_options,
            });

        Ok(())
    }

    fn platform_window(&self, _window_id: WindowId) -> OptionalPlatformWindow {
        self.inner
            .lock()
            .unwrap()
            .window
            .clone()
            .map(|w| w as Rc<dyn Window>)
    }

    fn remove_window(&mut self, _window_id: WindowId) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.active_window_id = None;
        inner.window = None;
    }

    fn active_window_id(&self) -> Option<WindowId> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_window_id
    }

    fn key_window_is_modal_panel(&self) -> bool {
        false
    }

    fn app_is_active(&self) -> bool {
        self.shared_state.is_focused()
    }

    fn activate_app(&self, last_active_window: Option<WindowId>) -> Option<WindowId> {
        last_active_window.or(self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_window_id)
    }

    fn show_window_and_focus_app(
        &self,
        _window_id: WindowId,
        _behavior: warpui_core::platform::WindowFocusBehavior,
    ) {
        self.shared_state.set_focused(true);
    }

    fn hide_app(&self) {
        self.shared_state.set_focused(false);
    }

    fn hide_window(&self, _window_id: WindowId) {
        self.shared_state.set_focused(false);
    }

    fn set_window_bounds(&self, _window_id: WindowId, _bound: RectF) {
        log::trace!("OhosWindowManager::set_window_bounds: XComponent size fixed by ArkTS layout");
    }

    fn set_all_windows_background_blur_radius(&self, _blur_radius_pixels: u8) {
        log::trace!("OhosWindowManager::set_all_windows_background_blur_radius: not supported");
    }

    fn set_all_windows_background_blur_texture(&self, _use_blur_texture: bool) {
        log::trace!("OhosWindowManager::set_all_windows_background_blur_texture: not supported");
    }

    fn set_window_title(&self, _window_id: WindowId, _title: &str) {
        log::trace!("OhosWindowManager::set_window_title: no title bar on Ohos");
    }

    fn close_window_async(
        &self,
        _window_id: WindowId,
        _termination_mode: warpui_core::platform::TerminationMode,
    ) {
        self.shared_state.set_surface_state(SurfaceState::Destroyed);
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.active_window_id = None;
        inner.window = None;
    }

    fn active_display_bounds(&self) -> RectF {
        let (w, h) = self.shared_state.surface_size();
        if w > 0 && h > 0 {
            RectF::new(Vector2F::new(0.0, 0.0), Vector2F::new(w as f32, h as f32))
        } else {
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .display_bounds
        }
    }

    fn active_display_id(&self) -> DisplayId {
        DisplayId::from(0)
    }

    fn display_count(&self) -> usize {
        1
    }

    fn bounds_for_display_idx(&self, _idx: DisplayIdx) -> Option<RectF> {
        Some(
            self.inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .display_bounds,
        )
    }

    fn active_cursor_position_updated(&self) {
        if let Ok(inner) = self.inner.lock() {
            let _ = inner
                .event_loop_proxy
                .send_event(CustomEvent::ActiveCursorPositionUpdated);
        }
    }

    fn windowing_system(&self) -> Option<warpui_core::windowing::System> {
        None
    }

    fn os_window_manager_name(&self) -> Option<String> {
        Some("ohos".to_owned())
    }

    fn is_tiling_window_manager(&self) -> bool {
        false
    }

    fn ordered_window_ids(&self) -> Vec<WindowId> {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active_window_id
            .into_iter()
            .collect()
    }
}
