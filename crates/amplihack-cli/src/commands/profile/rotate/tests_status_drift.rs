//! Issue #1658 (F): the limit-resume hook reads the rotate ledger through
//! `amplihack_utils::rotator_status::parse_cycle`, a loose reader that the
//! rotator does not use. These tests feed it what the rotator really writes,
//! so a ledger change that the hook would misread fails here.

use super::cycle::CycleOutcome;
use super::ledger::{LastCycle, OverflowBlock, RotateLedger, RotateState, spend_reset};
use amplihack_utils::rotator_status::{CycleStatus, ledger_path, parse_cycle};

const NOW: i64 = 1_792_592_000;

fn bytes(l: &RotateLedger) -> Vec<u8> {
    serde_json::to_vec_pretty(l).unwrap()
}

#[test]
fn full_ledger_round_trips_through_the_hook_reader() {
    let l = RotateLedger {
        live: Some("work".into()),
        account: Some("acct-uuid".into()),
        since: Some(NOW - 900),
        swapped_at: Some(NOW - 900),
        previous: Some("spare".into()),
        alert: Some("all-over:-".into()),
        last_cycle: Some(LastCycle {
            at: NOW - 5,
            outcome: CycleOutcome::Alerted.as_str().into(),
            overflow: None,
        }),
        ..RotateLedger::default()
    };
    assert_eq!(
        parse_cycle(&bytes(&l), NOW),
        Some(CycleStatus {
            at: Some(NOW - 5),
            outcome: Some("alerted".into()),
            alert: true,
            live: Some("work".into()),
            overflow: None,
        })
    );
}

#[test]
fn ledger_without_alert_reads_as_no_alert() {
    let l = RotateLedger {
        live: Some("spare".into()),
        last_cycle: Some(LastCycle {
            at: NOW,
            outcome: CycleOutcome::Swapped.as_str().into(),
            overflow: None,
        }),
        ..RotateLedger::default()
    };
    let s = parse_cycle(&bytes(&l), NOW).unwrap();
    assert!(!s.alert);
    assert!(s.has_capacity());
}

#[test]
fn default_ledger_is_readable_and_has_no_capacity() {
    let s = parse_cycle(&bytes(&RotateLedger::default()), NOW).expect("version 1");
    assert_eq!(s.at, None);
    assert_eq!(s.outcome, None);
    assert!(!s.alert);
    assert!(!s.has_capacity());
}

/// Exhaustive on purpose: a new rotator outcome must be classified here.
fn rotator_says_capacity(o: CycleOutcome) -> bool {
    match o {
        CycleOutcome::Swapped | CycleOutcome::Stayed => true,
        CycleOutcome::Alerted | CycleOutcome::Skipped | CycleOutcome::Stopped => false,
    }
}

#[test]
fn every_rotator_outcome_is_classified_the_same_way() {
    for o in [
        CycleOutcome::Swapped,
        CycleOutcome::Stayed,
        CycleOutcome::Alerted,
        CycleOutcome::Skipped,
        CycleOutcome::Stopped,
    ] {
        let l = RotateLedger {
            last_cycle: Some(LastCycle {
                at: NOW,
                outcome: o.as_str().into(),
                overflow: None,
            }),
            ..RotateLedger::default()
        };
        let s = parse_cycle(&bytes(&l), NOW).unwrap();
        assert_eq!(s.outcome.as_deref(), Some(o.as_str()));
        assert_eq!(s.has_capacity(), rotator_says_capacity(o), "{}", o.as_str());
    }
}

#[test]
fn hook_reads_the_ledger_the_rotator_writes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    assert_eq!(RotateState::new(&root).ledger, ledger_path(&root));
}

// --- Overflow (issue #1606) ------------------------------------------------

fn overflow_ledger(outcome: CycleOutcome, tag: &str) -> RotateLedger {
    let mut l = RotateLedger {
        live: Some("cred".into()),
        last_cycle: Some(LastCycle {
            at: NOW,
            outcome: outcome.as_str().into(),
            overflow: Some(tag.into()),
        }),
        ..RotateLedger::default()
    };
    l.overflow_blocked.insert(
        "cred2".into(),
        OverflowBlock {
            at: NOW - 60,
            until: spend_reset(NOW - 60),
            reason: "spend_limit".into(),
        },
    );
    l
}

#[test]
fn overflow_moves_and_stays_read_as_capacity_for_the_hook() {
    for (o, tag) in [
        (CycleOutcome::Swapped, "in"),
        (CycleOutcome::Stayed, "on"),
        (CycleOutcome::Swapped, "out"),
    ] {
        let s = parse_cycle(&bytes(&overflow_ledger(o, tag)), NOW).expect("readable");
        assert_eq!(s.overflow.as_deref(), Some(tag));
        assert!(s.has_capacity(), "{tag}");
        assert_eq!(s.on_overflow(), tag != "out", "{tag}");
    }
}

#[test]
fn the_overflow_field_follows_outcome_in_the_ledger_bytes() {
    let raw = String::from_utf8(bytes(&overflow_ledger(CycleOutcome::Swapped, "in"))).unwrap();
    let outcome = raw.find("\"outcome\"").unwrap();
    let overflow = raw.find("\"overflow\"").unwrap();
    assert!(outcome < overflow, "{raw}");
    assert!(raw.contains("\"overflow_blocked\""), "{raw}");
}

#[test]
fn a_ledger_without_overflow_activity_has_no_overflow_keys() {
    let l = RotateLedger {
        live: Some("spare".into()),
        last_cycle: Some(LastCycle {
            at: NOW,
            outcome: CycleOutcome::Stayed.as_str().into(),
            overflow: None,
        }),
        ..RotateLedger::default()
    };
    let raw = String::from_utf8(bytes(&l)).unwrap();
    assert!(!raw.contains("overflow"), "{raw}");
    let s = parse_cycle(raw.as_bytes(), NOW).unwrap();
    assert_eq!(s.overflow, None);
    assert!(!s.on_overflow());
}
