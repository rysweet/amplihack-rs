//! `amplihack cleanup` over run-owned items: eligibility, keep period,
//! idempotence and the safety rules (R6, R7, D6, D9; AC2, AC3, AC5, AC10).

use super::fixture::*;
use std::fs;

/// A run whose PR merged `hours` ago and that owns one worktree and one target.
fn merged_run(
    fx: &Fx,
    branch: &str,
    hours: i64,
) -> (SeedRun, std::path::PathBuf, std::path::PathBuf) {
    let wt = fx.worktree(branch);
    let tgt = fx.target(
        &format!("amplihack-precommit-target-0123456789ab-run-{branch}"),
        4096,
    );
    fx.gh_closed(branch, 1820, hours, true);
    let run = SeedRun::ended(hours)
        .owns_worktree(&wt, branch)
        .owns_target(&tgt);
    fx.seed_run(&run);
    (run, wt, tgt)
}

fn reason(entry: &serde_json::Value) -> String {
    entry["reason"].as_str().unwrap_or_default().to_string()
}

/// AC10: closed one hour ago with the default keep period is skipped with
/// `within keep period`; closed 25 hours ago is removed.
#[test]
fn ac10_keep_period_is_measured_from_pr_closed_at() {
    let fx = Fx::new();
    let (_, young_wt, young_tgt) = merged_run(&fx, "young", 1);
    let (old, old_wt, old_tgt) = merged_run(&fx, "old", 25);

    let report = fx.cleanup_json(&["--apply"]);

    let young = entry_for(&report, &young_wt);
    assert_eq!(young["action"], "skipped");
    assert!(reason(&young).contains("within keep period"), "{young}");
    assert!(young_wt.exists() && young_tgt.exists());

    for (path, category) in [(&old_wt, "run-worktree"), (&old_tgt, "run-target")] {
        let e = entry_for(&report, path);
        assert_eq!(e["action"], "removed", "{e}");
        assert_eq!(e["category"], category);
        assert_eq!(e["run_id"], old.id.as_str());
        assert!(e["size_bytes"].as_u64().is_some_and(|n| n > 0));
        assert!(!path.exists());
    }
    assert_eq!(
        fx.git_ok(&fx.repo, &["branch", "--list", "old"]).trim(),
        "old",
        "branch kept"
    );
}

/// `--keep-hours` overrides `AMPLIHACK_CLEANUP_KEEP_HOURS`, which overrides
/// the default.
#[test]
fn keep_hours_flag_overrides_env_which_overrides_default() {
    let fx = Fx::new();
    let (_, wt, _) = merged_run(&fx, "mid", 5);

    let env_only = fx.cleanup_with(&["--format", "json"], |c| {
        c.env("AMPLIHACK_CLEANUP_KEEP_HOURS", "2");
    });
    let report: serde_json::Value = serde_json::from_slice(&env_only.stdout).unwrap();
    assert_eq!(
        entry_for(&report, &wt)["action"],
        "would-remove",
        "env 2h < 5h"
    );

    let flag = fx.cleanup_with(&["--format", "json", "--keep-hours", "10"], |c| {
        c.env("AMPLIHACK_CLEANUP_KEEP_HOURS", "2");
    });
    let report: serde_json::Value = serde_json::from_slice(&flag.stdout).unwrap();
    assert_eq!(
        entry_for(&report, &wt)["action"],
        "skipped",
        "flag 10h > 5h"
    );
}

/// AC5: an open PR, a `gh` failure, a missing `gh` and a worktree that no
/// run recorded are each reported as skipped, and nothing is removed.
#[test]
fn ac5_open_pr_gh_failure_and_unrecorded_worktree_are_skipped() {
    let fx = Fx::new();
    let open_wt = fx.worktree("open-pr");
    fx.gh_open("open-pr", 1831);
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&open_wt, "open-pr"));

    let fail_wt = fx.worktree("gh-fails");
    fx.gh_fail("gh-fails");
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&fail_wt, "gh-fails"));

    let stray_wt = fx.worktree("not-recorded");

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    for (path, needle) in [
        (&open_wt, "open"),
        (&fail_wt, "gh failed"),
        (&stray_wt, "not recorded in owned-paths"),
    ] {
        let e = entry_for(&report, path);
        assert_eq!(e["action"], "skipped", "{e}");
        assert!(
            reason(&e).contains(needle),
            "reason for {path:?} should mention {needle:?}: {e}"
        );
        assert!(path.exists(), "{path:?} must survive");
    }
}

