//! Issue #3105 through whole cycles (the 23 November replay), at 85/90 and
//! at the hosts' 95/98. Stand-in logins (`held`, `other`) in temporary homes
//! through the `tests_issue_2910_fixture` fleet; no real login, rotator or
//! tmux session is touched.
//!
//! Cases:
//! - (a) a blocked login with a roomy reading, fresh, stale or kept, is never
//!   chosen, and the cycle logs one `reject: <login> blocked until <minute>
//!   (<kind>)` line for it, however many times the decision runs; a cycle
//!   that looks for no target writes none;
//! - (b) the 23 November replay: an old reading (0/33) held in the history
//!   while the watch keeps a newer one (100/44) does not lead to a swap,
//!   with the block in the ledger, with the block passed through a save for
//!   an older reader (which now keeps it), and with no block at all;
//! - (c) a confirming read older than one already held is rejected and the
//!   next login waits for another cycle; a passive reading older than the
//!   kept one is not ranked;
//! - (d) no swap is made without a fresh reading and no swap line says
//!   `unconfirmed`;
//! - (e) a block whose end has come no longer applies.

use super::tests_issue_2910_fixture::{
    BOTH, Fleet, Labelled, clock, fleet, lines_with, run, run_full, run_tuned, set_clock, stub,
    unread,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::cycle::{Ctx, CycleOutcome, Mode};
use crate::commands::profile::rotate::decide::Probed;
use crate::commands::profile::rotate::ledger::for_older_readers;
use crate::commands::profile::rotate::measure::ProbeLabel;
use crate::commands::profile::rotate::tests_issue_1870::{exh, rd};
use std::cell::Cell;
use std::time::Duration;

const KINDS: [&str; 4] = ["spend_limit", "session_limit", "week_limit", "credit_limit"];

thread_local! {
    static CONFIRMATIONS: Cell<usize> = const { Cell::new(0) };
}

#[test]
fn a_blocked_login_with_a_roomy_reading_is_not_chosen_and_is_logged_as_rejected() {
    for (label, s) in BOTH {
        for kind in KINDS {
            let f = fleet(&["held", "other"], &[]);
            f.block("held", kind, clock() + 3600);
            let table = [("held", rd(0.0, 33.0, None)), ("other", exh(100.0, 50.0))];
            let out = run(&f, s(), exh(100.0, 44.0), &table, &[]);
            let log = f.log();
            assert_ne!(out, CycleOutcome::Swapped, "{label} {kind}: {log}");
            assert!(f.live_is("work"), "{label} {kind}: {log}");
            let rejects = lines_with(&log, "reject: held blocked until ");
            assert_eq!(rejects.len(), 1, "{label} {kind}: one line:\n{log}");
        }
    }
}

#[test]
fn a_blocked_login_is_passed_over_for_the_next_candidate() {
    for (label, s) in BOTH {
        let f = fleet(&["held", "other"], &[]);
        f.block("held", "spend_limit", clock() + 3600);
        let table = [
            ("held", rd(0.0, 33.0, None)),
            ("other", rd(10.0, 50.0, None)),
        ];
        let out = run(&f, s(), exh(100.0, 44.0), &table, &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("other"), "{label}: {log}");
    }
}

// --- (b) The 23 November replay ----------------------------------------------------------
//
// 06:16:18Z: a cycle read `held` at s=100% w=44%, and the watch kept it.
// 06:18:21Z: `held`'s probe failed; the history's newest point for it was
// 0/33 from 05:44:55Z, 2006 seconds earlier. The live login was full and
// nothing else had room. Before #3105 that point stood in, ranked `held`
// first, and D7 took it unconfirmed eight seconds later.

/// How the block stands in the replayed cycle.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Block {
    /// In the ledger, as the step recorded it at 06:00:21Z.
    Held,
    /// Recorded, then the ledger saved for an older reader, as every save
    /// beside the replaced binary did (P1).
    ThroughOlderSave,
    /// Never there (a ledger an older build saved without it).
    Missing,
}

/// The watch keeps `p` as its reading of `held`, measured at `at`.
fn keep_held(ctx: &mut Ctx, p: Probed, at: i64) {
    let held = ProfileName::parse("held").unwrap();
    ctx.stored
        .keep(None, Some(&(unread(), vec![(held, p)])), at);
}

