//! Issue #2912: the standing-session decision and its room gate.
//!
//! The relaunch argv and the lines and record text are in
//! `tests_standing_lines.rs`, what counts as a model reply is in
//! `tests_standing_progress.rs`, and the loop itself, with a stand-in
//! session in its pane, is in `tests/issue_2912_standing_relaunch.rs`.

use super::relaunch::{Lost, Room, wait_for_room_until};
use super::{AfterExit, Quiet, after_exit};
use crate::command_error::exit_error;
use amplihack_utils::rotator_status::swap_record::{
    SWAP_FILE, SwapReading, SwapRecord, rotate_dir,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const NOW: i64 = 1_793_316_490;

// ── after_exit ──────────────────────────────────────────────────────────────

/// What a launch can return: a clean exit, Claude Code's own non-zero exit
/// status (a signal comes back as `128 + N`), and an error that carries no
/// exit status, which is the launch path failing.
fn results() -> [(&'static str, anyhow::Result<()>); 4] {
    [
        ("ok", Ok(())),
        ("status 1", Err(exit_error(1))),
        ("SIGKILL", Err(exit_error(137))),
        ("launch error", Err(anyhow::anyhow!("claude not found"))),
    ]
}

/// Every process exit nobody chose is relaunched: a non-zero status and a
/// signal alike, however often it happens. There is no count to consult.
#[test]
fn a_standing_session_whose_process_exits_badly_is_relaunched() {
    // standing, signalled, ended_marker, result
    for code in [1, 2, 137, 143] {
        assert_eq!(
            after_exit(true, false, false, &Err(exit_error(code))),
            AfterExit::Relaunch,
            "{code}"
        );
    }
}

#[test]
fn a_clean_return_is_the_owner_ending_the_session() {
    assert_eq!(
        after_exit(true, false, false, &Ok(())),
        AfterExit::OwnerEnded
    );
}

#[test]
fn the_ended_marker_is_the_owner_ending_the_session() {
    for (what, result) in results() {
        assert_eq!(
            after_exit(true, false, true, &result),
            AfterExit::OwnerEnded,
            "{what}"
        );
    }
}

/// An error with no exit status is the launch path failing, not a process
/// that exited: there is nothing to relaunch, and a relaunch would fail the
/// same way at once with no Claude Code start to pace the loop.
#[test]
fn a_launch_that_failed_without_an_exit_status_is_not_relaunched() {
    assert_eq!(
        after_exit(
            true,
            false,
            false,
            &Err(anyhow::anyhow!("claude not found"))
        ),
        AfterExit::LaunchFailed
    );
}

#[test]
fn a_signal_to_the_wrapper_means_the_pane_is_going_away() {
    for (what, result) in results() {
        for marker in [true, false] {
            assert_eq!(
                after_exit(true, true, marker, &result),
                AfterExit::GoingAway,
                "{what} marker={marker}"
            );
        }
    }
}

#[test]
fn a_session_that_is_not_standing_is_never_relaunched() {
    for (what, result) in results() {
        for signalled in [true, false] {
            for marker in [true, false] {
                assert_eq!(
                    after_exit(false, signalled, marker, &result),
                    AfterExit::GoingAway,
                    "{what} signalled={signalled} marker={marker}"
                );
            }
        }
    }
}

// ── the chain on the record ─────────────────────────────────────────────────

/// A reply ends the chain, whatever came before it.
#[test]
fn a_reply_during_the_run_clears_the_chain() {
    assert_eq!(Quiet::after(None, true, NOW), None);
    let before = Quiet {
        since: NOW - 600,
        relaunches: 40,
    };
    assert_eq!(Quiet::after(Some(before), true, NOW), None);
}

/// The first relaunch with no reply behind it starts a chain at its own
/// time; each later one adds to it and keeps the time it started.
#[test]
fn relaunches_without_a_reply_add_to_the_chain_and_keep_its_start() {
    let first = Quiet::after(None, false, NOW).unwrap();
    assert_eq!(
        first,
        Quiet {
            since: NOW,
            relaunches: 1
        }
    );
    let second = Quiet::after(Some(first), false, NOW + 3).unwrap();
    assert_eq!(
        second,
        Quiet {
            since: NOW,
            relaunches: 2
        }
    );
}

/// The chain is evidence and is never a limit, so it cannot overflow into
/// a panic however long a session runs.
#[test]
fn the_chain_saturates_rather_than_overflowing() {
    let long = Quiet {
        since: NOW,
        relaunches: u32::MAX,
    };
    assert_eq!(
        Quiet::after(Some(long), false, NOW + 1).map(|q| q.relaunches),
        Some(u32::MAX)
    );
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

/// A rotator directory others can write to is not watched, so a session
/// with no room is not held on it; the wait ends at once and says why.
#[test]
fn a_rotate_directory_others_can_write_is_not_waited_on() {
    let r = root();
    write_record(r.path(), &record(100, 100), 0o600);
    fs::set_permissions(rotate_dir(r.path()), fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(wait_for_room(r.path()), Room::Gone(Lost::Unwatchable));
}
