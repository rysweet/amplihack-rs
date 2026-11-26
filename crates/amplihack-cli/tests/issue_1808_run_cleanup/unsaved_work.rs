//! Issue #2785: before cleanup removes a folder, it judges whether the folder
//! holds a Git work tree with uncommitted changes, or commits that are not on
//! its upstream. If it does, the folder is kept and one line on stderr says
//! so. Otherwise the folder is removed as before. Keeping a folder never ends
//! or fails a run, and never changes the exit status.
//!
//! The specification is the section "Folders with unsaved work are kept" of
//! `docs/howto/clean-up-after-recipe-runs.md`. These tests run the real
//! `amplihack` binary, because the message is written to its stderr and an
//! in-process `eprintln!` cannot be captured from a unit test. Every clone
//! comes from the fixture's bare origin, so each has a real upstream: a clean
//! clone is pushed, a clone with a local commit is not.
//!
//! The message is `amplihack: leaving "<folder>" in place: <reason>`, where
//! `<reason>` is `has uncommitted changes` or another reason when the work
//! tree is the folder itself, and `work tree "<path>": <reason>` when it is a
//! folder below it. The report's `reason` is the text after the colon.
//!
//! The children start with an empty environment (`isolated`), so `RUST_LOG`
//! is unset and a `tracing` record cannot add a line: every `amplihack:
//! leaving` line is the one the contract promises.

use super::fixture::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const UNCOMMITTED: &str = "has uncommitted changes";
const UNPUSHED: &str = "has unpushed commits";
const CANNOT_CHECK: &str = "cannot check for unpushed commits: ";

fn clone_origin(fx: &Fx, dest: &Path) {
    fx.git_ok(
        &fx.root,
        &[
            "clone",
            "-q",
            fx.origin.to_str().unwrap(),
            dest.to_str().unwrap(),
        ],
    );
}

/// A file that was never committed.
fn make_dirty(clone: &Path) {
    fs::write(clone.join("unsaved.txt"), "not committed\n").unwrap();
}

/// A commit that is on no remote.
fn make_unpushed(fx: &Fx, clone: &Path) {
    fx.git_ok(
        clone,
        &["commit", "-q", "--allow-empty", "-m", "local only"],
    );
}

/// A run that ended two days ago, with a clone of the fixture's origin at
/// `runtime/tmp/repo`. Returns `(runtime folder, clone)`.
fn run_with_clone(fx: &Fx) -> (PathBuf, PathBuf) {
    let mut run = SeedRun::ended(48);
    run.with_runtime = true;
    let runtime = fx.seed_run(&run).join("runtime");
    let clone = runtime.join("tmp/repo");
    clone_origin(fx, &clone);
    (runtime, clone)
}

fn below(worktree: &Path, reason: &str) -> String {
    format!("work tree {worktree:?}: {reason}")
}

fn leaving(folder: &Path, reason: &str) -> String {
    format!("amplihack: leaving {folder:?} in place: {reason}")
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every `amplihack: leaving` line.
fn leaving_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.starts_with("amplihack: leaving "))
        .collect()
}

/// The `amplihack: leaving` lines whose folder is `folder` (and not a folder
/// below it).
fn lines_for<'a>(stderr: &'a str, folder: &Path) -> Vec<&'a str> {
    let head = format!("amplihack: leaving {folder:?} in place:");
    leaving_lines(stderr)
        .into_iter()
        .filter(|l| l.starts_with(&head))
        .collect()
}

fn assert_one_line(stderr: &str, folder: &Path, reason: &str) {
    let lines = lines_for(stderr, folder);
    assert_eq!(
        lines,
        vec![leaving(folder, reason).as_str()],
        "exactly one line naming {folder:?}, and it says why\nstderr:\n{stderr}"
    );
}

fn actions(report: &serde_json::Value) -> Vec<String> {
    entries(report)
        .iter()
        .map(|e| e["action"].as_str().unwrap_or("?").to_string())
        .collect()
}

