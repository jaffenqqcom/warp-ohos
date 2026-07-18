use std::mem::ManuallyDrop;

use futures_util::future::LocalBoxFuture;
use pathfinder_geometry::vector::Vector2F;
use warpui_core::assets::AssetProvider;
use warpui_core::integration::TestDriver;
use warpui_core::platform::app::TerminationResult;
use warpui_core::platform::{self, TerminationMode, WindowManager};
use warpui_core::AppContext;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoop;

use super::fonts::OhosFontDB;
use crate::windowing::ohos::{OhosDelegate, OhosWindowManager};
use crate::windowing::winit::app::CustomEvent;
use crate::windowing::winit::EventLoop as WinitEventLoop;

pub struct App {
    callbacks: platform::AppCallbacks,
    assets: Box<dyn AssetProvider>,
    #[allow(dead_code)]
    is_integration_test: bool,
}

impl App {
    pub(in crate::platform) fn new(
        callbacks: platform::AppCallbacks,
        assets: Box<dyn AssetProvider>,
        test_driver: Option<&TestDriver>,
    ) -> Self {
        Self {
            callbacks,
            assets,
            is_integration_test: test_driver.is_some(),
        }
    }

    pub(in crate::platform) fn run(
        self,
        init_fn: impl FnOnce(&mut AppContext, LocalBoxFuture<'static, crate::App>) + 'static,
    ) -> TerminationResult {
        use winit::platform::ohos::EventLoopBuilderExtOpenHarmony;

        let ohos_app = crate::windowing::ohos::get_warp_app()
            .expect("WARP_APP not initialized — init_ability must run before App::run")
            .clone();

        // ── 创建 winit EventLoop（OHOS 后端 + CustomEvent 类型） ────────────
        let mut builder = EventLoop::<CustomEvent>::with_user_event();
        builder.with_openharmony_app(ohos_app);
        let event_loop = builder
            .build()
            .expect("OhosApp::run: failed to create winit EventLoop");

        let _display_handle = event_loop.owned_display_handle();
        // 初始化全局 wgpu Instance（StandardWindow::open_window 内部
        // Resources::new() 会通过 get_wgpu_instance() 获取它）。
        crate::rendering::wgpu::init_wgpu_instance(Box::new(_display_handle));

        // ── 创建 OHOS 平台组件 ─────────────────────────────────────────────
        let wm = Box::new(OhosWindowManager::new(event_loop.create_proxy()));
        let delegate = Box::new(OhosDelegate::new());

        let wm_ptr = Box::into_raw(wm);
        let delegate_ptr = Box::into_raw(delegate);
        crate::windowing::ohos::set_window_manager_ptr(wm_ptr);
        crate::windowing::ohos::set_delegate_ptr(delegate_ptr);

        let wm = unsafe { Box::from_raw(wm_ptr) };
        let delegate = unsafe { Box::from_raw(delegate_ptr) };

        let font_db = Box::new(OhosFontDB::new());
        let font_db_ptr = Box::into_raw(font_db);
        crate::windowing::ohos::set_font_db_ptr(font_db_ptr);
        let font_db = unsafe { Box::from_raw(font_db_ptr) };

        let ui_app = crate::App::new(delegate, wm, font_db, self.assets)
            .expect("OhosApp: crate::App::new failed");

        // ── 创建标准 EventLoop 封装（CustomEvent::OpenWindow 现在能正确投递） ──
        let inner_event_loop = WinitEventLoop::new(
            ui_app,
            self.callbacks,
            Box::new(init_fn),
            None,
            event_loop.create_proxy(),
        );
        let mut inner_event_loop = ManuallyDrop::new(inner_event_loop);

        // ── 运行事件循环（所有事件由标准层处理） ──────────────────────────
        event_loop
            .run(move |evt, window_target| {
                // 只拦截 OHOS 特有生命周期事件
                match &evt {
                    Event::Resumed => {
                        log::info!("OhosApp::Resumed");
                        // 设置 OH_NativeWindow 缓冲区几何尺寸（SET_BUFFER_GEOMETRY）。
                        // winit::Window 的 raw_window_handle 已指向真实 native_window，
                        // 但缓冲区队列尺寸需单独配置。
                        if let Some(wm) = crate::windowing::ohos::get_window_manager() {
                            let (w, h) = wm.shared_state().surface_size();
                            if w > 0 && h > 0 {
                                wm.update_surface_size(Vector2F::new(w as f32, h as f32));
                            }
                        }
                    }
                    Event::Suspended => {
                        log::info!("OhosApp::Suspended");
                    }
                    Event::AboutToWait => {
                        if let Some(d) = crate::windowing::ohos::get_delegate() {
                            let tasks = d.drain_pending_dispatch_tasks();
                            if !tasks.is_empty() {
                                for task in tasks {
                                    task.run();
                                }
                            }
                        }
                    }
                    _ => {}
                }
                inner_event_loop.handle_event(evt, window_target);
            })
            .expect("OhosApp: event loop failed unexpectedly");

        crate::windowing::ohos::wait_for_termination();

        Ok(())
    }
}
