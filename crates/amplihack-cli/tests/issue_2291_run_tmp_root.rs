//! Issue #2291: a run's temp folder honours `AMPLIHACK_RUN_TMP_ROOT` or the
//! caller's `TMPDIR`, is removed at run end and by cleanup, and build targets
//! stay out of it.
//!
//! The specification is "Where a run's temp folder goes" in
//! `docs/howto/clean-up-after-recipe-runs.md`. These tests reuse the issue
//! #1808 fixture: the real `amplihack` binary under `isolated(..)`, a temp
//! `HOME` inside the fixture's `TMPDIR`, a temp work root and a stub recipe
//! runner. No test sleeps or depends on timing: liveness comes from this test
//! process's own pid and a pid that has exited.

#![cfg(target_os = "linux")]

use crate::issue_1808_run_cleanup::fixture::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const WARN_ROOT: &str = "amplihack: AMPLIHACK_RUN_TMP_ROOT=";
const WARN_TMPDIR: &str = "amplihack: TMPDIR=";
const FALLBACK_TAIL: &str = "; using the default run folder";

/// Records the step environment, whether `<run dir>/runtime` exists while
/// the step runs, and the `run.json` the step sees; writes a scratch file
/// into the step's `TMPDIR`.
const PROBE: &str = r#"
[ -e "$AMPLIHACK_RUN_DIR/runtime" ] && printf 'RUNTIME_IN_WORK=1\n' >> "$RECORD_DIR/env"
cat "$AMPLIHACK_RUN_DIR/run.json" > "$RECORD_DIR/run.json.during" 2>/dev/null
printf 'scratch' > "$TMPDIR/scratch"
"#;

fn runner(ok: bool) -> String {
    let tail = if ok {
        RUNNER_SUCCESS_JSON
    } else {
        RUNNER_FAILURE_JSON
    };
    format!("#!/bin/sh\n{RECORD_ENV_SNIPPET}\n{PROBE}\n{tail}\n")
}

fn short_id(run_id: &str) -> String {
    run_id.chars().filter(|c| *c != '-').take(12).collect()
}

/// The default layout: `<work>/runs/<id>/runtime`, or `<work>/r/<short id>`
/// when its `tmp/` would pass 80 bytes.
fn default_runtime(work: &Path, run_id: &str) -> PathBuf {
    let d = work.join("runs").join(run_id).join("runtime");
    if d.join("tmp").as_os_str().len() <= 80 {
        d
    } else {
        work.join("r").join(short_id(run_id))
    }
}

fn canon(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The stderr lines that are temp-root fallback warnings.
fn root_warnings(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.starts_with(WARN_ROOT) || l.starts_with(WARN_TMPDIR))
        .collect()
}

fn run_json_during(fx: &Fx) -> serde_json::Value {
    let raw = fs::read(fx.records.join("run.json.during")).expect("the step did not copy run.json");
    serde_json::from_slice(&raw).unwrap()
}

fn assert_all_under(env: &std::collections::BTreeMap<String, String>, dir: &Path) {
    let tmp = dir.join("tmp").display().to_string();
    for var in ["TMPDIR", "TMP", "TEMP"] {
        assert_eq!(env[var], tmp, "{var}: {env:?}");
    }
    let d = dir.display().to_string();
    assert_eq!(env["AMPLIHACK_WORKFLOW_RUNTIME_DIR"], d, "{env:?}");
    assert_eq!(env["AMPLIHACK_RUNTIME_ROOT"], d, "{env:?}");
    assert_eq!(
        env["AMPLIHACK_WORKFLOW_ARTIFACT_DIR"],
        dir.join("artifacts").display().to_string()
    );
    assert_eq!(env.get("TMPDIR_EXISTS").map(String::as_str), Some("1"));
}

// ----- a: AC1, the caller's TMPDIR --------------------------------------

/// AC1: the caller's `TMPDIR` (outside the work root) is honoured, the
/// default `runtime/` is never created, and the root is exported for nested
/// runs.
#[test]
fn ac1_caller_tmpdir_is_honoured_and_runtime_in_work_is_never_created() {
    let fx = Fx::new_short();
    let out = fx.recipe_run(&runner(true), |_| {});
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();
    let dir = canon(&fx.tmp).join(format!("amplihack-run-{}", short_id(&run_id)));
    assert_all_under(&env, &dir);
    assert_eq!(
        env["AMPLIHACK_RUN_TMP_ROOT"],
        canon(&fx.tmp).display().to_string(),
        "the root is exported so a sub-recipe makes a sibling folder"
    );
    assert!(
        !env.contains_key("RUNTIME_IN_WORK"),
        "<run dir>/runtime must never be created: {env:?}"
    );
    assert!(
        root_warnings(&stderr_of(&out)).is_empty(),
        "{}",
        describe(&out)
    );
    assert!(!dir.exists(), "the per-run folder is removed at run end");
    assert!(!fx.runs_dir().join(&run_id).join("runtime").exists());
    assert!(fx.tmp.is_dir(), "the root is never removed");
}

