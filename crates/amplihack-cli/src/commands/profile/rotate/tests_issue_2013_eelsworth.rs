//! Issue #2013 (D15): why the cycles of 6 November skipped `eelsworth`, and
//! the fix.
//!
//! Finding, drawn from the code (the candide evidence files were not read):
//! the threshold rule ranked by overall headroom and plan-room pass 1
//! filtered on session only. `eelsworth` (w=11%) was at or above
//! `session_at`, so the threshold rule's margin test failed on its small
//! overall headroom, plan-room pass 1 dropped it on session, and pass 1
//! then chose among the logins at w=97% (`ffarmer`, `oole`, `nnorth`).
//!
//! With one ranking (`rank.rs`): a login at `week_at` is never chosen while
//! an eligible login exists, and any other move needs `margin` points of
//! weekly headroom over the live login. Since the 7 November addendum,
//! `session_at` excludes no login with weekly room: `eelsworth` at s=88%
//! (past the default 85) is still chosen for its 89 points of weekly room.
//! Only a session at 100% (no room) drops it.
//!
//! The cycle tests use the real decision, gate, swap, ledger and log under
//! a temporary HOME and profile root; probing is the `Ctx.measure` stub and
//! the time is the injected clock.

use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Measured, Reason, decide};
use super::observe::{Measurement, Snap, decide_measured};
use super::plan_room::choose;
use super::tests_issue_2013_common::{ctx_at, exh, failed, lines, m, rd, settings};
use super::tests_revalidate_cause::{add_stored, probes_for};
use super::tests_swap::{Env, env, ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;

const T: i64 = 1_794_200_000;

/// The six logins of 6 November, with `nnorth` live (as `work` in the cycle
/// fixture) and the sessions of the reference example.
fn six(eelsworth_session: f64) -> (Measured, Vec<Measured>) {
    (
        m("nnorth", 17.0, 98.0),
        vec![
            m("ffarmer", 10.0, 97.0),
            m("oole", 41.0, 97.0),
            m("eelsworth", eelsworth_session, 11.0),
            m("iivanovich", 20.0, 52.0),
            m("llarame", 30.0, 52.0),
        ],
    )
}

// --- Pure: decide and plan_room::choose ------------------------------------------

/// The 7 November addendum to #2013: a login is never passed over for one
/// with less weekly room only because its session use is past
/// `session_at`. Before the addendum this test expected `iivanovich`
/// (w=52%), which was that forbidden outcome.
#[test]
fn with_eelsworth_past_session_at_it_is_still_chosen_for_its_weekly_room() {
    let (live, c) = six(88.0);
    let s = settings();
    assert_eq!(
        decide(&s, &live.reading, &c, None, T),
        Decision::Swap {
            to: n("eelsworth"),
            reason: Reason::Threshold
        }
    );
    assert_eq!(choose(&s, Some(&live), &c), Some(n("eelsworth")));
}

/// Only a session with no room left (100%) drops `eelsworth`; then the
/// next most weekly room is `iivanovich`, never a login at 97%.
#[test]
fn with_eelsworth_at_100_percent_session_the_cycle_picks_iivanovich_never_a_97_percent_login() {
    let (live, c) = six(100.0);
    let s = settings();
    assert_eq!(
        decide(&s, &live.reading, &c, None, T),
        Decision::Swap {
            to: n("iivanovich"),
            reason: Reason::Threshold
        }
    );
    assert_eq!(choose(&s, Some(&live), &c), Some(n("iivanovich")));
}

#[test]
fn with_eelsworth_below_session_at_it_is_chosen() {
    let (live, c) = six(10.0);
    let s = settings();
    assert_eq!(
        decide(&s, &live.reading, &c, None, T),
        Decision::Swap {
            to: n("eelsworth"),
            reason: Reason::Threshold
        }
    );
    assert_eq!(choose(&s, Some(&live), &c), Some(n("eelsworth")));
}

/// The 6 November path: `eelsworth` near its session limit and every other
/// login with weekly room also past `session_at`. Before #2013 the threshold
/// rule's margin test failed (overall headroom 4 against 2) and plan-room
/// pass 1, filtering on session only, chose `ffarmer` at w=97%.
#[test]
fn regression_6_nov_plan_room_never_moves_to_a_97_percent_login_while_eelsworth_has_weekly_room() {
    let live = m("nnorth", 17.0, 98.0);
    let c = vec![
        m("ffarmer", 10.0, 97.0),
        m("oole", 41.0, 97.0),
        m("eelsworth", 96.0, 11.0),
        m("iivanovich", 90.0, 52.0),
        m("llarame", 92.0, 52.0),
    ];
    let s = settings();
    let d = decide(&s, &live.reading, &c, None, T);
    assert_eq!(
        d,
        Decision::Swap {
            to: n("eelsworth"),
            reason: Reason::Threshold
        },
        "eelsworth has 89 points of weekly headroom against nnorth's 2"
    );
    let chosen = choose(&s, Some(&live), &c);
    assert!(
        !matches!(
            chosen.as_ref().map(|p| p.as_str()),
            Some("ffarmer" | "oole")
        ),
        "plan-room chose {chosen:?}"
    );
    assert_eq!(chosen, Some(n("eelsworth")));
}

// --- Cycle: the same choice through run_cycle -----------------------------------------

fn fx() -> Env {
    let e = env();
    for p in ["ffarmer", "oole", "eelsworth", "iivanovich", "llarame"] {
        add_stored(&e, p);
    }
    e
}

fn swap_lines(e: &Env) -> Vec<String> {
    lines(e)
        .into_iter()
        .filter(|l| l.starts_with("swap "))
        .collect()
}

fn decide_with(
    ctx: &super::cycle::Ctx,
    snap: &Snap,
    eelsworth: super::decide::Probed,
) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(
        snap,
        &[
            ("ffarmer", rd(10.0, 97.0, None)),
            ("oole", rd(41.0, 97.0, None)),
            ("eelsworth", eelsworth),
            ("iivanovich", rd(20.0, 52.0, None)),
            ("llarame", rd(30.0, 52.0, None)),
        ],
    );
    decide_measured(ctx, snap, rd(17.0, 98.0, None), probes, at)
}

