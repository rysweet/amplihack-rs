//! Issue #1658 (S0): the auto-switch file helpers moved from the CLI's
//! `auto_switch/fsx.rs` into `amplihack_utils::secure_files`, plus the three
//! helpers the limit-resume hook needs: `owned_by_euid`,
//! `open_private_nofollow` and `lock_exclusive_timeout`.
//!
//! The moved helpers keep their behaviour, so these tests pin the parts the
//! hook relies on. The new helpers are specified here first.

#![cfg(unix)]

use amplihack_utils::secure_files::{
    check_private_dir, ensure_private_dir, euid, lock_exclusive_timeout, open_private_nofollow,
    owned_by_euid, read_bounded, read_bounded_nofollow, writable_by_others, write_private_atomic,
    write_private_atomic_nosync,
};
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::time::{Duration, Instant};

fn mode(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().mode() & 0o7777
}

// ── Moved helpers: behaviour unchanged ──────────────────────────────────────

#[test]
fn read_bounded_returns_one_extra_byte_when_input_is_longer() {
    let data = [b'x'; 10];
    assert_eq!(read_bounded(&data[..], 10).unwrap().len(), 10);
    assert_eq!(read_bounded(&data[..], 4).unwrap().len(), 5);
}

#[test]
fn read_bounded_nofollow_missing_file_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        read_bounded_nofollow(&tmp.path().join("nope"), 16)
            .unwrap()
            .is_none()
    );
}

#[test]
fn read_bounded_nofollow_refuses_symlink_and_oversize() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    fs::write(&real, b"hello").unwrap();
    let link = tmp.path().join("link");
    symlink(&real, &link).unwrap();

    let err = read_bounded_nofollow(&link, 16).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);

    let err = read_bounded_nofollow(&real, 4).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);

    let (bytes, meta) = read_bounded_nofollow(&real, 16).unwrap().unwrap();
    assert_eq!(bytes, b"hello");
    assert!(meta.is_file());
}

#[test]
fn write_private_atomic_writes_0600_and_refuses_symlink_target() {
    let tmp = tempfile::tempdir().unwrap();
    write_private_atomic(tmp.path(), "f", b"one").unwrap();
    assert_eq!(fs::read(tmp.path().join("f")).unwrap(), b"one");
    assert_eq!(mode(&tmp.path().join("f")), 0o600);

    let other = tmp.path().join("other");
    fs::write(&other, b"keep").unwrap();
    symlink(&other, tmp.path().join("l")).unwrap();
    assert!(write_private_atomic(tmp.path(), "l", b"x").is_err());
    assert_eq!(fs::read(&other).unwrap(), b"keep");
}

#[test]
fn write_private_atomic_nosync_replaces_0600_and_refuses_symlink_target() {
    let tmp = tempfile::tempdir().unwrap();
    write_private_atomic_nosync(tmp.path(), "f", b"one").unwrap();
    write_private_atomic_nosync(tmp.path(), "f", b"two").unwrap();
    assert_eq!(fs::read(tmp.path().join("f")).unwrap(), b"two");
    assert_eq!(mode(&tmp.path().join("f")), 0o600);
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1, "no temp left");

    let other = tmp.path().join("other");
    fs::write(&other, b"keep").unwrap();
    symlink(&other, tmp.path().join("l")).unwrap();
    assert!(write_private_atomic_nosync(tmp.path(), "l", b"x").is_err());
    assert_eq!(fs::read(&other).unwrap(), b"keep");
}

#[test]
fn ensure_private_dir_creates_0700_and_checks_existing() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("d");
    ensure_private_dir(&d).unwrap();
    assert_eq!(mode(&d), 0o700);
    // Existing private dir: fine.
    ensure_private_dir(&d).unwrap();
    check_private_dir(&d).unwrap();
}

#[test]
fn ensure_private_dir_refuses_existing_0755_without_changing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("d");
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ensure_private_dir(&d).is_err());
    assert!(check_private_dir(&d).is_err());
    assert_eq!(mode(&d), 0o755, "an existing path is never chmod-ed");
}

#[test]
fn ensure_private_dir_does_not_create_parents() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("a").join("b");
    assert!(ensure_private_dir(&nested).is_err());
    assert!(!tmp.path().join("a").exists());
}

#[test]
fn check_private_dir_refuses_symlink_to_private_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    ensure_private_dir(&real).unwrap();
    let link = tmp.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(check_private_dir(&link).is_err());
}

