//! Durable intent records placement; it never grants fresh movement authority.
use super::super::super::ledger::Phase;
use super::super::super::recover::{RecoverOutcome, recover_with};
use super::*;

#[test]
fn a_damaged_ledger_reports_its_stop_before_any_custody_attempt() {
    let e = fixture();
    let mut c = context(&e);
    let damaged = r#"{"version":99,"live":"work"}"#;
    private(&e.rs.ledger, damaged);
    assert_eq!(drive(&e, &mut c).unwrap(), CycleOutcome::Alerted);
    assert!(log(&e).contains("rotate ledger unreadable"), "{}", log(&e));
    assert_eq!(fs::read_to_string(&e.rs.ledger).unwrap(), damaged);
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.rs.journal.exists());
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn duplicate_intent_cannot_park_credentials_when_fresh_server_verification_fails() {
    let e = fixture();
    private(
        &e.slot("work"),
        std::str::from_utf8(&fs::read(e.live()).unwrap()).unwrap(),
    );
    let duplicate = ino_at(&e.slot("work"));
    let mut c = context(&e);
    test_exchange::interrupted("duplicate-intent", || {
        let _ = drive(&e, &mut c);
    });
    assert!(e.rs.journal.exists());
    c.identity = |_| Err("synthetic endpoint unavailable");
    c.identity_budget.reset();
    let recovered = recover_with(&e.store, &e.rs, &e.home, c.identity_reader());
    assert!(
        matches!(recovered, Err(_) | Ok(RecoverOutcome::Inconsistent(_))),
        "persisted stamps cannot replace fresh authority: {recovered:?}"
    );
    assert_eq!(
        ino_at(&e.slot("work")),
        duplicate,
        "recovery must not move the duplicate without fresh verification"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(
        e.rs.journal.exists(),
        "retain uncertain custody for another explicit cycle"
    );
    assert!(!e.rs.dir.join("swap.json").exists());
}

#[test]
fn unsupported_or_unsafe_journals_preserve_all_credentials_and_the_original_journal() {
    for (field, value) in [
        ("version", serde_json::json!(999)),
        ("from", serde_json::json!("../outside")),
        ("to", serde_json::json!("/absolute")),
    ] {
        let e = fixture();
        let mut journal = serde_json::to_value(e.journal(Phase::J0)).unwrap();
        journal[field] = value;
        let bytes = journal.to_string();
        private(&e.rs.journal, &bytes);
        let c = context(&e);
        let recovered = recover_with(&e.store, &e.rs, &e.home, c.identity_reader());
        assert!(
            matches!(recovered, Err(_) | Ok(RecoverOutcome::Inconsistent(_))),
            "invalid {field}: {recovered:?}"
        );
        assert_eq!(fs::read(&e.rs.journal).unwrap(), bytes.as_bytes());
        assert_eq!(ino_at(&e.live()), Some(e.l));
        assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
        assert!(!e.slot("work").exists());
        assert!(!e.rs.dir.join("swap.json").exists());
    }
}

#[test]
fn legacy_version_one_exchange_still_recovers_from_observed_placement() {
    let e = fixture();
    // Deliberately stale phase: actual placement, not a phase label, decides recovery.
    e.rs.save_journal(&e.journal(Phase::J0)).unwrap();
    e.exchanged();
    let c = context(&e);
    let recovered = recover_with(&e.store, &e.rs, &e.home, c.identity_reader()).unwrap();
    assert!(matches!(recovered, RecoverOutcome::RolledForward { .. }));
    e.assert_finished();
    assert!(
        !e.rs.dir.join("swap.json").exists(),
        "a legacy journal without event intent must not invent an event"
    );
}

#[test]
fn a_version_two_checking_label_cannot_hide_an_observed_exchange() {
    let e = fixture();
    let mut c = context(&e);
    test_exchange::interrupted("s1", || {
        let _ = drive(&e, &mut c);
    });
    let mut journal = json(&e.rs.journal);
    journal["phase"] = "checking".into();
    private(&e.rs.journal, &journal.to_string());
    c.identity_budget.reset();
    let recovered = super::super::super::recover::recover_at(
        &e.store,
        &e.rs,
        &e.home,
        c.identity_reader(),
        c.now,
    )
    .unwrap();
    assert!(matches!(recovered, RecoverOutcome::RolledForward { .. }));
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
    assert!(!e.rs.journal.exists());
    assert_eq!(json(&e.rs.dir.join("swap.json"))["seq"], 1);
}
