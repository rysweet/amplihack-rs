//! Trusted release selection survives retirement of the process merge guard.
//! Real bundle directories and completed release links are accepted; links outside
//! releases, incomplete releases and unsafe modes remain rejected.

use crate::commands::orch::autodrive::trusted::pin_bundle;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// `bundle` with an executable shim and its bash-env, owner-only modes.
fn write_bundle(bundle: &Path) {
    fs::create_dir_all(bundle.join("tools")).unwrap();
    fs::write(bundle.join("tools/policy.md"), "trusted policy").unwrap();
    set_mode(bundle, 0o755);
}

/// `AMPLIHACK_HOME` with `amplifier-bundle -> releases/1.0.0/amplifier-bundle`.
fn linked_home() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".amplihack");
    let release = home.join("releases/1.0.0");
    write_bundle(&release.join("amplifier-bundle"));
    fs::write(release.join(".complete"), "").unwrap();
    set_mode(&release, 0o755);
    set_mode(&home.join("releases"), 0o700);
    set_mode(&home, 0o700);
    symlink(
        release.join("amplifier-bundle"),
        home.join("amplifier-bundle"),
    )
    .unwrap();
    (tmp, home, release)
}

#[test]
fn pin_resolves_a_link_into_a_complete_release_once() {
    let (_tmp, home, release) = linked_home();
    let pinned = pin_bundle(&home).unwrap();
    assert_eq!(
        pinned,
        fs::canonicalize(release.join("amplifier-bundle")).unwrap()
    );
    // A later switch does not change what the run already pinned.
    let next = home.join("releases/1.0.1");
    write_bundle(&next.join("amplifier-bundle"));
    fs::write(next.join(".complete"), "").unwrap();
    set_mode(&next, 0o755);
    fs::remove_file(home.join("amplifier-bundle")).unwrap();
    symlink(next.join("amplifier-bundle"), home.join("amplifier-bundle")).unwrap();
    assert!(pinned.join("tools/policy.md").is_file());
    assert_ne!(
        pinned,
        fs::canonicalize(home.join("amplifier-bundle")).unwrap()
    );
}

#[test]
fn pin_uses_a_real_bundle_dir_as_given() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    write_bundle(&home.join("amplifier-bundle"));
    assert_eq!(pin_bundle(&home).unwrap(), home.join("amplifier-bundle"));
    assert!(home.join("amplifier-bundle/tools").is_dir());
}

#[test]
fn pin_rejects_link_outside_releases() {
    let (tmp, home, _release) = linked_home();
    let outside = tmp.path().join("elsewhere/amplifier-bundle");
    write_bundle(&outside);
    fs::write(outside.parent().unwrap().join(".complete"), "").unwrap();
    fs::remove_file(home.join("amplifier-bundle")).unwrap();
    symlink(&outside, home.join("amplifier-bundle")).unwrap();
    assert!(pin_bundle(&home).is_err());

    // A sibling whose name only starts with `releases`.
    let sibling = home.join("releases-evil/1.0.0/amplifier-bundle");
    write_bundle(&sibling);
    fs::write(sibling.parent().unwrap().join(".complete"), "").unwrap();
    fs::remove_file(home.join("amplifier-bundle")).unwrap();
    symlink(&sibling, home.join("amplifier-bundle")).unwrap();
    assert!(pin_bundle(&home).is_err());
}

#[test]
fn pin_rejects_an_incomplete_release() {
    let (_tmp, home, release) = linked_home();
    fs::remove_file(release.join(".complete")).unwrap();
    assert!(pin_bundle(&home).is_err());
}

#[test]
fn pin_rejects_group_writable_release() {
    let (_tmp, home, release) = linked_home();
    set_mode(&release, 0o775);
    let err = pin_bundle(&home).expect_err("a group-writable release root is refused (S2)");
    assert!(
        format!("{err:#}").contains(&release.display().to_string())
            || format!("{err:#}").contains("1.0.0"),
        "the error names the directory: {err:#}"
    );
    set_mode(&release, 0o755);
    set_mode(&home.join("releases"), 0o777);
    assert!(
        pin_bundle(&home).is_err(),
        "releases/ itself is checked too"
    );
}

#[test]
fn pin_rejects_a_group_writable_bundle_dir_in_the_release() {
    let (_tmp, home, release) = linked_home();
    let bundle = release.join("amplifier-bundle");
    set_mode(&bundle, 0o775);
    let err =
        pin_bundle(&home).expect_err("a group-writable <release>/amplifier-bundle is refused");
    assert!(
        format!("{err:#}").contains("amplifier-bundle"),
        "the error names the directory: {err:#}"
    );
    set_mode(&bundle, 0o755);
    assert!(pin_bundle(&home).is_ok());
}

#[test]
fn bundle_dir_env_is_not_read() {
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (tmp, home, release) = linked_home();
    let attacker = tmp.path().join("attacker/amplifier-bundle");
    write_bundle(&attacker);
    let saved = std::env::var_os("AMPLIHACK_BUNDLE_DIR");
    // SAFETY: edition 2024 requires unsafe; env_lock() is held.
    unsafe { std::env::set_var("AMPLIHACK_BUNDLE_DIR", &attacker) };
    let pinned = pin_bundle(&home);
    // SAFETY: as above.
    unsafe {
        match saved {
            Some(v) => std::env::set_var("AMPLIHACK_BUNDLE_DIR", v),
            None => std::env::remove_var("AMPLIHACK_BUNDLE_DIR"),
        }
    }
    assert_eq!(
        pinned.unwrap(),
        fs::canonicalize(release.join("amplifier-bundle")).unwrap()
    );
}