// ----- b: AC2, AMPLIHACK_RUN_TMP_ROOT -----------------------------------

/// AC2: `AMPLIHACK_RUN_TMP_ROOT` wins over `TMPDIR`; a missing root is
/// created private and is kept after the run.
#[test]
fn ac2_run_tmp_root_wins_over_tmpdir_and_holds_every_runtime_variable() {
    let fx = Fx::new_short();
    let root = fx.root.join("runroot");
    let out = fx.recipe_run(&runner(true), |c| {
        c.env("AMPLIHACK_RUN_TMP_ROOT", &root);
    });
    assert!(out.status.success(), "{}", describe(&out));

    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();
    let dir = canon(&root).join(format!("amplihack-run-{}", short_id(&run_id)));
    assert_all_under(&env, &dir);
    assert_eq!(
        env["AMPLIHACK_RUN_TMP_ROOT"],
        canon(&root).display().to_string()
    );
    assert!(!env.contains_key("RUNTIME_IN_WORK"), "{env:?}");
    assert!(
        root_warnings(&stderr_of(&out)).is_empty(),
        "{}",
        describe(&out)
    );

    assert!(root.is_dir(), "the root is kept");
    assert_eq!(mode_of(&root), 0o700, "a created root is private");
    assert_eq!(
        fs::read_dir(&root).unwrap().count(),
        0,
        "the per-run folder is gone"
    );
    let caller_side = canon(&fx.tmp).join(format!("amplihack-run-{}", short_id(&run_id)));
    assert!(!caller_side.exists(), "TMPDIR is not used");
}

// ----- c: AC4, removal after success and failure ------------------------

/// AC4: a failing run also removes its per-run folder and never has a
/// `runtime/` in the run folder.
#[test]
fn ac4_failing_run_removes_the_per_run_folder() {
    let fx = Fx::new_short();
    let root = fx.root.join("runroot");
    let out = fx.recipe_run(&runner(false), |c| {
        c.env("AMPLIHACK_RUN_TMP_ROOT", &root);
    });
    assert!(!out.status.success(), "the stub fails: {}", describe(&out));

    let env = fx.recorded_env();
    let run_id = env["AMPLIHACK_RUN_ID"].clone();
    let dir = canon(&root).join(format!("amplihack-run-{}", short_id(&run_id)));
    assert_eq!(env["TMPDIR"], dir.join("tmp").display().to_string());
    assert!(!dir.exists(), "removed after a failure too");
    let run_dir = fx.runs_dir().join(&run_id);
    assert!(!run_dir.join("runtime").exists());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(report["outcome"], "failure", "{report:#}");
}

// ----- d: AC5, run.json records the folder ------------------------------

/// AC5 and D2: `run.json` names the external folder before any step runs, so
/// a killed run can still be cleaned up.
#[test]
fn ac5_run_json_records_the_external_folder_before_steps_run() {
    let fx = Fx::new_short();
    let root = fx.root.join("runroot");
    let out = fx.recipe_run(&runner(true), |c| {
        c.env("AMPLIHACK_RUN_TMP_ROOT", &root);
    });
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    let during = run_json_during(&fx);
    assert_eq!(
        during["runtime_dir"].as_str(),
        Some(env["AMPLIHACK_WORKFLOW_RUNTIME_DIR"].as_str()),
        "{during:#}"
    );
    assert_eq!(during["schema_version"], 1, "no schema bump");
}

/// D8: a run on the default layout does not write the field.
#[test]
fn default_layout_run_json_has_no_runtime_dir() {
    let fx = Fx::new_short();
    let out = fx.recipe_run(&runner(true), |c| {
        c.env("AMPLIHACK_RUN_TMP_ROOT", "relative/root");
    });
    assert!(out.status.success(), "{}", describe(&out));
    let during = run_json_during(&fx);
    assert!(during.get("runtime_dir").is_none(), "{during:#}");
}

// ----- e: AC5 and AC8, cleanup of recorded folders ----------------------

/// A folder named for `run`, with a file in it, under `root`.
fn external_for(root: &Path, run: &SeedRun) -> PathBuf {
    let dir = root.join(format!("amplihack-run-{}", short_id(&run.id)));
    fs::create_dir_all(dir.join("tmp")).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(dir.join("tmp/scratch"), "scratch").unwrap();
    dir
}

