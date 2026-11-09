//! `codex::link` (issue #2005): inspecting and relinking the default Codex
//! home with `lstat`, `readlink`, `stat`, `symlink` and `rename` only.
//!
//! Contract:
//! - `inspect(link: &Path, root: &Path) -> LinkState`, with
//!   `LinkState::{Link { target: PathBuf, label: String, ino: u64 },
//!   NotLink, Unreadable, OutsideRoot, Dangling, Missing}`. `target` is
//!   canonical.
//! - `swap_tmp(link) -> PathBuf` is `<link>.amplihack-swap`.
//! - `relink(link, new_target, fault) -> io::Result<()>` writes an absolute
//!   symlink at `swap_tmp(link)` and renames it over `link`; a rename
//!   error is returned and nothing is deleted.

use super::link::{LinkState, inspect, relink, swap_tmp};
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};

struct T {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    link: PathBuf,
}

fn setup() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("profiles");
    for a in ["a", "b"] {
        fs::create_dir_all(root.join(a)).unwrap();
    }
    T {
        link: base.join("home").join(".codex"),
        root,
        _tmp: tmp,
    }
}

fn mklink(t: &T, target: &Path) {
    fs::create_dir_all(t.link.parent().unwrap()).unwrap();
    symlink(target, &t.link).unwrap();
}

#[test]
fn a_link_to_an_account_gives_its_label_and_inode() {
    let t = setup();
    mklink(&t, &t.root.join("a"));
    match inspect(&t.link, &t.root) {
        LinkState::Link { target, label, ino } => {
            assert_eq!(target, t.root.join("a"));
            assert_eq!(label, "a");
            assert_eq!(ino, fs::metadata(t.root.join("a")).unwrap().ino());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_relative_link_is_resolved_from_its_parent() {
    let t = setup();
    mklink(&t, Path::new("../profiles/b"));
    assert!(matches!(inspect(&t.link, &t.root), LinkState::Link { label, .. } if label == "b"));
}

#[test]
fn a_link_that_cannot_be_examined_is_unreadable_not_a_directory() {
    // Review of #2162: a permission error is not reported as a real
    // directory (`home-not-link` with no detail).
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads through mode 000
    }
    let t = setup();
    mklink(&t, &t.root.join("a"));
    let parent = t.link.parent().unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o000)).unwrap();
    let state = inspect(&t.link, &t.root);
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(state, LinkState::Unreadable);
}

#[test]
fn the_link_states_are_told_apart() {
    let t = setup();
    assert_eq!(inspect(&t.link, &t.root), LinkState::Missing);

    fs::create_dir_all(&t.link).unwrap();
    assert_eq!(inspect(&t.link, &t.root), LinkState::NotLink);
    fs::remove_dir(&t.link).unwrap();

    fs::write(&t.link, "x").unwrap();
    assert_eq!(inspect(&t.link, &t.root), LinkState::NotLink);
    fs::remove_file(&t.link).unwrap();

    mklink(&t, &t.root.join("gone"));
    assert_eq!(inspect(&t.link, &t.root), LinkState::Dangling);
}

#[test]
fn containment_is_by_component_and_direct_child_only() {
    let t = setup();
    let evil = PathBuf::from(format!("{}-evil", t.root.display())).join("a");
    fs::create_dir_all(&evil).unwrap();
    mklink(&t, &evil);
    assert_eq!(
        inspect(&t.link, &t.root),
        LinkState::OutsideRoot,
        "sibling prefix"
    );
    fs::remove_file(&t.link).unwrap();

    fs::create_dir_all(t.root.join("a").join("inner")).unwrap();
    mklink(&t, &t.root.join("a").join("inner"));
    assert_eq!(inspect(&t.link, &t.root), LinkState::OutsideRoot, "nested");
    fs::remove_file(&t.link).unwrap();

    mklink(&t, &t.root);
    assert_eq!(
        inspect(&t.link, &t.root),
        LinkState::OutsideRoot,
        "the root itself"
    );
    fs::remove_file(&t.link).unwrap();

    // A symlinked account that resolves outside the root is outside.
    let out = t.root.parent().unwrap().join("elsewhere");
    fs::create_dir_all(&out).unwrap();
    symlink(&out, t.root.join("sneaky")).unwrap();
    mklink(&t, &t.root.join("sneaky"));
    assert_eq!(
        inspect(&t.link, &t.root),
        LinkState::OutsideRoot,
        "resolved outside"
    );
}

#[test]
fn a_label_starting_with_a_dot_is_not_an_account() {
    let t = setup();
    fs::create_dir_all(t.root.join(".hidden")).unwrap();
    mklink(&t, &t.root.join(".hidden"));
    assert_eq!(inspect(&t.link, &t.root), LinkState::OutsideRoot);
}

#[test]
fn the_temp_path_sits_beside_the_link() {
    assert_eq!(
        swap_tmp(Path::new("/h/.codex")),
        PathBuf::from("/h/.codex.amplihack-swap")
    );
}

#[test]
fn relink_replaces_the_link_atomically_with_an_absolute_target() {
    let t = setup();
    mklink(&t, Path::new("../profiles/a"));
    let ino_a = fs::metadata(t.root.join("a")).unwrap().ino();
    relink(&t.link, &t.root.join("b"), None).unwrap();
    assert_eq!(fs::read_link(&t.link).unwrap(), t.root.join("b"));
    assert!(fs::symlink_metadata(swap_tmp(&t.link)).is_err());
    assert_eq!(fs::metadata(t.root.join("a")).unwrap().ino(), ino_a);
}

#[test]
fn relink_over_a_real_directory_fails_and_deletes_nothing() {
    let t = setup();
    fs::create_dir_all(&t.link).unwrap();
    fs::write(t.link.join("keep"), "x").unwrap();
    assert!(relink(&t.link, &t.root.join("b"), None).is_err());
    assert_eq!(fs::read_to_string(t.link.join("keep")).unwrap(), "x");
}
