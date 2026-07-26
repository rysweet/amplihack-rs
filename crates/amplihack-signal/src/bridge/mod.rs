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

/// Whether `host` is a loopback address (or `localhost`).
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    // Strip IPv6 brackets, e.g. `[::1]`.
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// Validate a signal-cli daemon `endpoint` (`host:port`) for loopback safety.
///
/// A loopback endpoint is always accepted. A non-loopback endpoint is accepted
/// **only** when `unsafe_remote` is `true` (the explicit, documented
/// opt-in); otherwise it **fails closed** with
/// [`BridgeError::RemoteEndpointRejected`].
pub fn validate_endpoint(endpoint: &str, unsafe_remote: bool) -> Result<(), BridgeError> {
    if unsafe_remote {
        return Ok(());
    }
    // Split off the port from the right so IPv6 hosts (which contain colons)
    // are handled: `[::1]:7583` → host `[::1]`.
    let host = match endpoint.rsplit_once(':') {
        Some((host, _port)) => host,
        None => endpoint,
    };
    if is_loopback_host(host) {
        Ok(())
    } else {
        Err(BridgeError::RemoteEndpointRejected)
    }
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
