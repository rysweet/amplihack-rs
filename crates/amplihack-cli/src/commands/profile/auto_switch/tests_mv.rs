//! `mv`: the login-moving primitives shared by auto-switch and `profile
//! rotate` (issue #1592). Files are checked by inode only; no login file is
//! ever read, so the fixtures are mode 0000 where the user is not root.
//!
//! Contract (declared in `auto_switch/mod.rs` as `#[cfg(all(test, unix))] mod
//! tests_mv;`):
//! - `mv::exchange_logins(a, b) -> Result<(), MoveError>`: atomic exchange,
//!   both paths must pass `movable_login`; `EXDEV`/`EINVAL`/`ENOSYS` map to
//!   `MoveError::Unsupported`; no copy fallback.
//! - `mv::move_login(src, dst, show: &dyn Fn(&Path) -> String)`: no-replace
//!   rename; `EEXIST` maps to `MoveError::Exists`.
//! - `mv::take_refresh_lock_at(dir) -> io::Result<u64>` and
//!   `mv::release_refresh_lock_at(dir, ino) -> bool`.
//! - `mv::abort_point(step)`: a no-op in release builds.

use super::fsx;
use super::mv::{
    MoveError, abort_point, exchange_logins, move_login, release_refresh_lock_at,
    take_refresh_lock_at,
};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LOCK: &str = ".oauth_refresh.lock";

fn login(path: &Path) -> u64 {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "SENTINEL").unwrap();
    let mode = if fsx::euid() == 0 { 0o600 } else { 0o000 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    ino(path)
}

fn ino(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

fn show(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

struct Dirs {
    _tmp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    Dirs { _tmp: tmp, a, b }
}

#[cfg(target_os = "linux")]
#[test]
fn exchange_swaps_the_two_inodes() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    exchange_logins(&pa, &pb).unwrap_or_else(|_| panic!("exchange failed"));
    assert_eq!(ino(&pa), ib);
    assert_eq!(ino(&pb), ia);
}

#[test]
fn exchange_refuses_a_missing_side_and_moves_nothing() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let ia = login(&pa);
    assert!(exchange_logins(&pa, &pb).is_err());
    assert_eq!(ino(&pa), ia);
    assert!(!fsx::exists(&pb));
}

#[test]
fn exchange_refuses_a_hard_linked_or_symlinked_login() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let ia = login(&pa);
    let ib = login(&pb);
    fs::hard_link(&pb, d.b.join("extra-link")).unwrap();
    assert!(exchange_logins(&pa, &pb).is_err(), "two links");
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));

    fs::remove_file(&pb).unwrap();
    std::os::unix::fs::symlink(d.b.join("extra-link"), &pb).unwrap();
    assert!(exchange_logins(&pa, &pb).is_err(), "symlink");
    assert_eq!(ino(&pa), ia);
    assert!(fs::symlink_metadata(&pb).unwrap().file_type().is_symlink());
}

/// A login other users could read stays put: moving it into a
/// group-readable `~/.claude` would expose it.
#[test]
fn a_login_readable_by_others_is_not_moved() {
    let d = dirs();
    let (pa, pb) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (ia, ib) = (login(&pa), login(&pb));
    fs::set_permissions(&pb, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fsx::movable_login(&pb));
    assert!(exchange_logins(&pa, &pb).is_err(), "exchange");
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
    fs::remove_file(&pa).unwrap();
    assert!(move_login(&pb, &pa, &show).is_err(), "move");
    assert_eq!(ino(&pb), ib);
    assert!(!fsx::exists(&pa));
}

/// A cross-device exchange is `Unsupported`, and nothing is copied. Needs a
/// second filesystem; `/dev/shm` usually is one.
#[cfg(target_os = "linux")]
#[test]
fn exchange_across_devices_is_unsupported_without_a_copy() {
    let d = dirs();
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        return;
    };
    if fs::metadata(other.path()).unwrap().dev() == fs::metadata(&d.a).unwrap().dev() {
        return;
    }
    let (pa, pb) = (
        d.a.join(".credentials.json"),
        other.path().join(".credentials.json"),
    );
    let (ia, ib) = (login(&pa), login(&pb));
    assert!(matches!(
        exchange_logins(&pa, &pb),
        Err(MoveError::Unsupported)
    ));
    assert_eq!((ino(&pa), ino(&pb)), (ia, ib));
}

#[test]
fn move_login_never_replaces() {
    let d = dirs();
    let (src, dst) = (d.a.join(".credentials.json"), d.b.join(".credentials.json"));
    let (is, id) = (login(&src), login(&dst));
    assert!(matches!(
        move_login(&src, &dst, &show),
        Err(MoveError::Exists)
    ));
    assert_eq!((ino(&src), ino(&dst)), (is, id));

    fs::rename(&dst, d.b.join("aside")).unwrap();
    assert!(move_login(&src, &dst, &show).is_ok());
    assert_eq!(ino(&dst), is);
    assert!(!fsx::exists(&src));
}

#[test]
fn move_login_error_uses_the_display_closure() {
    let d = dirs();
    let src = d.a.join(".credentials.json");
    let Err(MoveError::Other(msg)) = move_login(&src, &d.b.join("x"), &|_| "SHOWN".into()) else {
        panic!("a missing source is an error");
    };
    assert!(msg.contains("SHOWN"), "{msg}");
    assert!(!msg.contains(&*d.a.to_string_lossy()), "{msg}");
}

#[test]
fn refresh_lock_is_taken_and_released_by_inode() {
    let d = dirs();
    let ino_taken = take_refresh_lock_at(&d.a).expect("free lock is taken");
    assert_eq!(ino(&d.a.join(LOCK)), ino_taken);
    assert!(take_refresh_lock_at(&d.a).is_err(), "already held");
    assert!(release_refresh_lock_at(&d.a, ino_taken));
    assert!(!fsx::exists(&d.a.join(LOCK)));
}

#[test]
fn a_foreign_refresh_lock_is_never_broken() {
    let d = dirs();
    fs::create_dir(d.a.join(LOCK)).unwrap();
    let foreign = ino(&d.a.join(LOCK));
    assert!(take_refresh_lock_at(&d.a).is_err());
    assert!(!release_refresh_lock_at(&d.a, foreign + 1));
    assert_eq!(ino(&d.a.join(LOCK)), foreign, "left in place");
}

#[test]
fn release_ignores_a_regular_file_with_the_lock_name() {
    let d = dirs();
    let i = login(&d.a.join(LOCK));
    assert!(!release_refresh_lock_at(&d.a, i));
    assert!(fsx::exists(&d.a.join(LOCK)));
}

#[cfg(not(debug_assertions))]
#[test]
fn abort_point_is_a_no_op_in_release_builds() {
    // SAFETY: test-only; no other thread reads this variable.
    unsafe { std::env::set_var("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER", "rotate-S1") };
    abort_point("rotate-S1");
    unsafe { std::env::remove_var("AMPLIHACK_AUTO_SWITCH_TEST_ABORT_AFTER") };
}

#[cfg(debug_assertions)]
#[test]
fn abort_point_ignores_other_steps() {
    abort_point("rotate-never-set");
}
