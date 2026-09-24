//! The OHOS application back-end.
//!
//! The ability app and warp's event loop live on different threads: the ability
//! (ArkTS) thread services platform callbacks and must never block, while warp's
//! loop runs until termination. [`spawn`] wires the two together and is called
//! from the NAPI entry, which itself runs on the ability thread.

use std::cell::RefCell;

use anyhow::{Context as _, anyhow};
use futures::future::LocalBoxFuture;

use super::delegate::{self, AppDelegate};
use super::event_loop::{self, EventReceiver};
use super::fonts::FontDB;
use super::windowing::WindowManager;
use crate::integration::TestDriver;
use crate::platform::app::TerminationResult;
use crate::platform::{self};
use crate::{AppContext, AssetProvider};

thread_local! {
    /// The queue the ability thread pushes onto, handed to the warp main thread
    /// by [`spawn`] and picked up by [`App::run`].
    static EVENT_RECEIVER: RefCell<Option<EventReceiver>> = const { RefCell::new(None) };
}

/// Starts warp's event loop on a dedicated thread and connects the ability's
/// callback loop to it.
///
/// Returns as soon as the thread is running, because the caller is the ability
/// thread and blocking it would stall every platform callback. Call this after
/// [`super::set_global_app`].
pub fn spawn(run: impl FnOnce() + Send + 'static) -> anyhow::Result<()> {
    let app = super::global_app().ok_or_else(|| {
        anyhow!("the OHOS ability app must be installed before the event loop is started")
    })?;

    let (sender, receiver) = event_loop::channel();
    event_loop::register(&app, sender);

    std::thread::Builder::new()
        .name("warp-main".to_owned())
        .spawn(move || {
            // The slot is thread-local, so the warp main thread installs its own
            // copy: every platform-layer lookup below runs on this thread.
            super::set_global_app(app);
            EVENT_RECEIVER.with(|slot| *slot.borrow_mut() = Some(receiver));
            run();
        })
        .context("failed to start the warp main thread")?;
    Ok(())
}

fn take_event_receiver() -> Option<EventReceiver> {
    EVENT_RECEIVER.with(|slot| slot.borrow_mut().take())
}

pub struct App {
    callbacks: platform::app::AppCallbacks,
    assets: Box<dyn AssetProvider>,
    query_microphone_access: bool,
}

impl App {
    pub(in crate::platform) fn new(
        callbacks: platform::app::AppCallbacks,
        assets: Box<dyn AssetProvider>,
        test_driver: Option<&TestDriver>,
    ) -> Self {
        // Other platforms swap in an alternate delegate when running integration
        // tests; OHOS has no such delegate.
        let _ = test_driver;
        Self {
            callbacks,
            assets,
            query_microphone_access: false,
        }
    }

    pub(in crate::platform) fn enable_microphone_access_query(&mut self) {
        self.query_microphone_access = true;
    }

    pub(in crate::platform) fn run(
        self,
        init_fn: impl FnOnce(&mut AppContext, LocalBoxFuture<'static, crate::App>) + 'static,
    ) -> TerminationResult {
        let App {
            callbacks,
            assets,
            query_microphone_access,
        } = self;

        delegate::mark_current_thread_as_main();

        let app = super::global_app().ok_or_else(|| {
            anyhow!("the OHOS ability app is not installed on the warp main thread")
        })?;
        let receiver = take_event_receiver()
            .ok_or_else(|| anyhow!("the OHOS event queue was not installed before `run`"))?;
        let sender = receiver.sender().clone();

        let platform_delegate = Box::new(AppDelegate::new(
            app.clone(),
            sender.clone(),
            query_microphone_access,
        ));
        let window_manager = Box::new(WindowManager::new(app, sender));
        let font_db: Box<dyn platform::FontDB> = Box::new(FontDB::new());

        let ui_app = crate::App::new(platform_delegate, window_manager, font_db, assets)
            .expect("should not fail to construct application");

        let mut callbacks =
            warpui_core::platform::app::AppCallbackDispatcher::new(callbacks, ui_app.clone());

        event_loop::run(ui_app, &mut callbacks, Box::new(init_fn), receiver)
    }
}
