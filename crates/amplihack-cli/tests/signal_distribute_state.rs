//! TDD contract — fleet distribute state model & resumability (#923).
//!
//! Run with: `cargo test -p amplihack-cli --features signal --test signal_distribute_state`
//!
//! `commands::signal::distribute` tracks per-VM rollout state in an atomic JSON
//! file (`~/.amplihack/signal-distribute-state.json`). Contract:
//!   * Per-VM status: pending | linking | linked | daemon-running |
//!     config-written | failed, each with an optional reason + updated_at.
//!   * Only `config-written` is terminal success.
//!   * Rollout is RESUMABLE: re-running skips terminal-success VMs and retries
//!     pending/failed ones.
//!   * One VM failing NEVER removes or aborts the others (isolation).
//!   * State file is written 0600 (secrets-adjacent).
//!   * A state file with a higher-than-known schema version is refused (no
//!     silent downgrade / data loss).
#![cfg(feature = "signal")]

use amplihack_cli::commands::signal::distribute::{DistributeState, VmStatus};

fn tmp_state_path() -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    // Keep the dir alive for the test by leaking it (test-scoped process).
    let p = dir.path().join("signal-distribute-state.json");
    std::mem::forget(dir);
    p
}

#[test]
fn only_config_written_is_terminal_success() {
    assert!(VmStatus::ConfigWritten.is_terminal_success());
    for s in [
        VmStatus::Pending,
        VmStatus::Linking,
        VmStatus::Linked,
        VmStatus::DaemonRunning,
        VmStatus::Failed,
    ] {
        assert!(
            !s.is_terminal_success(),
            "{s:?} must NOT count as terminal success"
        );
    }
}

#[test]
fn upsert_records_status_and_reason() {
    let mut st = DistributeState::new();
    st.upsert(
        "vm-a",
        VmStatus::Failed,
        Some("signal-cli install failed".into()),
    );
    let rec = st.get("vm-a").expect("record present");
    assert_eq!(rec.status, VmStatus::Failed);
    assert_eq!(rec.reason.as_deref(), Some("signal-cli install failed"));
    assert!(
        !rec.updated_at.is_empty(),
        "updated_at timestamp must be set"
    );
}

#[test]
fn save_then_load_round_trips() {
    let path = tmp_state_path();
    let mut st = DistributeState::new();
    st.upsert("vm-a", VmStatus::ConfigWritten, None);
    st.upsert("vm-b", VmStatus::Failed, Some("link timeout".into()));
    st.save(&path).expect("save");

    let loaded = DistributeState::load(&path).expect("load");
    assert_eq!(loaded.get("vm-a").unwrap().status, VmStatus::ConfigWritten);
    assert_eq!(loaded.get("vm-b").unwrap().status, VmStatus::Failed);
    assert_eq!(
        loaded.get("vm-b").unwrap().reason.as_deref(),
        Some("link timeout")
    );
}

#[test]
fn resumable_targets_skip_terminal_success_and_include_failed_and_new() {
    let mut st = DistributeState::new();
    st.upsert("vm-done", VmStatus::ConfigWritten, None);
    st.upsert("vm-failed", VmStatus::Failed, Some("boom".into()));
    st.upsert("vm-linked", VmStatus::Linked, None); // not terminal -> retry

    let all = vec![
        "vm-done".to_string(),
        "vm-failed".to_string(),
        "vm-linked".to_string(),
        "vm-new".to_string(), // never seen before
    ];
    let targets = st.resumable_targets(&all);

    assert!(
        !targets.contains(&"vm-done".to_string()),
        "must skip completed VM"
    );
    assert!(
        targets.contains(&"vm-failed".to_string()),
        "must retry failed VM"
    );
    assert!(
        targets.contains(&"vm-linked".to_string()),
        "must resume non-terminal VM"
    );
    assert!(
        targets.contains(&"vm-new".to_string()),
        "must include never-seen VM"
    );
}

#[test]
fn failure_of_one_vm_does_not_drop_others() {
    // Isolation: recording a failure must leave all sibling records intact.
    let mut st = DistributeState::new();
    st.upsert("vm-a", VmStatus::ConfigWritten, None);
    st.upsert("vm-b", VmStatus::DaemonRunning, None);
    st.upsert("vm-c", VmStatus::Failed, Some("unreachable".into()));

    assert_eq!(st.get("vm-a").unwrap().status, VmStatus::ConfigWritten);
    assert_eq!(st.get("vm-b").unwrap().status, VmStatus::DaemonRunning);
    assert_eq!(st.get("vm-c").unwrap().status, VmStatus::Failed);
}

#[test]
fn summary_counts_reflect_per_vm_status() {
    let mut st = DistributeState::new();
    st.upsert("a", VmStatus::ConfigWritten, None);
    st.upsert("b", VmStatus::ConfigWritten, None);
    st.upsert("c", VmStatus::Failed, Some("x".into()));
    // succeeded = terminal-success count, failed = failed count.
    assert_eq!(st.succeeded_count(), 2);
    assert_eq!(st.failed_count(), 1);
}

#[cfg(unix)]
#[test]
fn state_file_is_written_0600() {
    use std::os::unix::fs::PermissionsExt;
    let path = tmp_state_path();
    let mut st = DistributeState::new();
    st.upsert("vm-a", VmStatus::Pending, None);
    st.save(&path).expect("save");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "state file must be 0600, got {mode:o}");
}

#[test]
fn loading_a_higher_schema_version_is_refused() {
    let path = tmp_state_path();
    // A file claiming a future schema version must be rejected, not silently
    // reinterpreted (guards against work-discarding downgrades).
    std::fs::write(&path, r#"{"version": 99999, "vms": {}}"#).unwrap();
    assert!(
        DistributeState::load(&path).is_err(),
        "an unknown-higher schema version must be refused"
    );
}
