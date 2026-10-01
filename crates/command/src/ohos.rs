//! Command routing for HarmonyOS.
//!
//! The application runs in a sandbox that cannot exec the system's own
//! programs. A program that resolves inside the sandbox -- the bundled tools
//! under `/data/app/bin` and the application's own binary -- is therefore run
//! directly, while anything else is run through the `hitshell` bridge: the
//! bridge is started locally with [`PIPE_EXEC_FLAG`] and the program's name, it
//! asks `hitdaemon` (which runs outside the sandbox) to run that program, and
//! it connects the two sides' standard streams in both directions.
//!
//! Routing by resolvability, rather than by a list of names, is what keeps this
//! from needing a case per tool: whatever the sandbox can resolve runs here,
//! and everything else is the bridge's business. It also covers the programs
//! that must run here -- the bridge itself, the bundled shell, the application
//! re-executing itself -- without naming any of them.

use std::ffi::{CString, OsStr, OsString};
use std::path::{Path, PathBuf};

/// Where the bridge binary lands on device: the private `hitshell.hnp` unpacks
/// into the application's own bin directory.
const HITSHELL_PATH_DEFAULT: &str = "/data/app/bin/hitshell";
/// The argument that selects the bridge's pipe-exec mode. Kept in step with the
/// bridge: it is the first argument, followed by the program and its arguments.
pub(crate) const PIPE_EXEC_FLAG: &str = "--pipe-exec";
/// The variable naming the environment variables to hand the remote program,
/// separated by [`ENV_KEY_SEPARATOR`]. The variables themselves are set on this
/// command; this only says which of them the program is meant to see.
pub(crate) const ENV_KEYS_VARIABLE: &str = "WARP_HITSHELL_PIPE_ENV";
/// Separator between the names in [`ENV_KEYS_VARIABLE`]. A newline cannot occur
/// in an environment variable name.
pub(crate) const ENV_KEY_SEPARATOR: char = '\n';

/// Where a command's program will run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// In this sandbox, where the program resolves.
    Local,
    /// Through the bridge, on hitdaemon.
    Bridged,
}

/// The routing state a command carries on HarmonyOS.
///
/// The inner command cannot report back which program and arguments the caller
/// chose (once a routed command is pointed at the bridge, its program is the
/// bridge), so they are kept here. The environment is kept for the same reason:
/// the bridge is told which variables to hand on by name.
#[derive(Debug)]
pub(crate) struct Routing {
    /// The program the caller asked for.
    program: OsString,
    placement: Placement,
    /// The arguments the caller added, in order.
    args: Vec<OsString>,
    /// The environment variables the caller set, in the order they were set.
    environment: Vec<(OsString, OsString)>,
    /// Whether the bridge has been told about the environment for this run.
    applied: bool,
}

impl Routing {
    fn new(program: OsString, placement: Placement) -> Self {
        Self {
            program,
            placement,
            args: Vec::new(),
            environment: Vec::new(),
            applied: false,
        }
    }

    /// The program the caller asked for.
    pub(crate) fn program(&self) -> &OsStr {
        &self.program
    }

    /// Whether this command runs through the bridge.
    pub(crate) fn is_bridged(&self) -> bool {
        self.placement == Placement::Bridged
    }

    /// The arguments the caller added, in order.
    pub(crate) fn args(&self) -> &[OsString] {
        &self.args
    }

    /// Records an argument the caller added.
    pub(crate) fn push_arg(&mut self, arg: &OsStr) {
        self.args.push(arg.to_owned());
    }

    /// Records an environment variable the caller set.
    pub(crate) fn set_environment(&mut self, key: &OsStr, value: &OsStr) {
        match self.environment.iter_mut().find(|(name, _)| name == key) {
            Some(entry) => entry.1 = value.to_owned(),
            None => self
                .environment
                .push((key.to_owned(), value.to_owned())),
        }
    }

    /// Forgets an environment variable the caller removed.
    pub(crate) fn remove_environment(&mut self, key: &OsStr) {
        self.environment.retain(|(name, _)| name != key);
    }