fn ext_root(fx: &Fx) -> PathBuf {
    let root = fx.root.join("runroot");
    mkdir_0700(&root);
    canon(&root)
}

fn reason(e: &serde_json::Value) -> String {
    e["reason"].as_str().unwrap_or_default().to_string()
}

/// AC5 and AC8: one sweep removes a dead run's recorded folder and keeps a
/// live run's folder under the same root.
#[test]
fn ac8_cleanup_removes_a_dead_runs_folder_and_keeps_a_live_runs() {
    let fx = Fx::new_short();
    let root = ext_root(&fx);
    let mut dead = SeedRun::ended(0);
    dead.ended_hours_ago = None;
    dead.run_json_mtime_hours_ago = 0;
    let dead_dir = external_for(&root, &dead);
    dead.runtime_dir = Some(dead_dir.clone());
    fx.seed_run(&dead);
    let mut live = SeedRun::active();
    let live_dir = external_for(&root, &live);
    live.runtime_dir = Some(live_dir.clone());
    fx.seed_run(&live);
    assert_ne!(dead_dir, live_dir);

    let report = fx.cleanup_json(&["--apply"]);

    let e = entry_for(&report, &dead_dir);
    assert_eq!(e["category"], "run-tmp", "{e:#}");
    assert_eq!(e["action"], "removed", "{e:#}");
    assert!(!dead_dir.exists());
    let e = entry_for(&report, &live_dir);
    assert_eq!(e["action"], "skipped", "{e:#}");
    assert!(reason(&e).contains("run is active"), "{e:#}");
    assert!(
        live_dir.join("tmp/scratch").exists(),
        "a live run's folder is kept"
    );
    assert!(root.is_dir(), "the root is kept");
}

/// AC5: the end-of-run sweep also removes an earlier killed run's recorded
/// folder.
#[test]
fn ac5_end_of_run_sweep_removes_an_earlier_runs_recorded_folder() {
    let fx = Fx::new_short();
    let root = ext_root(&fx);
    let mut dead = SeedRun::ended(0);
    dead.ended_hours_ago = None;
    dead.run_json_mtime_hours_ago = 0;
    let dead_dir = external_for(&root, &dead);
    dead.runtime_dir = Some(dead_dir.clone());
    fx.seed_run(&dead);

    let out = fx.recipe_run(&runner(true), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!dead_dir.exists(), "the end-of-run sweep removes it");
}

/// AC5: recorded paths that fail the shape checks are reported and left
/// alone.
#[test]
fn ac5_cleanup_refuses_bad_recorded_folders() {
    let fx = Fx::new_short();
    let root = ext_root(&fx);
    let victim = fx.root.join("victim");
    mkdir_0700(&victim);
    fs::write(victim.join("keep-me"), "data").unwrap();

    let mut cases: Vec<(PathBuf, &str)> = Vec::new();

    // A symbolic link with the right name, pointing at a real folder.
    let mut link_run = SeedRun::ended(0);
    let link = root.join(format!("amplihack-run-{}", short_id(&link_run.id)));
    symlink(&victim, &link).unwrap();
    link_run.runtime_dir = Some(link.clone());
    fx.seed_run(&link_run);
    cases.push((link, "is a symbolic link"));

    // A real folder whose name is not this run's.
    let mut named_run = SeedRun::ended(0);
    let wrong = root.join("amplihack-run-ffffffffffff");
    fs::create_dir_all(&wrong).unwrap();
    named_run.runtime_dir = Some(wrong.clone());
    fx.seed_run(&named_run);
    cases.push((wrong, "wrong name"));

    // A file with the right name.
    let mut file_run = SeedRun::ended(0);
    let file = root.join(format!("amplihack-run-{}", short_id(&file_run.id)));
    fs::write(&file, "not a folder").unwrap();
    file_run.runtime_dir = Some(file.clone());
    fx.seed_run(&file_run);
    cases.push((file, "not a folder"));

    // The right name, inside the work root.
    let mut inside_run = SeedRun::ended(0);
    let inside = fx
        .work
        .join(format!("amplihack-run-{}", short_id(&inside_run.id)));
    fs::create_dir_all(&inside).unwrap();
    inside_run.runtime_dir = Some(inside.clone());
    fx.seed_run(&inside_run);
    cases.push((inside, "inside the work root"));

    let report = fx.cleanup_json(&["--apply"]);
    for (path, why) in &cases {
        let e = entry_for(&report, path);
        assert_eq!(e["action"], "skipped", "{e:#}");
        assert_eq!(e["category"], "run-tmp", "{e:#}");
        let r = reason(&e);
        assert!(
            r.contains("recorded temp folder refused") && r.contains(why),
            "{why}: {e:#}"
        );
        assert!(present(path), "{} must be left alone", path.display());
    }
    assert!(victim.join("keep-me").exists(), "the link target survives");
}

