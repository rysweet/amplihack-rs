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

// ----- issue #2486: a runtime folder in use is kept; tmp/ is re-created --------
//
// Contract:
// - `pub(crate) enum RuntimeRemoval { Removed, KeptInUse, Failed }`
//   (`Debug`, `PartialEq`); `RuntimeGuard::remove(self) -> RuntimeRemoval`.
//   A missing folder counts as `Removed`.
// - The guard takes one fresh `ProcScan` from `procfs::proc_root()` and keeps
//   the folder when the scan is available and a process has its working
//   folder, an open file or TMPDIR/TMP/TEMP under it. `Drop` does the same.
// - `RunWorkspace::prepare_attempt(&self) -> anyhow::Result<()>` re-creates a
//   missing `<runtime>/tmp` and `<runtime>/artifacts` (mode 0700), accepts a
//   real folder as it is, and refuses a symlink or a file there with an error
//   naming the path and "refusing to launch the step".
//
// Lib tests never read the host `/proc`: each test installs its own fake
// table and links in only the children it started (issue #2196). Children
// are reaped with `wait()`; nothing sleeps or polls.

use crate::test_support::proc::fake_proc_scope;
use std::io::BufRead;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};

/// A child the test started; killed and reaped if the test fails first.
struct Holder(Child);

impl Holder {
    /// Closes the child's stdin and waits for it to exit.
    fn finish(mut self) {
        drop(self.0.stdin.take());
        let status = self.0.wait().expect("wait for holder");
        assert!(status.success(), "holder exited with {status}");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn runtime_with_tmp() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let rt = tmp.path().join("runtime");
    std::fs::create_dir_all(rt.join("tmp")).unwrap();
    std::fs::create_dir_all(rt.join("artifacts")).unwrap();
    std::fs::write(rt.join("tmp/scratch"), "x").unwrap();
    (tmp, rt)
}

/// A shell with piped stdin: it runs until the test closes stdin. `spawn`
/// can return before the kernel has filled in the new program's
/// environment (`/proc/<pid>/environ` reads empty for a moment after a vfork
/// spawn), so the child prints `ready` once it runs and the test waits for
/// that line. It then reads stdin to EOF without another `exec`, so its
/// environment stays readable.
fn cat_holder(cwd: &Path, tmpdir: &Path) -> Holder {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("echo ready; while read -r _; do :; done")
        .current_dir(cwd)
        .env("TMPDIR", tmpdir)
        .env("TMP", tmpdir)
        .env("TEMP", tmpdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sh");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    Holder(child)
}

#[test]
fn runtime_guard_keeps_folder_held_by_child_then_removes_after_exit() {
    let procs = fake_proc_scope();
    let (_tmp, rt) = runtime_with_tmp();
    let holder = cat_holder(&rt.join("tmp"), &rt.join("tmp"));
    procs.fake.link_own(holder.0.id());

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::KeptInUse
    );
    assert!(
        rt.join("tmp/scratch").is_file(),
        "a folder in use is left in place, contents and all"
    );

    holder.finish();
    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists(), "removed once the holder has exited");
}

#[test]
fn runtime_guard_drop_also_keeps_a_folder_in_use() {
    let procs = fake_proc_scope();
    let (_tmp, rt) = runtime_with_tmp();
    let holder = cat_holder(&rt.join("tmp"), &rt.join("tmp"));
    procs.fake.link_own(holder.0.id());

    drop(RuntimeGuard::new(rt.clone()));
    assert!(rt.join("tmp").is_dir(), "Drop must check too");
    holder.finish();
}

/// A child whose only link to the folder is TMPDIR in its environment (cwd
/// elsewhere, nothing open) still keeps it: a killed run's child may have
/// nothing open between writes.
#[test]
fn runtime_guard_keeps_folder_named_only_by_a_tmpdir_variable() {
    let procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let holder = cat_holder(tmp.path(), &rt.join("tmp"));
    procs.fake.link_own(holder.0.id());

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::KeptInUse
    );
    assert!(rt.is_dir());
    holder.finish();
}

/// An open file under the folder keeps it. The child says `ready` after it
/// opened the file, so the test waits on that line, not on time.
#[test]
fn runtime_guard_keeps_folder_with_a_file_open_in_it() {
    let procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let mut child = Command::new("sh")
        .arg("-c")
        .arg("exec 3<\"$1\"; echo ready; exec cat")
        .arg("sh")
        .arg(rt.join("tmp/scratch"))
        .current_dir(tmp.path())
        .env_remove("TMPDIR")
        .env_remove("TMP")
        .env_remove("TEMP")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn sh");
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let holder = Holder(child);
    procs.fake.link_own(holder.0.id());

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::KeptInUse
    );
    assert!(rt.join("tmp/scratch").is_file());
    holder.finish();
}