/// The kept 06:16:18Z reading: what the watch's `Stored::keep` held.
fn keep_1616(ctx: &mut Ctx) {
    let at = clock() - 123;
    keep_held(ctx, exh(100.0, 44.0), at);
    let held = ProfileName::parse("held").unwrap();
    assert!(
        ctx.stored.times(clock()).contains(&(held, at)),
        "the 06:16:18Z reading is held"
    );
}

fn replay_cycle(f: &Fleet, s: Labelled, keep: bool) -> CycleOutcome {
    let table = [("held", unread()), ("other", exh(100.0, 50.0))];
    let labels = [("held", ProbeLabel::Transport)];
    run_tuned(
        f,
        (s.1)(),
        exh(100.0, 44.0),
        &table,
        &labels,
        Mode::Watch,
        &|ctx| {
            ctx.measure = Some(stub);
            if keep {
                keep_1616(ctx);
            }
        },
    )
}

fn replay(block: Block, keep: bool) {
    for s in BOTH {
        let label = s.0;
        let f = fleet(&["held", "other"], &[]);
        f.history("held", 2006, 0, 33);
        if block != Block::Missing {
            f.block("held", "spend_limit", clock() + 15_822);
        }
        if block == Block::ThroughOlderSave {
            let l = f.e.rs.load_ledger().unwrap().unwrap();
            f.e.rs.save_ledger(&for_older_readers(&l, true)).unwrap();
            let back = f.e.rs.load_ledger().unwrap().unwrap();
            assert_eq!(
                back.overflow_blocked.len(),
                1,
                "{label}: R2 keeps the block"
            );
        }
        let out = replay_cycle(&f, s, keep);
        let log = f.log();
        let what = format!("{label} {block:?} keep={keep}");
        assert_ne!(out, CycleOutcome::Swapped, "{what}: {log}");
        assert!(f.live_is("work"), "{what}: {log}");
        assert!(lines_with(&log, " swap ").is_empty(), "{what}: {log}");
        assert!(!log.contains("unconfirmed ("), "{what}: {log}");
        let rejects = lines_with(&log, "reject: held blocked until ");
        let want = usize::from(block != Block::Missing);
        assert_eq!(rejects.len(), want, "{what}:\n{log}");
        if let Some(line) = rejects.first() {
            assert!(line.ends_with("(spend limit)"), "{what}: {line}");
        }
        if keep {
            // The old point is older than the kept reading: it never stands
            // in, so `held` is not even ranked on it.
            assert!(!log.contains("last s=0%,w=33%"), "{what}: {log}");
            assert!(
                !log.contains("reject: held reading unknown"),
                "{what}: {log}"
            );
        } else if block == Block::Missing {
            // Nothing newer is held: the old point ranks `held`, and the
            // confirmation, which fails closed, rejects it.
            assert!(
                log.contains("reject: held reading unknown (transport)"),
                "{what}: {log}"
            );
            assert!(
                log.contains("stay: no candidate confirmed by a fresh reading"),
                "{what}: {log}"
            );
        }
    }
}

#[test]
fn the_replay_does_not_swap_with_the_block_in_the_ledger() {
    replay(Block::Held, true);
    replay(Block::Held, false);
}

#[test]
fn the_replay_does_not_swap_when_the_block_went_through_an_older_reader_save() {
    replay(Block::ThroughOlderSave, true);
    replay(Block::ThroughOlderSave, false);
}

#[test]
fn the_replay_does_not_swap_on_an_old_reading_when_the_block_is_missing() {
    replay(Block::Missing, true);
    replay(Block::Missing, false);
}

