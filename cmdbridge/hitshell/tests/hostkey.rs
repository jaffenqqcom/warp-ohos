//! Management-key split check: the halves compiled into each binary must match
//! the halves the other side holds.
//!
//! Both crates embed their keys with `include_str!`, so the files under the two
//! `conf` directories are the single source of truth; a mismatch would otherwise
//! only surface as an authentication failure on the device. The daemon's files
//! are read through a relative path because this is the only place both halves
//! are reachable at once.

use russh::keys::ssh_key::{PrivateKey, PublicKey};

/// Client half: the host key this crate verifies and the key it authenticates
/// with.
const CLIENT_KEY: &str = include_str!("../conf/mgmt_client_key");
const HOST_PUB: &str = include_str!("../conf/mgmt_host_key.pub");
/// Server half, from the daemon crate beside this one.
const DAEMON_HOST_KEY: &str = include_str!("../../hitdaemon/conf/mgmt_host_key");
const DAEMON_AUTHORIZED: &str = include_str!("../../hitdaemon/conf/authorized_keys");

/// Parses an OpenSSH public key the way both sides do at runtime: only the two
/// key tokens count, so a trailing comment cannot affect the result.
fn parse_public(text: &str) -> PublicKey {
    let canonical: String = text
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    PublicKey::from_openssh(&canonical)
        .unwrap_or_else(|err| panic!("parse public key '{canonical}': {err}"))
}

#[test]
fn management_key_halves_match() {
    // The client verifies the server with hitdaemon's host public key, so it has
    // to be the public half of the host private key hitdaemon serves with.
    let host_private = PrivateKey::from_openssh(DAEMON_HOST_KEY)
        .unwrap_or_else(|err| panic!("parse daemon host key: {err}"));
    assert_eq!(
        host_private.public_key(),
        &parse_public(HOST_PUB),
        "hitshell verifies a host key hitdaemon does not hold"
    );

    // hitdaemon accepts the client's public key, so it has to be the public half
    // of the client private key hitshell authenticates with.
    let client_private = PrivateKey::from_openssh(CLIENT_KEY)
        .unwrap_or_else(|err| panic!("parse client key: {err}"));
    assert_eq!(
        client_private.public_key(),
        &parse_public(DAEMON_AUTHORIZED),
        "hitdaemon authorizes a client key hitshell does not hold"
    );
}
