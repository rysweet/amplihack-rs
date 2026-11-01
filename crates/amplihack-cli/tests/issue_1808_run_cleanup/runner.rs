//! `amplihack recipe run`: run folder, run report, per-run TMPDIR, exported
//! variables and the end-of-run sweep (R1, R3, R4; AC1, AC7, AC8, AC15).

use super::fixture::*;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn only_run_dir(fx: &Fx) -> PathBuf {
    let dirs = fx.run_dirs();
    assert_eq!(dirs.len(), 1, "expected one run folder, found {dirs:?}");
    dirs.into_iter().next().unwrap()
}

fn run_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(dir.join("run.json")).expect("run.json missing")).unwrap()
}

fn names_in(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn success_runner() -> String {
    format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{RUNNER_SUCCESS_JSON}\n")
}

/// AC7 and R3: TMPDIR, TMP and TEMP all point at `<run dir>/runtime/tmp`,
/// which exists while the step runs and is gone afterwards; `run.json` stays.
#[test]
fn ac7_successful_run_gets_per_run_tmpdir_which_is_removed_at_end() {
    let fx = Fx::new();
    let out = fx.recipe_run(&success_runner(), |_| {});
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();
    assert!(is_uuid(&run_id), "AMPLIHACK_RUN_ID must be a UUID: {env:?}");
    let run_dir = fx.runs_dir().join(&run_id);
    let runtime = expected_runtime_dir(&fx.work, &run_id);
    let tmp = runtime.join("tmp");

    assert_eq!(env["AMPLIHACK_RUN_DIR"], run_dir.display().to_string());
    assert_eq!(env["AMPLIHACK_WORK_ROOT"], fx.work.display().to_string());
    assert_eq!(
        env["AMPLIHACK_WORKFLOW_RUNTIME_DIR"],
        runtime.display().to_string()
    );
    assert_eq!(env["AMPLIHACK_RUNTIME_ROOT"], runtime.display().to_string());
    assert_eq!(
        env["AMPLIHACK_WORKFLOW_ARTIFACT_DIR"],
        runtime.join("artifacts").display().to_string()
    );
    for var in ["TMPDIR", "TMP", "TEMP"] {
        assert_eq!(env[var], tmp.display().to_string(), "{var}");
    }
    assert_eq!(env.get("TMPDIR_EXISTS").map(String::as_str), Some("1"));

    assert!(!runtime.exists(), "runtime/ must be removed at run end");
    assert!(run_dir.join("run.json").is_file(), "run.json must remain");
    assert_eq!(mode_of(&run_dir), 0o700, "run folder mode");
    assert_eq!(mode_of(&run_dir.join("run.json")), 0o600, "run.json mode");
}

/// AC7: a failing run also loses `runtime/` and keeps `run.json`, with the
/// outcome recorded as a failure.
#[test]
fn ac7_failing_run_removes_runtime_and_records_failure() {
    let fx = Fx::new();
    let runner = format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{RUNNER_FAILURE_JSON}\n");
    let out = fx.recipe_run(&runner, |_| {});
    assert!(
        !out.status.success(),
        "the stub runner fails: {}",
        describe(&out)
    );

    let run_id = fx.recorded_env()["AMPLIHACK_RUN_ID"].clone();
    let run_dir = fx.runs_dir().join(&run_id);
    assert!(!expected_runtime_dir(&fx.work, &run_id).exists());
    let report = run_json(&run_dir);
    assert_eq!(report["outcome"], "failure", "{report:#}");
    assert!(report["ended_at"].as_str().is_some_and(|s| !s.is_empty()));
}

/// D2/D3 and the docs' run report: schema, run id versus attempt id, and no
/// environment or step output in the report.
#[test]
fn run_json_records_run_id_attempts_and_status_only() {
    let fx = Fx::new();
    let runner = format!(
        "#!/bin/sh\nexport SECRET_TOKEN_1808=do-not-record\n{RECORD_ENV_SNIPPET}\n{RUNNER_SUCCESS_JSON}\n"
    );
    let out = fx.recipe_run(&runner, |c| {
        c.env("SECRET_TOKEN_1808", "do-not-record");
    });
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_dir = only_run_dir(&fx);
    let report = run_json(&run_dir);
    let run_id = run_dir.file_name().unwrap().to_string_lossy().into_owned();

    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["run_id"], run_id.as_str());
    assert_eq!(env["AMPLIHACK_RUN_ID"], run_id);
    assert_eq!(report["outcome"], "success");
    assert!(report["pid"].as_u64().is_some_and(|p| p > 0));
    assert!(report["pid_start_ticks"].as_u64().is_some());
    assert!(report["started_at"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(report["ended_at"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(report["cwd"], fx.repo.display().to_string());

    let attempt = env["AMPLIHACK_RECIPE_RUN_ID"].clone();
    assert!(is_uuid(&attempt));
    assert_ne!(attempt, run_id, "the attempt id is not the run id");
    let attempts = report["attempts"].as_array().expect("attempts[]");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["correlation_id"], attempt.as_str());

    let raw = fs::read_to_string(run_dir.join("run.json")).unwrap();
    assert!(
        !raw.contains("do-not-record"),
        "run.json must not hold env values"
    );
}

/// R4 and the explicit flag: with `AMPLIHACK_WORK_ROOT` set, it is passed
/// through and marked explicit.
#[test]
fn explicit_work_root_is_exported_with_explicit_flag_1() {
    let fx = Fx::new();
    let out = fx.recipe_run(&success_runner(), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    assert_eq!(env["AMPLIHACK_WORK_ROOT_EXPLICIT"], "1");
}

/// Without `AMPLIHACK_WORK_ROOT` the default `$HOME/.amplihack/work` is
/// exported with `AMPLIHACK_WORK_ROOT_EXPLICIT=0`.
#[test]
fn default_work_root_is_under_home_with_explicit_flag_0() {
    let fx = Fx::new();
    let out = fx.recipe_run(&success_runner(), |c| {
        c.env_remove("AMPLIHACK_WORK_ROOT");
    });
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    let default = fx.home.join(".amplihack/work");
    assert_eq!(env["AMPLIHACK_WORK_ROOT"], default.display().to_string());
    assert_eq!(env["AMPLIHACK_WORK_ROOT_EXPLICIT"], "0");
    assert!(
        default
            .join("runs")
            .join(&env["AMPLIHACK_RUN_ID"])
            .join("run.json")
            .is_file()
    );
    assert!(
        fx.run_dirs().is_empty(),
        "nothing under the unused custom root"
    );
}

/// A nested run keeps its parent's `AMPLIHACK_WORK_ROOT_EXPLICIT=0` and gets
/// its own run id and run folder.
#[test]
fn nested_run_passes_explicit_flag_through_and_gets_its_own_run_folder() {
    let fx = Fx::new();
    let parent = uuid::Uuid::new_v4().to_string();
    let out = fx.recipe_run(&success_runner(), |c| {
        c.env("AMPLIHACK_WORK_ROOT_EXPLICIT", "0")
            .env("AMPLIHACK_RUN_ID", &parent)
            .env("AMPLIHACK_RUN_DIR", fx.runs_dir().join(&parent));
    });
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    assert_eq!(env["AMPLIHACK_WORK_ROOT_EXPLICIT"], "0");
    assert_ne!(
        env["AMPLIHACK_RUN_ID"], parent,
        "a nested run needs its own id"
    );
    assert_eq!(
        env["AMPLIHACK_RUN_DIR"],
        fx.runs_dir()
            .join(&env["AMPLIHACK_RUN_ID"])
            .display()
            .to_string()
    );
}

/// AC8: with a custom work root, run folders, hook targets and (because the
/// variable is set explicitly) worktrees all go under it; nothing is created
/// under `<HOME>/.amplihack/work`.
#[test]
fn ac8_custom_work_root_holds_runs_targets_and_worktree_base() {
    let fx = Fx::new();
    let hook = repo_root().join("scripts/git-hook-cargo.sh");
    let wt_helper = repo_root().join("amplifier-bundle/tools/workflow_worktree_root.sh");
    let runner = format!(
        r#"#!/bin/sh
{RECORD_ENV_SNIPPET}
HOME="{home}" XDG_CACHE_HOME="{home}/.cache" AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
  bash "{hook}" sh -c 'printf "HOOK_TARGET=%s\n" "$CARGO_TARGET_DIR" >> "$RECORD_DIR/env"'
printf 'WT_BASE=%s\n' "$(bash "{wt}" base "{repo}")" >> "$RECORD_DIR/env"
{RUNNER_SUCCESS_JSON}
"#,
        hook = hook.display(),
        wt = wt_helper.display(),
        repo = fx.repo.display(),
        home = fx.home.display(),
    );
    let out = fx.recipe_run(&runner, |c| {
        c.env("AMPLIHACK_CLEANUP_ON_EXIT", "0");
    });
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();

    let target = PathBuf::from(&env["HOOK_TARGET"]);
    assert!(
        target.starts_with(fx.work.join("targets")),
        "hook target {target:?} must be under the work root"
    );
    let name = target.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("amplihack-precommit-target-"), "{name}");
    assert!(name.ends_with(&format!("-run-{run_id}")), "{name}");
    assert!(
        target.join("CACHEDIR.TAG").is_file(),
        "hook target must be tagged"
    );

    let owned = fs::read_to_string(fx.runs_dir().join(&run_id).join("owned-paths"))
        .expect("the hook records its target in owned-paths");
    assert!(
        owned
            .lines()
            .any(|l| l.starts_with(&format!("target\t{}", target.display()))),
        "owned-paths: {owned}"
    );

    let wt_base = PathBuf::from(&env["WT_BASE"]);
    assert!(
        wt_base.starts_with(fx.work.join("worktrees")),
        "worktree base {wt_base:?} must be under the explicit work root"
    );
    let base_name = wt_base.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        base_name.starts_with("repo-") && base_name.len() == "repo-".len() + 8,
        "{base_name}"
    );

    assert!(
        !fx.home.join(".amplihack/work").exists(),
        "nothing under HOME's default root"
    );
}

/// AC15: `AMPLIHACK_CLEANUP_ON_EXIT=0` skips the sweep but still removes
/// `runtime/`.
#[test]
fn ac15_cleanup_on_exit_0_skips_sweep_but_removes_runtime() {
    let fx = Fx::new();
    let old_target = fx.target("amplihack-precommit-target-0123456789ab-run-old", 64);
    let seeded = SeedRun::ended(72).owns_target(&old_target);
    fx.seed_run(&seeded);

    let out = fx.recipe_run(&success_runner(), |c| {
        c.env("AMPLIHACK_CLEANUP_ON_EXIT", "0");
    });
    assert!(out.status.success(), "{}", describe(&out));
    let run_id = fx.recorded_env()["AMPLIHACK_RUN_ID"].clone();
    assert!(
        is_uuid(&run_id),
        "AMPLIHACK_RUN_ID must be exported: {run_id}"
    );
    assert!(fx.runs_dir().join(&run_id).join("run.json").is_file());
    assert!(!expected_runtime_dir(&fx.work, &run_id).exists());
    assert!(old_target.exists(), "the sweep must not run");
    assert!(
        !fx.runs_dir()
            .join(&seeded.id)
            .join("cleanup-record.json")
            .exists()
    );
}

/// D5: by default the end-of-run sweep also clears earlier expired runs.
#[test]
fn end_of_run_sweep_removes_expired_items_of_earlier_runs() {
    let fx = Fx::new();
    let old_target = fx.target("amplihack-precommit-target-0123456789ab-run-old", 64);
    let seeded = SeedRun::ended(72).owns_target(&old_target);
    fx.seed_run(&seeded);

    let out = fx.recipe_run(&success_runner(), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        !old_target.exists(),
        "an expired run-owned target is removed at run end"
    );
    let record: serde_json::Value = serde_json::from_slice(
        &fs::read(fx.runs_dir().join(&seeded.id).join("cleanup-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["run_id"], seeded.id.as_str());
}

/// D5: the end-of-run sweep covers run-owned categories only. Untagged
/// targets, in the hook cache or under the work root, are left to
/// `amplihack cleanup` however old they are.
#[test]
fn end_of_run_sweep_leaves_untagged_targets_alone() {
    let fx = Fx::new();
    let hook_cache = fx
        .home
        .join(".cache/amplihack/amplihack-precommit-target-0123456789ab");
    fs::create_dir_all(hook_cache.join("debug")).unwrap();
    fs::write(hook_cache.join("debug/blob"), "x").unwrap();
    age_tree(&hook_cache, 25);
    let untagged = fx.target("amplihack-precommit-target-fedcba987654", 64);
    age_tree(&untagged, 25);

    let out = fx.recipe_run(&success_runner(), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    assert!(hook_cache.join("debug/blob").exists(), "hook cache removed");
    assert!(
        untagged.join("debug/blob").exists(),
        "untagged target removed"
    );
}

/// D5: the end-of-run sweep never waits for a held lock and never changes the
/// run's exit status.
#[test]
fn end_of_run_sweep_is_skipped_when_cleanup_lock_is_held() {
    let mut fx = Fx::new();
    let old_target = fx.target("amplihack-precommit-target-0123456789ab-run-old", 64);
    fx.seed_run(&SeedRun::ended(72).owns_target(&old_target));
    let lock = fx.work.join(".cleanup.lock");
    fx.hold_flock(&lock);

    let started = std::time::Instant::now();
    let out = fx.recipe_run(&success_runner(), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "must not block on the lock"
    );
    let run_id = fx.recorded_env()["AMPLIHACK_RUN_ID"].clone();
    assert!(
        fx.runs_dir().join(&run_id).join("run.json").is_file(),
        "the run itself still has its report"
    );
    assert!(
        old_target.exists(),
        "nothing is removed while another cleanup holds the lock"
    );
}

/// AC1: a run whose PR merged long ago, with a keep period of 0, leaves only
/// its report and cleanup record; the worktree and run target are removed
/// (branch kept) and recorded with their sizes; recipe logs are untouched.
#[test]
fn ac1_run_with_merged_pr_and_keep_0_is_cleaned_at_run_end() {
    let fx = Fx::new();
    let branch = "feat/issue-1808-ac1";
    fx.gh_closed(branch, 1820, 48, true);
    let log_dir = fx.home.join(".amplihack/recipe-logs");
    fs::create_dir_all(&log_dir).unwrap();
    let old_log = log_dir.join("old-run.log");
    fs::write(&old_log, "old log\n").unwrap();
    set_mtime_days_ago(&old_log, 90);
    let logs_before = snapshot(&log_dir);

    let runner = format!(
        r#"#!/bin/sh
set -e
{RECORD_ENV_SNIPPET}
wt="$AMPLIHACK_WORK_ROOT/worktrees/repo-1a2b3c4d/{branch}"
mkdir -p "$(dirname "$wt")"
git -C "{repo}" worktree add -q -b "{branch}" "$wt" >&2
printf 'x\n' > "$wt/work.txt"
git -C "$wt" add work.txt >&2
git -C "$wt" commit -q -m ac1 >&2
git -C "$wt" push -q -u origin "{branch}" >&2
tgt="$AMPLIHACK_WORK_ROOT/targets/amplihack-precommit-target-abcdef012345-run-$AMPLIHACK_RUN_ID"
mkdir -p "$tgt/debug"
printf 'Signature: 8a477f597d28d172789f06886806bc55\n' > "$tgt/CACHEDIR.TAG"
dd if=/dev/zero of="$tgt/debug/blob" bs=1024 count=64 2>/dev/null
printf 'worktree\t%s\t%s\n' "$wt" "{branch}" >> "$AMPLIHACK_RUN_DIR/owned-paths"
printf 'target\t%s\t\n' "$tgt" >> "$AMPLIHACK_RUN_DIR/owned-paths"
printf 'WT=%s\nTGT=%s\n' "$wt" "$tgt" >> "$RECORD_DIR/env"
{RUNNER_SUCCESS_JSON}
"#,
        repo = fx.repo.display(),
    );
    let out = fx.recipe_run(&runner, |c| {
        c.env("AMPLIHACK_CLEANUP_KEEP_HOURS", "0");
    });
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_dir = fx.runs_dir().join(&env["AMPLIHACK_RUN_ID"]);
    let wt = PathBuf::from(&env["WT"]);
    let tgt = PathBuf::from(&env["TGT"]);

    assert!(!wt.exists(), "worktree must be removed");
    assert!(!tgt.exists(), "run target must be removed");
    assert!(
        !fx.git_ok(&fx.repo, &["worktree", "list"])
            .contains(wt.to_str().unwrap())
    );
    assert_eq!(
        fx.git_ok(&fx.repo, &["branch", "--list", branch])
            .trim_start_matches(['*', '+', ' ']),
        branch,
        "the branch is kept"
    );

    let names = names_in(&run_dir);
    assert!(
        names.contains("run.json") && names.contains("cleanup-record.json"),
        "{names:?}"
    );
    let allowed: BTreeSet<String> = ["run.json", "cleanup-record.json"]
        .into_iter()
        .map(String::from)
        .collect();
    assert!(
        names.is_subset(&allowed),
        "only the report and record remain: {names:?}"
    );

    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("cleanup-record.json")).unwrap()).unwrap();
    assert_eq!(record["run_id"], env["AMPLIHACK_RUN_ID"].as_str());
    let removed = record["sweeps"][0]["removed"]
        .as_array()
        .expect("sweeps[0].removed");
    for (kind, path) in [("run-worktree", &wt), ("run-target", &tgt)] {
        let item = removed
            .iter()
            .find(|r| r["kind"] == kind && r["path"] == path.display().to_string())
            .unwrap_or_else(|| panic!("{kind} {path:?} missing from {record:#}"));
        assert!(item["size_bytes"].as_u64().is_some_and(|n| n > 0), "{item}");
    }
    assert!(record["sweeps"][0]["cleaned_at"].as_str().is_some());
    assert_eq!(mode_of(&run_dir.join("cleanup-record.json")), 0o600);

    for (path, meta) in &logs_before {
        assert_eq!(
            snapshot(&log_dir).get(path),
            Some(meta),
            "recipe log {path:?} changed"
        );
    }
    assert!(
        fx.gh_calls().contains(&format!("--head={branch}")),
        "{}",
        fx.gh_calls()
    );
}