#[test]
fn a_target_with_no_room_for_a_confirming_read_is_not_taken_unconfirmed() {
    use crate::commands::profile::ProfileError;
    use crate::commands::profile::rotate::decide::Decision;
    use crate::commands::profile::rotate::observe::{Measurement, Snap};

    fn measure(ctx: &Ctx, snap: &Snap) -> Result<Measurement, ProfileError> {
        let measurement = stub(ctx, snap)?;
        assert!(
            matches!(&measurement.decision, Decision::Swap { to, .. } if to.as_str() == "held")
        );
        Ok(measurement)
    }

    fn unexpected_reread(_: &Ctx, _: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        panic!("a candidate whose probe already failed cannot be confirmed this cycle");
    }

    // The live login is full and the only candidate's probe failed with a
    // label, so its old figure ranks it. The failed probe rules out the
    // candidate at the confirmation step.
    for (label, s) in BOTH {
        let f = fleet(&["held"], &[]);
        f.history("held", 2000, 0, 33);
        let out = run_full(
            &f,
            s(),
            exh(100.0, 44.0),
            &[("held", unread())],
            &[("held", ProbeLabel::Http429)],
            Mode::Watch,
            Some(unexpected_reread),
            measure,
        );
        assert_ne!(out, CycleOutcome::Swapped, "{label}");
        assert!(f.live_is("work"), "{label}");
    }
}

#[test]
fn a_block_whose_end_has_come_no_longer_applies() {
    for (label, s) in BOTH {
        let f = fleet(&["held"], &[]);
        let until = clock() + 3600;
        f.block("held", "spend_limit", until);
        set_clock(until);
        let out = run(
            &f,
            s(),
            exh(100.0, 44.0),
            &[("held", rd(0.0, 33.0, None))],
            &[],
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("held"), "{label}: {log}");
        assert!(!log.contains("reject: held blocked"), "{label}: {log}");
    }
}

// --- (a) The block line, once per cycle --------------------------------------------------

/// `top` ranks first on a stale reading, is read again and found full; the
/// decision runs again but leaves `next` for another cycle. Two blocked
/// logins with roomy readings are left out of every pass, and each has
/// exactly one line.
#[test]
fn each_blocked_login_has_one_line_however_many_times_the_decision_runs() {
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        assert_eq!(who.as_str(), "top", "a blocked login is never read again");
        CONFIRMATIONS.with(|count| count.set(count.get() + 1));
        (exh(100.0, 40.0), None)
    }
    for (label, s) in BOTH {
        let f = fleet(&["aheld", "zheld", "top", "next"], &[]);
        f.block("aheld", "spend_limit", clock() + 3600);
        f.block("zheld", "week_limit", clock() + 7200);
        let table = [
            ("aheld", rd(0.0, 5.0, None)),
            ("zheld", rd(0.0, 5.0, Some(clock() - 600))),
            ("top", rd(0.0, 10.0, Some(clock() - 60))),
            ("next", rd(20.0, 60.0, None)),
        ];
        CONFIRMATIONS.set(0);
        let out = run_full(
            &f,
            s(),
            exh(100.0, 44.0),
            &table,
            &[],
            Mode::Watch,
            Some(reread),
            stub,
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::StayedUnread, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert_eq!(CONFIRMATIONS.get(), 1);
        assert!(
            log.contains("reject: top no room on a fresh reading"),
            "{label}: {log}"
        );
        let a = lines_with(&log, "reject: aheld blocked until ");
        let z = lines_with(&log, "reject: zheld blocked until ");
        assert_eq!((a.len(), z.len()), (1, 1), "{label}:\n{log}");
        assert!(a[0].ends_with("(spend limit)"), "{label}: {}", a[0]);
        assert!(z[0].ends_with("(week limit)"), "{label}: {}", z[0]);
    }
}

/// A kept reading, as an event cycle takes it (`carried`), is never a
/// reason to choose a blocked login either.
#[test]
fn a_blocked_login_with_a_roomy_kept_reading_is_not_chosen() {
    for (label, s) in BOTH {
        let f = fleet(&["held", "other"], &[]);
        f.block("held", "session_limit", clock() + 3600);
        let kept = Probed {
            carried: true,
            observed_at: Some(clock() - 30),
            ..rd(0.0, 33.0, None)
        };
        let table = [("held", kept), ("other", rd(10.0, 50.0, None))];
        let out = run(&f, s(), exh(100.0, 44.0), &table, &[]);
        let log = f.log();
        assert_eq!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("other"), "{label}: {log}");
        let rejects = lines_with(&log, "reject: held blocked until ");
        assert_eq!(rejects.len(), 1, "{label}:\n{log}");
        assert!(
            rejects[0].ends_with("(session limit)"),
            "{label}: {}",
            rejects[0]
        );
    }
}