/// The acceptance criterion, in one pass: a folder with an unpushed commit and
/// one with an uncommitted file both survive cleanup with one message each; a
/// clean, pushed one is removed with none. Two more kinds of folder go through
/// the apply-time check alone, because they are not judged while planning: an
/// `untagged-target` and a `tmp-leftover`.
#[test]
fn unpushed_and_uncommitted_folders_survive_with_one_message_each_and_a_clean_one_is_removed() {
    let fx = Fx::new();
    let (a_runtime, a_clone) = run_with_clone(&fx);
    make_unpushed(&fx, &a_clone);
    let (b_runtime, b_clone) = run_with_clone(&fx);
    make_dirty(&b_clone);
    let (c_runtime, _c_clone) = run_with_clone(&fx);

    let d_target = fx.target("stray-work", 64);
    let d_clone = d_target.join("checkout");
    clone_origin(&fx, &d_clone);
    make_dirty(&d_clone);
    age_tree(&d_target, 72);

    let e_leftover = fx.tmp.join("replay-2785");
    clone_origin(&fx, &e_leftover);
    make_dirty(&e_leftover);
    age_tree(&e_leftover, 72);

    let out = fx.cleanup(&["--apply", "--format", "json"]);

    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", describe(&out)));
    let stderr = stderr_of(&out);

    // On disk: the work is where it was left.
    assert!(a_runtime.is_dir() && a_clone.join(".git").is_dir());
    assert!(b_runtime.is_dir() && b_clone.join("unsaved.txt").is_file());
    assert!(d_clone.join("unsaved.txt").is_file());
    assert!(e_leftover.join("unsaved.txt").is_file());
    assert!(!present(&c_runtime), "the clean folder is removed");

    // In the report: skipped, with the reason the line gives.
    let a = entry_for(&report, &a_runtime);
    assert_eq!(
        (a["category"].as_str(), a["action"].as_str()),
        (Some("run-tmp"), Some("skipped")),
        "{a}"
    );
    assert_eq!(a["reason"], below(&a_clone, UNPUSHED).as_str(), "{a}");
    let b = entry_for(&report, &b_runtime);
    assert_eq!(b["action"], "skipped", "{b}");
    assert_eq!(b["reason"], below(&b_clone, UNCOMMITTED).as_str(), "{b}");
    let c = entry_for(&report, &c_runtime);
    assert_eq!(c["action"], "removed", "{c}");
    let d = entry_for(&report, &d_target);
    assert_eq!(
        (d["category"].as_str(), d["action"].as_str()),
        (Some("untagged-target"), Some("skipped")),
        "{d}"
    );
    assert_eq!(d["reason"], below(&d_clone, UNCOMMITTED).as_str(), "{d}");
    let e = entry_for(&report, &e_leftover);
    assert_eq!(
        (e["category"].as_str(), e["action"].as_str()),
        (Some("tmp-leftover"), Some("skipped")),
        "{e}"
    );
    assert_eq!(e["reason"], UNCOMMITTED, "the work tree is the folder: {e}");
    assert!(
        !actions(&report).iter().any(|a| a == "failed"),
        "keeping a folder is never a failure: {report:#}"
    );

    // On stderr: one line for each kept folder, none for the removed one.
    assert_one_line(&stderr, &a_runtime, &below(&a_clone, UNPUSHED));
    assert_one_line(&stderr, &b_runtime, &below(&b_clone, UNCOMMITTED));
    assert_one_line(&stderr, &d_target, &below(&d_clone, UNCOMMITTED));
    assert_one_line(&stderr, &e_leftover, UNCOMMITTED);
    assert!(
        lines_for(&stderr, &c_runtime).is_empty(),
        "no line for a folder that was removed\nstderr:\n{stderr}"
    );
    assert_eq!(
        leaving_lines(&stderr).len(),
        4,
        "one line per kept folder and no other\nstderr:\n{stderr}"
    );
}

