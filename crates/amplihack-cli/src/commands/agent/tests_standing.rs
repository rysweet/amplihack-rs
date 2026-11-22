//! Issue #2912: the standing-session decision and its room gate.
//!
//! The relaunch argv and the log lines are in `tests_standing_lines.rs`,
//! and the loop itself, with a stand-in session in its pane, is in
//! `tests/issue_2912_standing_relaunch.rs`.

use super::relaunch::{Room, wait_for_room_until};
use super::{AfterExit, Progress, after_exit, no_progress_alert, worth_relaunching};
use amplihack_utils::rotator_status::swap_record::{
    SWAP_FILE, SwapReading, SwapRecord, rotate_dir,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const NOW: i64 = 1_793_316_490;

// ── after_exit ──────────────────────────────────────────────────────────────

#[test]
fn a_standing_session_that_exits_badly_puts_a_relaunch_in_question() {
    // standing, ok, signalled, ended_marker
    assert_eq!(after_exit(true, false, false, false), AfterExit::Relaunch);
}

#[test]
fn a_clean_return_is_the_owner_ending_the_session() {
    assert_eq!(after_exit(true, true, false, false), AfterExit::OwnerEnded);
}

#[test]
fn the_ended_marker_is_the_owner_ending_the_session() {
    assert_eq!(after_exit(true, false, false, true), AfterExit::OwnerEnded);
}

#[test]
fn a_signal_to_the_wrapper_means_the_pane_is_going_away() {
    for ok in [true, false] {
        for marker in [true, false] {
            assert_eq!(
                after_exit(true, ok, true, marker),
                AfterExit::GoingAway,
                "ok={ok} marker={marker}"
            );
        }
    }
}

#[test]
fn a_session_that_is_not_standing_is_never_relaunched() {
    for ok in [true, false] {
        for signalled in [true, false] {
            for marker in [true, false] {
                assert_ne!(
                    after_exit(false, ok, signalled, marker),
                    AfterExit::Relaunch,
                    "ok={ok} signalled={signalled} marker={marker}"
                );
            }
        }
    }
}

// ── what a run left in its conversation ─────────────────────────────────────

/// Growth is work. A conversation first recorded during the run counts from
/// nothing, which is what every first launch of a session does.
#[test]
fn a_conversation_that_grew_across_the_run_is_progress() {
    for (before, after) in [(Some(10), Some(11)), (Some(0), Some(1)), (None, Some(1))] {
        assert_eq!(
            Progress::of(before, after),
            Progress::Advanced,
            "{before:?} -> {after:?}"
        );
    }
}

/// A conversation that is the size it was, or that has no transcript yet,
/// is a run that did nothing a `--continue` could carry on. A transcript
/// that shrank is not growth either.
#[test]
fn a_conversation_that_did_not_grow_is_an_idle_run() {
    for (before, after) in [
        (Some(10), Some(10)),
        (Some(10), Some(4)),
        (Some(0), Some(0)),
        (None, Some(0)),
    ] {
        assert_eq!(
            Progress::of(before, after),
            Progress::Idle,
            "{before:?} -> {after:?}"
        );
    }
}

/// No measurement after the exit is not a measurement of nothing: the
/// record named no conversation of this session's, so the run is neither
/// known to have worked nor known to have been idle.
#[test]
fn no_conversation_to_measure_is_neither_progress_nor_an_idle_run() {
    for before in [None, Some(0), Some(10)] {
        assert_eq!(
            Progress::of(before, None),
            Progress::Unmeasured,
            "{before:?}"
        );
    }
}

// ── the brake on the relaunch loop ──────────────────────────────────────────

/// Every exit is given a relaunch. A session that sat idle and was then
/// killed by the host or the out-of-memory killer has nothing to show for
/// its run and a login with room, and is still started again: the issue
/// asks for a relaunch whenever the process exits other than by its owner,
/// and "this run did no work" is not "this start is failing". The same
/// holds when there was no conversation of its own to measure at all.
#[test]
fn an_exit_that_is_not_on_trial_is_always_relaunched() {
    for progress in [Progress::Advanced, Progress::Idle, Progress::Unmeasured] {
        for room_appeared in [true, false] {
            assert!(
                worth_relaunching(false, progress, room_appeared),
                "{progress:?} room={room_appeared}"
            );
        }
    }
}

/// A run that added to the conversation is started again however often it
/// happens, trial or not: the evidence is work done, not a number of
/// attempts, so a working session is never capped.
#[test]
fn a_run_that_advanced_the_conversation_is_relaunched() {
    for room_appeared in [true, false] {
        assert!(
            worth_relaunching(true, Progress::Advanced, room_appeared),
            "{room_appeared}"
        );
    }
}

/// The usage-limit case the feature exists for: a process that died with
/// nothing to show is still started again once the wait for room has found
/// capacity, because the capacity is what has changed.
#[test]
fn a_run_that_waited_for_room_is_relaunched_with_nothing_else_to_show() {
    for progress in [Progress::Idle, Progress::Unmeasured] {
        assert!(worth_relaunching(true, progress, true), "{progress:?}");
    }
}

/// The brake, and the only case it stops: the run was itself the one trial
/// an exit with nothing to show is given, and it has come back with nothing
/// to show as well while a login had room. The chain is getting nowhere, so
/// another start would repeat the one that just failed.
#[test]
fn a_trial_run_with_neither_progress_nor_new_room_is_not_relaunched() {
    for progress in [Progress::Idle, Progress::Unmeasured] {
        assert!(!worth_relaunching(true, progress, false), "{progress:?}");
    }
}

/// The alert names the exit, the chain it stopped, and what was measured of
/// the conversation; it carries no control character and does not grow
/// without bound on a hostile exit reason.
#[test]
fn the_no_progress_alert_names_the_exit_and_the_evidence() {
    assert_eq!(
        no_progress_alert("exit-status-1", Progress::Idle),
        "not relaunched: the relaunch of an exit that showed nothing itself \
         exited exit-status-1 without advancing its conversation while a login \
         had room, so another relaunch would repeat it"
    );
    // The blind case says so rather than claiming the conversation stood
    // still: there was none of this session's to look at.
    assert_eq!(
        no_progress_alert("SIGKILL", Progress::Unmeasured),
        "not relaunched: the relaunch of an exit that showed nothing itself \
         exited SIGKILL with no conversation of its own on record while a login \
         had room, so another relaunch would repeat it"
    );
    let hostile = no_progress_alert(
        &format!("a\u{1b}[31mb\nc{}", "x".repeat(4096)),
        Progress::Idle,
    );
    assert!(!hostile.chars().any(char::is_control), "{hostile:?}");
    assert!(hostile.chars().count() <= super::MAX_LINE, "{hostile:?}");
}

// ── the room rule ───────────────────────────────────────────────────────────

fn record(session: u8, week: u8) -> SwapRecord {
    SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq: 1,
        live_login: "spare".into(),
        from_login: "main".into(),
        swapped_at: NOW - 10,
        reading: SwapReading {
            session,
            week,
            measured_at: NOW - 20,
        },
    }
}

