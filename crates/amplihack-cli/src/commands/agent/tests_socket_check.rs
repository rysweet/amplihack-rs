//! Unit tests for `socket_check.rs` (S12): the checks made before every
//! tmux call that passes `-S`, and the creation of missing socket
//! directories one level at a time at mode 0700. Also the A8 rows 1 to 3,
//! which `tmux.rs` decides from these checks without running tmux.
//!
//! API under test:
//! - `socket_check::check(socket: &Path) -> SocketCheck`
//! - `enum SocketCheck { Ok, MissingParent, NotPrivate }` (`Debug`, `PartialEq`):
//!   `Ok` also when the parent is fine and the socket file does not exist
//! - `socket_check::prepare(socket: &Path) -> Result<(), String>`: checks the
//!   deepest existing ancestor, creates each missing directory at 0700 one
//!   level at a time, then checks again; `Err("socket directory is not private")`
//! - `tmux::classify_socket(socket: &Path) -> Option<SessionState>`: A8 rows
//!   1 to 3; `None` means tmux must be asked

use super::socket_check::*;
use super::tmux::{SessionState, classify_socket};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const NOT_PRIVATE: &str = "socket directory is not private";

fn dir(p: &Path, mode: u32) -> PathBuf {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
    p.to_path_buf()
}

/// A temp root (0700) and `root/tmux-1000` with `mode`.
fn parent(mode: u32) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    dir(tmp.path(), 0o700);
    let p = dir(&tmp.path().join("tmux-1000"), mode);
    (tmp, p)
}

fn bind(p: &Path) {
    drop(std::os::unix::net::UnixListener::bind(p).unwrap());
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_private_parent_passes_with_or_without_a_socket() {
    let (_t, p) = parent(0o700);
    assert_eq!(check(&p.join("default")), SocketCheck::Ok);
    bind(&p.join("default"));
    assert_eq!(check(&p.join("default")), SocketCheck::Ok);
    let (_t, p) = parent(0o750);
    assert_eq!(
        check(&p.join("default")),
        SocketCheck::Ok,
        "no bits for others"
    );
}

#[test]
fn a_parent_open_to_others_is_refused() {
    for m in [0o777, 0o707, 0o701, 0o704] {
        let (_t, p) = parent(m);
        assert_eq!(check(&p.join("default")), SocketCheck::NotPrivate, "{m:o}");
    }
}

#[test]
fn a_symlinked_parent_is_refused() {
    let (tmp, real) = parent(0o700);
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(check(&link.join("default")), SocketCheck::NotPrivate);
}

#[test]
fn the_socket_path_must_be_a_socket_and_not_a_symlink() {
    let (_t, p) = parent(0o700);
    fs::write(p.join("default"), "").unwrap();
    assert_eq!(
        check(&p.join("default")),
        SocketCheck::NotPrivate,
        "regular file"
    );
    let (_t, p) = parent(0o700);
    bind(&p.join("real"));
    std::os::unix::fs::symlink(p.join("real"), p.join("default")).unwrap();
    assert_eq!(
        check(&p.join("default")),
        SocketCheck::NotPrivate,
        "symlink"
    );
}

#[test]
fn an_ancestor_writable_by_others_needs_the_sticky_bit() {
    let tmp = tempfile::tempdir().unwrap();
    dir(tmp.path(), 0o700);
    let shared = dir(&tmp.path().join("shared"), 0o777);
    let p = dir(&shared.join("tmux-1000"), 0o700);
    assert_eq!(check(&p.join("default")), SocketCheck::NotPrivate);
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    assert_eq!(
        check(&p.join("default")),
        SocketCheck::Ok,
        "sticky, as /tmp"
    );
}

#[test]
fn a_missing_parent_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let sock = tmp.path().join("gone/tmux-1000/default");
    assert_eq!(check(&sock), SocketCheck::MissingParent);
}

#[test]
fn prepare_creates_missing_directories_at_0700() {
    let tmp = tempfile::tempdir().unwrap();
    dir(tmp.path(), 0o700);
    let sock = tmp.path().join("a/b/tmux-1000/default");
    prepare(&sock).expect("missing directories are created");
    for d in ["a", "a/b", "a/b/tmux-1000"] {
        assert_eq!(mode(&tmp.path().join(d)), 0o700, "{d}");
    }
    assert!(!sock.exists(), "the socket itself is left to tmux");
    assert_eq!(check(&sock), SocketCheck::Ok);
    prepare(&sock).expect("an existing private parent is accepted");
}

#[test]
fn prepare_refuses_under_an_unsafe_ancestor_and_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    dir(tmp.path(), 0o700);
    let open = dir(&tmp.path().join("open"), 0o777);
    let sock = open.join("tmux-1000/default");
    assert_eq!(prepare(&sock).unwrap_err(), NOT_PRIVATE);
    assert!(!open.join("tmux-1000").exists());
    let (_t, p) = parent(0o777);
    assert_eq!(prepare(&p.join("default")).unwrap_err(), NOT_PRIVATE);
}

#[test]
fn has_session_rows_1_to_3_need_no_tmux() {
    let (_t, p) = parent(0o700);
    let sock = p.join("default");
    assert_eq!(
        classify_socket(&sock),
        Some(SessionState::Absent),
        "no socket file"
    );
    bind(&sock);
    assert_eq!(classify_socket(&sock), None, "tmux is asked");
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("gone/default");
    assert_eq!(
        classify_socket(&gone),
        Some(SessionState::Absent),
        "no directory"
    );
    let (_t, p) = parent(0o777);
    let want = Some(SessionState::Unknown(NOT_PRIVATE.into()));
    assert_eq!(
        classify_socket(&p.join("default")),
        want,
        "even with no socket file"
    );
}
