//! hitshell: an interactive shell on hitdaemon, bridged to this terminal.
//!
//! The host application runs in a sandbox that cannot exec programs installed by
//! the system command-line tools. hitdaemon runs outside that sandbox, so this
//! binary opens an interactive pty on it and connects the local terminal to that
//! pty: what is typed here reaches the shell, what the shell prints reaches the
//! terminal, and window changes are forwarded. The session therefore runs with
//! hitdaemon's permissions, which is how a sandboxed session reaches the
//! system's own programs.
//!
//! The management keys are compiled in (see `hitshell::keys`), so the bridge
//! needs no configuration. Without hitdaemon the bridge reports the failure and
//! then replaces itself with the zsh bundled in the private `zsh.hnp`, so a
//! terminal started before the daemon still gets a shell.

mod logger;

use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::time::Duration;

use hitshell::keys::{MGMT_CLIENT_KEY, MGMT_HOST_PUB};
use hitshell::{
    CommandEndpoint, ExecSpec, FdMode, RemoteCommandExecutor, RemotePty, SshCommandExecutor,
};
use smol::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Program hitdaemon runs on the pty. Absolute, so hitdaemon's own PATH cannot
/// decide which shell the bridge ends up talking to.
const SHELL_PROGRAM: &str = "/usr/bin/zsh";
/// zsh bundled in the private `zsh.hnp`, at the path this sandbox may exec. Used
/// when hitdaemon is not running, so the caller still ends up with a shell.
const FALLBACK_SHELL_PATH: &str = "/data/app/bin/zsh";
/// Arguments used when the caller passed none, which is the case when hitshell is
/// run by hand. They mirror the invocation the host application uses for its own
/// shells. Without `--no-rcs` the device's `/etc/zshrc` loads a system shell
/// plugin that hijacks the ZLE widgets and then stalls on a sandbox path
/// hitdaemon cannot reach, freezing input on the pty (Enter stops responding
/// until Ctrl-C).
const DEFAULT_SHELL_ARGS: &[&str] = &["-g", "--no-rcs"];
/// Reported when hitdaemon cannot be reached. Kept verbatim: it is the only thing
/// that tells the user how to get the privileged session back.
const HITDAEMON_NOT_READY_MESSAGE: &str = "hitdaemon is not ready, start hitdaemon from the system command line first. 请先在系统终端工具启动hitdaemon程序。";
/// How long to wait for the management handshake before giving up. A daemon that
/// is not running is reported at once, because the connection is refused; this
/// only bounds a listener that accepts and then never answers.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the local window size is compared against the size the remote pty
/// was last told.
const RESIZE_POLL: Duration = Duration::from_millis(200);
/// Terminal size assumed when the local terminal reports none (not a terminal,
/// or a zero-sized window).
const FALLBACK_COLS: u32 = 80;
const FALLBACK_ROWS: u32 = 24;
/// Bytes moved per relay iteration.
const RELAY_CHUNK: usize = 8192;
/// Status this process exits with when a command ended without a status of its
/// own -- killed by a signal, or its session dropped. 128 is what a shell
/// reports for an unknown termination, and it is non-zero, so the caller does
/// not read the run as a success.
const EXIT_WITHOUT_STATUS: i32 = 128;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        print_help();
        return;
    }
    let logging = args.iter().any(|arg| arg == "--log");
    logger::init(logging);
    match run(&args) {
        // The status is handed on rather than flattened: a caller that ran a
        // one-off command reads it to tell a command that ran from one that did
        // not, and an interactive session has no status of its own (0).
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // The same reason the logger is always installed: on the device there
            // is no console this text reaches, so the failure has to be recorded
            // there too or nothing about it survives.
            log::error!("hitshell: {err}");
            // This is the whole output of a failed run, so it prints with or
            // without --log.
            eprintln!("hitshell: {err}");
            std::process::exit(1);
        }
    }
}

/// Prints what this binary accepts. Runs before a logger is installed, so
/// `--help` reads the same whatever else is on the command line.
fn print_help() {
    println!("hitshell - run an interactive shell on hitdaemon from this terminal");
    println!();
    println!("Usage: hitshell [--log] [--help]");
    println!();
    println!("Opens {SHELL_PROGRAM} on the running hitdaemon and connects it to the current");
    println!("terminal, or runs one `-c` command on that shell and hands back what it printed");
    println!("and the status it exited with. Without hitdaemon the bridge prints the failure and");
    println!("replaces itself with {FALLBACK_SHELL_PATH} instead.");
    println!();
    println!("Options:");
    println!("  --log   Add debug-level diagnostics to hilog (on the device). Without");
    println!("          it the key nodes of a run are still recorded there.");
    println!("  --help  Print this help and exit.");
}

