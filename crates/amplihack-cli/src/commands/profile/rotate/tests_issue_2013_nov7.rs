//! Issue #2013, the 7 November addendum (ryan-mistt, 2026-11-07T02:44:46Z).
//!
//! From 02:33Z to 02:40Z the rotator swapped six times among logins at
//! 97-99% weekly while `eelsworth` (s=71% w=21%), `iivanovich` (s=75%
//! w=62%), `llarame` (s=73% w=62%) and `ryanc2-mistt-org` (s=71% w=82%) had
//! room. `--session-at` was 70, and plan-room pass 1 kept only logins with
//! session use below it, so every login with weekly room was dropped.
//!
//! The addendum asks for:
//! - session room as a tie-break, never a filter that drops a login with
//!   weekly room (`rank::eligible` no longer reads `--session-at`);
//! - a test with exactly those logins at `--session-at 70` that picks
//!   `eelsworth`;
//! - a test that at most one plan-room swap happens per `--hold` window
//!   among logins at or above `--week-at`.
//!
//! The cycle tests run the real decision, swap, ledger and log under a
//! temporary HOME and profile root. Probing is the `Ctx.measure` stub and
//! time is the injected clock (`tests_issue_2013_common::ctx_at`); nothing
//! waits on the wall clock.

use super::cycle::{CycleOutcome, Mode, run_cycle};
use super::decide::{Decision, Measured, Probed, Reason, decide};
use super::observe::{Measurement, Snap, decide_measured};
use super::plan_room::choose;
use super::settings::Settings;
use super::tests_issue_2013_common::{ctx_at, lines, m, rd, settings};
use super::tests_revalidate_cause::{add_stored, probes_for};
use super::tests_swap::{Env, env, ino_at, n};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;

const T: i64 = 1_794_200_000;

/// The settings on candide at 02:33Z: the defaults with `--session-at 70`.
fn session_at_70() -> Settings {
    Settings {
        session_at: 70,
        ..settings()
    }
}

/// The logins of 7 November. `kkane` (w=99%) was live; the session figures
/// of the logins at 97-99% weekly were not in the report, so they are
/// placed below 70 to give the old filter every chance to pick them.
const NOV7: [(&str, f64, f64); 7] = [
    ("ffarmer", 10.0, 97.0),
    ("oole", 41.0, 97.0),
    ("nnorth", 17.0, 98.0),
    ("eelsworth", 71.0, 21.0),
    ("iivanovich", 75.0, 62.0),
    ("llarame", 73.0, 62.0),
    ("ryanc2-mistt-org", 71.0, 82.0),
];

fn nov7() -> (Measured, Vec<Measured>) {
    (
        m("kkane", 30.0, 99.0),
        NOV7.iter().map(|(p, s, w)| m(p, *s, *w)).collect(),
    )
}

// --- Pure: decide and plan_room::choose ------------------------------------------

#[test]
fn nov7_at_session_at_70_the_threshold_swap_picks_eelsworth() {
    let (live, c) = nov7();
    assert_eq!(
        decide(&session_at_70(), &live.reading, &c, None, T),
        Decision::Swap {
            to: n("eelsworth"),
            reason: Reason::Threshold
        }
    );
}

#[test]
fn nov7_at_session_at_70_plan_room_picks_eelsworth() {
    let (live, c) = nov7();
    assert_eq!(
        choose(&session_at_70(), Some(&live), &c),
        Some(n("eelsworth"))
    );
}

/// Once on `eelsworth` (s=71% past `--session-at 70`, w=21%), no other
/// login ranks ahead of it on weekly use, so the rotator stays rather than
/// trade 41 or more points of weekly room for session room.
#[test]
fn nov7_on_eelsworth_past_session_at_the_rotator_stays() {
    let (kkane, c) = nov7();
    let live = c.iter().find(|x| x.name.as_str() == "eelsworth").unwrap();
    let others: Vec<Measured> = c
        .iter()
        .filter(|x| x.name.as_str() != "eelsworth")
        .cloned()
        .chain([kkane])
        .collect();
    let s = session_at_70();
    assert_eq!(decide(&s, &live.reading, &others, None, T), Decision::Stay);
    assert_eq!(choose(&s, Some(live), &others), None);
}

/// The same live login at session 100% has no room, so it moves, to the
/// login with the most weekly room among those with room.
#[test]
fn nov7_eelsworth_at_100_percent_session_moves_to_the_next_most_weekly_room() {
    let (kkane, c) = nov7();
    let live = m("eelsworth", 100.0, 21.0);
    let others: Vec<Measured> = c
        .iter()
        .filter(|x| x.name.as_str() != "eelsworth")
        .cloned()
        .chain([kkane])
        .collect();
    assert_eq!(
        decide(&session_at_70(), &live.reading, &others, None, T),
        Decision::Swap {
            to: n("llarame"),
            reason: Reason::Threshold
        },
        "iivanovich and llarame tie on week 62; llarame's session is lower"
    );
}

