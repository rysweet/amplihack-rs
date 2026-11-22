//! Issue #2910 (D6, D7, F5, F7, AC5, AC7, A1, A2, S2, S4, E5, E5b): the
//! confirmation of the target before a swap, and what an unknown reading
//! does. Wired from `target_confirm.rs`.
//!
//! The first part tests the pure step. For one candidate about to be chosen,
//! `next` says what to do from the facts of the cycle:
//!
//! | The candidate | Step |
//! |---------------|------|
//! | read directly this cycle and fresh | `Accept` (no second read) |
//! | stale, and this cycle's three re-reads are used | `NotTried(Count)` |
//! | its probe failed this cycle, live login has no room now | `AcceptUnconfirmed(label)` |
//! | its probe failed this cycle, live login has room | `Reject(Unknown(label))` |
//! | stale, and the cycle can read no more | `NotTried(Time)` |
//! | stale, and it can be read | `ReadAgain` |
//!
//! `after_unknown` is the same choice for a re-read that came back unknown.
//! A rejection made without a read counts against the cap of three (E5).
//!
//! The second part runs real cycles with a counting re-read hook (AC5) on the
//! shared fixture (`tests_issue_2910_fixture`).
//!
//! API assumed, from the design (in `target_confirm.rs`):
//!
//! ```text
//! struct Facts { stale: bool, failed_now: bool, attempts: usize, can_read: bool,
//!                live_no_room: bool, label: Option<ProbeLabel> }
//! enum Limit { Count, Time }
//! enum Why { Unknown(ProbeLabel), Unfit(Unfit) }
//! enum Step { Accept, AcceptUnconfirmed(ProbeLabel), ReadAgain, Reject(Why),
//!             NotTried(Limit) }
//! next(f: &Facts) -> Step
//! after_unknown(label: ProbeLabel, live_no_room: bool) -> Step
//! ```
//!
//! The re-read hook takes the new signature
//! `fn(&Ctx, &ProfileName, Duration) -> (Probed, Option<ProbeLabel>)`.

use super::*;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::config::{self, ReadMode};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode};
use crate::commands::profile::rotate::decide::Probed;
use crate::commands::profile::rotate::decide::target_seen::tests_issue_2910_fixture::{
    Fleet, alerts, clock, expired, fleet, lines_with, name, reporter, run, run_full, stub, unread,
};
use crate::commands::profile::rotate::fresh::{MAX_REREADS_PER_CYCLE, RereadFn};
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::observe::{Measurement, Snap};
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use crate::commands::profile::rotate::tests_swap::{CREDS, private};
use std::sync::Mutex;
use std::time::Duration;

use ProbeLabel::{Http429, Transport};

// --- The pure step ---------------------------------------------------------------------

fn facts() -> Facts {
    Facts {
        stale: true,
        failed_now: false,
        attempts: 0,
        can_read: true,
        live_no_room: false,
        label: None,
    }
}

#[test]
fn a_candidate_read_directly_and_fresh_is_accepted_whatever_else_is_true() {
    for attempts in [0, 1, MAX_REREADS_PER_CYCLE, 99] {
        for (can_read, live_no_room) in [(true, true), (true, false), (false, true), (false, false)]
        {
            let f = Facts {
                stale: false,
                attempts,
                can_read,
                live_no_room,
                ..facts()
            };
            assert_eq!(next(&f), Step::Accept);
        }
    }
}

#[test]
fn a_stale_candidate_that_can_be_read_is_read_again() {
    assert_eq!(next(&facts()), Step::ReadAgain);
    let f = Facts {
        attempts: MAX_REREADS_PER_CYCLE - 1,
        ..facts()
    };
    assert_eq!(next(&f), Step::ReadAgain, "the third read is still allowed");
}

#[test]
fn once_three_reads_are_used_the_candidate_is_not_tried() {
    let f = Facts {
        attempts: MAX_REREADS_PER_CYCLE,
        ..facts()
    };
    assert_eq!(next(&f), Step::NotTried(Limit::Count));
    let f = Facts {
        attempts: 40,
        ..facts()
    };
    assert_eq!(next(&f), Step::NotTried(Limit::Count));
}

#[test]
fn a_stale_candidate_the_cycle_cannot_read_is_not_tried_for_time() {
    let f = Facts {
        can_read: false,
        ..facts()
    };
    assert_eq!(next(&f), Step::NotTried(Limit::Time));
}