fn run(args: &[String]) -> Result<i32, String> {
    match requested_invocation(args) {
        Invocation::Interactive(requested) => run_interactive(&requested),
        Invocation::Command(shell_args) => run_command(&shell_args),
    }
}

/// Runs an interactive shell on hitdaemon and bridges this terminal to it.
fn run_interactive(requested: &RequestedShell) -> Result<i32, String> {
    log::info!(
        "hitshell: requested argv0={:?} args={}",
        requested.argv0,
        requested.args.join(" ")
    );

    // A terminal executor: this process runs exactly one pty for its whole life,
    // so the pool keeps no connection ready and opens only the one the pty needs.
    let executor = SshCommandExecutor::new_terminal(
        CommandEndpoint::ohos_default(),
        MGMT_CLIENT_KEY.to_string(),
        MGMT_HOST_PUB.to_string(),
    )
    .map_err(|err| format!("cannot start the bridge: {err}"))?;
    if let Err(err) = executor.wait_ready(READY_TIMEOUT) {
        log::error!("hitshell: hitdaemon is not ready: {err}");
        // The terminal has to end up with a shell either way, so say why the
        // bridge is stepping aside before becoming that shell.
        eprintln!("hitshell: {HITDAEMON_NOT_READY_MESSAGE}");
        return exec_fallback_shell(requested.argv0.as_deref(), &requested.args);
    }

    // Read the terminal size as late as this side can. The host application lays
    // the terminal out after it starts the shell, so a size read before the
    // handshake is often one this terminal has already left behind; a shell
    // started on such a size prints `%` (its `PROMPT_EOL_MARK`) on a line of its
    // own at the first prompt, too early for the correction in `bridge` to reach
    // it.
    let (cols, rows) = local_size().unwrap_or((FALLBACK_COLS, FALLBACK_ROWS));
    log::info!("hitshell: opening {SHELL_PROGRAM} on hitdaemon at {cols}x{rows}");

    // No working directory is requested: the daemon would `cd` into a path from
    // this side's sandbox, which is not a place its own account can use. The
    // shell starts where hitdaemon runs, and zsh resolves its own HOME.
    let pty = smol::block_on(executor.open_shell_pty(
        cols,
        rows,
        None,
        SHELL_PROGRAM,
        &requested.args,
    ))
    .map_err(|err| format!("cannot open a shell on hitdaemon: {err}"))?;
    log::info!("hitshell: shell pty open; bridging");

    // Raw mode only once the shell is up: a failure above leaves the terminal
    // untouched, so the error message stays readable. The guard restores the
    // saved settings when `run_interactive` returns.
    let _raw = RawMode::enable(libc::STDIN_FILENO)?;
    bridge(pty, cols, rows).map(|()| 0)
}

