//! Running one program on hitdaemon with piped stdio.
//!
//! The host application runs in a sandbox that cannot exec the system's own
//! programs. This mode is what the application's process-spawning wrapper uses
//! on OHOS: it is handed a program and its arguments as argv, asks hitdaemon to
//! run that program, and connects this process's own standard streams to the
//! remote process's -- both directions, for as long as the program runs. That
//! is what a language server needs: it reads requests on its stdin and writes
//! replies on its stdout for the whole session, so neither direction may be
//! closed early.
//!
//! When hitdaemon is not running the program is run locally instead, so a
//! program that does exist inside the sandbox still runs.

use std::collections::HashMap;
use std::os::unix::process::CommandExt;

use hitshell::keys::{MGMT_CLIENT_KEY, MGMT_HOST_PUB};
use hitshell::{CommandEndpoint, ExecSpec, RemoteCommandExecutor, SshCommandExecutor};

use crate::{
    EXIT_WITHOUT_STATUS, HITDAEMON_NOT_READY_MESSAGE, READY_TIMEOUT, relay_to_local, relay_to_remote,
    working_directory,
};

/// The argument that selects this mode.
///
/// It has to be the first argument: the program being run brings its own
/// arguments, and those may well include `--help` or `--log`, which this
/// binary's entry point scans for. Being first, and being a name no program is
/// invoked as, keeps the two apart.
pub(crate) const PIPE_EXEC_FLAG: &str = "--pipe-exec";

/// Names the environment variables to hand the remote program, separated by
/// [`ENV_KEY_SEPARATOR`]. The caller sets the variables themselves on this
/// process; this only says which of them the program is meant to see.
pub(crate) const ENV_KEYS_VARIABLE: &str = "WARP_HITSHELL_PIPE_ENV";
/// Separator between the names in [`ENV_KEYS_VARIABLE`]. A newline cannot occur
/// in an environment variable name.
pub(crate) const ENV_KEY_SEPARATOR: char = '\n';

/// Status reported for a program that could not be run at all, matching what a
/// shell reports for a command it could not find.
const EXIT_COMMAND_NOT_FOUND: i32 = 127;

/// Environment variables that are never handed to the remote program.
///
/// They name paths inside this side's sandbox (`HOME`, the temporary
/// directories) or a search path that does not hold the daemon's programs;
/// forwarding any of them would point the program at the wrong place, so the
/// daemon's own value is kept instead.
const NOT_FORWARDED: &[&str] = &["PATH", "HOME", "TMPDIR", "TMPPREFIX", "TERMINFO"];

/// Runs the program named by `arguments` on hitdaemon and returns the status
/// this process should exit with.
pub(crate) fn run(arguments: &[String]) -> Result<i32, String> {
    let Some((program, args)) = arguments.split_first() else {
        return Err(format!("{PIPE_EXEC_FLAG} needs a program to run"));
    };

    let executor = SshCommandExecutor::new_terminal(
        CommandEndpoint::ohos_default(),
        MGMT_CLIENT_KEY.to_string(),
        MGMT_HOST_PUB.to_string(),
    )
    .map_err(|err| format!("cannot start the bridge: {err}"))?;
    if let Err(err) = executor.wait_ready(READY_TIMEOUT) {
        log::error!("hitshell: hitdaemon is not ready for a pipe exec: {err}");
        eprintln!("hitshell: {HITDAEMON_NOT_READY_MESSAGE}");
        return exec_locally(program, args);
    }

    let mut spec = ExecSpec::new(program);
    spec.args = args.to_vec();
    spec.cwd_path = working_directory();
    // The three stream modes keep their `Piped` default: the program is a
    // client of this process's streams, on both sides.
    spec.env = forwarded_environment();

    let mut child = executor
        .spawn(spec)
        .map_err(|err| format!("cannot run {program} on hitdaemon: {err}"))?;
    let session_id = child.session_id;

    let mut remote_stdin = child
        .stdin
        .take()
        .ok_or_else(|| "hitdaemon accepted no input stream".to_string())?;
    let mut remote_stdout = child
        .stdout
        .take()
        .ok_or_else(|| "hitdaemon returned no output stream".to_string())?;
    let mut remote_stderr = child
        .stderr
        .take()
        .ok_or_else(|| "hitdaemon returned no error stream".to_string())?;

    let mut local_stdout = smol::Unblock::new(std::io::stdout());
    let mut local_stderr = smol::Unblock::new(std::io::stderr());
    let mut local_stdin = smol::Unblock::new(std::io::stdin());

    smol::block_on(async move {
        // Input is relayed by a task: it ends when this process's input closes,
        // which is after the program has already exited, so awaiting it here
        // would wait on the wrong side.
        let input = smol::spawn(async move {
            // A failed write here is the program closing its input, which is how
            // a run normally ends; it is recorded, not treated as a failure.
            if let Err(err) = relay_to_remote(&mut local_stdin, &mut remote_stdin).await {
                log::debug!("hitshell: input relay ended early: {err}");
            }
        });
        // The output streams end when the program exits, at which point the
        // daemon closes the channel -- that is the end of the whole run.
        let (stdout, stderr) = smol::future::zip(
            relay_to_local(&mut remote_stdout, &mut local_stdout),
            relay_to_local(&mut remote_stderr, &mut local_stderr),
        )
        .await;
        input.cancel().await;
        stdout.and(stderr)
    })
    .map_err(|err| format!("cannot relay the program's streams: {err}"))?;

    let exit = smol::block_on(executor.wait_exit_async(session_id))
        .map_err(|err| format!("cannot read the program's exit status: {err}"))?;
    let code = exit.unwrap_or(EXIT_WITHOUT_STATUS);
    Ok(code)
}

/// Runs `program` inside this sandbox, for when hitdaemon is unreachable.
///
/// The program is run exactly as asked: if it does exist inside the sandbox it
/// still runs, which is what makes this a fallback rather than a failure. It
/// replaces this process on success, and reports a missing program the way a
/// shell does otherwise.
fn exec_locally(program: &str, args: &[String]) -> Result<i32, String> {
    log::info!("hitshell: running {program} locally, hitdaemon is not available");
    let mut command = std::process::Command::new(program);
    command.args(args);
    let err = command.exec();
    log::error!("hitshell: cannot run {program} locally: {err}");
    eprintln!("hitshell: cannot run {program}: {err}");
    Ok(EXIT_COMMAND_NOT_FOUND)
}

/// The environment variables to hand the remote program.
///
/// Only the variables the caller set explicitly are forwarded; their names
/// arrive in [`ENV_KEYS_VARIABLE`]. Everything else the program sees is the
/// daemon's own environment, which is where the paths it needs are real.
fn forwarded_environment() -> HashMap<String, String> {
    let mut environment = HashMap::new();
    let Some(names) = std::env::var_os(ENV_KEYS_VARIABLE) else {
        return environment;
    };
    for name in names.to_string_lossy().split(ENV_KEY_SEPARATOR) {
        if name.is_empty() || NOT_FORWARDED.contains(&name) {
            continue;
        }
        if let Some(value) = std::env::var_os(name) {
            environment.insert(name.to_string(), value.to_string_lossy().into_owned());
        }
    }
    environment
}
