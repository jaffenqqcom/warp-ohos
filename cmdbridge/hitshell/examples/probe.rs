//! End-to-end probe against a running hitdaemon (bring-up check).
//!
//! Uses the management keys compiled into this crate (see `hitshell::keys`), so
//! no key directory has to be prepared. Boots a `SshCommandExecutor`, runs
//! `/bin/echo probe-ok`, prints its stdout and exit status, then runs a
//! long-lived command that is killed via `signal` to exercise the session table.
//! Usage:
//!
//! ```text
//! cargo run --example probe --manifest-path cmdbridge/hitshell/Cargo.toml
//! ```

use hitshell::keys::{MGMT_CLIENT_KEY, MGMT_HOST_PUB};
use hitshell::{ExecSpec, FdMode, RemoteCommandExecutor, SshCommandExecutor, Signal};
use smol::io::AsyncReadExt as _;

/// How long the probe waits for hitdaemon's management handshake.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn main() {
    env_logger::init();

    let executor = SshCommandExecutor::new(
        hitshell::CommandEndpoint::ohos_default(),
        MGMT_CLIENT_KEY.to_string(),
        MGMT_HOST_PUB.to_string(),
    )
    .expect("executor");
    executor.wait_ready(READY_TIMEOUT).expect("hitdaemon ready");
    let executor = std::sync::Arc::new(executor);

    smol::block_on(async {
        // Quick command: /bin/echo probe-ok
        let mut spec = ExecSpec::new("/bin/echo");
        spec.args.push("probe-ok".to_string());
        spec.stdout_mode = FdMode::Piped;
        let mut child = executor.spawn(spec).expect("spawn echo");
        let mut out = String::new();
        if let Some(stdout) = child.stdout.as_mut() {
            let _ = stdout.read_to_string(&mut out).await;
        }
        let exit = executor.wait_exit_async(child.session_id).await;
        println!("echo stdout={out:?} exit={exit:?}");

        // Long-lived command that we kill by session (exercises the server-side
        // session table + process-group signal).
        let mut sleep_spec = ExecSpec::new("/bin/sh");
        sleep_spec.args.push("-c".to_string());
        sleep_spec.args.push("echo start && sleep 300".to_string());
        sleep_spec.stdout_mode = FdMode::Piped;
        let mut sleeper = executor.spawn(sleep_spec).expect("spawn sleep");
        // Give it a moment to emit "start", then signal it.
        let mut first = [0u8; 32];
        if let Some(stdout) = sleeper.stdout.as_mut() {
            let _ = stdout.read(&mut first).await;
        }
        println!("sleep first bytes={:?}", String::from_utf8_lossy(&first));
        executor
            .signal(sleeper.session_id, Signal::SigKill)
            .expect("signal");
        let exit = executor.wait_exit_async(sleeper.session_id).await;
        println!("sleep exit after signal={exit:?}");
    });
}