/// Runs one command the caller asked for with `-c` on the same shell the
/// terminal runs, and returns the status this process should exit with.
///
/// The command is a shell's own: the caller reads what it printed, and its
/// status decides whether that output is usable (the terminal only accepts a
/// list of executables from a run that succeeded). Running it on an interactive
/// pty instead would corrupt both -- a pty turns every `\n` into `\r\n`, and a
/// failure to set up the terminal would be reported in place of the command's
/// own outcome.
fn run_command(shell_args: &[String]) -> Result<i32, String> {
    // The caller starts this process in the directory the command is meant to run
    // in -- the terminal's own working directory -- so the command is given the
    // same one. A child of the daemon would otherwise start in the daemon's own
    // directory, where a command like `git status` reads the wrong repository, or
    // none at all. The interactive bridge deliberately does not do this: there
    // the caller chose no directory, and this process's own is the host
    // application's.
    let cwd = working_directory();
    log::info!(
        "hitshell: running {SHELL_PROGRAM} {args} as a command in {cwd:?}",
        args = shell_args.join(" ")
    );

    // A terminal executor here too: this process serves one command and exits,
    // so a stock of ready connections would only open connections nobody pops.
    let executor = SshCommandExecutor::new_terminal(
        CommandEndpoint::ohos_default(),
        MGMT_CLIENT_KEY.to_string(),
        MGMT_HOST_PUB.to_string(),
    )
    .map_err(|err| format!("cannot start the bridge: {err}"))?;
    if let Err(err) = executor.wait_ready(READY_TIMEOUT) {
        log::error!("hitshell: hitdaemon is not ready for a command: {err}");
        eprintln!("hitshell: {HITDAEMON_NOT_READY_MESSAGE}");
        return exec_fallback_shell(None, shell_args);
    }

    let mut spec = ExecSpec::new(SHELL_PROGRAM);
    spec.args = shell_args.to_vec();
    spec.cwd_path = cwd;
    // The command is meant to see the environment its caller set up for it --
    // `PATH` above all, since it is what the command enumerates -- while the rest
    // of the environment stays the daemon's own. That is the one a program
    // outside the sandbox should start from: its `HOME` and the scratch
    // directory it is given per session are real paths there, while this side's
    // copies of those name sandbox paths (`TMPDIR` in particular, which the
    // daemon deliberately replaces per client).
    match std::env::var("PATH") {
        Ok(path) => {
            spec.env.insert("PATH".to_string(), path);
        }
        Err(err) => {
            log::warn!("hitshell: no PATH to hand the command: {err}");
        }
    }
    // A `-c` run is given no input by its caller (the terminal runs these with a
    // null stdin), so the command is given none either.
    spec.stdin_mode = FdMode::Null;

    let mut child = executor
        .spawn(spec)
        .map_err(|err| format!("cannot run the command on hitdaemon: {err}"))?;
    let session_id = child.session_id;
    log::info!("hitshell: command session={session_id} started");

    let copied: Result<(), String> = smol::block_on(async move {
        let mut child_stdout = child
            .stdout
            .take()
            .ok_or_else(|| "hitdaemon returned no command output stream".to_string())?;
        let mut child_stderr = child
            .stderr
            .take()
            .ok_or_else(|| "hitdaemon returned no command error stream".to_string())?;
        let mut local_stdout = smol::Unblock::new(std::io::stdout());
        let mut local_stderr = smol::Unblock::new(std::io::stderr());
        // Both streams are drained at once: waiting on one while the command
        // fills the other would block for good. The child -- and with it the
        // caller's end of its stdin -- is dropped when this block ends.
        let (stdout_done, stderr_done) = smol::future::zip(
            relay_to_local(&mut child_stdout, &mut local_stdout),
            relay_to_local(&mut child_stderr, &mut local_stderr),
        )
        .await;
        match (stdout_done, stderr_done) {
            (Err(err), _) | (_, Err(err)) => {
                Err(format!("cannot copy the command's output: {err}"))
            }
            (Ok(()), Ok(())) => Ok(()),
        }
    });
    copied?;

    let exit = smol::block_on(executor.wait_exit_async(session_id))
        .map_err(|err| format!("cannot read the command's exit status: {err}"))?;
    let code = exit.unwrap_or(EXIT_WITHOUT_STATUS);
    log::info!("hitshell: command session={session_id} exited with {code}");
    Ok(code)
}

/// What the caller asked this process to do.
enum Invocation {
    /// Start an interactive shell, the way the terminal starts this bridge as
    /// its shell.
    Interactive(RequestedShell),
    /// Run one command and report what it printed and the status it exited
    /// with: the caller started this process the way it starts a shell for a
    /// one-off command (`-c` with a command of its own) and reads the result.
    Command(Vec<String>),
}

/// The shell invocation the caller asked hitshell for.
struct RequestedShell {
    /// Name the shell reports as its own, which makes zsh a login shell. The
    /// terminal asks for `-zsh`. Only the local fallback can honour it: the
    /// hitdaemon path starts the shell through the daemon's `sh -c`, which has
    /// no way to set it.
    argv0: Option<String>,
    /// Arguments to hand the shell, exactly as the caller passed them.
    args: Vec<String>,
}

/// The invocation used when the caller passed none, i.e. when hitshell is run by
/// hand from a shell.
fn default_shell_invocation() -> RequestedShell {
    RequestedShell {
        argv0: None,
        args: DEFAULT_SHELL_ARGS.iter().map(|arg| arg.to_string()).collect(),
    }
}

