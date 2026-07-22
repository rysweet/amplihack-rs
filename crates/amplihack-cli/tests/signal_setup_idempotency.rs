//! TDD contract — `signal setup` idempotency & rollout planning (#921/#923).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_setup_idempotency`
//!
//! Two pure decision functions drive onboarding, both injectable/testable with
//! no real signal-cli, clock, or Azure calls:
//!
//!   * `setup::plan_setup(probes, force)` — from three independent probes
//!     (linked / daemon-running / config-written) decide which repair steps to
//!     run. Idempotent: an already-onboarded host does nothing; `--force` only
//!     rewrites config, it NEVER re-links an already-linked device.
//!
//!   * `distribute::plan_rollout(lister, state, rg)` — enumerate VMs via the
//!     injected `VmLister` and select the resumable targets. A fake lister lets
//!     us test orchestration with zero cloud dependency.
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::distribute::{self, DistributeState, VmStatus};
use amplihack_cli::commands::signal::seams::VmLister;
use amplihack_cli::commands::signal::setup::{self, Probes};

// ---------------------------------------------------------------------------
// plan_setup: the 3-probe idempotency matrix
// ---------------------------------------------------------------------------

#[test]
fn fresh_host_runs_all_three_steps() {
    let plan = setup::plan_setup(
        Probes {
            linked: false,
            daemon_running: false,
            config_written: false,
        },
        false,
    );
    assert!(plan.do_link, "unlinked host must link");
    assert!(plan.do_start_daemon, "no daemon → must start it");
    assert!(plan.do_write_config, "no config → must write it");
}

#[test]
fn fully_onboarded_host_is_a_noop() {
    let plan = setup::plan_setup(
        Probes {
            linked: true,
            daemon_running: true,
            config_written: true,
        },
        false,
    );
    assert!(!plan.do_link, "already linked → must NOT re-link");
    assert!(!plan.do_start_daemon, "daemon up → must not restart");
    assert!(!plan.do_write_config, "config present → must not clobber");
}

#[test]
fn linked_but_daemon_down_only_starts_daemon() {
    let plan = setup::plan_setup(
        Probes {
            linked: true,
            daemon_running: false,
            config_written: true,
        },
        false,
    );
    assert!(!plan.do_link);
    assert!(plan.do_start_daemon);
    assert!(!plan.do_write_config);
}

#[test]
fn force_rewrites_config_but_never_relinks() {
    let plan = setup::plan_setup(
        Probes {
            linked: true,
            daemon_running: true,
            config_written: true,
        },
        true, // --force
    );
    assert!(
        !plan.do_link,
        "--force must NEVER re-link an already-linked device (unsafe ratchet reset)"
    );
    assert!(plan.do_write_config, "--force must rewrite the config");
}

// ---------------------------------------------------------------------------
// plan_rollout: fleet target selection via an injected VmLister
// ---------------------------------------------------------------------------

struct FakeVmLister(Vec<String>);

impl VmLister for FakeVmLister {
    fn list_vms(&self, _resource_group: &str) -> anyhow::Result<Vec<String>> {
        Ok(self.0.clone())
    }
}

#[test]
fn rollout_uses_injected_lister_and_skips_completed_vms() {
    let lister = FakeVmLister(vec!["vm-a".into(), "vm-b".into(), "vm-c".into()]);
    let mut state = DistributeState::new();
    state.upsert("vm-a", VmStatus::ConfigWritten, None); // already done

    let targets = distribute::plan_rollout(&lister, &state, "rg-prod").expect("plan");
    assert!(
        !targets.contains(&"vm-a".to_string()),
        "completed VM must be skipped"
    );
    assert!(targets.contains(&"vm-b".to_string()));
    assert!(targets.contains(&"vm-c".to_string()));
}

#[test]
fn rollout_propagates_lister_errors_no_silent_fallback() {
    struct FailingLister;
    impl VmLister for FailingLister {
        fn list_vms(&self, _rg: &str) -> anyhow::Result<Vec<String>> {
            anyhow::bail!("az vm list failed: not logged in")
        }
    }
    let state = DistributeState::new();
    let res = distribute::plan_rollout(&FailingLister, &state, "rg-prod");
    assert!(
        res.is_err(),
        "VM discovery failure must surface as an error, not an empty/silent target list"
    );
}
