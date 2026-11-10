//! Proposed public contract for the new durable history component (#2136).
//! Intentionally compilation-red until swap_warning_history exists. JSON
//! intent fields are the persisted contract; handles/cursors are opaque.
//! Completion evidence is injected by the transaction owner, never inferred
//! from a timestamp or the continued existence of a pending intent.
use amplihack_utils::swap_warning_history::{Completion, WarningHistory};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

fn fixture() -> (tempfile::TempDir, WarningHistory) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let history = WarningHistory::open(tmp.path()).unwrap();
    (tmp, history)
}
fn event(tx: &str, affinity: &str, producer: &str, host: &str) -> Value {
    json!({"version":1, "transaction_id":tx, "producer":producer,
        "reason":"threshold", "host":host, "from_login":"work", "to_login":"spare",
        "from_org":"org-a", "to_org":if affinity == "same" {Some("org-a")} else {Some("org-b")},
        "affinity":affinity})
}

#[test]
fn pending_intent_is_durable_but_not_deliverable_before_commit() {
    let (tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    h.prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    drop(h);
    let h = WarningHistory::open(tmp.path()).unwrap();
    assert!(h.pending(&session).unwrap().is_empty());
}

#[test]
fn same_org_swap_does_not_erase_an_earlier_unconsumed_crossing() {
    let (tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    for (tx, affinity) in [("tx-1", "crossing"), ("tx-2", "same")] {
        let intent = h.prepare(&event(tx, affinity, "rotator", "home")).unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
    }
    drop(h);
    let h = WarningHistory::open(tmp.path()).unwrap();
    let warnings = h.pending(&session).unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].transaction_id, "tx-1");
}

#[test]
fn acknowledgement_is_per_session_and_survives_reopening() {
    let (tmp, mut h) = fixture();
    let a = h.register_current_process("session-a", "home").unwrap();
    let b = h.register_current_process("session-b", "home").unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    let warnings = h.pending(&a).unwrap();
    h.acknowledge(&a, warnings[0].id).unwrap();
    h.retain_required().unwrap();
    drop(h);
    let h = WarningHistory::open(tmp.path()).unwrap();
    assert!(h.pending(&a).unwrap().is_empty());
    assert_eq!(h.pending(&b).unwrap().len(), 1);
}

#[test]
fn auto_switch_warning_is_scoped_without_creating_a_rotator_resume_signal() {
    let (tmp, mut h) = fixture();
    let affected = h.register_current_process("session-a", "work").unwrap();
    let unrelated = h
        .register_current_process("session-b", "unrelated")
        .unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "auto-switch", "work"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    assert_eq!(h.pending(&affected).unwrap().len(), 1);
    assert!(h.pending(&unrelated).unwrap().is_empty());
    assert!(!tmp.path().join(".auto-switch.d/rotate/swap.json").exists());
}

#[test]
fn new_session_does_not_inherit_a_warning_from_before_registration() {
    let (_tmp, mut h) = fixture();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    let session = h.register_current_process("new-session", "home").unwrap();
    assert!(h.pending(&session).unwrap().is_empty());
}

