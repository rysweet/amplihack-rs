//! Issue #2085 (T3): `amplihack_utils::file_event`, one blocking `poll(2)`
//! on inotify watches and a pidfd. The limit-resume hook uses it in place of
//! reading the ledger on an interval.
//!
//! Contract (design C3):
//! - `Watch { dir: PathBuf, name: Option<&'static str>, mask: u32 }`;
//!   `SWAP_MASK == IN_MOVED_TO | IN_CLOSE_WRITE`, `PROMPT_MASK == IN_MOVED_TO`.
//! - `Waiter::new(&[Watch], pid: u32) -> io::Result<Waiter>`: every watch is
//!   added with `IN_ONLYDIR | IN_DONT_FOLLOW | IN_DELETE_SELF | IN_MOVE_SELF`
//!   and the pid is opened with `pidfd_open`; any failure is an error.
//! - `Waiter::wait(&mut self, deadline: Instant) -> io::Result<Woke>`:
//!   `Woke::{File(i), Overflow, Lost(i), ProcessGone, Deadline}`. Events
//!   whose name does not match are drained without returning.
//! - `Waiter::process_alive(&self) -> bool`: a non-blocking pidfd check.
//! - `timeout_ms(now, deadline) -> i32`: saturating, `0..=i32::MAX`.
//! - `parse_events(buf) -> Option<Vec<RawEvent>>`: bounds-checked; `None`
//!   for a truncated record.
//!
//! Timing rule: no sleeps and no wall-clock windows. Every test makes its
//! event happen first and then calls `wait` with a deadline of now, so the
//! poll timeout is zero: an event is either already queued or the result
//! is `Deadline`.

#![cfg(target_os = "linux")]

