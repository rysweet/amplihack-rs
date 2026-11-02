//! Unit tests for `targets.rs` (issue #1808, C6, AC6).
//!
//! Wire with, at the end of `targets.rs`:
//! `#[cfg(test)] #[path = "tests_targets.rs"] mod tests;`
//!
//! Contract:
//! - `pub(crate) const CACHEDIR_TAG: &str` starts with the standard signature
//!   line and mentions amplihack;
//! - `pub(crate) fn create_tagged_target_dir(path: &Path) -> std::io::Result<()>`
//!   creates the folder (and parents) and writes `CACHEDIR.TAG`; calling it
//!   again is harmless and never truncates other content;
//! - `pub(crate) fn is_tagged(path: &Path) -> bool` is true only when
//!   `CACHEDIR.TAG` is a regular file starting with the signature.

use super::*;
use std::fs;

const SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

#[test]
fn tag_constant_starts_with_standard_signature() {
    assert!(CACHEDIR_TAG.starts_with(SIGNATURE));
    assert!(CACHEDIR_TAG.contains("amplihack"));
    assert!(CACHEDIR_TAG.ends_with('\n'));
}

#[test]
fn create_tagged_target_dir_writes_standard_signature() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("a/b/target");
    create_tagged_target_dir(&target).unwrap();
    let tag = fs::read_to_string(target.join("CACHEDIR.TAG")).unwrap();
    assert_eq!(tag, CACHEDIR_TAG);
    assert!(is_tagged(&target));
}

#[test]
fn create_tagged_target_dir_is_idempotent_and_keeps_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("target");
    create_tagged_target_dir(&target).unwrap();
    fs::write(target.join("keep"), "x").unwrap();
    create_tagged_target_dir(&target).unwrap();
    assert_eq!(fs::read_to_string(target.join("keep")).unwrap(), "x");
    assert!(is_tagged(&target));
}

#[test]
fn is_tagged_rejects_missing_wrong_or_symlinked_tags() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing");
    fs::create_dir(&missing).unwrap();
    assert!(!is_tagged(&missing));

    let wrong = tmp.path().join("wrong");
    fs::create_dir(&wrong).unwrap();
    fs::write(wrong.join("CACHEDIR.TAG"), "Signature: nope\n").unwrap();
    assert!(!is_tagged(&wrong));

    let real = tmp.path().join("real-tag");
    fs::write(&real, CACHEDIR_TAG).unwrap();
    let linked = tmp.path().join("linked");
    fs::create_dir(&linked).unwrap();
    std::os::unix::fs::symlink(&real, linked.join("CACHEDIR.TAG")).unwrap();
    assert!(!is_tagged(&linked), "a symlinked tag is not trusted");
}
