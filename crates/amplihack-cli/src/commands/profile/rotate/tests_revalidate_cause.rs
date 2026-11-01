//! Issue #1721: confirming the cause, and the one-shot failed-probes case.
//!
//! This file uses only what already exists on `main` (`run_cycle`,
//! `Ctx::measure`, `Ctx::scan`, `decide_measured`), so it compiles and runs
//! against unchanged code. That is the hard gate in the design: test 1a must
//! return `Skipped` with `msg::CHANGED` on `main`, and `Swapped` after the
//! fix. The D9 tests (10a, 10b) are expected to pass on both.
//!
//! The fixture is `tests_swap::env()` (`work` live, `spare` stored) plus two
//! stored profiles, `spare2` and `third`, each with a login and an account.
//! `Ctx::scan` never reads `/proc`: it sees no session, or the sessions a
//! test's own static flag turns on. Logins are created and renamed inside
//! the temp dir only; no test reads, prints or asserts on their contents,
//! runs `claude` or touches a real home.

use super::cycle::{Ctx, CycleOutcome, MeasureFn, Mode, run_cycle};
use super::decide::{Decision, Probed, Reading, Windows};
use super::log::msg;
use super::observe::{Measurement, Snap, decide_measured, snapshot};
use super::settings;
use super::tests_swap::{CREDS, Env, env, ino_at, login, n, private};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{Liveness, State, now};
use crate::commands::profile::name::ProfileName;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

// --- Fixture ---------------------------------------------------------------

/// A stored profile `name` with an OAuth login and an account. Returns the
/// login's inode.
pub(super) fn add_stored(e: &Env, name: &str) -> u64 {
    e.store.add(&n(name)).unwrap();
    private(
        &e.root.join(name).join(".claude.json"),
        &serde_json::json!({"oauthAccount": {
            "accountUuid": format!("u-{name}"),
            "emailAddress": format!("{name}@example.com"),
        }})
        .to_string(),
    );
    login(&e.root.join(name).join(CREDS))
}

/// The fixture with `spare2` and `third` also stored. Returns their login
/// inodes.
pub(super) fn env3() -> (Env, u64, u64) {
    let e = env();
    let s2 = add_stored(&e, "spare2");
    let th = add_stored(&e, "third");
    (e, s2, th)
}

/// A watch or one-shot context whose scan sees no session.
pub(super) fn ctx_with(e: &Env, measure: MeasureFn) -> Ctx {
    let s = settings::resolve(&super::TuneFlags::default(), None, &|_| None).unwrap();
    let mut ctx = Ctx::new(e.store.clone(), e.home.clone(), s);
    ctx.scan = |_| Liveness::Known(BTreeSet::new());
    ctx.measure = Some(measure);
    ctx
}

pub(super) fn ok(session: f64, week: f64) -> Probed {
    Probed {
        reading: Reading::Measured { session, week },
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

pub(super) fn spent() -> Probed {
    Probed {
        reading: Reading::Exhausted,
        windows: Windows::default(),
        spend_limit: false,
        unknown: false,
    }
}

/// Probes for the candidates in `snap`, read from `table`; a candidate the
/// table does not name is not probed.
pub(super) fn probes_for(snap: &Snap, table: &[(&str, Probed)]) -> Vec<(ProfileName, Probed)> {
    table
        .iter()
        .filter(|(name, _)| snap.candidates.contains(&n(name)))
        .map(|(name, p)| (n(name), p.clone()))
        .collect()
}

/// Parks `name` by hand, as auto-switch does part-way through a move.
pub(super) fn park(root: &Path, name: &str) {
    let dir = root.join(".auto-switch.d").join("parked");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), b"").unwrap();
}

pub(super) fn log_of(e: &Env) -> String {
    fs::read_to_string(&e.rs.log).unwrap_or_default()
}

/// Asserts that `name` was a candidate when the cycle began.
pub(super) fn assert_candidate(snap: &Snap, name: &str) {
    assert!(
        snap.candidates.contains(&n(name)),
        "{name} must be a candidate when measuring starts: {:?}",
        snap.candidates
    );
}

