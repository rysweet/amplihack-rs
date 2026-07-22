//! Local signal-cli JSON-RPC daemon strategy + idempotent start planning (#921, D5).
//!
//! Pure decision logic, separated from the I/O in `run.rs` so it is unit
//! testable with no systemd, no process, and no socket. The runtime shell
//! consumes [`plan_daemon`] and only performs effects when
//! [`DaemonPlan::needs_start`] is true.

/// How the local daemon is launched as a managed background service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonStrategy {
    /// A `systemd --user` transient unit (preferred when a user manager exists).
    SystemdUser,
    /// A detached `nohup`/`setsid`-style process (fallback).
    Nohup,
}

/// Choose the daemon launch strategy: prefer a `systemd --user` unit when a
/// user systemd manager is available, otherwise fall back to a detached
/// process.
pub fn choose_strategy(systemd_user_available: bool) -> DaemonStrategy {
    if systemd_user_available {
        DaemonStrategy::SystemdUser
    } else {
        DaemonStrategy::Nohup
    }
}

/// An idempotent plan for the local daemon.
///
/// When the loopback JSON-RPC daemon is already running the plan is a no-op
/// ([`needs_start`](DaemonPlan::needs_start) is `false`); otherwise it carries
/// the [`DaemonStrategy`] to start it with and the resolved loopback endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonPlan {
    /// The strategy to use if a start is required.
    pub strategy: DaemonStrategy,
    /// Whether a daemon is already listening on `endpoint`.
    pub already_running: bool,
    /// The resolved loopback `host:port` the daemon binds.
    pub endpoint: String,
}

impl DaemonPlan {
    /// Whether the runtime must actually start the daemon. False when one is
    /// already running (the idempotent no-op case).
    pub fn needs_start(&self) -> bool {
        !self.already_running
    }
}

/// Build an idempotent daemon plan. Pure: given the same inputs it always
/// yields the same plan.
pub fn plan_daemon(systemd_available: bool, already_running: bool, endpoint: &str) -> DaemonPlan {
    DaemonPlan {
        strategy: choose_strategy(systemd_available),
        already_running,
        endpoint: endpoint.to_string(),
    }
}
