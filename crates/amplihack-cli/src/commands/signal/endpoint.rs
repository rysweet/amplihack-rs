//! Loopback endpoint resolution for `amplihack signal` (#921, D1).
//!
//! [`resolve_endpoint`] is a **pure** function (no process env / no I/O): the
//! caller reads the environment and passes the values in. It applies a single,
//! documented precedence and funnels every candidate through the one canonical
//! loopback choke-point ([`super::validate::validate_loopback_endpoint`]) so a
//! routable/wildcard bind can never slip through by any path.
//!
//! Precedence (highest first):
//!   1. `--port`              → `127.0.0.1:<port>`
//!   2. `AMPLIHACK_SIGNAL_PORT` → `127.0.0.1:<port>`
//!   3. `--endpoint`
//!   4. `AMPLIHACK_SIGNAL_ENDPOINT`
//!   5. [`DEFAULT_ENDPOINT`]

use super::error::SignalOpError;

/// The default signal-cli JSON-RPC daemon port.
pub const DEFAULT_SIGNAL_PORT: u16 = 7583;

/// The default loopback endpoint the daemon binds when nothing is supplied.
pub const DEFAULT_ENDPOINT: &str = "127.0.0.1:7583";

type OpResult<T> = Result<T, SignalOpError>;

/// Resolve the loopback `host:port` the local daemon should bind, applying the
/// documented precedence. `port`/`env_port` always bind loopback
/// (`127.0.0.1:<port>`). The result is validated loopback-only; a non-loopback
/// candidate is rejected as [`SignalOpError::Daemon`] (exit code 6) with **no**
/// side effects — never silently rewritten to a valid target.
pub fn resolve_endpoint(
    port: Option<u16>,
    env_port: Option<u16>,
    endpoint: Option<&str>,
    env_endpoint: Option<&str>,
) -> OpResult<String> {
    let candidate: String = if let Some(p) = port {
        loopback_port(p)
    } else if let Some(p) = env_port {
        loopback_port(p)
    } else if let Some(e) = endpoint {
        e.to_string()
    } else if let Some(e) = env_endpoint {
        e.to_string()
    } else {
        DEFAULT_ENDPOINT.to_string()
    };

    super::validate::validate_loopback_endpoint(&candidate)
        .map_err(|e| SignalOpError::Daemon(e.to_string()))?;
    Ok(candidate)
}

fn loopback_port(port: u16) -> String {
    format!("127.0.0.1:{port}")
}
