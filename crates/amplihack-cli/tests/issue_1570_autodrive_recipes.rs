//! Issue #1570 — the autodrive loop recipes hand the task to
//! `autodrive_loop.sh` through a file, never through the uppercase alias.
//!
//! The runner exports the uppercase `TASK_DESCRIPTION` alias only up to
//! 96 KiB, and not at all once the environment budget is spent. The loop
//! steps used to pass `TASK_DESCRIPTION="${TASK_DESCRIPTION:-}"`, so exactly
//! the large tasks this issue targets reached the loop as an empty string.
//! Each step now reads `task_description` with `write_context_value` and
//! passes the file with `--task-file`.
//!
//! These tests run the real step command from each recipe with only
//! `AMPLIHACK_CONTEXT_FILE` holding the task. `AMPLIHACK_HOME` points at a
//! directory holding the real `recipe_context_value.sh` and a fake
//! `autodrive_loop.sh` that copies the `--task-file` it receives. They skip,
//! with a note, when `jq` is not installed.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECIPES: [(&str, &str, &str); 2] = [
    (
        "autodrive-crusty-loop.yaml",
        "step-02-crusty-loop",
        "CRUSTY_LOOP_PREFLIGHT",
    ),
    (
        "autodrive-merge-loop.yaml",
        "step-02-merge-ready-loop",
        "MERGE_LOOP_PREFLIGHT",
    ),
];

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

/// `amplihack orch helper extract-json` passes stdin through and
/// `extract-field --field F` prints `.F` of it, which is all the step needs.
const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
case "${3:-}" in
  extract-json) cat ;;
  extract-field) jq -r --arg f "$5" '.[$f] // empty' ;;
esac
"#;

const LOOP_STUB: &str = r#"#!/usr/bin/env bash
while [ $# -gt 0 ]; do
  if [ "$1" = "--task-file" ]; then cp -- "$2" "$LOOP_OUT/task"; fi
  if [ "$1" = "--repo" ]; then printf '%s' "$2" > "$LOOP_OUT/repo"; fi
  shift
done
exit 0
"#;

/// Step-02 resolves the PR's linked worktree before the loop starts and runs
/// the rounds there (#1528). The stand-in prints `$PR_WORKTREE_OUT`.
const RESOLVER_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$PR_WORKTREE_OUT"
"#;

/// Step-02 takes the review dir from the guard's `review-dir`, the main
/// checkout of `repo_path`'s repository (#1528). The stand-in prints the
/// repo directory; `autodrive_pr_worktree_tests.rs` runs the real guard.
const GUARD_STUB: &str = r#"#!/usr/bin/env bash
[ "$1" = review-dir ] && printf '%s\n' "$REPO_PATH"
"#;

struct StepRun {
    out: Output,
    task_seen: Option<Vec<u8>>,
    repo_seen: Option<String>,
    pr_worktree: PathBuf,
    _tmp: tempfile::TempDir,
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run one recipe's loop step with `context` as the only context source.
fn run_step(recipe: &str, step_id: &str, preflight_var: &str, context: &str) -> StepRun {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    let tools = tmp.path().join("home/amplifier-bundle/tools");
    let loop_out = tmp.path().join("loop-out");
    let repo = tmp.path().join("repo");
    let private_tmp = tmp.path().join("tmp");
    let pr_worktree = tmp.path().join("pr-worktree");
    for d in [&bin, &tools, &loop_out, &repo, &private_tmp, &pr_worktree] {
        std::fs::create_dir_all(d).unwrap();
    }
    executable(&bin.join("amplihack"), AMPLIHACK_STUB);
    executable(&tools.join("autodrive_loop.sh"), LOOP_STUB);
    executable(&tools.join("autodrive_pr_worktree.sh"), RESOLVER_STUB);
    executable(&tools.join("autodrive_worktree_guard.sh"), GUARD_STUB);
    // merge-loop step-02 refuses to start without the crusty re-review beside
    // the loop driver (PR #1520); the stub loop never runs it.
    executable(
        &tools.join("autodrive_crusty_rereview.sh"),
        "#!/usr/bin/env bash\nexit 0\n",
    );
    std::fs::copy(
        repo_root().join("amplifier-bundle/tools/recipe_context_value.sh"),
        tools.join("recipe_context_value.sh"),
    )
    .unwrap();
    let ctx = tmp.path().join("context.json");
    std::fs::write(&ctx, context).unwrap();
    let preflight = serde_json::json!({
        "state_dir": tmp.path().join("state"),
        "pr": "1570",
    })
    .to_string();

    let out = Command::new("bash")
        .arg("-c")
        .arg(step_command(recipe, step_id))
        .env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
            ),
        )
        .env("HOME", tmp.path())
        .env("TMPDIR", &private_tmp)
        .env("AMPLIHACK_HOME", tmp.path().join("home"))
        .env("REPO_PATH", &repo)
        .env("AMPLIHACK_CONTEXT_FILE", &ctx)
        .env(preflight_var, preflight)
        .env("LOOP_OUT", &loop_out)
        .env("PR_WORKTREE_OUT", &pr_worktree)
        .current_dir(&repo)
        .output()
        .expect("spawn bash");
    StepRun {
        task_seen: std::fs::read(loop_out.join("task")).ok(),
        repo_seen: std::fs::read_to_string(loop_out.join("repo")).ok(),
        pr_worktree,
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
    for (recipe, step, preflight) in RECIPES {
        let run = run_step(recipe, step, preflight, &context);
        assert!(
            run.out.status.success(),
            "{recipe}: step failed with {:?}; stderr:\n{}",
            run.out.status,
            String::from_utf8_lossy(&run.out.stderr)
        );
        let seen = run
            .task_seen
            .unwrap_or_else(|| panic!("{recipe}: the loop was not given --task-file"));
        assert_eq!(seen.len(), task.len(), "{recipe}: length must match");
        assert!(
            seen == task.as_bytes(),
            "{recipe}: the loop must receive the task byte for byte"
        );
        // The same loop runs its rounds in the PR worktree (#1528).
        assert_eq!(
            run.repo_seen.as_deref(),
            Some(run.pr_worktree.to_str().unwrap()),
            "{recipe}: the loop must get the resolved PR worktree as --repo"
        );
    }
}

#[test]
fn a_missing_task_fails_the_step_before_the_loop_runs() {
    if !jq_available() {
        return;
    }
    let context = serde_json::json!({ "pr_url": "x" }).to_string();
    for (recipe, step, preflight) in RECIPES {
        let run = run_step(recipe, step, preflight, &context);
        assert!(
            !run.out.status.success(),
            "{recipe}: a missing task_description must fail the step"
        );
        assert!(
            String::from_utf8_lossy(&run.out.stderr).contains("task_description unavailable"),
            "{recipe}: stderr must say the task is unavailable; got:\n{}",
            String::from_utf8_lossy(&run.out.stderr)
        );
        assert!(
            run.task_seen.is_none(),
            "{recipe}: the loop must not run without a task"
        );
    }
}
