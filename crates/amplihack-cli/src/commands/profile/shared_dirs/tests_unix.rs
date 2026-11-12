//! The opened-directory re-checks of `unix.rs` (security review L1).
use super::*;
use std::os::unix::fs::PermissionsExt;

fn open_dir(p: &Path) -> OwnedFd {
    fdwalk::openat(libc::AT_FDCWD, p.as_os_str(), DIR_FLAGS, 0).unwrap()
}

fn chmod(p: &Path, mode: u32) {
    fs::set_permissions(p, fs::Permissions::from_mode(mode)).unwrap();
}

/// The rules are applied to what was opened: a directory that turns
/// world-writable (or gains group bits) after the path check fails.
#[test]
fn the_opened_directory_is_checked_not_only_the_path() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().join("d");
    fs::create_dir(&d).unwrap();
    chmod(&d, 0o700);
    let fd = open_dir(&d);
    let (fd, m) = opened_meta(fd).unwrap();
    assert!(root_ok(&m) && profile_ok(&m));
    chmod(&d, 0o777);
    let (_fd, m) = opened_meta(fd).unwrap();
    assert!(!root_ok(&m), "world-writable root accepted");
    assert!(!profile_ok(&m), "profile with group/other bits accepted");
    chmod(&d, 0o700);
}

/// End to end: the refusal message is the documented one.
#[test]
fn a_world_writable_root_is_refused_with_the_documented_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".claude");
    fs::create_dir(&root).unwrap();
    chmod(&root, 0o777);
    let mut r = Report::default();
    assert!(check_canonical_root(&root, &mut r).is_none());
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(
        r.warnings[0].ends_with(
            "is not owned by you or is writable by group or others; sessions not shared"
        ),
        "{:?}",
        r.warnings
    );
    chmod(&root, 0o700);
    let mut r = Report::default();
    assert!(check_canonical_root(&root, &mut r).is_some());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

/// A second process that finds the name already linked does not exchange a
/// link for a link and report it as a backup (adversarial review #2).
#[test]
fn the_swap_refuses_when_the_name_is_no_longer_a_directory() {
    use super::super::link::{Swap, swap_in_link};
    let tmp = tempfile::tempdir().unwrap();
    let profile = tmp.path().join("p");
    let target = tmp.path().join("shared");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir(&profile).unwrap();
    std::os::unix::fs::symlink(&target, profile.join("projects")).unwrap();
    let dir = open_dir(&profile);

    match swap_in_link(&dir, &profile, "projects", &target) {
        Swap::RolledBack(e) => assert!(e.to_string().contains("no longer a directory"), "{e}"),
        _ => panic!("swap went ahead on a link"),
    }
    let names: Vec<_> = fs::read_dir(&profile)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names,
        vec![std::ffi::OsString::from("projects")],
        "temp link or backup left"
    );
}