/// Reads what to do out of `args`.
///
/// An interactive shell is asked for the way the terminal starts hitshell:
/// `hitshell -c "exec -a <argv0> '<program>' <args...>"`. Any other `-c` is a
/// command the caller wants run -- that is how the terminal runs a one-off
/// command -- and the invocation is a shell's own, so it is forwarded as it
/// stands. Nothing about the arguments is assumed: whatever the caller passes is
/// what the shell gets, on both the hitdaemon path and the local fallback. A
/// hand-run hitshell passes no `-c` and falls back to [`DEFAULT_SHELL_ARGS`].
fn requested_invocation(args: &[String]) -> Invocation {
    let Some(command) = args
        .iter()
        .position(|arg| arg == "-c")
        .and_then(|index| args.get(index + 1))
    else {
        log::info!(
            "hitshell: no -c argument, using the default {}",
            DEFAULT_SHELL_ARGS.join(" ")
        );
        return Invocation::Interactive(default_shell_invocation());
    };

    let mut words = split_shell_words(command).into_iter();
    if words.next().as_deref() != Some("exec") {
        log::info!("hitshell: -c {command:?} is a command to run, not a shell to start");
        return Invocation::Command(forwarded_shell_args(args));
    }
    let mut words = words.peekable();
    let mut argv0 = None;
    if words.peek().map(String::as_str) == Some("-a") {
        words.next();
        argv0 = words.next();
    }
    // The next word is the program the terminal wanted to start, which is this
    // bridge itself. The shell is what has to run instead, so it is dropped
    // rather than forwarded.
    let program = words.next();
    let args: Vec<String> = words.collect();
    log::info!(
        "hitshell: the caller asked to run {program:?} with argv0={argv0:?} and args={}",
        args.join(" ")
    );
    Invocation::Interactive(RequestedShell { argv0, args })
}

/// Every argument but this process's own, i.e. the whole invocation to hand the
/// shell. `--log` widens this process's logging and is not the shell's; `--help`
/// is answered before this point.
fn forwarded_shell_args(args: &[String]) -> Vec<String> {
    args.iter()
        .filter(|arg| arg.as_str() != "--log")
        .cloned()
        .collect()
}

/// Splits `command` into words the way the shell that generated it parses them:
/// whitespace separates words, quotes group them, and a backslash escapes the
/// next character.
///
/// The terminal builds its invocation as a command string for the shell's `-c`,
/// so recovering the arguments means re-reading that string under the same
/// rules. Only the quoting the terminal itself emits has to be understood.
fn split_shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\'' => {
                started = true;
                for quoted in characters.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    word.push(quoted);
                }
            }
            '"' => {
                started = true;
                while let Some(quoted) = characters.next() {
                    if quoted == '"' {
                        break;
                    }
                    if quoted == '\\' {
                        if let Some(escaped) = characters.next() {
                            word.push(escaped);
                        }
                        continue;
                    }
                    word.push(quoted);
                }
            }
            '\\' => {
                started = true;
                if let Some(escaped) = characters.next() {
                    word.push(escaped);
                }
            }
            character if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            character => {
                started = true;
                word.push(character);
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// Replaces this process with the bundled zsh running `args`, so the requested
/// shell or command still runs while hitdaemon is not running. Returns only when
/// the replacement failed, because `exec` does not come back on success.
fn exec_fallback_shell(argv0: Option<&str>, args: &[String]) -> Result<i32, String> {
    log::info!(
        "hitshell: replacing this process with {FALLBACK_SHELL_PATH}, argv0={argv0:?}, args={}",
        args.join(" ")
    );
    let mut command = std::process::Command::new(FALLBACK_SHELL_PATH);
    if let Some(argv0) = argv0 {
        command.arg0(argv0);
    }
    command.args(args);
    let err = command.exec();
    log::error!("hitshell: exec {FALLBACK_SHELL_PATH} failed: {err}");
    Err(format!(
        "cannot start the fallback shell {FALLBACK_SHELL_PATH}: {err}"
    ))
}

/// Copies bytes in both directions between the local terminal and the remote
/// shell until either side closes, forwarding window changes as they happen.
///
/// `opened_cols`/`opened_rows` are the dimensions the remote pty was opened with.
fn bridge(mut pty: RemotePty, opened_cols: u32, opened_rows: u32) -> Result<(), String> {
    let mut remote_stdout = pty
        .stdout
        .take()
        .ok_or_else(|| "hitdaemon returned no shell output stream".to_string())?;
    let mut remote_stdin = pty
        .stdin
        .take()
        .ok_or_else(|| "hitdaemon accepted no shell input stream".to_string())?;
    // `pty` itself stays alive below: it owns the resize channel the relay task
    // reads from, and dropping it would end the relay.
    let resize = pty.resize_handle();

    // The local side is blocking stdio driven on a worker thread, so a read that
    // waits for a keystroke cannot stall the copy running the other way.
    let mut local_stdout = smol::Unblock::new(std::io::stdout());
    let mut local_stdin = smol::Unblock::new(std::io::stdin());

    // The size to compare against is the one the remote pty was opened with, not
    // a fresh read of the local terminal: the host application lays this terminal
    // out after it starts the shell, so it very often reports a different size by
    // the time the bridge runs. Comparing against a fresh read would treat that
    // size as the baseline and never forward it, leaving the shell on the stale
    // one -- and a shell whose width exceeds the terminal's own prints the
    // `PROMPT_EOL_MARK` (`%`) on a line of its own at every prompt. The first
    // check runs before the first sleep so that size is sent before the shell
    // prints its first prompt.
    let (mut last_cols, mut last_rows) = (opened_cols, opened_rows);
    let resize_task = smol::spawn(async move {
        loop {
            if let Some((cols, rows)) = local_size() {
                if cols != last_cols || rows != last_rows {
                    log::debug!("hitshell: window is now {cols}x{rows}");
                    resize.resize(cols, rows);
                    last_cols = cols;
                    last_rows = rows;
                }
            }
            smol::Timer::after(RESIZE_POLL).await;
        }
    });

    let outcome = smol::block_on(async {
        let outcome = smol::future::or(
            async {
                relay_to_local(&mut remote_stdout, &mut local_stdout)
                    .await
                    .map(|_| "the remote shell closed the session")
            },
            async {
                relay_to_remote(&mut local_stdin, &mut remote_stdin)
                    .await
                    .map(|_| "the local terminal closed its input")
            },
        )
        .await;
        // The poller loops for as long as the bridge lives, so it is cancelled
        // rather than awaited to completion.
        let _ = resize_task.cancel().await;
        outcome
    });
    match outcome {
        Ok(why) => {
            log::info!("hitshell: bridge ended: {why}");
            Ok(())
        }
        Err(err) => Err(format!("the session ended with an error: {err}")),
    }
}

/// Copies remote shell output into the local terminal.
///
/// The flush after every chunk is required, not cosmetic: `std::io::stdout` is
/// line-buffered, so a prompt or a key echo (neither ends in a newline) would
/// otherwise sit in the user-space buffer and never reach the terminal.
async fn relay_to_local<R, W>(remote: &mut R, local: &mut W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; RELAY_CHUNK];
    let mut total = 0usize;
    loop {
        let n = remote.read(&mut buf).await?;
        if n == 0 {
            log::info!("hitshell: the remote shell closed its output after {total} byte(s)");
            return Ok(());
        }
        total += n;
        local.write_all(&buf[..n]).await?;
        local.flush().await?;
    }
}