#[test]
fn a_login_has_room_when_both_windows_are_under_a_hundred_percent() {
    for (session, week, room) in [
        (0, 0, true),
        (99, 99, true),
        (100, 0, false),
        (0, 100, false),
        (100, 100, false),
    ] {
        assert_eq!(record(session, week).has_room(), room, "{session}/{week}");
    }
}

fn write_record(root: &Path, rec: &SwapRecord, mode: u32) {
    let dir = rotate_dir(root);
    fs::create_dir_all(&dir).unwrap();
    for d in [root.join(".auto-switch.d"), dir.clone()] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let f = dir.join(SWAP_FILE);
    fs::write(&f, serde_json::to_vec(rec).unwrap()).unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(mode)).unwrap();
}

/// The wait with nothing able to interrupt it and no waiting line.
fn wait_for_room(root: &Path) -> Room {
    wait_for_room_until(root, &AtomicBool::new(false), || {})
}

fn root() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    t
}

#[test]
fn no_swap_record_means_no_rotator_and_no_wait() {
    let r = root();
    assert_eq!(wait_for_room(r.path()), Room::Now);
}

#[test]
fn a_record_with_room_is_not_waited_for() {
    let r = root();
    write_record(r.path(), &record(10, 20), 0o600);
    assert_eq!(wait_for_room(r.path()), Room::Now);
}

/// A record the reader refuses is the same as no record, so an attacker who
/// can only damage the file gets an immediate relaunch, never a hold.
#[test]
fn a_refused_or_unparseable_record_is_not_waited_for() {
    let r = root();
    write_record(r.path(), &record(100, 100), 0o666);
    assert_eq!(wait_for_room(r.path()), Room::Now, "group/world writable");
    let f = rotate_dir(r.path()).join(SWAP_FILE);
    fs::write(&f, b"not json").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(wait_for_room(r.path()), Room::Now, "unparseable");
}
