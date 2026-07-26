//! The `/signal` topic bridge: reusable core for driving a Copilot session from
//! a Signal group.
//!
//! This module tree is the crate-side, reusable half of the bridge (the CLI
//! subcommand glue lives in `amplihack-cli`). It holds the pure, unit-tested
//! pieces — [`naming`], [`control`], [`allowlist`], [`chunk`], [`membership`],
//! [`outbound`] redaction, and the [`turn`] driver — plus the small set of
//! fail-closed I/O helpers below ([`validate_endpoint`], [`connect_daemon`])
//! and the shared [`BridgeError`] exit-code taxonomy.
//!
//! Security posture (see `docs/SIGNAL_BRIDGE.md`): least-privilege tools by
//! default, fail-closed outbound membership verification, loopback-only daemon
//! unless an explicit unsafe opt-in, and no silent fallbacks — every failure is
//! surfaced with a stable exit code.

pub mod allowlist;
pub mod chunk;
pub mod control;
pub mod membership;
pub mod naming;
pub mod outbound;
pub mod turn;

use std::time::Duration;

/// The bridge's stable exit-code taxonomy.
///
/// Together with a `0` (clean shutdown / normal end) this is the documented
/// **6-code exit contract** operators script against. Each variant maps to
/// exactly one non-zero code via [`BridgeError::exit_code`].
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// The account is not linked (or the feature/setup prerequisites are
    /// missing). Exit `1`.
    #[error("signal account not linked or bridge prerequisites missing")]
    NotLinked,
    /// A non-loopback daemon endpoint was rejected without the explicit
    /// `--unsafe-remote-endpoint` opt-in. Exit `2`.
    #[error("non-loopback signal-cli endpoint rejected (loopback safety)")]
    RemoteEndpointRejected,
    /// The operator-only group could not be created/joined. Exit `3`.
    #[error("failed to create the operator-only Signal group")]
    GroupCreateFailed,
    /// The signal-cli daemon was not reachable within the retry budget. Exit `4`.
    #[error("signal-cli daemon unavailable after exhausting the retry budget")]
    DaemonUnavailable,
    /// The installed `copilot` did not accept `--session-id` resume, so turn
    /// continuity cannot be guaranteed. Exit `5`.
    #[error("copilot session-resume probe failed (--session-id unsupported)")]
    ResumeProbeFailed,
}

impl BridgeError {
    /// The stable process exit code for this error.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            BridgeError::NotLinked => 1,
            BridgeError::RemoteEndpointRejected => 2,
            BridgeError::GroupCreateFailed => 3,
            BridgeError::DaemonUnavailable => 4,
            BridgeError::ResumeProbeFailed => 5,
        }
    }
}

/// Split a `host:port` (supporting bracketed IPv6 `[::1]:7583`) into borrowed
/// `(host, port)`. Returns `None` on a missing port or empty host/port.
///
/// This is the single canonical splitter shared by the runtime and CLI
/// validators (the CLI's `validate_loopback_endpoint` delegates here), so the
/// two entry points can never drift on how a `host:port` is parsed.
fn split_host_port(endpoint: &str) -> Option<(&str, &str)> {
    if endpoint.is_empty() {
        return None;
    }
    if let Some(rest) = endpoint.strip_prefix('[') {
        // Bracketed IPv6: `[host]:port`.
        let (host, port) = rest.split_once("]:")?;
        if host.is_empty() || port.is_empty() {
            return None;
        }
        return Some((host, port));
    }
    // Bare host: split off the port from the right so bracket-less IPv6 hosts
    // (which themselves contain colons, e.g. `::1`) keep their colons.
    let (host, port) = endpoint.rsplit_once(':')?;
    if host.is_empty() || port.is_empty() {
        return None;
    }
    Some((host, port))
}

/// Validate a signal-cli daemon `endpoint` (`host:port`) is **loopback-only**
/// and well-formed. Fails closed with [`BridgeError::RemoteEndpointRejected`].
///
/// Accepts `127.0.0.0/8`, IPv6 loopback `::1` (both bracketed `[::1]:port` and
/// bracket-less `::1:port`), and the literal `localhost`, each with a port in
/// `1..=65535`. Rejects wildcard binds (`0.0.0.0`, `::`), routable addresses,
/// DNS names, and any malformed / zero / out-of-range port.
///
/// This is the crate's single canonical loopback validator; both the runtime
/// [`validate_endpoint`] and the CLI's `validate_loopback_endpoint` delegate to
/// it so the two paths cannot diverge.
pub fn validate_loopback_endpoint(endpoint: &str) -> Result<(), BridgeError> {
    let Some((host, port)) = split_host_port(endpoint) else {
        return Err(BridgeError::RemoteEndpointRejected);
    };
    // Port must be a non-zero u16 (`parse::<u16>` rejects out-of-range for free).
    match port.parse::<u16>() {
        Ok(p) if p != 0 => {}
        _ => return Err(BridgeError::RemoteEndpointRejected),
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Ok(());
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) if ip.is_loopback() => Ok(()),
        _ => Err(BridgeError::RemoteEndpointRejected),
    }
}

/// Validate a signal-cli daemon `endpoint` (`host:port`) for loopback safety.
///
/// A loopback endpoint (see [`validate_loopback_endpoint`]) is always accepted.
/// A non-loopback endpoint is accepted **only** when `unsafe_remote` is `true`
/// (the explicit, documented opt-in); otherwise it **fails closed** with
/// [`BridgeError::RemoteEndpointRejected`].
pub fn validate_endpoint(endpoint: &str, unsafe_remote: bool) -> Result<(), BridgeError> {
    if unsafe_remote {
        return Ok(());
    }
    validate_loopback_endpoint(endpoint)
}

/// Connect to the signal-cli JSON-RPC daemon with a bounded retry budget.
///
/// Uses capped exponential backoff via
/// [`crate::transport::SignalTransport::connect_with_retry`]. If every attempt
/// fails, the transient I/O error is surfaced as the stable
/// [`BridgeError::DaemonUnavailable`] (exit `4`) rather than hanging or
/// silently disabling the bridge.
pub async fn connect_daemon(
    endpoint: &str,
    retry_budget: u32,
) -> Result<crate::transport::SignalTransport, BridgeError> {
    crate::transport::SignalTransport::connect_with_retry(
        endpoint,
        retry_budget,
        Duration::from_millis(100),
        Duration::from_secs(5),
    )
    .await
    .map_err(|_| BridgeError::DaemonUnavailable)
}
