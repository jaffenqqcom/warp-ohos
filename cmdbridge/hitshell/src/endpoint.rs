//! Command-backend endpoint: which daemon this client talks to.
//!
//! the host application links exactly one `hitshell` executor at a time (never two in
//! parallel): the backend is chosen once at startup by whether the embedded
//! QEMU guest is enabled.
//!
//! - OHOS mode talks to hitdaemon on loopback 40220/40230.
//! - QEMU mode talks to the daemon running inside the guest. The guest itself
//!   still listens on 40220/40230, but the host reaches it through the static
//!   slirp hostfwd rules, which expose those ports as 4122/4123 on the host's
//!   loopback. The command/mgmt port pair below is therefore the **host-side**
//!   pair, not the guest-internal one; the hostfwd rules that produce it map
//!   onto the daemon's own 40220/40230.

use crate::protocol::{COMMAND_PORT, LOOPBACK_ADDR, MANAGEMENT_PORT};

/// Host-side loopback port that forwards to the guest command listener (40220).
pub const GUEST_COMMAND_PORT: u16 = 4122;
/// Host-side loopback port that forwards to the guest management listener (40230).
pub const GUEST_MANAGEMENT_PORT: u16 = 4123;

/// The endpoint (host + host-side ports) of the daemon command backend.
#[derive(Clone, Debug)]
pub struct CommandEndpoint {
    /// Address of the fixed-key management listener used for bootstrap.
    pub mgmt_host: String,
    /// Port of the management listener on the host side.
    pub mgmt_port: u16,
    /// Address of the dynamic-key command listener used by the command pool.
    pub command_host: String,
    /// Port of the command listener on the host side.
    pub command_port: u16,
}

impl Default for CommandEndpoint {
    fn default() -> Self {
        Self {
            mgmt_host: LOOPBACK_ADDR.to_string(),
            mgmt_port: MANAGEMENT_PORT,
            command_host: LOOPBACK_ADDR.to_string(),
            command_port: COMMAND_PORT,
        }
    }
}

impl CommandEndpoint {
    /// The on-device (OHOS) hitdaemon backend: management 40230, command 40220.
    pub fn ohos_default() -> Self {
        Self::default()
    }

    /// The QEMU-guest daemon backend, reached through the static hostfwd rules:
    /// host loopback 4123 (management) and 4122 (command).
    pub fn qemu_guest() -> Self {
        Self {
            mgmt_host: LOOPBACK_ADDR.to_string(),
            mgmt_port: GUEST_MANAGEMENT_PORT,
            command_host: LOOPBACK_ADDR.to_string(),
            command_port: GUEST_COMMAND_PORT,
        }
    }
}
