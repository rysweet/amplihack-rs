//! #1753 review R2: an identity refusal after an advisory ALERT (#1907).
//!
//! A no-capacity ALERT on a live login below 100% is stored in
//! `ledger.advisory_alert`. If the next cycle refuses on the live login's
//! trusted identity, `alert::alert` stores the refusal in `ledger.alert`.
//! `valid_alerts` rejects a ledger with both fields set, so the refusal must
//! clear `advisory_alert`; otherwise `load_ledger` fails on every later
//! cycle and rotation stops until the file is repaired by hand.
//!
//! The three cycles run on the real swap, ledger and log under a temporary
//! HOME and profile root (`tests_swap::env()`: `work` live, `spare`
//! stored). Probing is stubbed with `Ctx::measure`; the identity read is
//! the test fixture reader, replaced by a failing reader for the refusal.
//! No test reads a real login, uses the network or starts `claude`.

use super::super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::super::observe::{Measurement, Snap, decide_measured};
use super::super::tests_issue_1870::{exh, rd};
use super::super::tests_revalidate_cause::{ctx_with, log_of, probes_for};
use super::super::tests_swap::{Env, env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;

/// `work` is past `--session-at` but below 100%; `spare` is full, so the
/// decision is the advisory no-capacity ALERT.
fn measure_advisory(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", exh(100.0, 100.0))]);
    decide_measured(ctx, snap, rd(92.0, 60.0, None), probes, now())
}

/// Both logins read well below the thresholds: a plain stay.
fn measure_fresh_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let probes = probes_for(snap, &[("spare", rd(40.0, 30.0, None))]);
    decide_measured(ctx, snap, rd(40.0, 30.0, None), probes, now())
}

fn unavailable(_: &std::path::Path) -> Result<super::super::identify::Account, &'static str> {
    Err("fake identity unavailable")
}

/// The ledger loads, and its alert fields are as given.
fn assert_loads(e: &Env, alert: Option<&str>, advisory: Option<&str>, step: &str) {
    let l =
        e.rs.load_ledger()
            .unwrap_or_else(|err| panic!("{step}: the ledger must load: {err}\n{}", log_of(e)))
            .unwrap_or_else(|| panic!("{step}: the cycle wrote a ledger"));
    assert_eq!(l.alert.as_deref(), alert, "{step}: alert");
    assert_eq!(
        l.advisory_alert.as_deref(),
        advisory,
        "{step}: advisory_alert"
    );
}

#[test]
fn an_identity_refusal_after_an_advisory_alert_leaves_a_loadable_ledger() {
    let e = env();

    let mut c = ctx_with(&e, measure_advisory);
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_loads(&e, None, Some("no-capacity:work"), "advisory cycle");

    let mut c = ctx_with(&e, measure_fresh_stay);
    c.identity = unavailable;
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(got, CycleOutcome::Alerted, "{}", log_of(&e));
    assert_loads(&e, Some("trusted-identity:work"), None, "identity refusal");

    let mut c = ctx_with(&e, measure_fresh_stay);
    let got = run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(got, CycleOutcome::Stayed, "{}", log_of(&e));
    assert_eq!(ino_at(&e.live()), Some(e.l), "work's login stays live");
    assert_loads(&e, None, None, "identity recovered");
}
