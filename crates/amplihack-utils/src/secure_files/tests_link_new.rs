//! Issue #1962, review fix: once `link_new` has linked the target, it reports
//! `Ok(true)`, even when the cleanup after the link fails.

use super::PrivateTemp;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn links_once_and_never_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let first = PrivateTemp::create(dir.path(), "rec.json").unwrap();
    assert!(first.link_new(b"one").unwrap());
    let second = PrivateTemp::create(dir.path(), "rec.json").unwrap();
    assert!(!second.link_new(b"two").unwrap());
    let target = dir.path().join("rec.json");
    assert_eq!(fs::read(&target).unwrap(), b"one");
    let mode = fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let left: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(left.len(), 1, "no temporary name is left behind");
}

#[test]
fn failed_cleanup_after_link_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = PrivateTemp::create(dir.path(), "rec.json").unwrap();
    t.fill(b"one", 0o600, true).unwrap();
    fs::hard_link(&t.tmp, &t.target).unwrap();
    // The temporary name is already gone, so its removal fails.
    fs::remove_file(&t.tmp).unwrap();
    t.after_link();
    assert!(
        !t.tmp.as_os_str().is_empty(),
        "a failed removal stays set for Drop to retry"
    );
    drop(t);
    assert_eq!(fs::read(dir.path().join("rec.json")).unwrap(), b"one");
}