/// AC5: a recorded folder that no longer exists produces no entry, and an
/// unrecorded `amplihack-run-*` folder in the temp folder is never swept.
#[test]
fn ac5_missing_or_unrecorded_folders_produce_no_entry() {
    let fx = Fx::new_short();
    let root = ext_root(&fx);
    let mut gone = SeedRun::ended(0);
    let missing = root.join(format!("amplihack-run-{}", short_id(&gone.id)));
    gone.runtime_dir = Some(missing.clone());
    fx.seed_run(&gone);
    let unrecorded = fx.tmp.join("amplihack-run-0123456789ab");
    fs::create_dir_all(unrecorded.join("tmp")).unwrap();
    set_mtime_hours_ago(&unrecorded, 24 * 30);

    let report = fx.cleanup_json(&["--apply", "--keep-hours", "0"]);
    for path in [&missing, &unrecorded] {
        let want = path.display().to_string();
        assert!(
            entries(&report)
                .iter()
                .all(|e| e["path"].as_str() != Some(want.as_str())),
            "no entry expected for {want}: {report:#}"
        );
    }
    assert!(unrecorded.exists(), "an unrecorded folder is never removed");
}

// ----- f: AC6, unusable roots --------------------------------------------

/// AC6: each unusable root prints exactly one warning, the run completes on
/// the default layout, and the caller's value reaches the step unchanged.
#[test]
fn ac6_unusable_root_warns_once_and_uses_the_default_layout() {
    let probe = Fx::new_short();
    let file = probe.root.join("afile");
    fs::write(&file, "x").unwrap();
    let long = probe.root.join("l".repeat(60));
    mkdir_0700(&long);
    let cases: Vec<(String, &str)> = vec![
        ("relative/root".into(), "is not an absolute path"),
        (file.display().to_string(), "is not a folder"),
        (
            file.join("below").display().to_string(),
            "could not be created",
        ),
        (long.display().to_string(), "would make the run temp path"),
        (probe.work.display().to_string(), "is inside the work root"),
    ];
    for (value, why) in cases {
        let fx = Fx::new_short();
        // The work-root case must name this fixture's own work root.
        let value = if why == "is inside the work root" {
            fx.work.display().to_string()
        } else {
            value
        };
        let out = fx.recipe_run(&runner(true), |c| {
            c.env("AMPLIHACK_RUN_TMP_ROOT", &value);
        });
        assert!(out.status.success(), "{value}: {}", describe(&out));
        let stderr = stderr_of(&out);
        let warnings = root_warnings(&stderr);
        assert_eq!(warnings.len(), 1, "{value}: exactly one warning: {stderr}");
        assert!(warnings[0].starts_with(WARN_ROOT), "{}", warnings[0]);
        assert!(warnings[0].contains(why), "{why}: {}", warnings[0]);
        assert!(warnings[0].ends_with(FALLBACK_TAIL), "{}", warnings[0]);

        let env = fx.recorded_env();
        let run_id = env["AMPLIHACK_RUN_ID"].clone();
        let default = default_runtime(&fx.work, &run_id);
        assert_eq!(
            env["TMPDIR"],
            default.join("tmp").display().to_string(),
            "{value}"
        );
        assert_eq!(
            env["AMPLIHACK_RUN_TMP_ROOT"], value,
            "a default-layout run passes the caller's value through"
        );
        assert!(!default.exists(), "the default runtime/ is removed at end");
    }
}

// ----- g: AC7, CARGO_TARGET_DIR -----------------------------------------

/// AC7: a caller's `CARGO_TARGET_DIR` reaches the step unchanged, and is not
/// inside the run's temp folder.
#[test]
fn ac7_caller_cargo_target_dir_reaches_the_step_unchanged() {
    let fx = Fx::new_short();
    let target = fx.root.join("target");
    let out = fx.recipe_run(&runner(true), |c| {
        c.env("CARGO_TARGET_DIR", &target);
    });
    assert!(out.status.success(), "{}", describe(&out));
    let env = fx.recorded_env();
    assert_eq!(env["CARGO_TARGET_DIR"], target.display().to_string());
    assert!(!target.starts_with(&env["AMPLIHACK_WORKFLOW_RUNTIME_DIR"]));
}

/// AC7: without a caller value, the run does not invent one.
#[test]
fn ac7_unset_cargo_target_dir_stays_unset() {
    let fx = Fx::new_short();
    let out = fx.recipe_run(&runner(true), |_| {});
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(fx.recorded_env()["CARGO_TARGET_DIR"], "__unset__");
}
