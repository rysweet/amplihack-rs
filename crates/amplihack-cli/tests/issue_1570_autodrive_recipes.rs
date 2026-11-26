//! Issue #1570 — the auto-drive loop recipe hands the task to its driver
//! through a file, never through the uppercase alias.
//!
//! The runner exports the uppercase `TASK_DESCRIPTION` alias only up to
//! 96 KiB, and not at all once the environment budget is spent. The loop
//! steps used to pass `TASK_DESCRIPTION="${TASK_DESCRIPTION:-}"`, so exactly
//! the large tasks this issue targets reached the loop as an empty string.
//! The step reads `task_description` with `write_context_value` and passes
//! the file with `--task-file`.
//!
//! #3129 replaced the Crusty loop and merge-ready loop recipes with the shared
//! goal loop. Its `goal-loop` step in `autodrive-goal-loop.yaml` starts the
//! driver, `autodrive_goal_loop.sh`, which every auto-drive now runs.
//!
//! This test runs the real step command with only `AMPLIHACK_CONTEXT_FILE`
//! holding the task. `AMPLIHACK_HOME` points at a directory holding the real
//! `recipe_context_value.sh` and a fake `autodrive_goal_loop.sh` that copies
//! the `--task-file` it receives. It skips, with a note, when `jq` is not
//! installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECIPE: &str = "autodrive-goal-loop.yaml";
const STEP: &str = "goal-loop";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn jq_available() -> bool {
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skip: `jq` not available on PATH; recipe_context_value.sh needs it");
    }
    ok
}

fn step_command(recipe: &str, step_id: &str) -> String {
    let path = repo_root().join("amplifier-bundle/recipes").join(recipe);
    let text = std::fs::read_to_string(&path).unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe}: step {step_id} has no command"))
        .to_string()
}

const LOOP_STUB: &str = r#"#!/usr/bin/env bash
while [ $# -gt 0 ]; do
  if [ "$1" = "--task-file" ]; then cp -- "$2" "$LOOP_OUT/task"; fi
  shift
done
exit 0
"#;

struct StepRun {
    out: Output,
    task_seen: Option<Vec<u8>>,
    _tmp: tempfile::TempDir,
}

fn executable(path: &Path, text: &str) {
    crate::support::exec_stub::write_executable(path, text, 0o755);
}

/// Run the goal loop's step with `context` as the only context source.
fn run_step(context: &str) -> StepRun {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tmp.path().join("home/amplifier-bundle/tools");
    let loop_out = tmp.path().join("loop-out");
    let repo = tmp.path().join("repo");
    let private_tmp = tmp.path().join("tmp");
    for d in [&tools, &loop_out, &repo, &private_tmp] {
        std::fs::create_dir_all(d).unwrap();
    }
    executable(&tools.join("autodrive_goal_loop.sh"), LOOP_STUB);
    crate::support::exec_stub::copy_executable(
        &repo_root().join("amplifier-bundle/tools/recipe_context_value.sh"),
        &tools.join("recipe_context_value.sh"),
    );
    let ctx = tmp.path().join("context.json");
    std::fs::write(&ctx, context).unwrap();

    let out = Command::new("bash")
        .arg("-c")
        .arg(step_command(RECIPE, STEP))
        .env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("HOME", tmp.path())
        .env("TMPDIR", &private_tmp)
        .env("AMPLIHACK_HOME", tmp.path().join("home"))
        .env("AMPLIHACK_CONTEXT_FILE", &ctx)
        .env("RECIPE_VAR_repo_path", &repo)
        .env("LOOP_OUT", &loop_out)
        .current_dir(&repo)
        .output()
        .expect("spawn bash");
    StepRun {
        task_seen: std::fs::read(loop_out.join("task")).ok(),
        out,
        _tmp: tmp,
    }
}

fn large_task() -> String {
    let unit = "Fix E2BIG: héllo € 😀 \"quoted\" 'single' $(touch pwned) %s\n";
    let mut s = String::new();
    while s.len() + unit.len() <= 100 * 1024 {
        s.push_str(unit);
    }
    assert!(s.len() > 96 * 1024, "the task must exceed the alias limit");
    s
}

#[test]
fn a_task_over_96_kib_only_in_the_context_file_reaches_the_loop_whole() {
    if !jq_available() {
        return;
    }
    let task = large_task();
    let context = serde_json::json!({ "task_description": task }).to_string();
    let run = run_step(&context);
    assert!(
        run.out.status.success(),
        "{RECIPE}: step failed with {:?}; stderr:\n{}",
        run.out.status,
        String::from_utf8_lossy(&run.out.stderr)
    );
    let seen = run
        .task_seen
        .unwrap_or_else(|| panic!("{RECIPE}: the loop was not given --task-file"));
    assert_eq!(seen.len(), task.len(), "{RECIPE}: length must match");
    assert!(
        seen == task.as_bytes(),
        "{RECIPE}: the loop must receive the task byte for byte"
    );
}