fn measure_d15(ctx: &super::cycle::Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    decide_with(ctx, snap, rd(88.0, 11.0, None))
}

#[cfg(target_os = "linux")]
#[test]
fn cycle_d15_swaps_to_eelsworth_and_logs_every_reading_with_its_age() {
    let e = fx();
    let mut c = ctx_at(&e, measure_d15, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    let log = lines(&e);
    assert_eq!(
        swap_lines(&e),
        ["swap work->eelsworth live s=17% w=98% age=0s target s=88% w=11% age=0s reason=threshold"],
        "{log:?}"
    );
    let cycle = log.iter().find(|l| l.starts_with("cycle ")).unwrap();
    assert!(cycle.contains("eelsworth:s=88%,w=11%,age=0s"), "{cycle}");
    assert!(cycle.contains("ffarmer:s=10%,w=97%,age=0s"), "{cycle}");
}

fn measure_eelsworth_failed(
    ctx: &super::cycle::Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    decide_with(ctx, snap, failed("timeout"))
}

#[cfg(target_os = "linux")]
#[test]
fn cycle_a_failed_eelsworth_is_left_out_and_shown_as_failed() {
    let e = fx();
    let mut c = ctx_at(&e, measure_eelsworth_failed, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    let log = lines(&e);
    let cycle = log.iter().find(|l| l.starts_with("cycle ")).unwrap();
    assert!(cycle.contains(" eelsworth:failed"), "{cycle}");
    assert!(
        log.iter()
            .any(|l| l == "reading: eelsworth probe failed: timeout"),
        "{log:?}"
    );
    assert!(
        swap_lines(&e)[0].starts_with("swap work->iivanovich "),
        "{log:?}"
    );
}

fn measure_eelsworth_carried(
    ctx: &super::cycle::Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    let mut p = rd(10.0, 11.0, None);
    p.carried = true;
    p.started_at = Some((ctx.clock)() - 700);
    decide_with(ctx, snap, p)
}

/// A result collected from a read started in an earlier cycle has no
/// reading time: it is stale, shown so, and never chosen, even with the
/// best figures.
#[cfg(target_os = "linux")]
#[test]
fn cycle_a_carried_eelsworth_reading_is_shown_stale_and_never_chosen() {
    let e = fx();
    let mut c = ctx_at(&e, measure_eelsworth_carried, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );
    let log = lines(&e);
    let cycle = log.iter().find(|l| l.starts_with("cycle ")).unwrap();
    assert!(cycle.contains(" eelsworth:stale"), "{cycle}");
    assert!(
        log.iter().any(
            |l| l.starts_with("reading: eelsworth s=10% w=11% stale age=")
                && l.ends_with("s (not used)")
        ),
        "{log:?}"
    );
    assert!(
        swap_lines(&e)[0].starts_with("swap work->iivanovich "),
        "{log:?}"
    );
}

fn measure_eelsworth_back(
    ctx: &super::cycle::Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    decide_with(ctx, snap, rd(10.0, 11.0, None))
}

/// `eelsworth` was swapped away from while full; a reading taken after
/// that swap shows room, so the damping entry clears and it is chosen.
#[cfg(target_os = "linux")]
#[test]
fn cycle_a_damped_eelsworth_with_a_later_fresh_reading_can_be_chosen() {
    let e = fx();
    let t = now();
    let mut l = e.rs.load_ledger().unwrap().unwrap();
    l.swapped_away.insert("eelsworth".into(), t - 600);
    e.rs.save_ledger(&l).unwrap();
    let eel = ino_at(&e.slot("eelsworth"));
    let mut c = ctx_at(&e, measure_eelsworth_back, settings(), t);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), eel, "eelsworth's login is live");
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert!(
        !l.swapped_away.contains_key("eelsworth"),
        "{:?}",
        l.swapped_away
    );
}

