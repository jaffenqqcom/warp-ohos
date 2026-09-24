//! The OHOS window manager and window.
//!
//! HarmonyOS hosts exactly one `XComponent` per ability, and that component
//! owns the `OHNativeWindow` the UI is drawn into. The window here therefore
//! does not create a native window: it binds to the surface the ability reports
//! through [`super::event_loop::AppEvent::SurfaceCreated`] and creates its wgpu
//! resources at that point.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use openharmony_ability::{OpenHarmonyApp, OpenHarmonyWaker};
use openharmony_ability::xcomponent::RawWindow;
use wgpu::rwh::{DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle};

use super::event_loop::{AppEvent, EventSender};
use crate::geometry::rect::RectF;
use crate::geometry::vector::Vector2F;
use crate::platform::{self, WindowBounds, WindowOptions};
use crate::rendering::wgpu::{Renderer, Resources, init_wgpu_instance, renderer};
use crate::rendering::{GlyphConfig, OnGPUDeviceSelected};
use crate::windowing::WindowCallbacks;
use crate::{DisplayId, DisplayIdx, OptionalPlatformWindow, Scene, WindowId, fonts};

/// Default logical window size, used until the ability reports a surface size.
const DEFAULT_LOGICAL_WIDTH: f32 = 1024.0;
/// Default logical window height, used until the ability reports a surface size.
const DEFAULT_LOGICAL_HEIGHT: f32 = 768.0;

/// Set while warp wants a frame drawn. The demand is raised on the warp main
/// thread while the frame callback is armed on the UI thread, so it is shared
/// between the two rather than kept in the window.
pub(super) static PENDING_REDRAW: AtomicBool = AtomicBool::new(false);

/// A one-shot callback invoked with a captured frame.
type FrameCaptureCallback = Box<dyn FnOnce(platform::CapturedFrame) + Send + 'static>;

pub(super) struct WindowManager {
    app: OpenHarmonyApp,
    event_sender: EventSender,
    windows: HashMap<WindowId, Rc<Window>>,
    active_window: RefCell<Option<WindowId>>,
}

impl WindowManager {
    pub(super) fn new(app: OpenHarmonyApp, event_sender: EventSender) -> Self {
        log::info!("ohos::windowing::WindowManager::new");
        Self {
            app,
            event_sender,
            windows: HashMap::new(),
            active_window: RefCell::new(None),
        }
    }

    fn set_active_window(&self, window_id: Option<WindowId>) {
        *self.active_window.borrow_mut() = window_id;
        if self
            .event_sender
            .send(AppEvent::ActiveWindowChanged(window_id))
            .is_err()
        {
            log::warn!(
                "ohos::windowing::WindowManager::set_active_window: the event loop is no longer \
                 running"
            );
        }
    }
}

impl warpui_core::platform::WindowManager for WindowManager {
    fn open_window(
        &mut self,
        window_id: WindowId,
        window_options: WindowOptions,
        callbacks: WindowCallbacks,
    ) -> Result<()> {
        log::info!(
            "ohos::windowing::WindowManager::open_window: window_id={window_id:?} bounds={:?}",
            window_options.bounds
        );
        if !self.windows.is_empty() {
            // The ability mounts a single XComponent, so there is no surface for
            // a second window to bind to. Surface creation is logged rather than
            // silently ignored so that the limitation is visible at runtime.
            log::warn!(
                "ohos::windowing::WindowManager::open_window: OHOS presents one XComponent per \
                 ability, so window {window_id:?} has no surface to render into"
            );
        }
        let window = Rc::new(Window::new(
            self.app.clone(),
            self.event_sender.clone(),
            window_options,
            callbacks,
        ));
        self.windows.insert(window_id, window);
        self.set_active_window(Some(window_id));
        Ok(())
    }

    fn platform_window(&self, window_id: WindowId) -> OptionalPlatformWindow {
        self.windows
            .get(&window_id)
            .map(Rc::clone)
            .map(|inner| inner as Rc<dyn platform::Window>)
    }

    fn remove_window(&mut self, window_id: WindowId) {
        log::info!("ohos::windowing::WindowManager::remove_window: window_id={window_id:?}");
        self.windows.remove(&window_id);
        if *self.active_window.borrow() == Some(window_id) {
            self.set_active_window(None);
        }
    }