#[test]
fn writable_by_others_detects_world_write() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("f");
    fs::write(&f, b"").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(!writable_by_others(&fs::metadata(&f).unwrap()));
    fs::set_permissions(&f, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(writable_by_others(&fs::metadata(&f).unwrap()));
}

// ── New: owned_by_euid ──────────────────────────────────────────────────────

#[test]
fn owned_by_euid_is_true_for_own_files() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = fs::metadata(tmp.path()).unwrap();
    assert_eq!(meta.uid(), euid());
    assert!(owned_by_euid(&meta));
}

#[test]
fn owned_by_euid_is_false_for_root_owned_files_when_not_root() {
    if euid() == 0 {
        return;
    }
    assert!(!owned_by_euid(&fs::metadata("/").unwrap()));
}

// ── New: open_private_nofollow ──────────────────────────────────────────────

#[test]
fn open_private_nofollow_creates_missing_file_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("new");
    let mut f = open_private_nofollow(&p).unwrap();
    f.write_all(b"a").unwrap();
    assert_eq!(mode(&p), 0o600);
    assert_eq!(fs::read(&p).unwrap(), b"a");
}

#[test]
fn open_private_nofollow_appends_to_existing_private_file() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("f");
    fs::write(&p, b"one\n").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let mut f = open_private_nofollow(&p).unwrap();
    f.write_all(b"two\n").unwrap();
    drop(f);
    assert_eq!(fs::read(&p).unwrap(), b"one\ntwo\n");

    // Readable too, for the session record.
    let mut f = open_private_nofollow(&p).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    assert_eq!(s, "one\ntwo\n");
}

#[test]
fn open_private_nofollow_refuses_symlink() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    fs::write(&target, b"keep").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = tmp.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(open_private_nofollow(&link).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
}

#[test]
fn open_private_nofollow_refuses_dangling_symlink_without_creating_target() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("would-be-created");
    let link = tmp.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(open_private_nofollow(&link).is_err());
    assert!(!target.exists());
}

#[test]
fn open_private_nofollow_refuses_group_or_world_readable_file() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("f");
    fs::write(&p, b"").unwrap();
    for m in [0o644, 0o640, 0o620, 0o602] {
        fs::set_permissions(&p, fs::Permissions::from_mode(m)).unwrap();
        assert!(open_private_nofollow(&p).is_err(), "mode {m:o} accepted");
        assert_eq!(mode(&p), m, "mode {m:o} was changed");
    }
}

#[test]
fn open_private_nofollow_refuses_hard_linked_file() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("f");
    fs::write(&p, b"").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&p, tmp.path().join("g")).unwrap();
    assert!(open_private_nofollow(&p).is_err());
}

#[test]
fn open_private_nofollow_refuses_directory_and_fifo() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("d");
    fs::create_dir(&d).unwrap();
    fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(open_private_nofollow(&d).is_err());

    let fifo = tmp.path().join("fifo");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: valid NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    // Must not block on open, and must refuse.
    let start = Instant::now();
    assert!(open_private_nofollow(&fifo).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}

// ── New: lock_exclusive_timeout ─────────────────────────────────────────────

#[test]
fn lock_exclusive_timeout_is_exclusive_and_released_on_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("lock");
    let a = open_private_nofollow(&p).unwrap();
    let b = open_private_nofollow(&p).unwrap();

    let held = lock_exclusive_timeout(a, Duration::from_millis(500)).unwrap();

    let start = Instant::now();
    let err = lock_exclusive_timeout(b, Duration::from_millis(300)).unwrap_err();
    let waited = start.elapsed();
    assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    assert!(
        waited >= Duration::from_millis(250),
        "gave up too early: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "waited far past the timeout: {waited:?}"
    );

    drop(held);
    let c = open_private_nofollow(&p).unwrap();
    lock_exclusive_timeout(c, Duration::from_millis(300)).expect("lock is free after drop");
}

#[test]
fn lock_exclusive_timeout_retries_until_holder_releases() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("lock");
    let held = lock_exclusive_timeout(open_private_nofollow(&p).unwrap(), Duration::ZERO).unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        drop(held);
    });
    let got = lock_exclusive_timeout(open_private_nofollow(&p).unwrap(), Duration::from_secs(3));
    releaser.join().unwrap();
    assert!(got.is_ok(), "a lock freed within the timeout must be taken");
}