#[test]
fn ac5_missing_gh_skips_runs_with_a_branch() {
    let fx = Fx::new();
    let wt = fx.worktree("no-gh");
    fx.seed_run(&SeedRun::ended(72).owns_worktree(&wt, "no-gh"));
    fs::remove_file(fx.bin.join("gh")).unwrap();

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    let e = entry_for(&report, &wt);
    assert_eq!(e["action"], "skipped", "{e}");
    assert!(reason(&e).contains("gh"), "{e}");
    assert!(wt.exists());
}

/// Dirty and unpushed worktrees are never removed, even when eligible.
#[test]
fn dirty_or_unpushed_worktrees_are_skipped() {
    let fx = Fx::new();
    let (_, dirty, _) = merged_run(&fx, "dirty", 72);
    fs::write(dirty.join("uncommitted.txt"), "work in progress").unwrap();
    let (_, ahead, _) = merged_run(&fx, "ahead", 72);
    fs::write(ahead.join("more.txt"), "x").unwrap();
    fx.git_ok(&ahead, &["add", "more.txt"]);
    fx.git_ok(&ahead, &["commit", "-q", "-m", "unpushed"]);

    let report = fx.cleanup_json(&["--apply"]);
    for wt in [&dirty, &ahead] {
        assert_eq!(entry_for(&report, wt)["action"], "skipped");
        assert!(wt.exists(), "{wt:?} must survive");
    }
    assert!(dirty.join("uncommitted.txt").exists());
}

/// A worktree that a live process has as its working folder is kept.
#[test]
fn worktree_used_as_cwd_by_a_live_process_is_skipped() {
    let mut fx = Fx::new();
    let (_, wt, _) = merged_run(&fx, "in-use", 72);
    fx.live_process(&wt);
    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&report, &wt)["action"], "skipped");
    assert!(wt.exists());
}

/// D6: a run whose pid is alive with the recorded start ticks is active and
/// never touched, not even its `runtime/`.
#[test]
fn active_run_is_never_touched() {
    let mut fx = Fx::new();
    let pid = fx.live_process(&fx.root.clone());
    let mut run = SeedRun::ended(72);
    run.pid = pid;
    run.pid_start_ticks = start_ticks(pid);
    run.ended_hours_ago = None;
    run.with_runtime = true;
    let active_tgt = fx.target("amplihack-precommit-target-0123456789ab-run-active", 64);
    run = run.owns_target(&active_tgt);
    let dir = fx.seed_run(&run);

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    assert!(
        dir.join("runtime/tmp/scratch").exists(),
        "an active run's runtime is kept"
    );
    assert!(active_tgt.exists());
    assert_eq!(entry_for(&report, &active_tgt)["action"], "skipped");
}

/// D6: a reused pid (alive, wrong start ticks) does not make a dead run look
/// active, so its leftover `runtime/` is removed as `run-tmp`.
#[test]
fn reused_pid_with_wrong_start_ticks_is_not_active() {
    let mut fx = Fx::new();
    let pid = fx.live_process(&fx.root.clone());
    let mut run = SeedRun::ended(0);
    run.pid = pid;
    run.pid_start_ticks = start_ticks(pid) + 12345;
    run.ended_hours_ago = None;
    run.run_json_mtime_hours_ago = 0;
    run.with_runtime = true;
    let dir = fx.seed_run(&run);

    let report = fx.cleanup_json(&["--apply"]);
    let e = entry_for(&report, &dir.join("runtime"));
    assert_eq!(e["category"], "run-tmp");
    assert_eq!(e["action"], "removed");
    assert!(!dir.join("runtime").exists());
    assert!(dir.join("run.json").exists());
}

/// `run-tmp` ignores the keep period: a killed run's `runtime/` (and the
/// short `r/<id>` form) is removed as soon as the run is inactive.
#[test]
fn run_tmp_of_killed_run_is_removed_without_keep_period() {
    let fx = Fx::new();
    let mut run = SeedRun::ended(0);
    run.ended_hours_ago = None;
    run.run_json_mtime_hours_ago = 0;
    run.with_runtime = true;
    let dir = fx.seed_run(&run);
    let short: String = run.id.chars().filter(|c| *c != '-').take(12).collect();
    let short_dir = fx.work.join("r").join(&short);
    fs::create_dir_all(short_dir.join("tmp")).unwrap();

    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(
        entry_for(&report, &dir.join("runtime"))["action"],
        "removed"
    );
    assert!(!dir.join("runtime").exists());
    assert!(
        !short_dir.exists(),
        "the r/<short id> runtime is run-tmp too"
    );
}

