//! Issue #1658 (F): the limit-resume hook reads the rotate ledger through
//! `amplihack_utils::rotator_status::parse_cycle`, a loose reader that the
//! rotator does not use. These tests feed it what the rotator really writes,
//! so a ledger change that the hook would misread fails here.

use super::cycle::CycleOutcome;
use super::ledger::{LastCycle, RotateLedger, RotateState};
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
