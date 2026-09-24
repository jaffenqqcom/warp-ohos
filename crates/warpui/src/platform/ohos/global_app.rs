//! Process-global hand-off of the OHOS ability app to the platform back-end.
//!
//! The ArkTS host builds the ability app on the UI thread and calls the NAPI
//! `launch_app` entry, which runs before any warpui type exists. The platform
//! back-end is constructed later, from inside `warp::run()`, so it cannot take
//! the app as a constructor argument without threading `OpenHarmonyApp` through
//! every layer in between. It picks the app up from this slot instead, which
//! keeps the native ability type out of the framework's public API.
//!
//! The slot is thread-local, and two threads need it:
//!
//! * The ability thread stores the app in `launch_app` and reads it back in
//!   [`super::spawn`], which runs there.
//! * The `warp-main` thread stores its own copy in [`super::spawn`] before
//!   running warp, because every platform-layer lookup happens on that thread.

use std::cell::RefCell;

use openharmony_ability::OpenHarmonyApp;

thread_local! {
    static GLOBAL_APP: RefCell<Option<OpenHarmonyApp>> = const { RefCell::new(None) };
}

/// Stores `app` for the platform back-end to pick up.
pub fn set_global_app(app: OpenHarmonyApp) {
    log::info!(
        "ohos::global_app::set_global_app: module_name={:?}",
        app.module_name()
    );
    GLOBAL_APP.with(|slot| *slot.borrow_mut() = Some(app));
}

/// Returns the app stored by [`set_global_app`] on this thread, if any.
pub fn global_app() -> Option<OpenHarmonyApp> {
    GLOBAL_APP.with(|slot| {
        let app = slot.borrow();
        log::debug!("ohos::global_app::global_app: present={}", app.is_some());
        app.clone()
    })
}
