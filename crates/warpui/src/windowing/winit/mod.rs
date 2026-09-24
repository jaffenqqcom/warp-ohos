#[cfg(winit)]
pub(crate) mod app;
#[cfg(winit)]
pub mod delegate;
#[cfg(winit)]
mod event_loop;
// The cosmic-text font stack below is not winit-specific: only font enumeration is,
// and OHOS substitutes its own for the fontconfig-backed one. The stack is therefore
// shared between the winit platforms and OHOS rather than duplicated.
#[cfg(any(winit, ohos))]
pub(crate) mod fonts;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[cfg(winit)]
pub mod linux;

#[cfg(winit)]
mod notifications;
#[cfg(target_family = "wasm")]
pub mod wasm;

#[cfg(winit)]
mod window;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(winit)]
use app::CustomEvent;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[cfg(winit)]
pub use app::WindowingSystem;
#[cfg(winit)]
use event_loop::EventLoop;
#[cfg(winit)]
use window::Window;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[cfg(winit)]
pub use window::get_os_window_manager_name;
