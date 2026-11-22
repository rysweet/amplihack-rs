//! Issue #2912, crusty's stale-record case on PR #3009: what the rotator's
//! files say about a run that the brake would refuse, read by
//! [`answer`]. The wait around it, with a stand-in rotator and session, is
//! in `tests/issue_2912_standing_relaunch.rs`.
//!
//! The run began at `SINCE` and exited at `EXITED`. A swap record older than
//! the run says nothing about whether the run spent its login; only a swap
//! since the run began, or a rotator cycle that measured after the exit,
//! does.

use super::recheck::{Answer, answer};
use amplihack_utils::rotator_status::CycleStatus;
use amplihack_utils::rotator_status::swap_record::{SwapReading, SwapRecord};

const SINCE: i64 = 1_793_316_000;
const EXITED: i64 = SINCE + 40;

fn swap(swapped_at: i64, session: u8) -> SwapRecord {
    SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq: 3,
        live_login: "spare".into(),
        from_login: "main".into(),
        swapped_at,
        reading: SwapReading {
            session,
            week: 10,
            measured_at: swapped_at - 5,
        },
    }
}

fn cycle(outcome: &str, measured_at: Option<i64>, alert: bool) -> CycleStatus {
    CycleStatus {
        at: Some(measured_at.unwrap_or(EXITED + 60)),
        measured_at,
        outcome: Some(outcome.to_string()),
        alert,
        live: Some("spare".into()),
        overflow: None,
        platform_live: false,
        live_room: Some(true),
    }
}

/// The race crusty named: a record with room, written by a swap made hours
/// before the run, and no cycle since the exit. That is no evidence either
/// way, so the wrapper waits for the rotator rather than refusing on it.
#[test]
fn a_record_older_than_the_run_is_not_an_answer() {
    let old = swap(SINCE - 4 * 3600, 10);
    assert_eq!(answer(Some(&old), None, SINCE, EXITED), Answer::Pending);
    let before = cycle("stayed", Some(SINCE - 60), false);
    assert_eq!(
        answer(Some(&old), Some(&before), SINCE, EXITED),
        Answer::Pending
    );
}

/// A swap since the run began onto a login with room replaces the capacity
/// the run had, whatever the ledger says.
#[test]
fn a_swap_since_the_run_began_onto_room_is_new_capacity() {
    for at in [SINCE, EXITED, EXITED + 30] {
        assert_eq!(
            answer(Some(&swap(at, 10)), None, SINCE, EXITED),
            Answer::Replaced,
            "{at}"
        );
    }
    // Onto a login at 100% it is not.
    assert_eq!(
        answer(Some(&swap(EXITED + 30, 100)), None, SINCE, EXITED),
        Answer::Pending
    );
}

/// A cycle that measured after the exit and stayed with room rules out a
/// limit; one that swapped, or a `profile switch` made after the exit, is
/// new capacity even before the swap record is written.
#[test]
fn a_cycle_measured_after_the_exit_answers() {
    let old = swap(SINCE - 3600, 10);
    let at = Some(EXITED + 1);
    let cases = [
        (cycle("stayed", at, false), Answer::Room),
        (cycle("swapped", at, false), Answer::Replaced),
        (cycle("switched", at, false), Answer::Replaced),
        (cycle("alerted", at, true), Answer::Spent),
        (cycle("stayed", at, true), Answer::Spent),
        (cycle("stayed-unread", at, false), Answer::Unanswered),
    ];
    for (c, want) in cases {
        assert_eq!(answer(Some(&old), Some(&c), SINCE, EXITED), want, "{c:?}");
    }
}

/// A measurement taken in the exit's own second may predate the exit, and a
/// reading from before a limit must not refuse the session, so it waits for
/// the next cycle. A passive reading recorded at its own earlier time is the
/// same case.
#[test]
fn a_measurement_from_the_exit_second_or_before_is_not_an_answer() {
    for m in [EXITED, EXITED - 1, SINCE] {
        for outcome in ["stayed", "alerted", "swapped"] {
            let c = cycle(outcome, Some(m), outcome == "alerted");
            assert_eq!(
                answer(None, Some(&c), SINCE, EXITED),
                Answer::Pending,
                "{outcome} at {m}"
            );
        }
    }
}

/// A login pinned with `profile switch` holds every cycle without a
/// measurement, so no answer will come and the wrapper does not wait for one.
#[test]
fn a_pinned_rotator_gives_no_answer() {
    assert_eq!(
        answer(None, Some(&cycle("held", None, false)), SINCE, EXITED),
        Answer::Unanswered
    );
    let switched_before = cycle("switched", Some(SINCE - 60), false);
    assert_eq!(
        answer(None, Some(&switched_before), SINCE, EXITED),
        Answer::Unanswered
    );
}

/// Cycles that say nothing about room yet leave the wait for the next one.
#[test]
fn a_cycle_that_measured_nothing_usable_leaves_the_wait_open() {
    for outcome in ["skipped", "stopped", "a-later-outcome"] {
        let c = cycle(outcome, Some(EXITED + 1), false);
        assert_eq!(
            answer(None, Some(&c), SINCE, EXITED),
            Answer::Pending,
            "{outcome}"
        );
    }
}
