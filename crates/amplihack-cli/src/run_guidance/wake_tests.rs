//! Unit tests for `run_guidance::wake` (issue #3129, D4 and D-11).
//!
//! Wire with `#[cfg(test)] mod wake_tests;` in `run_guidance/wake.rs`, and
//! `pub(crate) mod wake;` in `run_guidance/mod.rs`.
//!
//! Contract of the crate-private API under test:
//!
//! - `wake::WAKE_DIR` is `"guidance-wake"`, a 0700 folder beside `guidance/`.
//! - `Waiter::arm(parent: &Path) -> Result<Waiter, GuideError>` makes a FIFO
//!   under `.tmp-<pid>-<n>`, opens the read end and its own write end, then
//!   renames it to `<pid>-<n>.fifo`. A FIFO under a final name therefore
//!   always has a reader (D-11).
//! - `Waiter::path()`, `Waiter::take() -> usize` (non-blocking: how many
//!   wake bytes were waiting) and `Waiter::as_raw_fd()` for a poller.
//!   Dropping a `Waiter` unlinks its FIFO.
//! - `notify(parent: &Path) -> Option<String>` wakes every waiter and returns
//!   a warning line only when something unexpected happened. It never blocks
//!   and never fails. It opens with `O_WRONLY|O_NONBLOCK|O_NOFOLLOW`, checks
//!   the open descriptor with `fstat` (FIFO, owned by the euid) and only then
//!   writes one byte. It unlinks only names matching `^\d+-\d+\.fifo$`, and
//!   only after `ENXIO`.
//! - `append_entry` calls `notify` after publishing.
//!
//! Nothing here sleeps, polls or sets a timer: every test reads or writes
//! a FIFO that is already in the state the test needs.

use super::wake::{Waiter, WAKE_DIR, notify};
use super::*;

use std::ffi::CString;
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, root)
}

