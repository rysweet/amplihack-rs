//! Issue #1551: when the `gh` merge guard applies, and where it is found.

use super::merge_guard::{GUARD_REL, MERGE_GUARD_ENV, guard_applies, locate_guard_dir};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn constants_name_the_marker_and_the_bundle_dir() {
    assert_eq!(MERGE_GUARD_ENV, "AMPLIHACK_MERGE_GUARD");
    assert_eq!(GUARD_REL, "amplifier-bundle/tools/gh-merge-guard");
}

#[test]
fn guard_applies_to_auto_drive_recipes_by_name_or_stem() {
    let cases: &[(&str, &str)] = &[
        ("auto-drive-to-merge", "/b/recipes/auto-drive-to-merge.yaml"),
        ("autodrive-build", "/b/recipes/autodrive-build.yaml"),
        (
            "autodrive-merge-loop",
            "/b/recipes/autodrive-merge-loop.yaml",
        ),
        // Name only: launched from a file with another stem.
        ("autodrive-crusty-round", "/b/elsewhere/plain.yaml"),
        // Stem only: the file was given another `name:`.
        ("renamed", "/b/recipes/autodrive-merge-round.yaml"),
        ("renamed", "/b/recipes/auto-drive-to-merge.yaml"),
    ];
    for (name, path) in cases {
        assert!(
            guard_applies(name, Path::new(path), None),
            "{name} at {path} must be guarded"
        );
    }
}

#[test]
fn guard_does_not_apply_to_other_recipes() {
    let cases: &[(&str, &str)] = &[
        ("quality-loop", "/b/recipes/quality-loop.yaml"),
        ("default-workflow", "/b/recipes/default-workflow.yaml"),
        ("smart-orchestrator", "/b/recipes/smart-orchestrator.yaml"),
        // Near misses: no hyphen, a prefix inside the name, an extension only.
        ("autodrivex", "/b/recipes/autodrivex.yaml"),
        ("my-autodrive-build", "/b/recipes/my-autodrive-build.yaml"),
        (
            "auto-drive-to-merge-notes",
            "/b/recipes/auto-drive-to-merge-notes.yaml",
        ),
        ("x", "/b/autodrive-dir/x.yaml"),
    ];
    for (name, path) in cases {
        assert!(
            !guard_applies(name, Path::new(path), None),
            "{name} at {path} must not be guarded"
        );
    }
}

/// AZ-1: the marker can only switch the guard on. Values that read as "off"
/// elsewhere ("0", "false", "no", "off") still keep it on.
#[test]
fn any_non_empty_inherited_marker_applies_the_guard() {
    let unrelated = Path::new("/b/recipes/quality-loop.yaml");
    for value in ["1", "yes", "0", "true", "false", "FALSE", "no", "off", " "] {
        assert!(
            guard_applies("quality-loop", unrelated, Some(value)),
            "inherited marker {value:?} must keep the guard on"
        );
    }
}

#[test]
fn an_empty_inherited_marker_does_not_apply_the_guard() {
    let unrelated = Path::new("/b/recipes/quality-loop.yaml");
    assert!(!guard_applies("quality-loop", unrelated, Some("")));
    assert!(!guard_applies("quality-loop", unrelated, None));
}

fn bundle_with_shim(mode: Option<u32>) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let dir = home.join(GUARD_REL);
    fs::create_dir_all(&dir).unwrap();
    // Explicit, so the umask (often 002) cannot make the dir group-writable.
    set_mode(&dir, 0o755);
    if let Some(mode) = mode {
        let shim = dir.join("gh");
        fs::write(&shim, "#!/usr/bin/env bash\n# amplihack-gh-merge-guard\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&shim, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
    (tmp, home)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
#[test]
fn locate_guard_dir_returns_the_dir_holding_an_executable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    let dir = locate_guard_dir(&home).expect("an executable shim is found");
    assert_eq!(dir, home.join(GUARD_REL));
}

fn assert_refused(home: &Path) {
    let err = locate_guard_dir(home)
        .expect_err("must fail closed")
        .to_string();
    let want = format!(
        "amplihack: merge guard not found at {}; refusing to run auto-drive without it (#1551)",
        home.join(GUARD_REL).display()
    );
    assert_eq!(err, want);
}

#[test]
fn locate_guard_dir_refuses_a_missing_shim() {
    let (_tmp, home) = bundle_with_shim(None);
    assert_refused(&home);
}

#[test]
fn locate_guard_dir_refuses_a_missing_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    assert_refused(tmp.path());
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_non_executable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o644));
    assert_refused(&home);
}

