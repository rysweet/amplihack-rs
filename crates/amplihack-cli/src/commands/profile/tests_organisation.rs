//! #2136: organisation resolution for auto-switch, on temporary metadata.
use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};
fn fixture() -> (tempfile::TempDir, State) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let state = State::new(tmp.path()).unwrap();
    state
        .write_sub_json(
            "expected",
            "work",
            &serde_json::json!({"account":"u-work", "org":"org-a"}),
        )
        .unwrap();
    (tmp, state)
}
#[test]
fn issue_2136_hosting_changes_source_but_not_the_targets_own_slot() {
    let (_tmp, state) = fixture();
    state
        .write_sub_json(
            "expected",
            "other",
            &serde_json::json!({"account":"u-other", "org":"org-b"}),
        )
        .unwrap();
    let mut ledger = Ledger::default();
    ledger.hosts.insert(
        "work".into(),
        serde_json::from_value(
            serde_json::json!({"live":"other","swapped_at":1,"pre_swap_fingerprint":null}),
        )
        .unwrap(),
    );
    assert_eq!(resolve(&state, &ledger, "work").as_deref(), Some("org-b"));
    assert_eq!(resolve_login(&state, "work").as_deref(), Some("org-a"));
}
#[test]
fn issue_2136_doubtful_metadata_reads_as_unknown_like_expected_lookup() {
    let (_tmp, state) = fixture();
    let record = state.dir.join("expected/work.json");
    let name = ProfileName::parse("work").unwrap();
    std::fs::write(&record, "invalid JSON").unwrap();
    assert_eq!(resolve_login(&state, "work"), None);
    assert!(expected::lookup(&state, &name).is_err());
    std::fs::write(&record, r#"{"account":"u-work"}"#).unwrap();
    assert_eq!(resolve_login(&state, "work"), None, "no org entry");
    std::fs::remove_file(&record).unwrap();
    symlink("missing", &record).unwrap();
    assert_eq!(resolve_login(&state, "work"), None, "unsafe file");
    std::fs::remove_file(&record).unwrap();
    state
        .write_sub_json(
            "expected",
            "work",
            &serde_json::json!({"account":"u-work","org":null}),
        )
        .unwrap();
    assert_eq!(resolve_login(&state, "work"), None, "null org");
}