#[test]
fn ambiguous_recovery_retains_intent_without_publishing() {
    let (_tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    assert!(h.complete(&intent, Completion::Unknown).is_err());
    assert!(h.pending(&session).unwrap().is_empty());
    h.complete(&intent, Completion::Committed).unwrap();
    assert_eq!(h.pending(&session).unwrap().len(), 1);
}

#[test]
fn completion_recovery_is_idempotent() {
    let (_tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    assert_eq!(h.pending(&session).unwrap().len(), 1);
}

#[test]
fn event_ids_are_ordered_across_independent_history_handles() {
    let (tmp, mut first) = fixture();
    let session = first.register_current_process("session-a", "home").unwrap();
    let mut second = WarningHistory::open(tmp.path()).unwrap();
    for (h, tx) in [(&mut first, "tx-1"), (&mut second, "tx-2")] {
        let intent = h
            .prepare(&event(tx, "crossing", "rotator", "home"))
            .unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
    }
    let warnings = first.pending(&session).unwrap();
    assert_eq!(warnings.len(), 2);
    assert!(warnings[0].id < warnings[1].id);
}

#[test]
fn session_handle_cannot_consume_another_roots_history() {
    let (_a, mut a) = fixture();
    let (_b, b) = fixture();
    let session = a.register_current_process("same-name", "home").unwrap();
    assert!(b.pending(&session).is_err());
}

#[test]
fn journal_refuses_symlink_root_without_touching_target() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let link = tmp.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(WarningHistory::open(&link).is_err());
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

#[test]
fn journal_refuses_a_shared_writable_root() {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(WarningHistory::open(tmp.path()).is_err());
}

#[test]
fn invalid_or_inconsistent_intents_are_rejected_before_publication() {
    let (_tmp, mut h) = fixture();
    for (key, value) in [
        ("affinity", json!("same")),
        ("producer", json!("unknown")),
        ("version", json!(999)),
        ("from_org", json!("org\u{1b}[2J")),
        ("host", json!("../escape")),
    ] {
        let mut v = event("tx-1", "crossing", "rotator", "home");
        v[key] = value;
        assert!(h.prepare(&v).is_err(), "{v}");
    }
}

#[test]
fn failed_delivery_leaves_warning_available_for_retry() {
    let (_tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    let first = h.pending(&session).unwrap();
    // A failed stderr/tmux delivery makes no acknowledgement.
    let retry = h.pending(&session).unwrap();
    assert_eq!(retry[0].id, first[0].id);
    h.acknowledge(&session, retry[0].id).unwrap();
    assert!(h.pending(&session).unwrap().is_empty());
}

#[test]
fn reregistering_a_live_session_preserves_its_unconsumed_warning() {
    let (_tmp, mut h) = fixture();
    let first = h.register_current_process("session-a", "home").unwrap();
    let intent = h
        .prepare(&event("tx-1", "crossing", "rotator", "home"))
        .unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    let again = h.register_current_process("session-a", "home").unwrap();
    assert_eq!(
        h.pending(&first).unwrap()[0].id,
        h.pending(&again).unwrap()[0].id
    );
}

#[test]
fn acknowledgement_does_not_consume_later_events_or_move_backwards() {
    let (_tmp, mut h) = fixture();
    let session = h.register_current_process("session-a", "home").unwrap();
    for tx in ["tx-1", "tx-2"] {
        let intent = h
            .prepare(&event(tx, "crossing", "rotator", "home"))
            .unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
    }
    let before = h.pending(&session).unwrap();
    h.acknowledge(&session, before[0].id).unwrap();
    let remaining = h.pending(&session).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, before[1].id);
    h.acknowledge(&session, before[1].id).unwrap();
    let _ = h.acknowledge(&session, before[0].id);
    assert!(h.pending(&session).unwrap().is_empty());
}

#[test]
fn completed_consumed_history_is_compacted_during_persistence() {
    let (tmp, mut h) = fixture();
    let s = h.register_current_process("s", "home").unwrap();
    for n in 0..100 {
        let intent = h
            .prepare(&event(&format!("tx-{n}"), "crossing", "rotator", "home"))
            .unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
        let w = h.pending(&s).unwrap();
        h.acknowledge(&s, w[0].id).unwrap();
        h.retire_completed_except(&[]).unwrap();
    }
    let raw = fs::read(tmp.path().join(".swap-warning-history/journal.json")).unwrap();
    assert!(raw.len() < 1024);
}

#[test]
fn malformed_persisted_records_have_fixed_diagnostics() {
    let (tmp, h) = fixture();
    drop(h);
    let path = tmp.path().join(".swap-warning-history/journal.json");
    fs::write(&path, b"{\"secret-\\u001b[31m\":42}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let error = WarningHistory::open(tmp.path()).err().unwrap().to_string();
    assert_eq!(error, "invalid warning journal encoding");
}