#[test]
fn the_count_limit_comes_before_the_time_limit() {
    let f = Facts {
        attempts: MAX_REREADS_PER_CYCLE,
        can_read: false,
        ..facts()
    };
    assert_eq!(next(&f), Step::NotTried(Limit::Count));
}

/// A probe that already failed this cycle is not probed again in the same
/// cycle (D6): there is nothing to read, only to decide.
#[test]
fn a_candidate_whose_probe_failed_this_cycle_is_never_read_again() {
    let f = Facts {
        failed_now: true,
        label: Some(Http429),
        ..facts()
    };
    assert_eq!(next(&f), Step::Reject(Why::Unknown(Http429)));
    let f = Facts {
        failed_now: true,
        label: Some(Http429),
        live_no_room: true,
        ..facts()
    };
    assert_eq!(next(&f), Step::AcceptUnconfirmed(Http429));
    // Even when the cycle could read, and the count is not used.
    let f = Facts {
        failed_now: true,
        can_read: true,
        attempts: 0,
        label: Some(Transport),
        live_no_room: true,
        ..facts()
    };
    assert_eq!(next(&f), Step::AcceptUnconfirmed(Transport));
}

#[test]
fn a_rejection_without_a_read_still_counts_against_the_limit() {
    // At the cap, a failed candidate is not tried rather than rejected, so the
    // cycle writes at most three `reject:` lines.
    let f = Facts {
        failed_now: true,
        attempts: MAX_REREADS_PER_CYCLE,
        label: Some(Http429),
        ..facts()
    };
    assert_eq!(next(&f), Step::NotTried(Limit::Count));
}

#[test]
fn a_reread_that_came_back_unknown_is_accepted_only_when_the_live_login_has_no_room() {
    assert_eq!(
        after_unknown(Http429, true),
        Step::AcceptUnconfirmed(Http429)
    );
    assert_eq!(
        after_unknown(Transport, true),
        Step::AcceptUnconfirmed(Transport)
    );
    assert_eq!(
        after_unknown(Http429, false),
        Step::Reject(Why::Unknown(Http429))
    );
    assert_eq!(
        after_unknown(Transport, false),
        Step::Reject(Why::Unknown(Transport))
    );
}

// --- Re-read hooks: each test has its own statics (tests run in parallel) -------------------

type Calls = Mutex<Vec<String>>;

fn record(calls: &Calls, who: &ProfileName) {
    calls.lock().unwrap().push(who.to_string());
}

fn calls(c: &Calls) -> Vec<String> {
    c.lock().unwrap().clone()
}

fn stale_at() -> Option<i64> {
    Some(clock() - 60)
}

fn run_with(
    f: &Fleet,
    live: Probed,
    table: &[(&str, Probed)],
    labels: &[(&str, ProbeLabel)],
    reread: Option<RereadFn>,
) -> CycleOutcome {
    run_full(
        f,
        reporter(),
        live,
        table,
        labels,
        Mode::Watch,
        reread,
        stub,
    )
}

// --- AC5: one re-read per candidate tried, none for a direct or failed one ----------------------

static CALLS_AC5: Calls = Mutex::new(Vec::new());

fn reread_ac5(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&CALLS_AC5, who);
    match who.as_str() {
        "top" => (exh(100.0, 40.0), None),
        "second" => (rd(10.0, 50.0, None), None),
        other => panic!("{other} must not be read again"),
    }
}