// --- Cycle: the 7 November fixture through run_cycle ------------------------------

fn swap_lines(e: &Env) -> Vec<String> {
    lines(e)
        .into_iter()
        .filter(|l| l.starts_with("swap "))
        .collect()
}

/// The fixture's live login `work` stands in for `kkane`; `spare` is the
/// fixture's second login, put at 99% weekly with the other full ones.
fn nov7_env() -> Env {
    let e = env();
    for (p, _, _) in NOV7 {
        add_stored(&e, p);
    }
    e
}

fn measure_nov7(ctx: &super::cycle::Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let mut table: Vec<(&str, Probed)> = NOV7
        .iter()
        .map(|(p, s, w)| (*p, rd(*s, *w, None)))
        .collect();
    table.push(("spare", rd(5.0, 99.0, None)));
    decide_measured(
        ctx,
        snap,
        rd(30.0, 99.0, None),
        probes_for(snap, &table),
        at,
    )
}

#[cfg(target_os = "linux")]
#[test]
fn cycle_nov7_at_session_at_70_swaps_kkane_to_eelsworth() {
    let e = nov7_env();
    let eel = ino_at(&e.slot("eelsworth"));
    let mut c = ctx_at(&e, measure_nov7, session_at_70(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    let log = lines(&e);
    assert_eq!(
        swap_lines(&e),
        ["swap work->eelsworth live s=30% w=99% age=0s target s=71% w=21% age=0s reason=threshold"],
        "{log:?}"
    );
    assert_eq!(ino_at(&e.live()), eel, "eelsworth's login is live");
}

// --- Cycle: at most one plan-room swap per --hold window among full logins --------

/// Every login is at or above `--week-at` (90). Readings depend only on
/// which login is live, never on the time:
/// - while `work` is live: `work` s=20% w=99%, `aa` s=10% w=93%, `bb`
///   s=10% w=99%. `aa` has 7 points of weekly room against `work`'s 1,
///   which beats `--margin` (5), so the plan-room path moves to `aa`.
/// - once `aa` is live: `aa` s=30% w=99% (spent), `bb` s=10% w=93%. `bb`
///   now beats `aa` by the margin, so without the hold the path would move
///   again at once.
fn measure_full(ctx: &super::cycle::Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let aa_live = !snap.candidates.contains(&n("aa"));
    let table: Vec<(&str, Probed)> = if aa_live {
        vec![
            ("aa", rd(30.0, 99.0, None)),
            ("bb", rd(10.0, 93.0, None)),
            ("work", rd(20.0, 99.0, None)),
            ("spare", rd(5.0, 99.0, None)),
        ]
    } else {
        vec![
            ("work", rd(20.0, 99.0, None)),
            ("aa", rd(10.0, 93.0, None)),
            ("bb", rd(10.0, 99.0, None)),
            ("spare", rd(5.0, 99.0, None)),
        ]
    };
    let live = table
        .iter()
        .find(|(p, _)| !snap.candidates.contains(&n(p)))
        .map(|(_, p)| p.clone())
        .expect("one login in the table is live");
    decide_measured(ctx, snap, live, probes_for(snap, &table), at)
}

#[cfg(target_os = "linux")]
#[test]
fn cycle_at_most_one_plan_room_swap_per_hold_window_among_logins_at_week_at() {
    let e = env();
    add_stored(&e, "aa");
    add_stored(&e, "bb");
    let bb = ino_at(&e.slot("bb"));
    let s = settings();
    assert_eq!(s.hold, 3600);
    let t0 = now();

    // First cycle: the move to aa.
    let mut c = ctx_at(&e, measure_full, s, t0);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(swap_lines(&e).len(), 1, "{:?}", lines(&e));
    assert!(
        swap_lines(&e)[0].starts_with("swap work->aa "),
        "{:?}",
        lines(&e)
    );
    let aa_live = ino_at(&e.live());

    // Ten minutes later, inside the hold: bb beats aa by the margin, but a
    // second move among full logins waits for the hold to pass.
    let mut c = ctx_at(&e, measure_full, s, t0 + 600);
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(swap_lines(&e).len(), 1, "{:?}", lines(&e));
    assert_eq!(ino_at(&e.live()), aa_live, "aa stays live inside the hold");

    // Just before the hold ends: still one swap.
    let mut c = ctx_at(&e, measure_full, s, t0 + 3590);
    run_cycle(&mut c, Mode::Watch).unwrap();
    assert_eq!(swap_lines(&e).len(), 1, "{:?}", lines(&e));

    // After the hold: the next move is allowed.
    let mut c = ctx_at(&e, measure_full, s, t0 + 3600 + 60);
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    let swaps = swap_lines(&e);
    assert_eq!(swaps.len(), 2, "{swaps:?}");
    assert!(swaps[1].starts_with("swap aa->bb "), "{swaps:?}");
    assert_eq!(ino_at(&e.live()), bb);
}