/// A process in a sibling whose name only starts with the folder's name
/// (`runtime-sib`) does not keep `runtime`.
#[test]
fn runtime_guard_ignores_a_sibling_with_a_longer_name() {
    let procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let sibling = tmp.path().join("runtime-sib");
    std::fs::create_dir_all(&sibling).unwrap();
    let holder = cat_holder(&sibling, &sibling);
    procs.fake.link_own(holder.0.id());

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists());
    assert!(sibling.is_dir(), "the sibling is never touched");
    holder.finish();
}

#[test]
fn runtime_guard_with_no_users_removes_and_missing_counts_as_removed() {
    let _procs = fake_proc_scope();
    let (_tmp, rt) = runtime_with_tmp();
    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists());
    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed,
        "a folder that is already gone is not a failure"
    );
}

/// Without a readable process table (as on macOS) the guard cannot prove a
/// folder is in use, so it removes it as before. `in_use` alone would say
/// "in use" here and keep every runtime folder for good.
#[test]
fn runtime_guard_without_a_process_table_removes_as_before() {
    let procs = fake_proc_scope();
    std::fs::remove_dir_all(procs.path()).unwrap();
    let (_tmp, rt) = runtime_with_tmp();
    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists());
}

fn scoped_workspace(base: &Path) -> RunWorkspace {
    let work = WorkRoot {
        path: base.join("work"),
        explicit: true,
    };
    let (scope, guard) = RunScope::begin(work, "issue-2486", base).unwrap();
    RunWorkspace::Scoped {
        scope: Box::new(scope),
        guard,
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn prepare_attempt_recreates_missing_tmp_and_artifacts() {
    let _procs = fake_proc_scope();
    let base = tempfile::tempdir().unwrap();
    let ws = scoped_workspace(base.path());
    let rt = ws.runtime_dir().to_path_buf();
    std::fs::remove_dir_all(rt.join("tmp")).unwrap();
    std::fs::remove_dir_all(rt.join("artifacts")).unwrap();

    ws.prepare_attempt().expect("prepare_attempt");
    for name in ["tmp", "artifacts"] {
        assert!(rt.join(name).is_dir(), "{name} re-created");
        assert_eq!(mode(&rt.join(name)), 0o700, "{name} mode");
    }
    // Same path every attempt: the env still names it.
    let env = ws.env();
    let tmpdir = env.iter().find(|(k, _)| *k == "TMPDIR").unwrap();
    assert_eq!(PathBuf::from(&tmpdir.1), rt.join("tmp"));
}

/// The whole runtime folder gone (another sweep or a stray `rm -rf`): the
/// next attempt still gets an existing TMPDIR at the same path.
#[test]
fn prepare_attempt_recreates_a_runtime_folder_removed_entirely() {
    let _procs = fake_proc_scope();
    let base = tempfile::tempdir().unwrap();
    let ws = scoped_workspace(base.path());
    let rt = ws.runtime_dir().to_path_buf();
    std::fs::remove_dir_all(&rt).unwrap();

    ws.prepare_attempt().expect("prepare_attempt");
    assert!(rt.join("tmp").is_dir());
    assert!(rt.join("artifacts").is_dir());
}

#[test]
fn prepare_attempt_keeps_an_existing_tmp_and_its_contents() {
    let _procs = fake_proc_scope();
    let base = tempfile::tempdir().unwrap();
    let ws = scoped_workspace(base.path());
    let rt = ws.runtime_dir().to_path_buf();
    std::fs::write(rt.join("tmp/from-attempt-1"), "kept").unwrap();

    ws.prepare_attempt().expect("prepare_attempt");
    assert_eq!(
        std::fs::read_to_string(rt.join("tmp/from-attempt-1")).unwrap(),
        "kept",
        "a retry reuses its scratch space"
    );
}

#[test]
fn prepare_attempt_refuses_symlinked_tmp_and_leaves_the_target_alone() {
    let _procs = fake_proc_scope();
    let base = tempfile::tempdir().unwrap();
    let ws = scoped_workspace(base.path());
    let rt = ws.runtime_dir().to_path_buf();
    let elsewhere = base.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("sentinel"), "s").unwrap();
    std::fs::remove_dir_all(rt.join("tmp")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, rt.join("tmp")).unwrap();

    let err = format!("{:#}", ws.prepare_attempt().expect_err("must refuse"));
    assert!(
        err.contains(&format!("{:?}", rt.join("tmp"))),
        "names the path: {err}"
    );
    assert!(err.contains("refusing to launch the step"), "{err}");
    assert!(
        std::fs::symlink_metadata(rt.join("tmp"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(elsewhere.join("sentinel").is_file(), "target untouched");
}

#[test]
fn prepare_attempt_refuses_artifacts_that_is_a_file() {
    let _procs = fake_proc_scope();
    let base = tempfile::tempdir().unwrap();
    let ws = scoped_workspace(base.path());
    let rt = ws.runtime_dir().to_path_buf();
    std::fs::remove_dir_all(rt.join("artifacts")).unwrap();
    std::fs::write(rt.join("artifacts"), "not a folder").unwrap();

    let err = format!("{:#}", ws.prepare_attempt().expect_err("must refuse"));
    assert!(
        err.contains(&format!("{:?}", rt.join("artifacts"))),
        "{err}"
    );
    assert!(err.contains("refusing to launch the step"), "{err}");
    assert_eq!(
        std::fs::read_to_string(rt.join("artifacts")).unwrap(),
        "not a folder"
    );
}

#[test]
fn prepare_attempt_recreates_tmp_in_a_dry_run_workspace() {
    let ws = RunWorkspace::dry_runtime().expect("dry runtime");
    let rt = ws.runtime_dir().to_path_buf();
    std::fs::remove_dir_all(rt.join("tmp")).unwrap();
    ws.prepare_attempt().expect("prepare_attempt");
    assert!(rt.join("tmp").is_dir());
}

// ----- issue #2486 (13 Nov): locked folders inside the runtime folder ----------
//
// Contract:
// - Before `remove_dir_all`, and only after the process scan found no user,
//   `RuntimeGuard` adds the owner's `rwx` bits to every folder the run owns
//   (lstat uid == euid, same `dev()` as the root). No other bit is added or
//   removed. Files are never chmodded. Symlinks are neither followed nor
//   chmodded.
// - Entries the run does not own are left unchanged, reported in a warning,
//   and do not fail the run: no new `RuntimeRemoval` variant.
// - `fn restore_owner_access(root: &Path, uid: u32) -> Vec<PathBuf>` (private,
//   in `run_dir.rs`) does the walk and returns the paths it skipped because
//   they are not owned by `uid` or are on another filesystem.
//
// Nothing here needs root: the not-owned case passes a uid other than the
// test's own. Under root the bug cannot be reproduced (root ignores modes),
// so the removal tests print a note and still assert.

fn test_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn note_if_root(test: &str) {
    if test_euid() == 0 {
        eprintln!(
            "{test}: running as root, which ignores folder modes; this run does not exercise #2486"
        );
    }
}

fn set_mode(path: &Path, bits: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap();
}

/// Puts every real folder under the given roots back to 0700 when dropped,
/// so a failed test leaves nothing `TempDir` cannot delete. lstat only;
/// symlinks are skipped; the chmod comes before `read_dir`; errors ignored.
struct RestoreModes(Vec<PathBuf>);

impl Drop for RestoreModes {
    fn drop(&mut self) {
        let mut stack = self.0.clone();
        while let Some(dir) = stack.pop() {
            let Ok(meta) = std::fs::symlink_metadata(&dir) else {
                continue;
            };
            if !meta.file_type().is_dir() {
                continue;
            }
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            if let Ok(entries) = std::fs::read_dir(&dir) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        }
    }
}

/// `tmp/locked000/inner/file` with both folders at 000, and `tmp/ro0500/file`
/// with `ro0500` at 0500, as fixtures from #2574 leave them.
fn lock_fixtures(rt: &Path) {
    let locked = rt.join("tmp/locked000");
    std::fs::create_dir_all(locked.join("inner")).unwrap();
    std::fs::write(locked.join("inner/file"), "x").unwrap();
    let ro = rt.join("tmp/ro0500");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::write(ro.join("file"), "x").unwrap();
    set_mode(&locked.join("inner"), 0o000);
    set_mode(&locked, 0o000);
    set_mode(&ro, 0o500);
}

/// The 13 Nov case: e1044c618 returns `Failed` here with "Permission denied
/// (os error 13)" and leaves the runtime folder behind.
#[test]
fn removes_runtime_with_mode_000_and_0500_folders() {
    note_if_root("removes_runtime_with_mode_000_and_0500_folders");
    let _procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    lock_fixtures(&rt);

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(
        !rt.exists(),
        "the runtime folder and the locked folders are gone"
    );
}

/// `Drop` takes the same path as `remove()`.
#[test]
fn runtime_guard_drop_removes_locked_folders_too() {
    note_if_root("runtime_guard_drop_removes_locked_folders_too");
    let _procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    lock_fixtures(&rt);

    drop(RuntimeGuard::new(rt.clone()));
    assert!(!rt.exists());
}

/// A runtime folder itself left at 0500 is restored and removed as well (D2).
#[test]
fn removes_runtime_whose_root_is_0500() {
    note_if_root("removes_runtime_whose_root_is_0500");
    let _procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    set_mode(&rt, 0o500);

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists());
}

/// A folder still in use is not touched at all: the modes are restored only
/// after the scan finds no user (D8).
#[test]
fn a_folder_in_use_keeps_its_locked_modes() {
    let procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    let ro = rt.join("tmp/ro0500");
    std::fs::create_dir_all(&ro).unwrap();
    set_mode(&ro, 0o500);
    let holder = cat_holder(&rt.join("tmp"), &rt.join("tmp"));
    procs.fake.link_own(holder.0.id());

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::KeptInUse
    );
    assert_eq!(mode(&ro), 0o500, "an in-use folder is never changed");
    holder.finish();
}

/// A symlink inside the runtime folder that points at a 0500 folder outside
/// it: the link is removed, the target is neither entered nor chmodded.
#[test]
fn restore_does_not_follow_symlinks() {
    let _procs = fake_proc_scope();
    let (tmp, rt) = runtime_with_tmp();
    let target = tmp.path().join("outside/target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("sentinel"), "s").unwrap();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    set_mode(&target, 0o500);
    std::os::unix::fs::symlink(&target, rt.join("tmp/link")).unwrap();

    assert_eq!(
        RuntimeGuard::new(rt.clone()).remove(),
        RuntimeRemoval::Removed
    );
    assert!(!rt.exists());
    assert_eq!(mode(&target), 0o500, "the link's target keeps its mode");
    assert!(
        target.join("sentinel").is_file(),
        "the target is not emptied"
    );
}

/// Entries the run does not own are reported and left unchanged. Passing a
/// uid other than the test's own makes the root itself "not owned", so the
/// walk reports it and changes nothing below it (D2, D3).
#[test]
fn restore_skips_entries_not_owned() {
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    let ro = rt.join("tmp/ro0500");
    std::fs::create_dir_all(&ro).unwrap();
    set_mode(&ro, 0o500);
    let root_mode = mode(&rt);

    let skipped = restore_owner_access(&rt, test_euid().wrapping_add(1));

    assert_eq!(skipped, vec![rt.clone()], "only the root is reported");
    assert_eq!(mode(&ro), 0o500, "nothing below a not-owned root changes");
    assert_eq!(mode(&rt), root_mode, "the not-owned root keeps its mode");
}

/// Only the owner's bits are added; group and other bits stay as they were;
/// files keep their mode; folders that already have u+rwx are left as they
/// are; the walk reaches folders below a 000 folder (chmod before read_dir).
#[test]
fn restore_adds_only_owner_bits() {
    let (tmp, rt) = runtime_with_tmp();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    let ro = rt.join("tmp/ro0500");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::write(ro.join("file"), "x").unwrap();
    set_mode(&ro.join("file"), 0o400);
    set_mode(&ro, 0o500);
    let grp = rt.join("tmp/g0050");
    std::fs::create_dir_all(&grp).unwrap();
    set_mode(&grp, 0o050);
    let deep = rt.join("tmp/a000/b000");
    std::fs::create_dir_all(&deep).unwrap();
    set_mode(&deep, 0o000);
    set_mode(&rt.join("tmp/a000"), 0o000);
    let open = rt.join("tmp/open0755");
    std::fs::create_dir_all(&open).unwrap();
    set_mode(&open, 0o755);

    let skipped = restore_owner_access(&rt, test_euid());

    assert!(skipped.is_empty(), "everything is owned: {skipped:?}");
    assert_eq!(mode(&ro), 0o700);
    assert_eq!(mode(&ro.join("file")), 0o400, "files are never chmodded");
    assert_eq!(mode(&grp), 0o750, "group bits kept, owner bits added");
    assert_eq!(mode(&rt.join("tmp/a000")), 0o700);
    assert_eq!(mode(&deep), 0o700, "reached below a 000 folder");
    assert_eq!(mode(&open), 0o755, "no bit is removed");
}

/// The walk never follows a symlink, even one pointing at a folder the run
/// owns, and never reports one as not owned.
#[test]
fn restore_neither_enters_nor_reports_symlinks() {
    let (tmp, rt) = runtime_with_tmp();
    let target = tmp.path().join("outside/target");
    std::fs::create_dir_all(target.join("sub")).unwrap();
    let _restore = RestoreModes(vec![tmp.path().to_path_buf()]);
    set_mode(&target.join("sub"), 0o500);
    set_mode(&target, 0o500);
    std::os::unix::fs::symlink(&target, rt.join("tmp/link")).unwrap();

    let skipped = restore_owner_access(&rt, test_euid());

    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(mode(&target), 0o500);
    assert_eq!(mode(&target.join("sub")), 0o500, "not entered");
}

/// A missing root is not an error and reports nothing.
#[test]
fn restore_on_a_missing_root_reports_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(restore_owner_access(&tmp.path().join("gone"), test_euid()).is_empty());
}
