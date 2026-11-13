//! #2029: real blocking backends and a deterministic loop integration seam.
//! Retains the saved lane's directory-replacement tests, removes every
//! deadline and supplies completion/shutdown readiness as an escape event.

use super::watch_source::{self, Event, Events};
use std::fs;
use std::os::fd::RawFd;
use std::os::unix::fs::{PermissionsExt, symlink};

#[path = "tests_issue_2029_native_recovery.rs"]
mod recovery;

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let dir = tmp.path().join("observations");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    (tmp, dir)
}

fn close_on_exec(fd: RawFd) -> bool {
    // SAFETY: F_GETFD reads descriptor flags; fd remains owned by the source.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags >= 0 && flags & libc::FD_CLOEXEC != 0
}

#[test]
fn native_directory_watch_survives_two_atomic_record_replacements() {
    let (_tmp, dir) = fixture();
    fs::write(dir.join("work.json"), b"old").unwrap();
    let mut source = watch_source::open(std::slice::from_ref(&dir)).unwrap();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        fs::write(dir.join(".pending"), bytes).unwrap();
        fs::rename(dir.join(".pending"), dir.join("work.json")).unwrap();
        // Both are queued before waiting. If the backend loses the file
        // event the test fails on Completion instead of hanging indefinitely.
        source.wake_handle().completion().unwrap();
        let mut got = source.wait().unwrap();
        got.extend(source.drain().unwrap());
        assert!(
            got.contains(&Event::Changed),
            "lost atomic replacement: {got:?}"
        );
        assert!(
            got.contains(&Event::Completion),
            "lost completion readiness: {got:?}"
        );
    }
}

#[test]
fn replacing_a_profile_directory_can_be_reconciled_without_losing_its_new_inode() {
    let (tmp, dir) = fixture();
    let mut source = watch_source::open(&[tmp.path().to_path_buf(), dir.clone()]).unwrap();
    fs::rename(&dir, tmp.path().join("retired")).unwrap();
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    source
        .watch(&[tmp.path().to_path_buf(), dir.clone()])
        .unwrap();
    source.drain().unwrap();
    fs::write(dir.join("work.json"), b"new inode").unwrap();
    source.wake_handle().completion().unwrap();
    let mut got = source.wait().unwrap();
    got.extend(source.drain().unwrap());
    assert!(
        got.contains(&Event::Changed),
        "replacement directory was not watched"
    );
}

#[test]
fn shutdown_readiness_ends_a_wait_with_no_filesystem_activity() {
    let (_tmp, dir) = fixture();
    let mut source = watch_source::open(&[dir]).unwrap();
    source.wake_handle().stop().unwrap();
    assert!(source.wait().unwrap().contains(&Event::Stop));
}

#[test]
fn completion_notifications_are_coalescible_readiness_and_never_fake_changes() {
    let (_tmp, dir) = fixture();
    let mut source = watch_source::open(&[dir]).unwrap();
    for _ in 0..128 {
        source.wake_handle().completion().unwrap();
    }
    let mut got = source.wait().unwrap();
    got.extend(source.drain().unwrap());
    assert!(got.contains(&Event::Completion));
    assert!(!got.contains(&Event::Changed));
    assert!(source.drain().unwrap().is_empty());
}

#[test]
fn every_native_watch_and_wake_descriptor_is_close_on_exec() {
    let (_tmp, dir) = fixture();
    let source = watch_source::open(&[dir]).unwrap();
    let fds = source.raw_fds_for_test();
    assert!(
        fds.len() >= 3,
        "expected kernel source and wake descriptors"
    );
    assert!(
        fds.iter().all(|fd| close_on_exec(*fd)),
        "probe children could inherit {fds:?}"
    );
}

#[test]
fn symlink_directories_are_never_followed_by_the_watch_backend() {
    let (tmp, dir) = fixture();
    let alias = tmp.path().join("alias");
    symlink(&dir, &alias).unwrap();
    if let Ok(source) = watch_source::open(std::slice::from_ref(&alias)) {
        assert!(!source.watched().contains(&alias));
    }
}

#[test]
fn rewriting_an_existing_login_file_wakes_the_native_watcher() {
    let (_tmp, dir) = fixture();
    let login = dir.join(".credentials.json");
    fs::write(&login, b"synthetic old inode fixture").unwrap();
    fs::set_permissions(&login, fs::Permissions::from_mode(0o600)).unwrap();
    let mut source = watch_source::open(std::slice::from_ref(&dir)).unwrap();
    fs::write(&login, b"synthetic replacement fixture").unwrap();
    source.wake_handle().completion().unwrap();
    let mut events = source.wait().unwrap();
    events.extend(source.drain().unwrap());
    assert!(
        events.contains(&Event::Changed),
        "in-place login change lost: {events:?}"
    );
}

#[test]
fn a_private_profile_root_can_subscribe_to_its_protected_system_parent() {
    let (tmp, dir) = fixture();
    let parent = tmp.path().parent().unwrap().canonicalize().unwrap();
    let source = watch_source::open(&[parent.clone(), tmp.path().to_path_buf(), dir]).unwrap();
    assert!(source.watched().contains(&parent));
}
