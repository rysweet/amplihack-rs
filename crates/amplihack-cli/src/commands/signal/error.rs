//! Error taxonomy for `amplihack signal` operations (#921/#923).
//!
//! Every variant maps to a **stable** process exit code so the fleet
//! orchestrator and downstream tooling can branch on outcomes without parsing
//! human-readable messages:
//!
//! | code | meaning |
//! |------|---------|
//! | 2 | usage / bad arguments |
//! | 3 | unsupported (built without `--features signal`, or an unimplemented identity mode) |
//! | 4 | signal-cli detection / installation failure |
//! | 5 | partial fleet rollout (some VMs failed; run is resumable) |
//! | 6 | local daemon / port failure |
//! | 7 | device-linking failure (including Signal's linked-device limit) |
//!
//! There is intentionally no `0` variant: success is not an error.

use std::fmt;

/// A Signal onboarding / distribution operation failure with a stable exit code.
#[derive(Debug)]
pub enum SignalOpError {
    /// Bad arguments / usage error. Exit code 2.
    Usage(String),
    /// The requested capability is unavailable: the binary was built without
    /// `--features signal`, or an identity mode (e.g. `dedicated-number`) is
    /// not implemented yet. Exit code 3.
    Unsupported(String),
    /// signal-cli could not be detected or installed. Exit code 4.
    SignalCli(String),
    /// A fleet rollout completed with some VMs failing. The run is resumable;
    /// re-running retries only the failed/pending hosts. Exit code 5.
    Partial {
        /// Count of VMs that reached terminal success (`config-written`).
        succeeded: usize,
        /// Total VMs targeted this run.
        total: usize,
        /// `(vm_name, reason)` for each failed VM.
        failures: Vec<(String, String)>,
    },
    /// The local signal-cli JSON-RPC daemon could not be started/reached, or the
    /// endpoint/port was unusable. Exit code 6.
    Daemon(String),
    /// Device linking failed, including hitting Signal's linked-device limit.
    /// Exit code 7.
    Link(String),
}

impl SignalOpError {
    /// The stable process exit code for this error. Never `0`.
    pub fn exit_code(&self) -> i32 {
        match self {
            SignalOpError::Usage(_) => 2,
            SignalOpError::Unsupported(_) => 3,
            SignalOpError::SignalCli(_) => 4,
            SignalOpError::Partial { .. } => 5,
            SignalOpError::Daemon(_) => 6,
            SignalOpError::Link(_) => 7,
        }
    }
}

impl fmt::Display for SignalOpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalOpError::Usage(m) => write!(f, "usage error: {m}"),
            SignalOpError::Unsupported(m) => write!(f, "unsupported: {m}"),
            SignalOpError::SignalCli(m) => write!(f, "signal-cli error: {m}"),
            SignalOpError::Partial {
                succeeded,
                total,
                failures,
            } => {
                write!(
                    f,
                    "partial rollout: {succeeded}/{total} VMs onboarded, {} failed",
                    failures.len()
                )?;
                for (vm, reason) in failures {
                    write!(f, "; {vm}: {reason}")?;
                }
                Ok(())
            }
            SignalOpError::Daemon(m) => write!(f, "signal-cli daemon error: {m}"),
            SignalOpError::Link(m) => write!(f, "device-linking error: {m}"),
        }
    }
}

impl std::error::Error for SignalOpError {}