    fn active_window_id(&self) -> Option<WindowId> {
        // The ability shows and hides its single window along with the app, so a
        // minimized window is not "active". Reporting that mirrors the macOS
        // back-end, where hiding the app clears the active window, and it is what
        // keeps `show_or_hide_non_quake_mode_windows` toggling instead of always
        // taking the hide branch.
        if !openharmony_ability::window_visibility() {
            log::debug!(
                "ohos::windowing::WindowManager::active_window_id: the window is not visible, \
                 reporting no active window"
            );
            return None;
        }
        *self.active_window.borrow()
    }

    fn key_window_is_modal_panel(&self) -> bool {
        false
    }

    fn app_is_active(&self) -> bool {
        // The window is displayed exactly while the ability is in the foreground,
        // so the platform window visibility is what "the app is active" means here.
        openharmony_ability::window_visibility()
    }

    fn activate_app(&self, last_active_window: Option<WindowId>) -> Option<WindowId> {
        log::info!(
            "ohos::windowing::WindowManager::activate_app: last_active_window={last_active_window:?}"
        );
        if !openharmony_ability::show_and_focus_main_window() {
            log::warn!(
                "ohos::windowing::WindowManager::activate_app: the ArkTS main-window actions are \
                 not registered, so the window was not restored"
            );
        }
        self.set_active_window(last_active_window);
        last_active_window
    }

    fn show_window_and_focus_app(
        &self,
        window_id: WindowId,
        _behavior: platform::WindowFocusBehavior,
    ) {
        log::info!(
            "ohos::windowing::WindowManager::show_window_and_focus_app: window_id={window_id:?}"
        );
        if !openharmony_ability::show_and_focus_main_window() {
            log::warn!(
                "ohos::windowing::WindowManager::show_window_and_focus_app: the ArkTS main-window \
                 actions are not registered, so the window was not restored"
            );
        }
        self.set_active_window(Some(window_id));
    }

    fn hide_app(&self) {
        log::info!("ohos::windowing::WindowManager::hide_app: minimizing the main window");
        // Clearing the active window is what makes the shortcut toggle back: the
        // caller hides while a window is active and shows otherwise, so a leftover
        // id would keep every later press on the hide branch. Dropping the id
        // leaves the frontmost-window stack untouched, so `activate_app` can still
        // restore this window.
        self.set_active_window(None);
        if !openharmony_ability::minimize_main_window() {
            log::warn!(
                "ohos::windowing::WindowManager::hide_app: the ArkTS main-window actions are not \
                 registered, so the window was not minimized"
            );
        }
    }

    fn hide_window(&self, window_id: WindowId) {
        if *self.active_window.borrow() == Some(window_id) {
            self.set_active_window(None);
        }
    }

    fn set_window_bounds(&self, window_id: WindowId, bound: RectF) {
        if let Some(window) = self.windows.get(&window_id) {
            window.set_bounds(bound);
        }
    }

    fn set_all_windows_background_blur_radius(&self, _blur_radius_pixels: u8) {
        log::debug!(
            "ohos::windowing::WindowManager::set_all_windows_background_blur_radius: the OHOS \
             back-end does not support background blur"
        );
    }

    fn set_window_title(&self, _window_id: WindowId, _title: &str) {
        log::debug!(
            "ohos::windowing::WindowManager::set_window_title: OHOS draws no native title bar"
        );
    }

    fn close_window_async(
        &self,
        window_id: WindowId,
        _termination_mode: platform::TerminationMode,
    ) {
        if self
            .event_sender
            .send(AppEvent::CloseWindow(window_id))
            .is_err()
        {
            log::warn!(
                "ohos::windowing::WindowManager::close_window_async: the event loop is no longer \
                 running"
            );
        }
    }

    fn active_display_bounds(&self) -> RectF {
        let rect = self.app.content_rect();
        RectF::new(
            Vector2F::zero(),
            Vector2F::new(rect.width as f32, rect.height as f32),
        )
    }

    fn active_display_id(&self) -> DisplayId {
        DisplayId::from(0)
    }

    fn display_count(&self) -> usize {
        1
    }

    fn bounds_for_display_idx(&self, display_idx: DisplayIdx) -> Option<RectF> {
        match display_idx {
            DisplayIdx::Primary => Some(self.active_display_bounds()),
            DisplayIdx::External(_) => None,
        }
    }

    fn active_cursor_position_updated(&self) {}

    fn windowing_system(&self) -> Option<crate::windowing::System> {
        None
    }

    fn os_window_manager_name(&self) -> Option<String> {
        None
    }

    fn is_tiling_window_manager(&self) -> bool {
        false
    }
}

