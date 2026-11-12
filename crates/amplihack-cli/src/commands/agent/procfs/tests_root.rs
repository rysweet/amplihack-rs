//! Unit tests for `procfs/root.rs` (S27, issue #2042): which values of
//! `AMPLIHACK_TEST_PROC_ROOT` the seam honours, and the reason for each
//! refusal. `resolve(var, guard, temp_dir)` is the pure part of
//! `proc_root()`, which passes the variable, `AMPLIHACK_TEST_HOME_GUARD` and
//! `std::env::temp_dir()`, and falls back to `/proc` on any refusal.
//! Another user's directory (`WrongOwner`) needs a second account and is not
//! built here.

use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn the_proc_root_seam_needs_the_guard_a_temp_path_and_a_private_root() {
    let tmp = tempfile::tempdir().unwrap();
    let temp = tmp.path().canonicalize().unwrap();
    let inside = temp.join("proc");
    fs::create_dir(&inside).unwrap();
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o700)).unwrap();
    let g = Some(OsStr::new("1"));
    let seam = |var: &Path, guard, temp: &Path| resolve(Some(var.as_os_str()), guard, temp);
    assert_eq!(seam(&inside, g, &temp), Ok(Some(inside.clone())));
    assert_eq!(seam(&inside, None, &temp), Err(Refusal::NoGuard));
    let empty = Some(OsStr::new(""));
    assert_eq!(seam(&inside, empty, &temp), Err(Refusal::NoGuard));
    let elsewhere = temp.join("elsewhere");
    assert_eq!(seam(&inside, g, &elsewhere), Err(Refusal::OutsideTemp));
    assert_eq!(seam(&temp, g, &temp), Err(Refusal::OutsideTemp));
    assert_eq!(seam(&temp.join("missing"), g, &temp), Err(Refusal::Missing));
    assert_eq!(resolve(None, g, &temp), Ok(None));
    assert_eq!(
        resolve(empty, g, &temp),
        Ok(None),
        "an empty value is unset"
    );
    let file = temp.join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(seam(&file, g, &temp), Err(Refusal::NotADirectory));
    let escape = temp.join("link");
    std::os::unix::fs::symlink("/", &escape).unwrap();
    assert_eq!(
        seam(&escape, g, &temp),
        Err(Refusal::OutsideTemp),
        "canonical"
    );
    fs::set_permissions(&inside, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(seam(&inside, g, &temp), Err(Refusal::WritableByOthers));
}
