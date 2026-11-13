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

// ----- issue #2039: five-field target lines (D2) -------------------------------
//
// Contract:
// - `pub(crate) enum TargetOrigin { Created, Reused }` with
//   `as_str()` -> "created" / "reused";
// - `pub(crate) struct TargetRecord { pub origin: TargetOrigin,
//   pub checkout: PathBuf }`;
// - `OwnedPath` gains `pub target: Option<TargetRecord>`. `Some` only when
//   `kind == Target` and `branch == None`;
// - `parse_owned_paths` accepts 2 or 3 fields (legacy, `target: None`) and
//   exactly 5 fields `target<TAB><abs path><TAB><TAB><origin><TAB><abs checkout>`.
//   Every other shape is dropped, never repaired;
// - `format_owned_line` writes a parsed line back byte for byte.

fn one(line: &[u8]) -> Vec<OwnedPath> {
    parse_owned_paths(line)
}

#[test]
fn five_field_target_lines_parse_with_origin_and_checkout() {
    let text = b"target\t/w/targets/amplihack-precommit-target-0123456789ab\t\tcreated\t/w/worktrees/r-1/feat/x\n\
                 target\t/c/amplihack/amplihack-precommit-target-abcdef012345\t\treused\t/src/repo\n";
    let parsed = parse_owned_paths(text);
    assert_eq!(parsed.len(), 2, "{parsed:?}");

    assert_eq!(parsed[0].kind, OwnedKind::Target);
    assert_eq!(parsed[0].branch, None);
    let rec = parsed[0].target.as_ref().expect("record");
    assert_eq!(rec.origin, TargetOrigin::Created);
    assert_eq!(rec.checkout, PathBuf::from("/w/worktrees/r-1/feat/x"));

    let rec = parsed[1].target.as_ref().expect("record");
    assert_eq!(rec.origin, TargetOrigin::Reused);
    assert_eq!(rec.checkout, PathBuf::from("/src/repo"));
    assert_eq!(TargetOrigin::Created.as_str(), "created");
    assert_eq!(TargetOrigin::Reused.as_str(), "reused");
}

#[test]
fn legacy_lines_have_no_target_record() {
    let parsed = parse_owned_paths(
        b"target\t/w/targets/t1\t\ntarget\t/w/targets/t2\nworktree\t/w/wt/a\tfeat/a\n",
    );
    assert_eq!(parsed.len(), 3);
    assert!(parsed.iter().all(|o| o.target.is_none()), "{parsed:?}");
}

#[test]
fn every_accepted_shape_round_trips_byte_for_byte() {
    for line in [
        &b"target\t/w/targets/t1\t\n"[..],
        b"worktree\t/w/wt/a\tfeat/a\n",
        b"worktree\t/w/wt/b\t\n",
        b"target\t/w/targets/amplihack-precommit-target-0123456789ab\t\tcreated\t/w/wt/a\n",
        b"target\t/w/targets/amplihack-precommit-target-0123456789ab\t\treused\t/src/repo\n",
        "target\t/w/targets/t\u{e9}\t\treused\t/src/r\u{e9}po with space\n".as_bytes(),
    ] {
        let parsed = one(line);
        assert_eq!(parsed.len(), 1, "{:?}", String::from_utf8_lossy(line));
        assert_eq!(
            format_owned_line(&parsed[0]),
            line,
            "{:?}",
            String::from_utf8_lossy(line)
        );
    }
}

#[test]
fn two_field_legacy_line_is_kept_by_format() {
    // A 2-field line is written back as the equivalent 3-field form, which
    // the parser reads the same way.
    let parsed = one(b"target\t/w/targets/t2\n");
    let again = parse_owned_paths(&format_owned_line(&parsed[0]));
    assert_eq!(again, parsed);
}

#[test]
fn malformed_five_field_shapes_are_dropped() {
    for bad in [
        // 4 fields
        &b"target\t/w/targets/t\t\tcreated\n"[..],
        // 6 fields
        b"target\t/w/targets/t\t\tcreated\t/w/wt/a\textra\n",
        // unknown or empty origin
        b"target\t/w/targets/t\t\tadopted\t/w/wt/a\n",
        b"target\t/w/targets/t\t\tCreated\t/w/wt/a\n",
        b"target\t/w/targets/t\t\t\t/w/wt/a\n",
        // relative or empty checkout
        b"target\t/w/targets/t\t\tcreated\trelative/checkout\n",
        b"target\t/w/targets/t\t\tcreated\t\n",
        // relative target path
        b"target\trel/t\t\tcreated\t/w/wt/a\n",
        // worktree with five fields
        b"worktree\t/w/wt/a\t\tcreated\t/w/wt/a\n",
        // target with a branch and five fields
        b"target\t/w/targets/t\tfeat/x\tcreated\t/w/wt/a\n",
        // control characters in the checkout
        b"target\t/w/targets/t\t\tcreated\t/w/wt/a\x07b\n",
        b"target\t/w/targets/t\t\tcreated\t/w/wt/a\rb\n",
        // invalid UTF-8 in the origin
        b"target\t/w/targets/t\t\t\xff\t/w/wt/a\n",
    ] {
        assert!(
            one(bad).is_empty(),
            "must be dropped: {:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn a_dropped_line_does_not_hide_its_neighbours() {
    let text = b"target\t/w/targets/a\t\tcreated\n\
                 target\t/w/targets/b\t\tcreated\t/w/wt/b\n\
                 target\t/w/targets/c\t\n";
    let parsed = parse_owned_paths(text);
    let paths: Vec<_> = parsed.iter().map(|o| o.path.clone()).collect();
    assert_eq!(
        paths,
        vec![PathBuf::from("/w/targets/b"), PathBuf::from("/w/targets/c")]
    );
}

#[test]
fn five_field_lines_count_toward_the_10000_line_cap() {
    let mut text = Vec::new();
    for i in 0..10_050 {
        text.extend_from_slice(
            format!("target\t/w/targets/t{i}\t\treused\t/w/wt/{i}\n").as_bytes(),
        );
    }
    let parsed = parse_owned_paths(&text);
    assert_eq!(parsed.len(), 10_000);
    assert!(parsed.iter().all(|o| o.target.is_some()));
}

/// Issue #2528 (D10, T5b): the per-run bundle snapshot lives under the
/// runtime folder, so the guard removes it on unwind too; no cleanup process
/// is involved.
#[cfg(unix)]
#[test]
fn runtime_guard_removes_the_bundle_snapshot_on_unwind() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let runtime = tmp.path().join("runs/r/runtime");
    let snapshot = runtime.join("bundle/amplifier-bundle");
    std::fs::create_dir_all(snapshot.join("tools")).unwrap();
    std::fs::write(snapshot.join("tools/identity.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        snapshot.join("tools/identity.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();

    let guarded = runtime.clone();
    let unwound = std::panic::catch_unwind(move || {
        let _guard = RuntimeGuard::new(guarded);
        panic!("a step failed with a panic");
    });
    assert!(unwound.is_err());
    assert!(!snapshot.exists(), "the snapshot is gone after unwind");
    assert!(!runtime.exists());
    assert!(
        tmp.path().join("runs/r").is_dir(),
        "only runtime/ is removed"
    );
}