pub(super) struct Window {
    app: OpenHarmonyApp,
    event_sender: EventSender,
    /// Wakes the UI thread so it re-arms the frame callback when a frame is
    /// wanted: the callback registry is thread-local to the UI thread, so the
    /// arm cannot be done from here.
    waker: OpenHarmonyWaker,
    callbacks: WindowCallbacks,
    /// Logical bounds, used until the ability reports a real surface size.
    bounds: RefCell<RectF>,
    /// The surface size, in physical pixels, that the ability last reported.
    surface_size: Cell<Vector2F>,
    /// The surface size the swap chain was last configured with.
    configured_surface_size: Cell<Vector2F>,
    /// Set when the swap chain must be reconfigured before the next frame.
    surface_requires_reconfiguration: Cell<bool>,
    scene: RefCell<Option<Rc<Scene>>>,
    rendering_resources: RefCell<Option<RenderingResources>>,
    /// Whether a surface is currently bound, so that repeated platform callbacks
    /// stay idempotent.
    surface_attached: Cell<bool>,
    fullscreen_state: Cell<platform::FullscreenState>,
    titlebar_height: Cell<f32>,
    gpu_power_preference: crate::rendering::GPUPowerPreference,
    backend_preference: Option<wgpu::Backend>,
    on_gpu_device_selected: Box<OnGPUDeviceSelected>,
    capture_callback: RefCell<Option<FrameCaptureCallback>>,
}

struct RenderingResources {
    resources: Resources,
    renderer: Renderer,
}

impl Window {
    fn new(
        app: OpenHarmonyApp,
        event_sender: EventSender,
        options: WindowOptions,
        callbacks: WindowCallbacks,
    ) -> Self {
        let bounds = match options.bounds {
            WindowBounds::Default => {
                let rect = app.window_rect();
                let size = match rect.width > 0 && rect.height > 0 {
                    true => Vector2F::new(rect.width as f32, rect.height as f32),
                    false => Vector2F::new(DEFAULT_LOGICAL_WIDTH, DEFAULT_LOGICAL_HEIGHT),
                };
                RectF::new(Vector2F::zero(), size / app.scale().max(1.0))
            }
            WindowBounds::ExactSize(size) => RectF::new(Vector2F::zero(), size),
            WindowBounds::ExactPosition(rect) => rect,
        };
        let waker = app.create_waker();
        Self {
            app,
            event_sender,
            waker,
            callbacks,
            bounds: RefCell::new(bounds),
            surface_size: Cell::new(Vector2F::zero()),
            configured_surface_size: Cell::new(Vector2F::zero()),
            surface_requires_reconfiguration: Cell::new(false),
            scene: RefCell::new(None),
            rendering_resources: RefCell::new(None),
            surface_attached: Cell::new(false),
            fullscreen_state: Cell::new(options.fullscreen_state),
            titlebar_height: Cell::new(0.0),
            gpu_power_preference: options.gpu_power_preference,
            backend_preference: options
                .backend_preference
                .map(crate::rendering::wgpu::to_wgpu_backend),
            on_gpu_device_selected: options.on_gpu_device_info_reported,
            capture_callback: RefCell::new(None),
        }
    }

    fn set_bounds(&self, rect: RectF) {
        *self.bounds.borrow_mut() = rect;
    }

    /// Binds the ability's surface and creates the GPU resources for it.
    ///
    /// Called on the warp main thread once the ability reports that the
    /// XComponent surface exists.
    pub(super) fn attach_surface(&self, window: RawWindow, size: Vector2F) {
        if self.surface_attached.replace(true) {
            log::warn!(
                "ohos::windowing::Window::attach_surface: a surface is already attached, ignoring \
                 the duplicate"
            );
            return;
        }
        log::info!("ohos::windowing::Window::attach_surface: size={size:?}");
        self.surface_size.set(size);
        // A zero-sized surface cannot be configured, so keep the swap chain at a
        // minimal size until the first real resize arrives.
        let initial_size = size.max(Vector2F::splat(1.0));

        // The wgpu instance is process-global and must exist before any GPU
        // resource is built; `Resources::new` below reaches for it. OHOS has a
        // single built-in display and the handle carries no per-window state, so
        // it is created here, ahead of the surface. Initialization is
        // idempotent, which keeps reattaching a surface from rebuilding it.
        init_wgpu_instance(Box::new(OhosDisplayHandle));

        let resources = match Resources::new(
            OhosSurfaceTarget::new(window),
            self.gpu_power_preference,
            self.backend_preference,
            &self.on_gpu_device_selected,
            initial_size,
            // The GL back-end has no NVIDIA Vulkan adapter to downrank.
            false,
        ) {
            Ok(resources) => resources,
            Err(err) => {
                log::error!("ohos::windowing::Window::attach_surface: {err:#}");
                self.surface_attached.set(false);
                return;
            }
        };

        let renderer = Renderer::new(&resources, GlyphConfig::default());
        *self.rendering_resources.borrow_mut() = Some(RenderingResources {
            resources,
            renderer,
        });
        self.configured_surface_size.set(initial_size);
        self.surface_requires_reconfiguration.set(false);
        // The per-frame callback is armed from the ability's UI thread when the
        // surface is created, not here: the XComponent callback registry is
        // thread-local, so arming from the warp main thread would register a
        // callback that the UI thread, which receives the frames, never sees.
    }