    /// Forgets every environment variable the caller cleared.
    pub(crate) fn clear_environment(&mut self) {
        self.environment.clear();
    }

    /// Writes the bridge metadata, once, before an async run.
    pub(crate) fn apply_to_async(&mut self, command: &mut async_process::Command) {
        if !self.is_bridged() || self.applied {
            return;
        }
        command.env(ENV_KEYS_VARIABLE, self.environment_keys());
        self.applied = true;
    }

    /// Writes the bridge metadata, once, before a blocking run.
    pub(crate) fn apply_to_blocking(&mut self, command: &mut std::process::Command) {
        if !self.is_bridged() || self.applied {
            return;
        }
        command.env(ENV_KEYS_VARIABLE, self.environment_keys());
        self.applied = true;
    }

    /// The names of the environment variables to hand the remote program.
    ///
    /// A name that is not a plain identifier is dropped: the bridge passes these
    /// names to the daemon, which writes them into the shell command it runs, so
    /// anything else there would be read as shell syntax. Such a name cannot
    /// denote an environment variable in the first place.
    fn environment_keys(&self) -> String {
        let mut keys = String::new();
        for (key, _) in &self.environment {
            let name = key.to_string_lossy();
            if !is_environment_name(&name) {
                continue;
            }
            if !keys.is_empty() {
                keys.push(ENV_KEY_SEPARATOR);
            }
            keys.push_str(&name);
        }
        keys
    }
}

/// Whether `name` is a POSIX environment variable name: a letter or underscore
/// followed by letters, digits, and underscores.
fn is_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

/// Decides where `program` will run.
pub(crate) fn placement(program: &OsStr) -> Placement {
    if is_local(program) {
        Placement::Local
    } else {
        Placement::Bridged
    }
}

/// The program to start and the arguments to place before the caller's own.
pub(crate) fn invocation(program: &OsStr, placement: Placement) -> (OsString, Vec<OsString>) {
    match placement {
        Placement::Local => (program.to_owned(), Vec::new()),
        Placement::Bridged => (
            hitshell_path(),
            vec![OsString::from(PIPE_EXEC_FLAG), program.to_owned()],
        ),
    }
}

/// Builds the inner async command for `program`, with its routing state.
pub(crate) fn start_async_command(program: &OsStr) -> (async_process::Command, Routing) {
    let placement = placement(program);
    let (start, prefix_args) = invocation(program, placement);
    let mut command = async_process::Command::new(start);
    command.args(prefix_args);
    (command, Routing::new(program.to_owned(), placement))
}

/// Builds the `std::process::Command` a constructor converts into the inner
/// async command, with its routing state.
pub(crate) fn start_blocking_command(program: &OsStr) -> (std::process::Command, Routing) {
    let placement = placement(program);
    let (start, prefix_args) = invocation(program, placement);
    let mut command = std::process::Command::new(start);
    command.args(prefix_args);
    (command, Routing::new(program.to_owned(), placement))
}

/// The bridge binary's path.
fn hitshell_path() -> OsString {
    std::env::var_os("WARP_HITSHELL_PATH").unwrap_or_else(|| OsString::from(HITSHELL_PATH_DEFAULT))
}

/// Whether `program` can be run inside this sandbox.
fn is_local(program: &OsStr) -> bool {
    let path = Path::new(program);
    if path.as_os_str().is_empty() {
        // An empty program is the caller's error; run it here so the spawn
        // reports it rather than handing it to the bridge.
        return true;
    }
    if path.components().count() > 1 {
        return executable(path);
    }
    search_path()
        .iter()
        .any(|directory| executable(&directory.join(path)))
}

/// The directories in this process's search path.
fn search_path() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Whether this process may execute the file at `path`.
fn executable(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    let Ok(path) = CString::new(path) else {
        return false;
    };
    // SAFETY: `path` is a NUL-terminated C string alive for the call.
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
}