/// The live login (86/65) has room 14, so a move needs 17.
///
/// - `top` (room 60) has a passive reading; its fresh reading shows it full.
/// - `lg` (room 55 on its last good reading) answered 429 this cycle.
/// - `second` (room 50) has a passive reading; its fresh reading is as read.
/// - `direct` (room 40) was read directly this cycle.
#[test]
fn ac5_a_rejected_target_is_named_and_the_next_one_is_confirmed_with_one_read_each() {
    let f = fleet(&["top", "lg", "second", "direct"], &[]);
    f.history("lg", 1800, 0, 45);
    let out = run_with(
        &f,
        rd(86.0, 65.0, None),
        &[
            ("top", rd(0.0, 40.0, stale_at())),
            ("lg", unread()),
            ("second", rd(10.0, 50.0, stale_at())),
            ("direct", rd(20.0, 60.0, None)),
        ],
        &[("lg", Http429)],
        Some(reread_ac5),
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("second"), "{log}");
    // Exactly one read per candidate tried: none for `lg` (its probe failed in
    // this cycle) and none for `direct` (read directly in it).
    assert_eq!(calls(&CALLS_AC5), ["top", "second"], "{log}");
    assert!(
        log.contains("reject: top no room on a fresh reading"),
        "{log}"
    );
    assert!(
        log.contains("reject: lg reading unknown (HTTP 429)"),
        "{log}"
    );
    assert!(
        log.find("reject: top").unwrap() < log.find("reject: lg").unwrap(),
        "in the order the candidates were tried:\n{log}"
    );
    assert!(log.contains("swap work->second"), "{log}");
    assert!(!log.contains("unconfirmed"), "second was confirmed:\n{log}");
    assert!(!log.contains("no profile has capacity"), "{log}");
}

#[test]
fn ac5_a_candidate_that_confirms_is_read_once_and_chosen() {
    static CALLS: Calls = Mutex::new(Vec::new());
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        record(&CALLS, who);
        (rd(0.0, 40.0, None), None)
    }
    let f = fleet(&["top", "other"], &[]);
    let out = run_with(
        &f,
        rd(86.0, 65.0, None),
        &[
            ("top", rd(0.0, 40.0, stale_at())),
            ("other", rd(10.0, 50.0, stale_at())),
        ],
        &[],
        Some(reread),
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.live_is("top"));
    assert_eq!(
        calls(&CALLS),
        ["top"],
        "the second candidate was not needed"
    );
    assert!(lines_with(&f.log(), "reject:").is_empty(), "{}", f.log());
}

// --- The cap of three ---------------------------------------------------------------------------------

static CALLS_CAP: Calls = Mutex::new(Vec::new());

fn reread_cap(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&CALLS_CAP, who);
    (exh(100.0, 10.0), None)
}

#[test]
fn at_most_three_candidates_are_read_and_the_rest_are_counted_as_not_tried() {
    let names = ["c1", "c2", "c3", "c4", "c5"];
    let f = fleet(&names, &[]);
    let table: Vec<(&str, Probed)> = names
        .iter()
        .enumerate()
        .map(|(i, who)| (*who, rd(0.0, 10.0 * (i as f64 + 1.0), stale_at())))
        .collect();
    let out = run_with(&f, exh(100.0, 40.0), &table, &[], Some(reread_cap));
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert_eq!(calls(&CALLS_CAP), ["c1", "c2", "c3"], "{log}");
    assert_eq!(lines_with(&log, "reject: ").len(), 3, "{log}");
    assert!(
        log.contains("reread: c4 limit reached; treated as unknown"),
        "{log}"
    );
    assert!(
        log.contains("reread: c5 limit reached; treated as unknown"),
        "{log}"
    );
    assert!(
        log.contains(
            "ALERT: no capacity known live=work s=100% w=40% (candidates: 3 exhausted, 2 not tried (re-read limit 3))"
        ),
        "{log}"
    );
}

// --- D7: a re-read that comes back unknown ------------------------------------------------------------------

static CALLS_UNK: Calls = Mutex::new(Vec::new());

fn reread_unknown(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
    record(&CALLS_UNK, who);
    (unread(), Some(Http429))
}

#[test]
fn d7_an_unknown_reread_goes_ahead_unconfirmed_when_the_live_login_has_no_room() {
    let f = fleet(&["top"], &[]);
    let out = run_with(
        &f,
        exh(100.0, 40.0),
        &[("top", rd(0.0, 40.0, stale_at()))],
        &[],
        Some(reread_unknown),
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("top"), "{log}");
    assert!(log.contains("unconfirmed (HTTP 429)"), "{log}");
    assert!(!log.contains("reject: top"), "{log}");
}

#[test]
fn d7_an_unknown_reread_is_a_rejection_when_the_live_login_has_room() {
    static CALLS: Calls = Mutex::new(Vec::new());
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        record(&CALLS, who);
        (unread(), Some(Transport))
    }
    let f = fleet(&["top", "next"], &[]);
    let out = run_with(
        &f,
        rd(86.0, 65.0, None),
        &[
            ("top", rd(0.0, 40.0, stale_at())),
            ("next", rd(20.0, 60.0, None)),
        ],
        &[],
        Some(reread),
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("next"), "{log}");
    assert_eq!(calls(&CALLS), ["top"]);
    assert!(
        log.contains("reject: top reading unknown (transport)"),
        "{log}"
    );
    assert!(!log.contains("unconfirmed"), "{log}");
}

