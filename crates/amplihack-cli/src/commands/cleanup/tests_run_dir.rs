//! Unit tests for `run_dir.rs` (issue #1808, C2, D2, D4, R3).
//!
//! Wire with, at the end of `run_dir.rs`:
//! `#[cfg(test)] #[path = "tests_run_dir.rs"] mod tests;`
//!
//! Contract:
//! - `pub(crate) fn new_run_id() -> String`: a lowercase UUID v4;
//! - `pub(crate) fn runtime_dir_for(work_root: &Path, run_id: &str) -> PathBuf`:
//!   `<root>/runs/<id>/runtime`, or `<root>/r/<first 12 hex digits>` when
//!   `<that>/tmp` would be longer than 80 bytes;
//! - `pub(crate) struct RuntimeGuard` with `RuntimeGuard::new(path: PathBuf)`;
//!   dropping it (also during unwinding) removes the folder;
//! - `pub(crate) fn parse_start_ticks(stat: &str) -> Option<u64>`: field 22 of
//!   `/proc/<pid>/stat`, read from the fields after the last `)`;
//! - `pub(crate) enum OwnedKind { Worktree, Target }` and
//!   `pub(crate) struct OwnedPath { pub kind: OwnedKind, pub path: PathBuf,
//!   pub branch: Option<String> }`;
//! - `pub(crate) fn parse_owned_paths(bytes: &[u8]) -> Vec<OwnedPath>`: reads
//!   at most 10 000 lines, ignores malformed, relative, unknown-kind and
//!   control-character lines; an empty or missing branch is `None`.

use super::*;
use std::path::{Path, PathBuf};

#[test]
fn run_ids_are_unique_lowercase_uuid_v4() {
    let a = new_run_id();
    let b = new_run_id();
    assert_ne!(a, b);
    let parsed = uuid::Uuid::parse_str(&a).unwrap();
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(a, a.to_lowercase());
    assert_eq!(a.len(), 36);
}

#[test]
fn runtime_dir_uses_run_folder_when_short_enough() {
    let id = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
    let root = Path::new("/w");
    assert_eq!(
        runtime_dir_for(root, id),
        PathBuf::from(format!("/w/runs/{id}/runtime"))
    );
}

#[test]
fn runtime_dir_falls_back_to_short_form_over_80_bytes() {
    let id = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
    let root = PathBuf::from(format!("/home/{}/.amplihack/work", "u".repeat(30)));
    let dir = runtime_dir_for(&root, id);
    assert_eq!(dir, root.join("r/3f0c9a526d1e"));
    assert!(dir.join("tmp").as_os_str().len() <= 80);
}

#[test]
fn runtime_dir_boundary_is_80_bytes_of_runtime_tmp() {
    let id = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
    // "/runs/" + 36 + "/runtime/tmp" = 54 bytes after the root.
    let exact = PathBuf::from(format!("/{}", "a".repeat(80 - 54 - 1)));
    assert!(runtime_dir_for(&exact, id).starts_with(exact.join("runs")));
    let over = PathBuf::from(format!("/{}", "a".repeat(80 - 54)));
    assert!(runtime_dir_for(&over, id).starts_with(over.join("r")));
}

#[test]
fn runtime_guard_removes_folder_on_drop_and_on_panic() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("runtime");
    std::fs::create_dir_all(dir.join("tmp/nested")).unwrap();
    drop(RuntimeGuard::new(dir.clone()));
    assert!(!dir.exists());

    std::fs::create_dir_all(dir.join("tmp")).unwrap();
    let d = dir.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = RuntimeGuard::new(d);
        panic!("step blew up");
    });
    assert!(result.is_err());
    assert!(!dir.exists(), "removed during unwinding");
}

#[test]
fn start_ticks_parse_after_last_paren() {
    let fields_after = "S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20";
    let plain = format!("4242 (sleep) {fields_after}");
    assert_eq!(parse_start_ticks(&plain), Some(987654));
    let tricky = format!("4242 (a) b) (c d) {fields_after}");
    assert_eq!(parse_start_ticks(&tricky), Some(987654));
    assert_eq!(parse_start_ticks("4242 (x) S 1 2"), None);
    assert_eq!(parse_start_ticks("garbage"), None);
}

#[test]
fn owned_paths_parse_three_and_two_field_lines() {
    let text = b"worktree\t/w/worktrees/r-1a2b3c4d/feat/x\tfeat/x\n\
                 target\t/w/targets/t1\t\n\
                 target\t/w/targets/t2\n";
    let parsed = parse_owned_paths(text);
    assert_eq!(parsed.len(), 3);
    assert!(matches!(parsed[0].kind, OwnedKind::Worktree));
    assert_eq!(
        parsed[0].path,
        PathBuf::from("/w/worktrees/r-1a2b3c4d/feat/x")
    );
    assert_eq!(parsed[0].branch.as_deref(), Some("feat/x"));
    assert!(matches!(parsed[1].kind, OwnedKind::Target));
    assert_eq!(parsed[1].branch, None);
    assert_eq!(parsed[2].path, PathBuf::from("/w/targets/t2"));
    assert_eq!(parsed[2].branch, None);
}

#[test]
fn owned_paths_ignore_malformed_lines() {
    let text = b"bogus\t/w/x\t\n\
                 target\trelative/path\t\n\
                 target\n\
                 \n\
                 target\t/w/a\x07b\t\n\
                 worktree\t/w/ok\tok\n\
                 \xff\xfe\t/w/y\t\n";
    let parsed = parse_owned_paths(text);
    assert_eq!(parsed.len(), 1, "only the valid line survives");
    assert_eq!(parsed[0].path, PathBuf::from("/w/ok"));
}

#[test]
fn owned_paths_are_capped_at_10000_lines() {
    let mut text = Vec::new();
    for i in 0..10_050 {
        text.extend_from_slice(format!("target\t/w/targets/t{i}\t\n").as_bytes());
    }
    assert_eq!(parse_owned_paths(&text).len(), 10_000);
}