use amplihack_utils::file_event::{
    PROMPT_MASK, RawEvent, SWAP_MASK, Waiter, Watch, Woke, parse_events, timeout_ms,
};
use amplihack_utils::secure_files::{write_private_atomic, write_private_atomic_nosync};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Dirs {
    _tmp: tempfile::TempDir,
    rotate: PathBuf,
    prompted: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let rotate = tmp.path().join("rotate");
    let prompted = tmp.path().join("prompted");
    for d in [&rotate, &prompted] {
        fs::create_dir(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    Dirs {
        _tmp: tmp,
        rotate,
        prompted,
    }
}

fn watches(d: &Dirs) -> Vec<Watch> {
    vec![
        Watch {
            dir: d.rotate.clone(),
            name: Some("swap.json"),
            mask: SWAP_MASK,
        },
        Watch {
            dir: d.prompted.clone(),
            name: None,
            mask: PROMPT_MASK,
        },
    ]
}

fn waiter(d: &Dirs) -> Waiter {
    Waiter::new(&watches(d), std::process::id()).expect("watches on private dirs")
}

/// A zero-timeout wait: whatever is already queued, else `Deadline`.
fn now_wait(w: &mut Waiter) -> Woke {
    w.wait(Instant::now()).expect("wait")
}

// ── Masks ───────────────────────────────────────────────────────────────────

#[test]
fn masks_are_the_documented_ones() {
    assert_eq!(SWAP_MASK, libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE);
    assert_eq!(PROMPT_MASK, libc::IN_MOVED_TO);
}

// ── Real inotify ────────────────────────────────────────────────────────────

#[test]
fn nothing_queued_is_deadline() {
    let d = dirs();
    let mut w = waiter(&d);
    assert_eq!(now_wait(&mut w), Woke::Deadline);
}

#[test]
fn an_atomic_write_of_the_record_wakes_file_zero() {
    let d = dirs();
    let mut w = waiter(&d);
    write_private_atomic(&d.rotate, "swap.json", b"{}").unwrap();
    assert_eq!(now_wait(&mut w), Woke::File(0));
}

#[test]
fn an_in_place_write_of_the_record_wakes_file_zero() {
    // IN_CLOSE_WRITE: a writer that does not rename still wakes the hook.
    let d = dirs();
    let mut w = waiter(&d);
    fs::write(d.rotate.join("swap.json"), b"{}").unwrap();
    assert_eq!(now_wait(&mut w), Woke::File(0));
}

#[test]
fn other_names_are_drained_without_waking() {
    let d = dirs();
    let mut w = waiter(&d);
    write_private_atomic(&d.rotate, "ledger.json", b"{}").unwrap();
    fs::write(d.rotate.join("rotate.log"), b"x").unwrap();
    // Names are compared exactly: neither a prefix nor a suffix matches.
    fs::write(d.rotate.join("swap.json.1"), b"x").unwrap();
    fs::write(d.rotate.join("xswap.json"), b"x").unwrap();
    assert_eq!(now_wait(&mut w), Woke::Deadline);
}

#[test]
fn a_non_matching_name_then_the_record_wakes_file_zero_once() {
    let d = dirs();
    let mut w = waiter(&d);
    write_private_atomic(&d.rotate, "ledger.json", b"{}").unwrap();
    write_private_atomic(&d.rotate, "swap.json", b"{}").unwrap();
    assert_eq!(now_wait(&mut w), Woke::File(0));
    assert_eq!(now_wait(&mut w), Woke::Deadline, "drained after the match");
}

#[test]
fn a_prompt_marker_moved_in_wakes_file_one() {
    let d = dirs();
    let mut w = waiter(&d);
    // What `record_prompt` does: a temp file renamed over the marker.
    write_private_atomic_nosync(&d.prompted, "0123abcd", b"").unwrap();
    assert_eq!(now_wait(&mut w), Woke::File(1));
}

#[test]
fn removing_the_watched_directory_is_lost() {
    let d = dirs();
    let mut w = waiter(&d);
    fs::remove_dir(&d.rotate).unwrap();
    assert_eq!(now_wait(&mut w), Woke::Lost(0));
}

#[test]
fn moving_the_watched_directory_away_is_lost() {
    let d = dirs();
    let mut w = waiter(&d);
    fs::rename(&d.prompted, d.prompted.with_file_name("aside")).unwrap();
    assert_eq!(now_wait(&mut w), Woke::Lost(1));
}

#[test]
fn an_overflowed_queue_is_overflow() {
    let limit: usize = fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(usize::MAX);
    if limit > 65_536 {
        eprintln!("skipped: max_queued_events={limit} is too large to fill here");
        return;
    }
    let d = dirs();
    let mut w = waiter(&d);
    // Distinct names: the kernel merges an event identical to the last one.
    for i in 0..=limit + 1 {
        fs::write(d.rotate.join(format!("noise-{i}")), b"").unwrap();
    }
    // The hook treats an overflow as a possible swap and re-reads the record.
    assert_eq!(now_wait(&mut w), Woke::Overflow);
}

#[test]
fn a_missing_directory_is_an_error() {
    let d = dirs();
    let mut ws = watches(&d);
    ws[0].dir = d.rotate.join("missing");
    assert!(Waiter::new(&ws, std::process::id()).is_err());
}

#[test]
fn a_symlinked_directory_is_an_error() {
    let d = dirs();
    let link = d.rotate.with_file_name("link");
    symlink(&d.rotate, &link).unwrap();
    let mut ws = watches(&d);
    ws[0].dir = link;
    assert!(Waiter::new(&ws, std::process::id()).is_err());
}

#[test]
fn a_regular_file_in_place_of_a_directory_is_an_error() {
    let d = dirs();
    let file = d.rotate.join("plain");
    fs::write(&file, b"").unwrap();
    let mut ws = watches(&d);
    ws[1].dir = file;
    assert!(Waiter::new(&ws, std::process::id()).is_err());
}

// ── pidfd ───────────────────────────────────────────────────────────────────

fn blocked_child() -> std::process::Child {
    Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cat")
}

#[test]
fn a_running_process_is_alive_and_does_not_wake() {
    let d = dirs();
    let mut child = blocked_child();
    let mut w = Waiter::new(&watches(&d), child.id()).unwrap();
    assert!(w.process_alive());
    assert_eq!(now_wait(&mut w), Woke::Deadline);
    drop(child.stdin.take());
    child.wait().unwrap();
}

#[test]
fn the_process_exiting_is_process_gone() {
    let d = dirs();
    let mut child = blocked_child();
    let mut w = Waiter::new(&watches(&d), child.id()).unwrap();
    // Closing stdin ends `cat`; `wait` returns once it has exited, so the
    // pidfd is readable before the hook's wait starts.
    drop(child.stdin.take());
    child.wait().unwrap();
    assert!(!w.process_alive());
    assert_eq!(now_wait(&mut w), Woke::ProcessGone);
}

#[test]
fn a_pid_that_no_longer_exists_is_an_error() {
    let d = dirs();
    let mut child = blocked_child();
    let pid = child.id();
    drop(child.stdin.take());
    child.wait().unwrap();
    // Reaped: pidfd_open fails with ESRCH (unless the pid was reused, which
    // a test cannot rule out, so only an Ok for a live reused pid is let by).
    if let Ok(w) = Waiter::new(&watches(&d), pid) {
        assert!(
            w.process_alive(),
            "a pidfd for a reaped pid must not be opened"
        );
    }
}

// ── timeout_ms ──────────────────────────────────────────────────────────────

#[test]
fn timeout_is_zero_at_or_after_the_deadline() {
    let now = Instant::now();
    assert_eq!(timeout_ms(now, now), 0);
    assert_eq!(timeout_ms(now + Duration::from_secs(5), now), 0);
}

#[test]
fn timeout_is_the_milliseconds_left() {
    let now = Instant::now();
    assert_eq!(timeout_ms(now, now + Duration::from_millis(1500)), 1500);
    assert_eq!(timeout_ms(now, now + Duration::from_secs(60)), 60_000);
}

#[test]
fn timeout_saturates_at_i32_max_and_is_never_negative() {
    // A negative poll timeout means "wait forever": it must never be passed.
    let now = Instant::now();
    let far = now + Duration::from_secs(30 * 24 * 3600);
    assert_eq!(timeout_ms(now, far), i32::MAX);
    assert!(timeout_ms(now, now + Duration::from_millis(i32::MAX as u64 + 1)) >= 0);
}

// ── parse_events ────────────────────────────────────────────────────────────

/// One `struct inotify_event`: wd, mask, cookie, len, then `len` name bytes
/// padded with NULs.
fn event_bytes(wd: i32, mask: u32, name: &str, len: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&wd.to_ne_bytes());
    b.extend_from_slice(&mask.to_ne_bytes());
    b.extend_from_slice(&0u32.to_ne_bytes());
    b.extend_from_slice(&len.to_ne_bytes());
    let mut n = name.as_bytes().to_vec();
    n.resize(len as usize, 0);
    b.extend_from_slice(&n);
    b
}

#[test]
fn an_empty_buffer_has_no_events() {
    assert_eq!(parse_events(&[]), Some(vec![]));
}

#[test]
fn records_are_parsed_and_names_lose_their_padding() {
    let mut buf = event_bytes(1, libc::IN_MOVED_TO, "swap.json", 16);
    buf.extend(event_bytes(2, libc::IN_DELETE_SELF, "", 0));
    assert_eq!(
        parse_events(&buf),
        Some(vec![
            RawEvent {
                wd: 1,
                mask: libc::IN_MOVED_TO,
                name: b"swap.json".to_vec(),
            },
            RawEvent {
                wd: 2,
                mask: libc::IN_DELETE_SELF,
                name: Vec::new(),
            },
        ])
    );
}

#[test]
fn an_unaligned_buffer_parses_the_same() {
    let inner = event_bytes(3, libc::IN_CLOSE_WRITE, "swap.json", 16);
    let mut shifted = vec![0xAA];
    shifted.extend_from_slice(&inner);
    assert_eq!(parse_events(&shifted[1..]), parse_events(&inner));
}

#[test]
fn a_truncated_header_or_name_is_none() {
    let full = event_bytes(1, libc::IN_MOVED_TO, "swap.json", 16);
    // S9: a short header, a name cut short, and a length past the buffer.
    assert_eq!(parse_events(&full[..10]), None);
    assert_eq!(parse_events(&full[..full.len() - 4]), None);
    let huge = event_bytes(1, libc::IN_MOVED_TO, "", 0);
    let mut huge = huge;
    huge[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert_eq!(parse_events(&huge), None);
}

#[test]
fn a_good_record_followed_by_a_truncated_one_is_none() {
    let mut buf = event_bytes(1, libc::IN_MOVED_TO, "swap.json", 16);
    buf.extend_from_slice(&event_bytes(1, libc::IN_MOVED_TO, "swap.json", 16)[..20]);
    assert_eq!(parse_events(&buf), None);
}