#[test]
fn d7_when_every_rejection_was_unknown_and_the_live_login_has_room_it_is_a_stay_with_a_note() {
    let f = fleet(&["top"], &[]);
    let out = run_with(
        &f,
        rd(86.0, 65.0, None),
        &[("top", rd(0.0, 40.0, stale_at()))],
        &[],
        Some(reread_unknown),
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        log.contains("stay: no candidate confirmed by a fresh reading"),
        "{log}"
    );
    assert!(alerts(&log).is_empty(), "{log}");
    let l = f.e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(
        l.advisory_alert.as_deref(),
        Some("no-capacity/unconfirmed:work")
    );
}

#[test]
fn d7_when_every_rejection_showed_no_room_the_no_target_line_is_written_instead() {
    static CALLS: Calls = Mutex::new(Vec::new());
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        record(&CALLS, who);
        (exh(100.0, 40.0), None)
    }
    let f = fleet(&["top"], &[]);
    run_with(
        &f,
        rd(86.0, 65.0, None),
        &[("top", rd(0.0, 40.0, stale_at()))],
        &[],
        Some(reread),
    );
    let log = f.log();
    assert!(
        log.contains("reject: top no room on a fresh reading"),
        "{log}"
    );
    assert!(
        log.contains(
            "ALERT: no profile has capacity live=work s=86% w=65% (candidates: 1 exhausted)"
        ),
        "{log}"
    );
    assert!(!log.contains("stay: no candidate confirmed"), "{log}");
}

#[test]
fn d7_a_rejected_target_below_the_thresholds_leaves_a_stay_with_no_note() {
    static CALLS: Calls = Mutex::new(Vec::new());
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        record(&CALLS, who);
        (exh(100.0, 10.0), None)
    }
    // `work` (60/60) is below both thresholds; `top` is an upgrade target.
    let f = fleet(&["top"], &[]);
    let out = run_with(
        &f,
        rd(60.0, 60.0, None),
        &[("top", rd(0.0, 10.0, stale_at()))],
        &[],
        Some(reread),
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Stayed, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        lines_with(&log, "stay: ").is_empty(),
        "nothing forced a move:\n{log}"
    );
    assert!(alerts(&log).is_empty(), "{log}");
}

// --- AC7: an identity that cannot be read is unknown --------------------------------------------------------

#[test]
fn ac7_a_login_whose_identity_cannot_be_read_counts_as_unknown_and_is_never_a_target() {
    let f = fleet(&["noid", "gone"], &[]);
    // A login with OAuth credentials and no readable identity: skipped as
    // `no account`, so it is not probed and not a candidate.
    private(
        &f.e.root.join("noid").join(CREDS),
        r#"{"claudeAiOauth":{"accessToken":"fake-token-no-identity"}}"#,
    );
    f.history("noid", 600, 0, 10);
    let out = run(
        &f,
        reporter(),
        exh(100.0, 67.0),
        &[("gone", exh(0.0, 100.0))],
        &[],
    );
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(
        log.contains(
            "ALERT: no capacity known live=work s=100% w=67% (candidates: 1 unknown (identity unavailable), 1 exhausted)"
        ),
        "{log}"
    );
}

// --- A1, S2: a login that needs signing in again is never chosen on an old reading ---------------------------

#[test]
fn a1_a_token_expired_login_with_a_good_history_point_is_failed_and_never_chosen() {
    let f = fleet(&["aa"], &[]);
    f.history("aa", 1800, 0, 20);
    let out = run(&f, reporter(), exh(100.0, 40.0), &[("aa", expired())], &[]);
    let log = f.log();
    assert_eq!(out, CycleOutcome::Alerted, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(log.contains("aa:failed"), "{log}");
    assert!(
        log.contains("ALERT: no capacity known live=work s=100% w=40% (candidates: 1 failed)"),
        "{log}"
    );
}

#[test]
fn a1_an_excluded_login_with_a_good_history_point_is_never_chosen() {
    let f = fleet(&["aa"], &[]);
    f.history("aa", 1800, 0, 20);
    config::update(&f.e.root, ReadMode::Editable, |c| {
        c.exclude.push(name("aa"));
        Ok(())
    })
    .unwrap();
    let out = run(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[("aa", unread())],
        &[("aa", Http429)],
    );
    let log = f.log();
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("swap work->aa"), "{log}");
}