/// A run report from a newer release is skipped.
#[test]
fn newer_schema_version_is_skipped() {
    let fx = Fx::new();
    let tgt = fx.target("amplihack-precommit-target-0123456789ab-run-future", 64);
    let mut run = SeedRun::ended(72).owns_target(&tgt);
    run.schema_version = 2;
    fx.seed_run(&run);
    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(entry_for(&report, &tgt)["action"], "skipped");
    assert!(tgt.exists());
}

/// No-PR rule: a run with no recorded branch is dated by the later of
/// `ended_at` and the mtime of `run.json`.
#[test]
fn no_pr_rule_uses_later_of_ended_at_and_run_json_mtime() {
    let fx = Fx::new();
    let tgt = fx.target("amplihack-precommit-target-0123456789ab-run-touched", 64);
    let mut run = SeedRun::ended(72).owns_target(&tgt);
    run.run_json_mtime_hours_ago = 1;
    fx.seed_run(&run);
    let report = fx.cleanup_json(&["--apply"]);
    assert_eq!(
        entry_for(&report, &tgt)["action"],
        "skipped",
        "touched 1h ago"
    );
    assert!(tgt.exists());
    assert!(
        !fx.gh_calls().contains("--head="),
        "no branch, so no gh lookup"
    );
}

/// D9: an `owned-paths` entry is a request, not a permission. Paths outside
/// the managed roots, the roots themselves and symlinks are skipped.
#[test]
fn owned_paths_outside_managed_roots_and_symlinks_are_refused() {
    let fx = Fx::new();
    let outside = fx.root.join("precious");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep").unwrap();
    let link = fx.work.join("targets/amplihack-precommit-target-link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let run = SeedRun::ended(72)
        .owns_target(&outside)
        .owns_target(&link)
        .owns_target(&fx.work.join("targets"))
        .owns_target(&fx.work)
        .owns_target(&fx.home);
    fx.seed_run(&run);

    let out = fx.cleanup(&["--apply", "--format", "json"]);
    assert!(outside.join("keep.txt").exists(), "{}", describe(&out));
    assert!(
        fs::symlink_metadata(&link).is_ok(),
        "the symlink itself is not followed or removed"
    );
    assert!(fx.work.join("targets").is_dir() && fx.home.is_dir());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entry_for(&report, &outside)["action"], "skipped");
}

/// `untagged-target`: unrecorded folders directly under `<work>/targets/`
/// older than the keep period are removed; fresh ones and those whose
/// `.cargo-lock` is held are kept.
#[test]
fn untagged_targets_respect_keep_period_and_cargo_lock() {
    let mut fx = Fx::new();
    let old = fx.target("stray-old", 64);
    age_tree(&old, 72);
    let fresh = fx.target("stray-fresh", 64);
    let busy = fx.target("stray-busy", 64);
    fx.hold_flock(&busy.join(".cargo-lock"));
    age_tree(&busy, 72);

    let report = fx.cleanup_json(&["--apply"]);
    let e = entry_for(&report, &old);
    assert_eq!(e["category"], "untagged-target");
    assert_eq!(e["action"], "removed");
    assert!(!old.exists());
    assert!(fresh.exists());
    assert!(busy.exists(), "a target with a held .cargo-lock is in use");
}

/// AC2: a second `--apply` reports nothing and changes nothing on disk; the
/// lock file is not rewritten.
#[test]
fn ac2_second_apply_is_a_no_op() {
    let fx = Fx::new();
    merged_run(&fx, "done", 72);
    let stray = fx.target("stray-old", 64);
    age_tree(&stray, 72);
    let log_dir = fx.home.join(".amplihack/recipe-logs");
    fs::create_dir_all(&log_dir).unwrap();
    let log = log_dir.join("old.log");
    fs::write(&log, "old").unwrap();
    set_mtime_days_ago(&log, 60);

    let first = fx.cleanup_json(&["--apply"]);
    assert!(!entries(&first).is_empty());
    let lock = fx.work.join(".cleanup.lock");
    assert!(lock.exists(), "--apply takes <work root>/.cleanup.lock");
    assert_eq!(mode_of(&lock), 0o600);

    let before = (snapshot(&fx.work), snapshot(&fx.tmp), snapshot(&log_dir));
    let second = fx.cleanup_json(&["--apply"]);
    let after = (snapshot(&fx.work), snapshot(&fx.tmp), snapshot(&log_dir));
    assert!(entries(&second).is_empty(), "second apply: {second:#}");
    assert_eq!(before, after, "the second apply must not change anything");
}

