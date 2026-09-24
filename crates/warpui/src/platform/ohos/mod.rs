//! OHOS platform back-end.
//!
//! HarmonyOS NEXT reports `target_os = "linux"` together with
//! `target_env = "ohos"`, so it is selected by the `ohos` cfg alias rather than
//! by the OS triple. The back-end is hosted by the ArkTS ability: the window,
//! surface and input callbacks arrive through `openharmony-ability`, and
//! rendering goes through the wgpu GL back-end on the surface the ability
//! reports.
//!
//! The ArkTS (ability) thread services every platform callback and must never
//! block, so warp's blocking event loop runs on a separate `warp-main` thread
//! that [`spawn`] starts. See [`app`] and [`event_loop`] for the hand-off.

mod app;
mod delegate;
mod event_loop;
mod global_app;
mod hotkey;
mod windowing;

pub mod clipboard;
pub mod fonts;
pub mod keycodes;

pub use app::{App, spawn};
/// Re-exported for the headless back-end, whose `open_url` has no OHOS
/// implementation of its own.
pub(crate) use delegate::open_url_in_system;
pub use global_app::{global_app, set_global_app};