/// Copies the local terminal's input to the remote shell.
async fn relay_to_remote<R, W>(local: &mut R, remote: &mut W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; RELAY_CHUNK];
    loop {
        let n = local.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        remote.write_all(&buf[..n]).await?;
        remote.flush().await?;
    }
}

/// Terminal raw mode, restored when dropped.
struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    /// Puts `fd` into raw mode, saving the settings to restore on drop.
    fn enable(fd: RawFd) -> Result<Self, String> {
        let mut saved = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr fills the whole struct when it succeeds.
        if unsafe { libc::tcgetattr(fd, saved.as_mut_ptr()) } != 0 {
            return Err(format!(
                "cannot read the terminal settings: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: tcgetattr succeeded, so the struct is initialized.
        let saved = unsafe { saved.assume_init() };
        let mut raw = saved;
        // SAFETY: raw is a valid termios; cfmakeraw only writes into it.
        unsafe { libc::cfmakeraw(&mut raw) };
        // SAFETY: raw is a valid termios for this terminal fd.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(format!(
                "cannot put the terminal into raw mode: {}",
                std::io::Error::last_os_error()
            ));
        }
        log::info!("hitshell: terminal {fd} is in raw mode");
        Ok(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: `saved` was read from this fd by tcgetattr.
        if unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) } != 0 {
            log::warn!(
                "hitshell: cannot restore the terminal settings: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

/// The directory this process was started in, or `None` when it cannot be named.
/// The protocol carries text, so a directory that is not valid text -- which a
/// caller that passes its own as text cannot produce -- is reported as none
/// rather than sent on in a form that would name a different directory.
fn working_directory() -> Option<String> {
    match std::env::current_dir() {
        Ok(cwd) => match cwd.to_str() {
            Some(cwd) => Some(cwd.to_string()),
            None => {
                log::warn!("hitshell: the working directory {cwd:?} is not valid text");
                None
            }
        },
        Err(err) => {
            log::warn!("hitshell: cannot read the working directory: {err}");
            None
        }
    }
}

/// Size of the local terminal, or `None` when it is not a terminal or reports a
/// zero-sized window.
fn local_size() -> Option<(u32, u32)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes a winsize through the pointer it is given.
    if unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return None;
    }
    if size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    Some((size.ws_col as u32, size.ws_row as u32))
}