/// The wake folder, created 0700 by hand, for tests that plant names in it.
fn wake_dir(root: &Path) -> PathBuf {
    let dir = root.join(WAKE_DIR);
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn mkfifo(path: &Path) {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
}

#[test]
fn arm_then_notify_wakes_the_waiter_with_one_byte() {
    let (_t, root) = root();
    let mut waiter = Waiter::arm(&root).expect("arm");
    assert_eq!(waiter.take(), 0, "nothing was sent yet");
    assert_eq!(notify(&root), None, "a healthy notify has no warning");
    assert_eq!(waiter.take(), 1);
    assert_eq!(waiter.take(), 0, "a second take blocks no one and finds nothing");
}

#[test]
fn a_spurious_byte_is_one_wake_not_a_state() {
    let (_t, root) = root();
    let mut waiter = Waiter::arm(&root).unwrap();
    notify(&root);
    notify(&root);
    assert!(waiter.take() >= 1);
    assert_eq!(waiter.take(), 0, "the caller reads again and blocks again");
}

#[test]
fn notify_with_no_wake_folder_does_nothing_and_creates_nothing() {
    let (_t, root) = root();
    assert_eq!(notify(&root), None);
    assert!(!root.join(WAKE_DIR).exists(), "notify never creates the folder");
}

#[test]
fn arm_creates_a_private_folder_and_a_final_named_fifo() {
    let (_t, root) = root();
    let waiter = Waiter::arm(&root).unwrap();
    let dir = root.join(WAKE_DIR);
    let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    let name = waiter.path().file_name().unwrap().to_str().unwrap().to_owned();
    let (pid, rest) = name.split_once('-').expect("<pid>-<n>.fifo");
    assert!(pid.bytes().all(|b| b.is_ascii_digit()), "{name}");
    assert!(rest.strip_suffix(".fifo").unwrap().bytes().all(|b| b.is_ascii_digit()));
    assert!(fs::symlink_metadata(waiter.path()).unwrap().file_type().is_fifo());
    assert_eq!(
        fs::symlink_metadata(waiter.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "the temp name is gone after the rename: {leftovers:?}");
}

#[test]
fn a_fifo_under_a_final_name_always_has_a_reader() {
    // D-11: ENXIO means stale, so a final name must never lack a reader.
    let (_t, root) = root();
    let waiter = Waiter::arm(&root).unwrap();
    let c = CString::new(waiter.path().as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid path; the descriptor is closed below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    assert!(fd >= 0, "opening the write end must not give ENXIO right after arm");
    // SAFETY: `fd` was opened above.
    unsafe { libc::close(fd) };
}

#[test]
fn dropping_the_waiter_unlinks_its_fifo() {
    let (_t, root) = root();
    let waiter = Waiter::arm(&root).unwrap();
    let path = waiter.path().to_path_buf();
    drop(waiter);
    assert!(fs::symlink_metadata(&path).is_err(), "the guard removed {path:?}");
}

#[test]
fn a_stale_fifo_is_unlinked_on_enxio() {
    let (_t, root) = root();
    let dir = wake_dir(&root);
    let stale = dir.join("424242-1.fifo");
    mkfifo(&stale); // no reader: open for write gives ENXIO
    assert_eq!(notify(&root), None);
    assert!(fs::symlink_metadata(&stale).is_err(), "stale FIFO was removed");
}

#[test]
fn a_symlink_with_a_fifo_name_is_left_alone_and_nothing_is_written() {
    let (_t, root) = root();
    let dir = wake_dir(&root);
    let target = root.join("target.txt");
    fs::write(&target, b"keep").unwrap();
    let link = dir.join("1-1.fifo");
    symlink(&target, &link).unwrap();
    notify(&root);
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
}

#[test]
fn a_regular_file_named_like_a_fifo_is_left_alone_and_not_written() {
    let (_t, root) = root();
    let dir = wake_dir(&root);
    let file = dir.join("1-1.fifo");
    fs::write(&file, b"keep").unwrap();
    notify(&root);
    assert_eq!(fs::read(&file).unwrap(), b"keep", "fstat said not a FIFO, so no write");
    assert!(file.exists(), "and it is not unlinked either");
}

#[test]
fn a_fifo_still_arming_under_a_tmp_name_is_never_touched() {
    let (_t, root) = root();
    let dir = wake_dir(&root);
    let arming = dir.join(".tmp-1-1");
    mkfifo(&arming);
    notify(&root);
    assert!(
        fs::symlink_metadata(&arming).unwrap().file_type().is_fifo(),
        "notify ignores `.tmp-*`, even with no reader"
    );
}

#[test]
fn names_outside_the_fifo_grammar_are_never_unlinked() {
    let (_t, root) = root();
    let dir = wake_dir(&root);
    for name in ["x-1.fifo", "1-1.fifo.bak", "1.fifo", "1-1-1.fifo"] {
        mkfifo(&dir.join(name));
    }
    notify(&root);
    for name in ["x-1.fifo", "1-1.fifo.bak", "1.fifo", "1-1-1.fifo"] {
        assert!(dir.join(name).exists(), "{name} must survive: it is not ours");
    }
}

#[test]
fn append_entry_wakes_an_armed_waiter() {
    let (_t, root) = root();
    let mut waiter = Waiter::arm(&root).unwrap();
    let appended = append_entry(&root.join(GUIDANCE_DIR), "hold the merge", "operator").unwrap();
    assert!(appended.wake_warning.is_none(), "{:?}", appended.wake_warning);
    assert!(appended.path.exists(), "the entry is published before the wake");
    assert_eq!(waiter.take(), 1);
}

#[test]
fn append_entry_without_a_waiter_succeeds_with_no_warning() {
    let (_t, root) = root();
    let appended = append_entry(&root.join(GUIDANCE_DIR), "note", "operator").unwrap();
    assert!(appended.wake_warning.is_none());
}

#[test]
fn recipe_cleanup_style_removal_of_the_wake_folder_does_not_hang() {
    // A run folder holding an armed waiter is removed recursively: a FIFO is
    // unlinked like a file and never opened.
    let (_t, root) = root();
    let waiter = Waiter::arm(&root).unwrap();
    fs::remove_dir_all(root.join(WAKE_DIR)).unwrap();
    drop(waiter); // the guard tolerates the path already being gone
}
