//! hitshell: SSH client that talks to the on-device hitdaemon command server.
//!
//! The host application (whose app sandbox forbids exec'ing external programs) links this
//! crate: `util::command`'s OHOS path routes remote commands through a
//! [`SshCommandExecutor`] registered here, which connects to hitdaemon over
//! loopback SSH (management bootstrap on 40230, command pool on 40220). This
//! crate is self-contained: it depends on no other gpui_ohos crate and carries
//! its own command-execution contract ([`types`]).
//!
//! The `hitshell` binary in this package is the standalone bridge the terminal
//! runs by hand: it opens an interactive shell on hitdaemon (`/usr/bin/zsh`) and
//! connects the local terminal to it, which is how a sandboxed session reaches
//! programs installed by the system command-line tools.

pub mod bootstrap;
pub mod command;
pub mod endpoint;
pub mod executor;
pub mod keys;
pub mod pool;
pub mod protocol;
pub mod pty;
pub mod types;

use std::sync::{Arc, OnceLock};

pub use endpoint::CommandEndpoint;
pub use executor::SshCommandExecutor;
pub use pty::{RemotePty, ResizeHandle};
pub use types::{
    ExecSpec, FdMode, RemoteChild, RemoteCommandExecutor, ShellPtyFuture, Signal,
};

static EXECUTOR: OnceLock<Arc<dyn RemoteCommandExecutor>> = OnceLock::new();

/// Registers the process-wide remote command executor. Called once at startup
/// by launch-zed after the executor is constructed.
pub fn init_executor(executor: Arc<dyn RemoteCommandExecutor>) -> Result<(), ()> {
    EXECUTOR.set(executor).map_err(|_| ())
}

/// Returns the registered executor, or `None` if not yet initialized.
pub fn executor() -> Option<Arc<dyn RemoteCommandExecutor>> {
    EXECUTOR.get().cloned()
}

/// True once a command executor is registered and ready for use.
pub fn is_ready() -> bool {
    EXECUTOR.get().is_some()
}
