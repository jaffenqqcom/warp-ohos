//! Fixed management keys compiled into the client.
//!
//! The bootstrap authenticates against the daemon's management listener, so this
//! side needs the host key it must verify and the private key it authenticates
//! with. Both are embedded with `include_str!`: hitshell carries its credentials
//! inside the binary and reads no conf directory at runtime.
//!
//! The matching halves live in the hitdaemon crate, which embeds the host key's
//! private part and this client key's public part. Replacing either key means
//! editing the files under `conf` and rebuilding both binaries.

/// Management host public key (ed25519, one OpenSSH line).
pub const MGMT_HOST_PUB: &str = include_str!("../conf/mgmt_host_key.pub");
/// Management client private key (ed25519, OpenSSH format).
pub const MGMT_CLIENT_KEY: &str = include_str!("../conf/mgmt_client_key");