// --- Test 1a: an unrelated profile is parked during measurement -------------

/// `work` is spent; `spare` has room; `third` is spent and then parked by
/// auto-switch while the cycle measures. Nothing about `work` or `spare`
/// changed.
fn park_third_while_measuring(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_candidate(snap, "spare");
    assert_candidate(snap, "third");
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0)), ("third", spent())]);
    park(ctx.store.root(), "third");
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn issue_1721_an_unrelated_profile_parked_during_measurement_does_not_stop_the_swap() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, park_third_while_measuring);
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(
        outcome,
        CycleOutcome::Swapped,
        "issue #1721: `third` was never the target, so its being parked must \
         not cancel the swap to `spare`. On main this is Skipped with \
         `{}`. Log:\n{log}",
        msg::CHANGED
    );
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert_eq!(ino_at(&e.slot("work")), Some(e.l), "work's login was moved");
    assert!(!log.contains(msg::CHANGED), "{log}");
}

// --- Test 1b: a session starts in an unrelated profile ----------------------

static THIRD_SESSION_1B: AtomicBool = AtomicBool::new(false);

fn scan_1b(_: &Path) -> Liveness {
    let mut live = BTreeSet::new();
    if THIRD_SESSION_1B.load(Ordering::SeqCst) {
        live.insert("third".to_string());
    }
    Liveness::Known(live)
}

fn session_in_third_while_measuring(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    assert_candidate(snap, "third");
    let at = now();
    let probes = probes_for(snap, &[("spare", ok(0.0, 43.0)), ("third", spent())]);
    THIRD_SESSION_1B.store(true, Ordering::SeqCst);
    decide_measured(ctx, snap, spent(), probes, at)
}

#[cfg(target_os = "linux")]
#[test]
fn issue_1721_a_session_started_in_an_unrelated_profile_does_not_stop_the_swap() {
    let (e, _, _) = env3();
    let mut ctx = ctx_with(&e, session_in_third_while_measuring);
    ctx.scan = scan_1b;
    let outcome = run_cycle(&mut ctx, Mode::Watch).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
}

// --- Test 10: failed probes on some profiles (the 20:36:58Z one-shot run) ---

/// `work` exhausted; `spare2` and `third` failed; `spare` at 0%/43%.
fn some_probes_failed(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = now();
    let probes = probes_for(
        snap,
        &[
            ("spare2", Probed::failed()),
            ("third", Probed::failed()),
            ("spare", ok(0.0, 43.0)),
        ],
    );
    decide_measured(ctx, snap, spent(), probes, at)
}

#[test]
fn issue_1721_failed_probes_on_other_candidates_do_not_stop_a_swap_decision() {
    let (e, _, _) = env3();
    let ctx = ctx_with(&e, some_probes_failed);
    let state = State::new(ctx.store.root()).unwrap();
    let snap = snapshot(&ctx, &state).unwrap();
    for c in ["spare", "spare2", "third"] {
        assert_candidate(&snap, c);
    }
    let m = some_probes_failed(&ctx, &snap).unwrap();
    match m.decision {
        Decision::Swap { to, .. } => assert_eq!(to, n("spare")),
        other => panic!("expected a swap to spare, got {other:?}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
fn issue_1721_a_one_shot_run_swaps_to_the_one_candidate_that_measured() {
    let (e, _, _) = env3();
    // The shared fixture keeps a key in `$HOME`, which has its own ALERT.
    private(
        &e.home.join(".claude.json"),
        &serde_json::json!({
            "oauthAccount": {"accountUuid": "u-work", "emailAddress": "work@example.com"}
        })
        .to_string(),
    );
    let mut ctx = ctx_with(&e, some_probes_failed);
    let outcome = run_cycle(&mut ctx, Mode::OneShot).unwrap();
    let log = log_of(&e);
    assert_eq!(outcome, CycleOutcome::Swapped, "{log}");
    assert_eq!(ino_at(&e.live()), Some(e.t), "spare's login is live");
    assert!(!log.contains("ALERT"), "{log}");
}