#[test]
fn locate_guard_dir_refuses_a_shim_that_is_a_directory() {
    let (_tmp, home) = bundle_with_shim(None);
    fs::create_dir_all(home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused(&home);
}

/// A symlinked shim is followed for the checks, and the directory is returned
/// as given, never canonicalised, so PATH de-duplication still matches it.
#[cfg(unix)]
#[test]
fn locate_guard_dir_follows_a_symlinked_shim_without_canonicalising() {
    use std::os::unix::fs::PermissionsExt;
    let (_tmp, home) = bundle_with_shim(None);
    let target = home.join("real-gh");
    fs::write(&target, "#!/bin/bash -p\n# amplihack-gh-merge-guard\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink(&target, home.join(GUARD_REL).join("gh")).unwrap();
    let dir = locate_guard_dir(&home).expect("a symlink to an executable shim is found");
    assert_eq!(dir, home.join(GUARD_REL));
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_dangling_symlink() {
    let (_tmp, home) = bundle_with_shim(None);
    std::os::unix::fs::symlink(home.join("nowhere"), home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused(&home);
}

/// A relative `AMPLIHACK_HOME` would resolve against the working directory.
#[test]
fn locate_guard_dir_refuses_a_relative_home() {
    let home = Path::new("relative/amplihack-home");
    let err = locate_guard_dir(home)
        .expect_err("a relative home must fail closed")
        .to_string();
    assert!(err.contains("(#1551)"), "{err}");
}

/// G11: a shim or guard directory that group or others can write is not
/// trusted: anyone with that access could replace the guard.
#[cfg(unix)]
fn assert_refused_as_writable(home: &Path, what: &str) {
    let err = locate_guard_dir(home)
        .expect_err(&format!("{what} must fail closed"))
        .to_string();
    assert!(err.contains("(#1551)"), "{what}: {err}");
    assert!(
        err.contains(&home.join(GUARD_REL).display().to_string()),
        "{what}: the error must name the guard dir: {err}"
    );
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_group_writable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o775));
    assert_refused_as_writable(&home, "a 0775 shim");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_an_other_writable_shim() {
    let (_tmp, home) = bundle_with_shim(Some(0o757));
    assert_refused_as_writable(&home, "a 0757 shim");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_group_writable_guard_dir() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    set_mode(&home.join(GUARD_REL), 0o775);
    assert_refused_as_writable(&home, "a 0775 guard dir");
}

#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_an_other_writable_guard_dir() {
    let (_tmp, home) = bundle_with_shim(Some(0o755));
    // Sticky does not help: others can still add files beside the shim.
    set_mode(&home.join(GUARD_REL), 0o1777);
    assert_refused_as_writable(&home, "a 1777 guard dir");
}

/// A symlinked shim is judged by its target's mode.
#[cfg(unix)]
#[test]
fn locate_guard_dir_refuses_a_symlink_to_a_writable_shim() {
    let (_tmp, home) = bundle_with_shim(None);
    let target = home.join("real-gh");
    fs::write(&target, "#!/bin/bash -p\n# amplihack-gh-merge-guard\n").unwrap();
    set_mode(&target, 0o777);
    std::os::unix::fs::symlink(&target, home.join(GUARD_REL).join("gh")).unwrap();
    assert_refused_as_writable(&home, "a symlink to a 0777 shim");
}

/// Owner-only modes are fine: 0700 and 0500 are as safe as 0755.
#[cfg(unix)]
#[test]
fn locate_guard_dir_accepts_owner_only_modes() {
    for (shim_mode, dir_mode) in [(0o700, 0o700), (0o500, 0o755), (0o755, 0o555)] {
        let (_tmp, home) = bundle_with_shim(Some(shim_mode));
        set_mode(&home.join(GUARD_REL), dir_mode);
        let found = locate_guard_dir(&home);
        // Restore so the tempdir can be removed.
        set_mode(&home.join(GUARD_REL), 0o755);
        assert_eq!(
            found.expect("owner-only modes are accepted"),
            home.join(GUARD_REL),
            "shim {shim_mode:o}, dir {dir_mode:o}"
        );
    }
}