/// A report without `--apply` removes nothing, and shows what it can judge
/// while planning: a `run-tmp` with unsaved work is `skipped`, a clean one is
/// `would-remove`.
#[test]
fn a_report_without_apply_removes_nothing_and_lists_the_folder_with_unsaved_work_as_skipped() {
    let fx = Fx::new();
    let (kept_runtime, kept_clone) = run_with_clone(&fx);
    make_dirty(&kept_clone);
    let (clean_runtime, _) = run_with_clone(&fx);

    let report = fx.cleanup_json(&[]);

    let kept = entry_for(&report, &kept_runtime);
    assert_eq!(kept["action"], "skipped", "{kept}");
    assert_eq!(kept["reason"], below(&kept_clone, UNCOMMITTED).as_str());
    assert_eq!(entry_for(&report, &clean_runtime)["action"], "would-remove");
    assert!(kept_clone.join("unsaved.txt").is_file());
    assert!(clean_runtime.is_dir(), "a report never removes");
}

/// A `.git` that cannot be checked is kept, and a kept folder is a skip, not a
/// failure: the exit status is the same `0` it was when the folder was simply
/// removed. This is the case most likely to be mistaken for an error.
#[test]
fn a_folder_that_cannot_be_checked_is_kept_and_the_exit_status_stays_zero() {
    let fx = Fx::new();
    let mut run = SeedRun::ended(48);
    run.with_runtime = true;
    let runtime = fx.seed_run(&run).join("runtime");
    let broken = runtime.join("tmp/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join(".git"), "gitdir: /nonexistent\n").unwrap();

    let out = fx.cleanup(&["--apply", "--format", "json"]);

    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let e = entry_for(&report, &runtime);
    assert_eq!(e["action"], "skipped", "{e}");
    let reason = e["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(&format!("work tree {broken:?}: {CANNOT_CHECK}")),
        "{reason}"
    );
    assert!(
        !actions(&report).iter().any(|a| a == "failed"),
        "{report:#}"
    );
    assert!(broken.join(".git").is_file() && runtime.is_dir());
    let stderr = stderr_of(&out);
    assert_eq!(lines_for(&stderr, &runtime).len(), 1, "{stderr}");
    assert!(
        lines_for(&stderr, &runtime)[0].starts_with(&format!(
            "amplihack: leaving {runtime:?} in place: {}",
            below(&broken, CANNOT_CHECK)
        )),
        "{stderr}"
    );
}

/// A run folder under a work root that sits inside a repository has no `.git`
/// of its own, so it is judged by what is in it, not by the enclosing
/// repository, which is dirty and has an unpushed commit. Judged by the
/// enclosing repository, nothing under such a work root would ever be removed.
#[test]
fn a_run_folder_under_a_work_root_inside_a_dirty_repository_is_removed() {
    let mut fx = Fx::new();
    let outer = fx.root.join("enclosing");
    clone_origin(&fx, &outer);
    make_dirty(&outer);
    make_unpushed(&fx, &outer);
    fx.work = outer.join("work");
    mkdir_0700(&fx.work);
    let mut run = SeedRun::ended(48);
    run.with_runtime = true;
    let runtime = fx.seed_run(&run).join("runtime");

    let out = fx.cleanup(&["--apply", "--format", "json"]);

    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(
        !present(&runtime),
        "a folder with no .git of its own is not judged by the repository around it\n{}",
        describe(&out)
    );
    assert!(
        leaving_lines(&stderr_of(&out)).is_empty(),
        "{}",
        describe(&out)
    );
    assert!(
        outer.join("unsaved.txt").is_file(),
        "the repository is untouched"
    );
}

/// What a run's step leaves in its TMPDIR at the end of the run. A stub runner
/// clones the fixture's origin there and leaves a file that was never
/// committed.
fn runner_leaving_a_dirty_clone(fx: &Fx) -> String {
    format!(
        "#!/bin/sh\n{RECORD_ENV_SNIPPET}\n\
         git clone -q '{origin}' \"$TMPDIR/repo\" || exit 3\n\
         printf 'unsaved\\n' > \"$TMPDIR/repo/unsaved.txt\"\n\
         {RUNNER_SUCCESS_JSON}\n",
        origin = fx.origin.display()
    )
}

/// `(runtime folder, clone)` of the run that just ended, from what its step
/// recorded.
fn runtime_and_clone(fx: &Fx) -> (PathBuf, PathBuf) {
    let env = fx.recorded_env();
    let runtime = expected_runtime_dir(&fx.work, &env["AMPLIHACK_RUN_ID"]);
    let tmpdir = PathBuf::from(&env["TMPDIR"]);
    assert_eq!(
        tmpdir,
        runtime.join("tmp"),
        "the step's TMPDIR is the runtime's"
    );
    (runtime, tmpdir.join("repo"))
}

/// The run's own removal of `runtime/` at the end of a run: it keeps the
/// folder, says so once, and the run ends as it would have. With the sweep
/// switched off, that is the only pass there is.
#[test]
fn the_runners_own_removal_keeps_a_runtime_with_unsaved_work_and_the_run_still_succeeds() {
    let fx = Fx::new();

    let out = fx.recipe_run(&runner_leaving_a_dirty_clone(&fx), |c| {
        c.env("AMPLIHACK_CLEANUP_ON_EXIT", "0");
    });

    assert!(out.status.success(), "{}", describe(&out));
    let (runtime, clone) = runtime_and_clone(&fx);
    assert!(clone.join("unsaved.txt").is_file(), "{}", describe(&out));
    let stderr = stderr_of(&out);
    assert_one_line(&stderr, &runtime, &below(&clone, UNCOMMITTED));
    assert_eq!(leaving_lines(&stderr).len(), 1, "{stderr}");
    let run_dir = fx.run_dirs().into_iter().next().expect("the run folder");
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(
        report["outcome"], "success",
        "keeping does not change it: {report:#}"
    );
    assert!(
        report["ended_at"].as_str().is_some_and(|s| !s.is_empty()),
        "{report:#}"
    );
}

/// The way a person gets the folder back, as the how-to describes it: save the
/// work, run cleanup again, and the folder is judged afresh and removed.
///
/// With the sweep on, a run's end has two passes that can reach the same
/// folder, the runner's own removal and then the sweep; each says so once, so
/// there are one or two lines, and they are the same line.
#[test]
fn saving_the_work_lets_the_next_cleanup_remove_the_kept_runtime() {
    let fx = Fx::new();
    let out = fx.recipe_run(&runner_leaving_a_dirty_clone(&fx), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    let (runtime, clone) = runtime_and_clone(&fx);
    let reason = below(&clone, UNCOMMITTED);
    let stderr = stderr_of(&out);
    let lines = lines_for(&stderr, &runtime);
    assert!(
        (1..=2).contains(&lines.len()),
        "one line per pass, and at most two passes: {stderr}"
    );
    assert!(
        lines.iter().all(|l| *l == leaving(&runtime, &reason)),
        "{stderr}"
    );
    assert!(clone.join("unsaved.txt").is_file());

    // Still unsaved: another pass keeps it and says so once.
    let again = fx.cleanup(&["--apply", "--only", "run-tmp", "--format", "json"]);
    assert_eq!(again.status.code(), Some(0), "{}", describe(&again));
    assert_one_line(&stderr_of(&again), &runtime, &reason);
    assert!(clone.join("unsaved.txt").is_file());

    // Save it, as the how-to says; the next pass removes the folder.
    fx.git_ok(&clone, &["add", "-A"]);
    fx.git_ok(
        &clone,
        &["commit", "-q", "-m", "save the work from the run"],
    );
    fx.git_ok(&clone, &["push", "-q"]);
    let report = fx.cleanup_json(&["--apply", "--only", "run-tmp"]);
    assert_eq!(
        entry_for(&report, &runtime)["action"],
        "removed",
        "{report:#}"
    );
    assert!(!present(&runtime));
}
