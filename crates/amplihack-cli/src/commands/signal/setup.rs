//! `amplihack signal setup` idempotency planning (#921/#923).
//!
//! Onboarding is driven by three **independent probes** — is this host already
//! linked, is the local daemon running, is a valid config present — and a
//! pure decision function, [`plan_setup`], that turns them into the minimal set
//! of repair steps. This keeps idempotency logic fully unit-testable with no
//! real signal-cli, clock, or Azure call.

/// The three independent onboarding probes.
#[derive(Debug, Clone, Copy)]
pub struct Probes {
    /// signal-cli already has a linked device for the account.
    pub linked: bool,
    /// The local signal-cli JSON-RPC daemon is running and reachable.
    pub daemon_running: bool,
    /// A valid `~/.amplihack/signal-config.toml` is present.
    pub config_written: bool,
}

/// The repair steps onboarding will perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Run device linking (QR / URI). Only ever true when NOT already linked —
    /// re-linking an existing device is an unsafe ratchet reset and is never
    /// planned, even under `--force`.
    pub do_link: bool,
    /// Start the local daemon.
    pub do_start_daemon: bool,
    /// (Re)write the config file.
    pub do_write_config: bool,
}

/// Decide the onboarding plan from probes.
///
/// Idempotent: a fully-onboarded host yields an all-false (no-op) plan.
/// `force` only rewrites the config — it **never** re-links an already-linked
/// device and does not needlessly restart a healthy daemon.
pub fn plan_setup(probes: Probes, force: bool) -> Plan {
    Plan {
        do_link: !probes.linked,
        do_start_daemon: !probes.daemon_running,
        do_write_config: !probes.config_written || force,
    }
}
