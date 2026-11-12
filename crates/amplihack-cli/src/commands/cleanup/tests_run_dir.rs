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

// ----- issue #2291 (D8): the optional `runtime_dir` field -------------------

fn minimal_report_json() -> &'static str {
    r#"{"schema_version":1,"run_id":"3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3"}"#
}

#[test]
fn report_without_runtime_dir_reads_as_none() {
    let report: RunReport = serde_json::from_str(minimal_report_json()).unwrap();
    assert_eq!(report.runtime_dir, None);
    assert_eq!(report.schema_version, RUN_SCHEMA_VERSION);
}

#[test]
fn runtime_dir_none_is_not_written() {
    let report: RunReport = serde_json::from_str(minimal_report_json()).unwrap();
    let json = serde_json::to_value(&report).unwrap();
    assert!(json.get("runtime_dir").is_none(), "{json:#}");
}

#[test]
fn runtime_dir_round_trips() {
    let mut report: RunReport = serde_json::from_str(minimal_report_json()).unwrap();
    report.runtime_dir = Some("/scratch/ahr-3f0c9a526d1e".into());
    let json = serde_json::to_string(&report).unwrap();
    let back: RunReport = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.runtime_dir.as_deref(),
        Some("/scratch/ahr-3f0c9a526d1e")
    );
}

#[test]
fn schema_version_stays_1() {
    assert_eq!(RUN_SCHEMA_VERSION, 1);
}

#[test]
fn runtime_tmp_limit_is_shared_and_80_bytes() {
    assert_eq!(MAX_RUNTIME_TMP_BYTES, 80);
}

/// Issue #2291: `begin_with` takes explicit inputs, so a run can be started
/// under an external root without touching the process environment. The
/// root is recorded in `run.json` and the per-run folder is removed when the
/// guard drops.
#[test]
fn begin_with_explicit_root_records_and_removes_the_external_folder() {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let ext = base.join("ext");
    let work = WorkRoot {
        path: base.join("work"),
        explicit: false,
    };
    let inputs = TmpRootInputs {
        run_tmp_root: Some(ext.clone().into_os_string()),
        // SAFETY: geteuid has no preconditions and cannot fail.
        euid: unsafe { libc::geteuid() },
        ..TmpRootInputs::default()
    };

    let (scope, guard) = RunScope::begin_with(work, "test", &base, &inputs).unwrap();

    assert_eq!(scope.runtime_root.as_deref(), Some(ext.as_path()));
    let runtime = guard.path().to_path_buf();
    assert_eq!(runtime.parent(), Some(ext.as_path()));
    assert!(runtime.join("tmp").is_dir());
    let report = RunReport::read(&scope.run_dir).unwrap();
    assert_eq!(
        report.runtime_dir.as_deref(),
        Some(runtime.to_str().unwrap())
    );

    drop(guard);
    assert!(!runtime.exists(), "the guard removes the external folder");
    assert!(ext.is_dir(), "the root itself is kept");
}

/// Issue #2291: a dry run under an explicit root at the 59-byte budget still
/// gets a `tmp/` within 80 bytes, because `ahd-<6 random>` is no longer than
/// `ahr-<12 hex>`. The folder is removed when the workspace drops.
#[test]
fn dry_run_under_a_root_at_the_budget_stays_within_80_bytes() {
    let tmp = tempfile::tempdir_in("/tmp").unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let pad = 59 - base.as_os_str().len() - 1;
    let root = base.join("r".repeat(pad));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(root.as_os_str().len(), 59);
    let inputs = TmpRootInputs {
        run_tmp_root: Some(root.clone().into_os_string()),
        // SAFETY: geteuid has no preconditions and cannot fail.
        euid: unsafe { libc::geteuid() },
        ..TmpRootInputs::default()
    };

    let ws = RunWorkspace::dry_runtime_with(&inputs).unwrap();

    let dir = ws.runtime_dir().to_path_buf();
    assert_eq!(dir.parent(), Some(root.as_path()));
    let name = dir.file_name().unwrap().to_str().unwrap();
    assert!(name.starts_with("ahd-"), "{name}");
    assert_eq!(name.len(), "ahd-".len() + 6, "{name}");
    assert!(dir.join("tmp").is_dir());
    assert!(dir.join("tmp").as_os_str().len() <= 80, "{dir:?}");
    drop(ws);
    assert!(!dir.exists(), "the dry-run folder is removed on drop");
    assert!(root.is_dir(), "the root itself is kept");
}
