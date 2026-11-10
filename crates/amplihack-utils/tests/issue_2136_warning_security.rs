//! Failure boundaries for the durable journal, using isolated private roots.
use amplihack_utils::swap_warning_history::{Completion, WarningHistory};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};
fn event(tx: &str) -> serde_json::Value {
    json!({"version":1,"transaction_id":tx,"producer":"rotator","reason":"threshold",
        "host":"home","from_login":"work","to_login":"spare",
        "from_org":"org-a","to_org":"org-b","affinity":"crossing"})
}
fn root() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    tmp
}
#[test]
fn acknowledgement_cannot_skip_an_undelivered_prefix() {
    let root = root();
    let mut h = WarningHistory::open(root.path()).unwrap();
    let session = h.register_current_process("session", "home").unwrap();
    for tx in ["first", "second"] {
        let intent = h.prepare(&event(tx)).unwrap();
        h.complete(&intent, Completion::Committed).unwrap();
    }
    let warnings = h.pending(&session).unwrap();
    assert!(h.acknowledge(&session, warnings[1].id).is_err());
    assert_eq!(h.pending(&session).unwrap().len(), 2);
}
#[test]
fn corrupt_committed_history_is_reported_and_preserved() {
    let root = root();
    drop(WarningHistory::open(root.path()).unwrap());
    let journal = root.path().join(".swap-warning-history/journal.json");
    fs::write(&journal, "corrupt committed state").unwrap();
    fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(WarningHistory::open(root.path()).is_err());
    assert_eq!(
        fs::read_to_string(journal).unwrap(),
        "corrupt committed state"
    );
}
#[test]
fn independent_publishers_allocate_unique_ordered_ids() {
    let root = root();
    let mut h = WarningHistory::open(root.path()).unwrap();
    let session = h.register_current_process("session", "home").unwrap();
    std::thread::scope(|scope| {
        for tx in ["first", "second", "third", "fourth"] {
            let path = root.path();
            scope.spawn(move || {
                let mut h = WarningHistory::open(path).unwrap();
                let intent = h.prepare(&event(tx)).unwrap();
                h.complete(&intent, Completion::Committed).unwrap();
            });
        }
    });
    let warnings = h.pending(&session).unwrap();
    assert_eq!(warnings.len(), 4);
    assert!(warnings.windows(2).all(|pair| pair[0].id < pair[1].id));
}
#[test]
fn transaction_recovery_checks_scope_before_publishing() {
    let root = root();
    let mut h = WarningHistory::open(root.path()).unwrap();
    let session = h.register_current_process("session", "home").unwrap();
    h.prepare(&event("tx")).unwrap();
    assert!(
        h.complete_transaction("tx", "auto-switch", "home", "work", "spare")
            .is_err()
    );
    assert!(h.pending(&session).unwrap().is_empty());
    assert!(
        h.complete_transaction("tx", "rotator", "home", "work", "spare")
            .unwrap()
    );
    assert!(
        h.complete_transaction("tx", "rotator", "home", "work", "spare")
            .unwrap()
    );
    assert_eq!(h.pending(&session).unwrap().len(), 1);
}

#[test]
fn registration_between_intent_and_publication_cannot_miss_the_event() {
    let root = root();
    let mut h = WarningHistory::open(root.path()).unwrap();
    let intent = h.prepare(&event("in-flight")).unwrap();
    let session = h.register_current_process("during-swap", "home").unwrap();
    h.complete(&intent, Completion::Committed).unwrap();
    assert_eq!(h.pending(&session).unwrap().len(), 1);
}

#[test]
fn a_private_root_below_an_unsafe_parent_is_refused() {
    let tmp = root();
    let child = tmp.path().join("profiles");
    fs::create_dir(&child).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(WarningHistory::open(&child).is_err());
    assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
}