    /// Drops the GPU resources bound to the ability's surface.
    pub(super) fn detach_surface(&self) {
        if !self.surface_attached.replace(false) {
            log::warn!(
                "ohos::windowing::Window::detach_surface: no surface was attached, ignoring the \
                 duplicate"
            );
            return;
        }
        log::info!("ohos::windowing::Window::detach_surface");
        let _ = self.rendering_resources.borrow_mut().take();
        self.surface_size.set(Vector2F::zero());
        self.configured_surface_size.set(Vector2F::zero());
    }

    /// Records a new surface size, in physical pixels. Returns whether it changed.
    pub(super) fn set_surface_size(&self, size: Vector2F) -> bool {
        if self.surface_size.get() == size {
            return false;
        }
        log::info!(
            "ohos::windowing::Window::set_surface_size: {size:?} (was {:?})",
            self.surface_size.get()
        );
        self.surface_size.set(size);
        self.surface_requires_reconfiguration.set(true);
        true
    }

    pub(super) fn has_scene(&self) -> bool {
        self.scene.borrow().is_some()
    }

    /// Reconfigures the swap chain if the surface size changed.
    pub(super) fn update_size_if_needed(&self) -> Result<(), renderer::Error> {
        let size = self.surface_size.get();
        let mut rendering = self.rendering_resources.borrow_mut();
        let Some(rendering) = rendering.as_mut() else {
            return Ok(());
        };
        if !self.surface_requires_reconfiguration.get()
            && self.configured_surface_size.get() == size
        {
            return Ok(());
        }
        log::debug!("ohos::windowing::Window::update_size_if_needed: reconfiguring to {size:?}");
        rendering.resources.update_surface_size(size)?;
        self.configured_surface_size.set(size);
        self.surface_requires_reconfiguration.set(false);
        Ok(())
    }

    /// Draws the pending scene, if any.
    pub(super) fn render(
        &self,
        new_scene: Option<Rc<Scene>>,
        font_cache: &fonts::Cache,
    ) -> Result<(), renderer::Error> {
        let mut scene = self.scene.borrow_mut();
        if scene.is_none() {
            *scene = new_scene;
        }

        let Some(scene) = scene.clone() else {
            log::debug!(
                "ohos::windowing::Window::render: a frame was requested but no scene is available"
            );
            return Ok(());
        };

        let mut rendering = self.rendering_resources.borrow_mut();
        let Some(rendering) = rendering.as_mut() else {
            log::debug!("ohos::windowing::Window::render: no surface is attached");
            return Ok(());
        };

        let capture_callback = self.capture_callback.borrow_mut().take();
        let surface_size = self.surface_size.get();
        rendering.renderer.render(
            scene.as_ref(),
            &rendering.resources,
            &|glyph_key, scale, subpixel_alignment, glyph_config, format| {
                font_cache.rasterized_glyph(
                    glyph_key,
                    scale,
                    subpixel_alignment,
                    glyph_config,
                    format,
                )
            },
            &|glyph_key, scale, alignment| {
                font_cache.glyph_raster_bounds(glyph_key, scale, alignment)
            },
            surface_size,
            // OHOS has no `pre_present_notify` equivalent.
            None,
            capture_callback,
        )
    }
}

impl platform::Window for Window {
    fn minimize(&self) {
        log::debug!("ohos::windowing::Window::minimize: OHOS has no per-window minimize");
    }

    fn toggle_maximized(&self) {
        log::debug!("ohos::windowing::Window::toggle_maximized: the ability is always full-screen");
    }

    fn toggle_fullscreen(&self) {
        log::debug!(
            "ohos::windowing::Window::toggle_fullscreen: full-screen mode is owned by the ability"
        );
    }

