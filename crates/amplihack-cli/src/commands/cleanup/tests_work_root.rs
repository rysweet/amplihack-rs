//! Unit tests for `work_root.rs` (issue #1808, C1, D7, D9, R4).
//!
//! Wire with, at the end of `work_root.rs`:
//! `#[cfg(test)] #[path = "tests_work_root.rs"] mod tests;`
//!
//! Contract (pure functions; no environment reads, so no env lock needed):
//! - `pub(crate) struct WorkRoot { pub path: PathBuf, pub explicit: bool }`
//! - `pub(crate) fn resolve(work_root_var: Option<&OsStr>, explicit_var: Option<&OsStr>,
//!   home: &Path) -> anyhow::Result<WorkRoot>`: `AMPLIHACK_WORK_ROOT` or
//!   `<home>/.amplihack/work`; `explicit` is the pass-through of
//!   `AMPLIHACK_WORK_ROOT_EXPLICIT` when that is `0` or `1`, otherwise whether
//!   `AMPLIHACK_WORK_ROOT` was set. It validates and never creates anything.
//! - `pub(crate) fn validate(path: &Path, home: &Path) -> anyhow::Result<()>`
//! - `impl WorkRoot { pub(crate) fn ensure_created(&self) -> anyhow::Result<()> }`
//!   creates the root with mode 0700 regardless of umask.

use super::*;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn home_in(tmp: &Path) -> std::path::PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    home
}

#[test]
fn default_is_home_amplihack_work_and_not_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let home = home_in(tmp.path());
    let root = resolve(None, None, &home).unwrap();
    assert_eq!(root.path, home.join(".amplihack/work"));
    assert!(!root.explicit);
    assert!(!root.path.exists(), "resolve never creates the root");
}

#[test]
fn set_variable_is_explicit_unless_flag_passes_through_0() {
    let tmp = tempfile::tempdir().unwrap();
    let home = home_in(tmp.path());
    let custom = tmp.path().join("custom");
    let var = Some(custom.as_os_str());

    let root = resolve(var, None, &home).unwrap();
    assert_eq!(root.path, custom);
    assert!(root.explicit);

    assert!(!resolve(var, Some(OsStr::new("0")), &home).unwrap().explicit);
    assert!(
        resolve(None, Some(OsStr::new("1")), &home)
            .unwrap()
            .explicit
    );
}

#[test]
fn unsafe_roots_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let home = home_in(tmp.path());
    let checkout = tmp.path().join("checkout");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    for bad in [
        Path::new("relative/root").to_path_buf(),
        Path::new("/").to_path_buf(),
        home.clone(),
        tmp.path().to_path_buf(),
        Path::new("/usr").to_path_buf(),
        Path::new("/etc").to_path_buf(),
        std::env::temp_dir(),
        checkout,
    ] {
        assert!(validate(&bad, &home).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn existing_root_must_not_be_group_or_world_writable() {
    let tmp = tempfile::tempdir().unwrap();
    let home = home_in(tmp.path());
    let root = tmp.path().join("work");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o770)).unwrap();
    let err = validate(&root, &home).unwrap_err().to_string();
    assert!(err.contains("writable by group or others"), "{err}");
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    validate(&root, &home).unwrap();
}

#[test]
fn missing_absolute_root_is_valid_and_created_0700() {
    let tmp = tempfile::tempdir().unwrap();
    let home = home_in(tmp.path());
    let path = tmp.path().join("new/work");
    validate(&path, &home).unwrap();
    let root = resolve(Some(path.as_os_str()), None, &home).unwrap();
    root.ensure_created().unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}
