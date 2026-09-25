//! Fixed management keys compiled into the daemon.
//!
//! The management listener authenticates clients with a fixed ed25519 keypair
//! generated once at build time. This side embeds both halves it needs with
//! `include_str!`, so the daemon carries its credentials inside the binary and
//! reads no conf directory at runtime.
//!
//! The matching halves live in the hitshell crate, which embeds this host key's
//! public part and the private part of the authorized client key. Replacing
//! either key means editing these files and rebuilding both binaries.

use russh::keys::ssh_key::{PrivateKey, PublicKey};

/// Fixed management host private key (ed25519, OpenSSH format).
const MGMT_HOST_KEY_PEM: &str = include_str!("../conf/mgmt_host_key");
/// Authorized management client public keys (one OpenSSH line; blank lines and
/// `#` comments are ignored).
const MGMT_AUTHORIZED_KEYS: &str = include_str!("../conf/authorized_keys");

/// Parses the compiled-in management host private key and the authorized
/// management client public key.
///
/// Only structural progress is logged; no key material ever reaches the log.
pub(crate) fn mgmt_keys() -> Result<(PrivateKey, PublicKey), String> {
    log::info!("keys: parsing management keys embedded in the binary");
    let host_key = PrivateKey::from_openssh(MGMT_HOST_KEY_PEM)
        .map_err(|err| format!("parse embedded management host key: {err}"))?;
    let pub_line = MGMT_AUTHORIZED_KEYS
        .lines()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .ok_or_else(|| "embedded authorized_keys holds no key line".to_string())?;
    // Keep only the two key tokens so a trailing comment or extra whitespace
    // (as `ssh-keygen` appends) never leaks into the parsed value.
    let canonical: String = pub_line
        .trim()
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let authorized = PublicKey::from_openssh(&canonical)
        .map_err(|err| format!("parse embedded authorized key: {err}"))?;
    Ok((host_key, authorized))
}