    fn fullscreen_state(&self) -> platform::FullscreenState {
        self.fullscreen_state.get()
    }

    fn set_titlebar_height(&self, height: f64) {
        self.titlebar_height.set(height as f32);
    }

    fn supports_transparency(&self) -> bool {
        false
    }

    fn graphics_backend(&self) -> platform::GraphicsBackend {
        platform::GraphicsBackend::Gl
    }

    fn supported_backends(&self) -> Vec<platform::GraphicsBackend> {
        vec![platform::GraphicsBackend::Gl]
    }

    fn uses_native_window_decorations(&self) -> bool {
        // The ability draws the window frame; the XComponent covers it.
        true
    }

    fn as_ctx(&self) -> &dyn platform::WindowContext {
        self
    }

    fn callbacks(&self) -> &WindowCallbacks {
        &self.callbacks
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl platform::WindowContext for Window {
    fn size(&self) -> Vector2F {
        let surface_size = self.surface_size.get();
        if surface_size.is_zero() {
            return self.bounds.borrow().size();
        }
        surface_size / self.backing_scale_factor()
    }

    fn origin(&self) -> Vector2F {
        // The XComponent fills the ability's content area, so the window origin
        // is the content origin.
        Vector2F::zero()
    }

    fn backing_scale_factor(&self) -> f32 {
        self.app.scale().max(1.0)
    }

    fn max_texture_dimension_2d(&self) -> Option<u32> {
        self.rendering_resources
            .borrow()
            .as_ref()
            .map(|rendering| rendering.resources.device.limits().max_texture_dimension_2d)
    }

    fn render_scene(&self, scene: Rc<Scene>) {
        self.scene.borrow_mut().replace(scene);
    }

    fn request_redraw(&self) {
        // Dropping the cached scene makes the next frame rebuild it, matching
        // the winit back-end.
        let _ = self.scene.borrow_mut().take();
        // The frame callback is armed on the UI thread while this runs on the
        // warp main thread, so record the demand where that thread can see it
        // and wake it, which re-arms the callback after an idle stop.
        log::debug!("ohos::windowing::Window::request_redraw: waking the UI thread for a frame");
        PENDING_REDRAW.store(true, Ordering::Release);
        self.waker.wake();
    }

    fn request_frame_capture(
        &self,
        callback: Box<dyn FnOnce(platform::CapturedFrame) + Send + 'static>,
    ) {
        *self.capture_callback.borrow_mut() = Some(callback);
        // The capture rides on the next rendered frame, so the UI thread has to
        // be woken the same way `request_redraw` does: without this an idle
        // window's frame callback stays parked and the capture never happens.
        log::debug!(
            "ohos::windowing::Window::request_frame_capture: waking the UI thread for a capture"
        );
        PENDING_REDRAW.store(true, Ordering::Release);
        self.waker.wake();
    }
}

/// The [`wgpu`] surface target for the ability's `OHNativeWindow`.
///
/// `OHNativeWindow` is a producer/consumer buffer queue owned by the ability,
/// so it may be handed to wgpu from the warp main thread. wgpu's GL back-end
/// recognises the handle and creates the EGL window surface from it.
struct OhosSurfaceTarget {
    window: RawWindow,
}

impl OhosSurfaceTarget {
    fn new(window: RawWindow) -> Self {
        Self { window }
    }
}

impl HasWindowHandle for OhosSurfaceTarget {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = self
            .window
            .raw_window_handle()
            .ok_or(HandleError::Unavailable)?;
        // SAFETY: the handle comes from a live `OHNativeWindow` reported by the
        // ability. `Window::detach_surface` drops the surface before the ability
        // releases the native window, so it stays valid for the surface's life.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// A bare display handle used to initialize the process-global wgpu instance.
///
/// `DisplayHandle::ohos()` carries no per-window state, so the instance can be
/// created before a surface exists. Unlike [`OhosSurfaceTarget`] this holds no
/// native pointers, so it is `Send + Sync` without any unsafe assertion.
#[derive(Debug)]
struct OhosDisplayHandle;

impl HasDisplayHandle for OhosDisplayHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::ohos())
    }
}

impl HasDisplayHandle for OhosSurfaceTarget {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::ohos())
    }
}

/// Downcasts a platform window to the OHOS implementation.
pub(super) fn downcast_window(window: &dyn platform::Window) -> &Window {
    window
        .as_any()
        .downcast_ref::<Window>()
        .expect("OHOS platform windows are always `ohos::windowing::Window`")
}
