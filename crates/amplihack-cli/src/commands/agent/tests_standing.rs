//! Issue #2912: the standing-session decision and its room gate.
//!
//! The relaunch argv and the log lines are in `tests_standing_lines.rs`,
//! and the loop itself, with a stand-in session in its pane, is in
//! `tests/issue_2912_standing_relaunch.rs`.

use super::relaunch::{Room, wait_for_room_until};
use super::{AfterExit, after_exit};
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
fn a_standing_session_that_exits_badly_after_starting_is_relaunched() {
    // standing, ok, signalled, ended_marker, saw_claude
    assert_eq!(
        after_exit(true, false, false, false, true),
        AfterExit::Relaunch
    );
}

#[test]
fn a_clean_return_is_the_owner_ending_the_session() {
    assert_eq!(
        after_exit(true, true, false, false, true),
        AfterExit::OwnerEnded
    );
}

#[test]
fn the_ended_marker_is_the_owner_ending_the_session() {
    assert_eq!(
        after_exit(true, false, false, true, true),
        AfterExit::OwnerEnded
    );
}

#[test]
fn a_signal_to_the_wrapper_means_the_pane_is_going_away() {
    for ok in [true, false] {
        for marker in [true, false] {
            assert_eq!(
                after_exit(true, ok, true, marker, true),
                AfterExit::GoingAway,
                "ok={ok} marker={marker}"
            );
        }
    }
}

#[test]
fn a_session_whose_claude_never_started_is_not_relaunched() {
    assert_eq!(
        after_exit(true, false, false, false, false),
        AfterExit::NeverStarted
    );
}

#[test]
fn a_session_that_is_not_standing_is_never_relaunched() {
    for ok in [true, false] {
        for signalled in [true, false] {
            for marker in [true, false] {
                for saw in [true, false] {
                    assert_ne!(
                        after_exit(false, ok, signalled, marker, saw),
                        AfterExit::Relaunch,
                        "ok={ok} signalled={signalled} marker={marker} saw={saw}"
                    );
                }
            }
        }
    }
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