fn measure_all_near_limit(
    ctx: &super::cycle::Ctx,
    snap: &Snap,
) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(
        snap,
        &[
            ("ffarmer", rd(10.0, 97.0, None)),
            ("oole", rd(41.0, 97.0, None)),
            ("eelsworth", exh(100.0, 99.0)),
            ("iivanovich", rd(20.0, 99.0, None)),
            ("llarame", rd(30.0, 100.0, None)),
        ],
    );
    decide_measured(ctx, snap, rd(17.0, 98.0, None), probes, at)
}

/// D4: no login is eligible and none beats the live login's weekly headroom
/// by `margin`, so the cycle stays and lists every candidate's reading.
#[cfg(target_os = "linux")]
#[test]
fn cycle_with_every_login_near_its_weekly_limit_stays_and_says_so() {
    let e = fx();
    let live = ino_at(&e.live());
    let mut c = ctx_at(&e, measure_all_near_limit, settings(), now());
    run_cycle(&mut c, Mode::Watch).unwrap();
    let log = lines(&e);
    assert!(swap_lines(&e).is_empty(), "{log:?}");
    assert_eq!(ino_at(&e.live()), live);
    let stay = log
        .iter()
        .find(|l| l.starts_with("stay: no login with room; live work s=17% w=98% age=0s;"))
        .unwrap_or_else(|| panic!("no stay line: {log:?}"));
    for want in [
        "ffarmer s=10% w=97% age=0s",
        "oole s=41% w=97% age=0s",
        "iivanovich s=20% w=99% age=0s",
    ] {
        assert!(stay.contains(want), "{want} missing from {stay}");
    }
}
