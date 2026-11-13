//! Boundary regressions using only the parent's owned synthetic fixture.
use super::*;
use crate::commands::profile::ProfileError;
use crate::commands::profile::rotate::cycle::{CycleOutcome, Mode, run_cycle};
use crate::commands::profile::rotate::login_identity::{Budget, Reader};
use crate::commands::profile::rotate::observe::{Measurement, Snap, decide_measured};
use crate::commands::profile::rotate::tests_revalidate_cause::{ok, probes_for, spent};
use std::time::Duration;

fn choose_spare(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", ok(10.0, 20.0))]);
    decide_measured(ctx, snap, spent(), probes, now())
}

fn slow_measurement(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    // Exercise the actual unlocked measurement boundary, rather than only
    // testing a budget helper that the cycle might forget to call.
    std::thread::sleep(
        crate::commands::profile::rotate::cred_identity::IDENTITY_BUDGET + Duration::from_secs(1),
    );
    choose_spare(ctx, snap)
}

#[test]
fn slow_usage_measurement_does_not_expire_unchanged_login_identity() {
    let e = fixture();
    let mut ctx = context(&e);
    ctx.measure = Some(slow_measurement);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    assert_eq!(outcome, CycleOutcome::Swapped);
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("work")), Some(e.l));
}

#[test]
fn passive_live_routing_keeps_the_ledger_location_without_an_identity_rpc() {
    let e = fixture();
    // This fixture's injected reader understands these fake tokens, while
    // the unit default deliberately has no evidence for their format.
    assert!(
        (crate::commands::profile::rotate::cred_identity::DEFAULT_READ)(&e.home.join(".claude"))
            .is_err()
    );
    assert!(crate::commands::profile::rotate::identify::is_live(
        &e.store,
        Some(&e.home),
        &n("work")
    ));
}

#[test]
fn invalidated_completion_reports_a_pending_transaction_instead_of_no_movement() {
    let e = fixture();
    let budget = Budget::new();
    let reader = Reader {
        read: &read_fake,
        budget: &budget,
    };
    let incoming = reader.bound(&e.root.join("spare"), false).unwrap();
    e.exchanged();
    let incoming = incoming.moved_to(&e.home.join(".claude")).unwrap();
    let mut journal = e.journal(Phase::S1);
    e.rs.save_journal(&journal).unwrap();
    credential(&e.live(), "work");
    let error = crate::commands::profile::rotate::swap_finish::finish(
        &e.rs,
        &e.home,
        &mut journal,
        &incoming,
    )
    .unwrap_err();
    assert!(
        matches!(error, ProfileError::RotateInconsistent(_)),
        "{error}"
    );
    assert!(e.rs.journal.exists());
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.l));
}

fn change_target_after_revalidation(dir: &Path) -> Result<Account, &'static str> {
    assert!(dir.starts_with(std::env::temp_dir()));
    let base = dir.parent().unwrap().parent().unwrap();
    let arm = base.join("task1753-arm-final-change");
    if dir.file_name().is_some_and(|n| n == ".claude") && arm.exists() {
        credential(&base.join("root/spare/.credentials.json"), "other");
    }
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join(".credentials.json")).unwrap()).unwrap();
    let found = match raw["claudeAiOauth"]["accessToken"].as_str() {
        Some("fake-task1753-other") => account("other"),
        _ => read_fake(dir)?,
    };
    if dir.file_name().is_some_and(|n| n == "spare") && !arm.exists() {
        let counter = base.join("task1753-target-reads");
        let count = fs::read_to_string(&counter)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            + 1;
        private(&counter, &count.to_string());
        if count == 2 {
            private(&arm, "replace only after the accepted fresh snapshot");
        }
    }
    Ok(found)
}

#[test]
fn final_guard_cannot_move_a_different_unrecorded_account_than_was_measured() {
    let e = fixture();
    expected::forget(&e.root, &n("spare"));
    let mut ctx = context(&e);
    ctx.identity = change_target_after_revalidation;
    ctx.measure = Some(choose_spare);
    let outcome = run_cycle(&mut ctx, Mode::Watch);
    assert!(e._tmp.path().join("task1753-arm-final-change").exists());
    assert!(
        !matches!(outcome, Ok(CycleOutcome::Swapped)),
        "the new account was not measured: {outcome:?}"
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
    assert_eq!(ino_at(&e.slot("spare")), Some(e.t));
    assert!(!e.slot("work").exists());
    assert!(!e.root.join(".rejected-logins").exists());
}

#[test]
fn legacy_journal_still_recovers_with_matching_expected_records() {
    let e = fixture();
    let mut journal = e.journal(Phase::S1);
    journal.identity = None;
    e.exchanged();
    e.rs.save_journal(&journal).unwrap();
    let budget = Budget::new();
    let outcome = crate::commands::profile::rotate::recover::recover_with(
        &e.store,
        &e.rs,
        &e.home,
        Reader {
            read: &read_fake,
            budget: &budget,
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::commands::profile::rotate::recover::RecoverOutcome::RolledForward { .. }
    ));
    assert!(!e.rs.journal.exists());
    assert_eq!(ino_at(&e.live()), Some(e.t));
}

#[test]
fn legacy_journal_without_incoming_ownership_evidence_moves_nothing() {
    let e = fixture();
    expected::forget(&e.root, &n("spare"));
    let mut journal = e.journal(Phase::S1);
    journal.identity = None;
    e.exchanged();
    e.rs.save_journal(&journal).unwrap();
    let before = e.all_logins();
    let budget = Budget::new();
    let outcome = crate::commands::profile::rotate::recover::recover_with(
        &e.store,
        &e.rs,
        &e.home,
        Reader {
            read: &read_fake,
            budget: &budget,
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        crate::commands::profile::rotate::recover::RecoverOutcome::Inconsistent(_)
    ));
    assert_eq!(e.all_logins(), before);
    assert!(e.rs.journal.exists());
}
