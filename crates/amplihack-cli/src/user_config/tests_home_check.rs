//! Issue #2931: the test home check's "is this path under an accepted base" rule.
//!
//! `under_any(path, bases)` is pure: it reads the file system only to resolve
//! links, never the process environment. Every case builds its own folders.

use std::fs;
use std::path::{Path, PathBuf};

use super::under_any;

/// A canonical temporary root, so that an expected answer does not depend on
/// whether the host reaches its temporary folder through a link.
fn root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

#[test]
fn a_path_inside_each_base_is_accepted() {
    let (_tmp, root) = root();
    let bases = [root.join("a"), root.join("b")];
    for base in &bases {
        fs::create_dir_all(base.join("home")).unwrap();
    }
    for base in &bases {
        assert!(
            under_any(&base.join("home"), &bases),
            "{} is inside {}",
            base.join("home").display(),
            base.display()
        );
    }
}

#[test]
fn a_path_outside_every_base_is_refused() {
    let (_tmp, root) = root();
    let bases = [root.join("a"), root.join("b")];
    fs::create_dir_all(&bases[0]).unwrap();
    fs::create_dir_all(&bases[1]).unwrap();
    fs::create_dir_all(root.join("elsewhere")).unwrap();
    assert!(!under_any(&root.join("elsewhere"), &bases));
}

#[test]
fn a_dot_dot_component_is_refused_even_when_the_text_starts_inside_a_base() {
    let (_tmp, root) = root();
    let base = root.join("a");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(root.join("elsewhere")).unwrap();
    let sneaky = base.join("..").join("elsewhere");
    assert!(!under_any(&sneaky, std::slice::from_ref(&base)));
}

#[test]
fn a_sibling_whose_name_starts_with_the_base_name_is_refused() {
    // `/tmpevil/x` must not pass for `/tmp`: the comparison is by component.
    let (_tmp, root) = root();
    let base = root.join("tmp");
    let evil = root.join("tmpevil");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&evil).unwrap();
    assert!(!under_any(&evil.join("x"), std::slice::from_ref(&base)));
}

#[test]
fn a_missing_leaf_under_a_base_is_accepted_and_under_a_missing_base_is_not_invented() {
    let (_tmp, root) = root();
    let base = root.join("a");
    fs::create_dir_all(&base).unwrap();
    assert!(under_any(
        &base.join("not").join("yet").join("made"),
        std::slice::from_ref(&base)
    ));
    assert!(!under_any(
        &root.join("other").join("made"),
        std::slice::from_ref(&base)
    ));
}

#[cfg(unix)]
#[test]
fn a_link_that_points_outside_every_base_is_refused() {
    let (_tmp, root) = root();
    let base = root.join("a");
    let outside = root.join("outside");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let link = base.join("looks-inside");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(!under_any(&link, std::slice::from_ref(&base)));
    assert!(!under_any(&link.join("file"), std::slice::from_ref(&base)));
}

#[cfg(unix)]
#[test]
fn a_dangling_link_pointing_outside_is_refused() {
    let (_tmp, root) = root();
    let base = root.join("a");
    fs::create_dir_all(&base).unwrap();
    let link = base.join("dangling");
    std::os::unix::fs::symlink(root.join("nowhere"), &link).unwrap();
    assert!(!under_any(&link, std::slice::from_ref(&base)));
}

#[cfg(unix)]
#[test]
fn a_base_reached_through_a_link_accepts_the_path_in_either_spelling() {
    // macOS reaches `/var/folders/..` as `/private/var/folders/..`.
    let (_tmp, root) = root();
    let real = root.join("real");
    fs::create_dir_all(real.join("home")).unwrap();
    let link = root.join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let bases = [link.clone()];
    assert!(under_any(&link.join("home"), &bases));
    assert!(under_any(&real.join("home"), &bases));
}

#[test]
fn an_empty_list_of_bases_accepts_nothing() {
    let (_tmp, root) = root();
    assert!(!under_any(Path::new(&root), &[]));
}

/// Under `cfg(test)` of the library the check also accepts `/var/tmp` and
/// `/tmp`, so a fixture made there is not treated as the user's real home
/// (issue #2931, the fixture-home group). The shipped binary keeps
/// `temp_dir()` alone; that half is proved by the binary-level case in
/// `issue_1606_config_cli`.
#[test]
fn the_library_test_build_accepts_var_tmp_and_tmp_but_not_a_home_folder() {
    for base in ["/var/tmp", "/tmp"] {
        assert!(
            super::under_temp_dir(&Path::new(base).join("amplihack-base-case")),
            "{base} is a temporary base in the test build"
        );
    }
    assert!(!super::under_temp_dir(Path::new(
        "/Users/someone/.amplihack"
    )));
    assert!(!super::under_temp_dir(Path::new(
        "/home/someone/.amplihack"
    )));
}