// --- E5b: an overflow live login takes no unconfirmed move ---------------------------------------------------------

#[test]
fn e5b_a_healthy_overflow_login_does_not_leave_for_a_login_known_only_from_history() {
    let f = fleet(&["llarame"], &[]);
    f.mark_overflow("work");
    f.history("llarame", 1800, 20, 30);
    let out = run(
        &f,
        reporter(),
        rd(10.0, 10.0, None),
        &[("llarame", unread())],
        &[("llarame", Http429)],
    );
    let log = f.log();
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("unconfirmed"), "{log}");
}

#[test]
fn e5b_a_blocked_overflow_login_takes_no_unconfirmed_move_either() {
    let f = fleet(&["llarame"], &[]);
    f.mark_overflow("work");
    f.block("work", "spend_limit", clock() + 3600);
    f.history("llarame", 1800, 20, 30);
    let out = run(
        &f,
        reporter(),
        rd(10.0, 10.0, None),
        &[("llarame", unread())],
        &[("llarame", Http429)],
    );
    let log = f.log();
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("work"), "{log}");
    assert!(!log.contains("unconfirmed"), "{log}");
}

// --- S4: an unconfirmed swap relaxes only the usage confirmation ----------------------------------------------------

/// The measurement runs unlocked. Here the target's login loses its readable
/// identity while it does, as a `/login` in another window would.
fn stub_identity_lost(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    private(
        &ctx.store.root().join("llarame").join(CREDS),
        r#"{"claudeAiOauth":{"accessToken":"fake-token-no-identity"}}"#,
    );
    stub(ctx, snap)
}

#[test]
fn s4_a_429_login_whose_identity_cannot_be_read_is_not_swapped_to() {
    let f = fleet(&["llarame"], &[]);
    f.history("llarame", 1800, 78, 92);
    let out = run_full(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[("llarame", unread())],
        &[("llarame", Http429)],
        Mode::Watch,
        None,
        stub_identity_lost,
    );
    let log = f.log();
    assert_ne!(out, CycleOutcome::Swapped, "{log}");
    assert!(f.live_is("work"), "~/.claude is untouched:\n{log}");
    assert!(!log.contains(" swap work->llarame"), "{log}");
    let l = f.e.rs.load_ledger().unwrap().unwrap();
    assert_eq!(l.swapped_at, None, "no swap time was recorded");
    assert_eq!(l.live.as_deref(), Some("work"));
}

// --- A2: one Extra, one decision --------------------------------------------------------------------------------------

/// With nothing to substitute, the decision with an empty `Extra` is the one
/// `decide_measured` makes: the signature everything else calls is unchanged.
fn stub_both_ways(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
    use crate::commands::profile::rotate::decide::target_seen::Extra;
    use crate::commands::profile::rotate::observe::{decide_measured, decide_with};
    let at = (ctx.now)();
    let probes = crate::commands::profile::rotate::tests_revalidate_cause::probes_for(
        snap,
        &[("top", rd(0.0, 40.0, None)), ("other", unread())],
    );
    let live = exh(100.0, 40.0);
    let a = decide_measured(ctx, snap, live.clone(), probes.clone(), at)?;
    let b = decide_with(ctx, snap, live, probes, at, &Extra::default())?;
    assert_eq!(a.decision, b.decision);
    assert_eq!(a.note, b.note);
    assert_eq!(a.unknown_plan, b.unknown_plan);
    assert_eq!(a.failed, b.failed);
    Ok(a)
}

#[test]
fn a2_an_empty_extra_changes_nothing() {
    let f = fleet(&["top", "other"], &[]);
    let out = run_full(
        &f,
        reporter(),
        exh(100.0, 40.0),
        &[],
        &[],
        Mode::Watch,
        None,
        stub_both_ways,
    );
    assert_eq!(out, CycleOutcome::Swapped, "{}", f.log());
    assert!(f.live_is("top"));
}
