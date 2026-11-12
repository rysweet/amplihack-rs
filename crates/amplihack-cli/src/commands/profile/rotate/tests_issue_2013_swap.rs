//! Issue #2013 (R3/D13, R13/D17): the swap's write order and proactive
//! rotation.
//!
//! D13: the login file and `~/.claude.json` are separate files, so a swap
//! cannot write both at once. Each is written atomically, the journal order
//! stays J1/J2, s3 (the block, from the verified identity), s4 (the
//! ledger), recovery replays `finish`, and the per-cycle repair is the
//! backstop. A swap stopped before s3 is the state the existing
//! `abort_point("rotate-S3")` crash leaves; it is built here by hand from
//! the journal, as `tests_recover` does, because `abort_point` ends the
//! process.
//!
//! D17: a live login past `session_at` (or `week_at`) on a fresh reading
//! moves in that cycle to an eligible login, before it is exhausted, with
//! `reason=threshold`. Since the 7 November addendum, a live login past
//! `session_at` only moves to a login that ranks ahead of it on week use;
//! it never trades weekly room for session room.

use super::cycle::{Ctx, CycleOutcome, Mode, run_cycle};
use super::ledger::Phase;
use super::observe::{Measurement, Snap, decide_measured};
use super::swap::RecoverOutcome;
use super::tests_issue_2013_common::{ctx_at, home_block, home_uuid, lines, rd, record, settings};
use super::tests_revalidate_cause::probes_for;
use super::tests_swap::{env, ino_at};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use std::fs;

fn measure_stay(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("work", rd(10.0, 10.0, None))]);
    decide_measured(ctx, snap, rd(10.0, 10.0, None), probes, at)
}

/// The swap stopped after s2: the logins are exchanged and the journal
/// says S2, so `~/.claude.json` still names `work`. Recovery finishes s3
/// from the verified incoming identity. A session then writes `work`'s
/// block back; the next cycle repairs it, so both files name `spare`.
#[cfg(target_os = "linux")]
#[test]
fn d13_a_swap_stopped_before_s3_ends_with_both_files_naming_the_new_login() {
    let e = env();
    e.rs.save_journal(&e.journal(Phase::S2)).unwrap();
    e.exchanged();
    fs::rename(e.slot("spare"), e.slot("work")).unwrap();
    assert_eq!(home_uuid(&e.home).as_deref(), Some("u-work"), "before s3");
    assert!(matches!(
        e.recover().unwrap(),
        RecoverOutcome::RolledForward { .. }
    ));
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert_eq!(
        home_uuid(&e.home).as_deref(),
        Some("u-spare"),
        "s3 replayed"
    );
    let l = e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.live.as_deref(), Some("spare"));

    // A Claude Code session started under `work` writes its block back.
    home_block(&e.home, Some(("u-work", "work@example.com")));
    record(&e.root, "spare", "u-spare", "spare@example.com");
    let mut c = ctx_at(&e, measure_stay, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed,
        "{:?}",
        lines(&e)
    );
    assert_eq!(home_uuid(&e.home).as_deref(), Some("u-spare"), "repaired");
    assert!(
        lines(&e).iter().any(|l| l
            == "repair: home account metadata named work@example.com, verified identity is spare@example.com; rewritten"),
        "{:?}",
        lines(&e)
    );
}

fn measure_d17(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    // spare's weekly headroom (62) beats work's (60) by less than
    // --margin, but spare is eligible and ranks ahead on week.
    let probes = probes_for(snap, &[("spare", rd(10.0, 38.0, None))]);
    decide_measured(ctx, snap, rd(86.0, 40.0, None), probes, at)
}

fn measure_d17_less_week_room(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(10.0, 82.0, None))]);
    decide_measured(ctx, snap, rd(86.0, 40.0, None), probes, at)
}

/// The 7 November addendum: spare has session room but 42 fewer points of
/// weekly room, so `work`, past `session_at` only, stays.
#[cfg(target_os = "linux")]
#[test]
fn d17_a_live_login_past_session_at_never_moves_to_less_weekly_room() {
    let e = env();
    let live = ino_at(&e.live());
    let mut c = ctx_at(&e, measure_d17_less_week_room, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), live);
    assert!(
        !lines(&e).iter().any(|l| l.starts_with("swap ")),
        "{:?}",
        lines(&e)
    );
}

#[cfg(target_os = "linux")]
#[test]
fn d17_a_live_login_past_session_at_moves_before_it_is_exhausted() {
    let e = env();
    let mut c = ctx_at(&e, measure_d17, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.t));
    assert!(
        lines(&e).iter().any(|l| l
            == "swap work->spare live s=86% w=40% age=0s target s=10% w=38% age=0s reason=threshold"),
        "{:?}",
        lines(&e)
    );
}

fn measure_week_at(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    let at = (ctx.clock)();
    let probes = probes_for(snap, &[("spare", rd(10.0, 40.0, None))]);
    decide_measured(ctx, snap, rd(10.0, 91.0, None), probes, at)
}

/// The weekly threshold comes from `week_at` in the settings: at 91% the
/// default (90) moves, and `--week-at 95` stays (with upgrades turned off,
/// so only the threshold rule could move it).
#[cfg(target_os = "linux")]
#[test]
fn d17_the_weekly_threshold_comes_from_the_settings() {
    let e = env();
    let mut c = ctx_at(&e, measure_week_at, settings(), now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Swapped
    );

    let e = env();
    let s = super::settings::Settings {
        week_at: 95,
        upgrade_margin: 100,
        ..settings()
    };
    let mut c = ctx_at(&e, measure_week_at, s, now());
    assert_eq!(
        run_cycle(&mut c, Mode::Watch).unwrap(),
        CycleOutcome::Stayed,
        "{:?}",
        lines(&e)
    );
    assert_eq!(ino_at(&e.live()), Some(e.l));
}