/// A cycle whose live login is below both thresholds and has no better
/// login to move to looks for no target: it writes no block line.
#[test]
fn a_cycle_that_looks_for_no_target_writes_no_block_line() {
    for (label, s) in BOTH {
        let f = fleet(&["held"], &[]);
        f.block("held", "spend_limit", clock() + 3600);
        let out = run(
            &f,
            s(),
            rd(10.0, 10.0, None),
            &[("held", rd(0.0, 5.0, None))],
            &[],
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Stayed, "{label}: {log}");
        assert!(lines_with(&log, "reject: ").is_empty(), "{label}: {log}");
    }
}

// --- (c) A reading older than one already held -------------------------------------------

/// `held` ranks first on a passive reading taken at the time the watch kept
/// it; its confirming read comes back dated 140 seconds before that. It is
/// rejected, and the next login waits for confirmation in another cycle.
#[test]
fn a_confirming_read_older_than_the_kept_reading_leaves_the_next_login_for_another_cycle() {
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        assert_eq!(who.as_str(), "held");
        CONFIRMATIONS.with(|count| count.set(count.get() + 1));
        (rd(0.0, 30.0, Some(clock() - 200)), None)
    }
    for (label, s) in BOTH {
        let f = fleet(&["held", "other"], &[]);
        let table = [
            ("held", rd(0.0, 30.0, Some(clock() - 60))),
            ("other", rd(10.0, 50.0, None)),
        ];
        CONFIRMATIONS.set(0);
        let out = run_tuned(
            &f,
            s(),
            exh(100.0, 44.0),
            &table,
            &[],
            Mode::Watch,
            &|ctx| {
                ctx.reread = Some(reread);
                ctx.measure = Some(stub);
                keep_held(ctx, rd(0.0, 30.0, Some(clock() - 60)), clock() - 60);
            },
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::StayedUnread, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert_eq!(CONFIRMATIONS.get(), 1);
    }
}

/// With nothing else to try, a superseded confirming read leaves the stay
/// note, as an unknown one does, whatever room the live login has.
#[test]
fn a_superseded_confirming_read_with_nothing_else_leaves_the_stay_note() {
    fn reread(_: &Ctx, _: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        (rd(0.0, 30.0, Some(clock() - 200)), None)
    }
    for (label, s) in BOTH {
        let f = fleet(&["held"], &[]);
        let table = [("held", rd(0.0, 30.0, Some(clock() - 60)))];
        let out = run_tuned(
            &f,
            s(),
            exh(100.0, 44.0),
            &table,
            &[],
            Mode::Watch,
            &|ctx| {
                ctx.reread = Some(reread);
                ctx.measure = Some(stub);
                keep_held(ctx, rd(0.0, 30.0, Some(clock() - 60)), clock() - 60);
            },
        );
        let log = f.log();
        assert_eq!(out, CycleOutcome::Alerted, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert!(
            log.contains("stay: no candidate confirmed by a fresh reading"),
            "{label}: {log}"
        );
    }
}

/// A passive reading older than the one the watch kept is superseded: its
/// roomy figures are not ranked, and the cycle reads nothing again for it.
#[test]
fn a_passive_reading_older_than_the_kept_one_is_not_ranked() {
    fn reread(_: &Ctx, who: &ProfileName, _: Duration) -> (Probed, Option<ProbeLabel>) {
        panic!("{who} must not be read again: it was never a target");
    }
    for (label, s) in BOTH {
        let f = fleet(&["held", "other"], &[]);
        let table = [
            ("held", rd(0.0, 33.0, Some(clock() - 200))),
            ("other", exh(100.0, 50.0)),
        ];
        let out = run_tuned(
            &f,
            s(),
            exh(100.0, 44.0),
            &table,
            &[],
            Mode::Watch,
            &|ctx| {
                ctx.reread = Some(reread);
                ctx.measure = Some(stub);
                keep_1616(ctx);
            },
        );
        let log = f.log();
        assert_ne!(out, CycleOutcome::Swapped, "{label}: {log}");
        assert!(f.live_is("work"), "{label}: {log}");
        assert!(lines_with(&log, " swap ").is_empty(), "{label}: {log}");
    }
}