/// AC3: sentinels in HOME, in a sibling (active) run's folder, in the main
/// checkout and in the work root itself are untouched.
#[test]
fn ac3_sentinels_outside_candidates_are_untouched() {
    let mut fx = Fx::new();
    merged_run(&fx, "done", 72);
    let pid = fx.live_process(&fx.root.clone());
    let mut sibling = SeedRun::ended(0);
    sibling.pid = pid;
    sibling.pid_start_ticks = start_ticks(pid);
    sibling.ended_hours_ago = None;
    sibling.with_runtime = true;
    let sibling_dir = fx.seed_run(&sibling);

    let sentinels = [
        fx.home.join("sentinel.txt"),
        sibling_dir.join("runtime/tmp/sentinel.txt"),
        fx.repo.join("sentinel.txt"),
        fx.work.join("driver.sh"),
        fx.work.join("tasks/task.md"),
        fx.work.join("clones/repo-1a2b3c4d/README"),
    ];
    for s in &sentinels {
        fs::create_dir_all(s.parent().unwrap()).unwrap();
        fs::write(s, "sentinel").unwrap();
        set_mtime_hours_ago(s, 500);
    }
    let before: Vec<_> = sentinels
        .iter()
        .map(|s| fs::metadata(s).unwrap().modified().unwrap())
        .collect();

    let out = fx.cleanup(&["--apply", "--keep-hours", "0"]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    for (s, m) in sentinels.iter().zip(before) {
        let meta = fs::metadata(s).unwrap_or_else(|_| panic!("{s:?} was removed"));
        assert_eq!(meta.modified().unwrap(), m, "{s:?} changed");
        assert_eq!(fs::read_to_string(s).unwrap(), "sentinel");
    }
}

/// D9 (3): `tmp-leftover` collects only aged top-level `replay-*`,
/// `release-*` and `amplihack-workflow-*` folders in TMPDIR that are not in
/// use, never symlinks and never other names.
#[test]
fn tmp_leftovers_are_limited_to_known_prefixes_age_and_idleness() {
    let mut fx = Fx::new();
    let make = |name: &str, hours: i64| {
        let p = fx.tmp.join(name);
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("f"), "x").unwrap();
        age_tree(&p, hours);
        p
    };
    let removed = [
        make("replay-old", 72),
        make("release-old", 72),
        make("amplihack-workflow-old", 72),
    ];
    let kept = [make("replay-fresh", 0), make("other-old", 72)];
    let busy = make("amplihack-workflow-busy", 72);
    fx.live_process(&busy);
    age_tree(&busy, 72);
    let elsewhere = fx.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("keep"), "x").unwrap();
    let link = fx.tmp.join("replay-link");
    std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
    set_mtime_hours_ago(&link, 72);

    let report = fx.cleanup_json(&["--apply", "--only", "tmp-leftover"]);
    for p in &removed {
        assert!(!p.exists(), "{p:?} should be removed");
        assert_eq!(entry_for(&report, p)["category"], "tmp-leftover");
    }
    for p in kept.iter().chain([&busy]) {
        assert!(p.exists(), "{p:?} must be kept");
    }
    assert!(
        elsewhere.join("keep").exists(),
        "symlinks are never followed"
    );
}

/// The dry run (the default) writes nothing at all: same listing before and
/// after, no lock file, no archive.
#[test]
fn dry_run_writes_nothing() {
    let fx = Fx::new();
    let (_, wt, _) = merged_run(&fx, "done", 72);
    let log_dir = fx.home.join(".amplihack/recipe-logs");
    fs::create_dir_all(&log_dir).unwrap();
    let log = log_dir.join("old.log");
    fs::write(&log, "old").unwrap();
    set_mtime_days_ago(&log, 60);

    let before = (snapshot(&fx.work), snapshot(&fx.home), snapshot(&fx.tmp));
    let report = fx.cleanup_json(&[]);
    let after = (snapshot(&fx.work), snapshot(&fx.home), snapshot(&fx.tmp));
    assert_eq!(before, after);
    assert_eq!(report["dry_run"], true);
    assert_eq!(entry_for(&report, &wt)["action"], "would-remove");
    assert_eq!(entry_for(&report, &log)["action"], "would-archive");
    assert!(!fx.work.join(".cleanup.lock").exists());
    assert!(!fx.work.join("archive").exists());
}
