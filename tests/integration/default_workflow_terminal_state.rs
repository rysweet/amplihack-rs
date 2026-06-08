//! tests/integration/default_workflow_terminal_state.rs
//!
//! TDD-red contracts for the reusable default-workflow terminal-state probe.
//! These tests define the evidence and output vocabulary required before the
//! implementation exists.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value as JsonValue;
use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipe_path(name: &str) -> PathBuf {
    workspace_root()
        .join("amplifier-bundle")
        .join("recipes")
        .join(format!("{name}.yaml"))
}

fn recipe_text(name: &str) -> String {
    let path = recipe_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_commands(recipe: &Value) -> String {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .filter_map(|step| step.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn step_condition(recipe: &Value, id: &str) -> String {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get("condition").and_then(Value::as_str))
        .unwrap_or_else(|| panic!("recipe step `{id}` must declare a condition"))
        .to_string()
}

struct TerminalRun {
    success: bool,
    stdout: String,
    stderr: String,
    json: JsonValue,
}

struct GitFixture {
    _tmp: TempDir,
    repo: PathBuf,
}

fn run_cmd(dir: &Path, program: &str, args: &[&str]) {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("run {program} {args:?} in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "command failed: {program} {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, content).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn setup_repo() -> GitFixture {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("create repo dir");
    run_cmd(&repo, "git", &["init", "-b", "main"]);
    run_cmd(&repo, "git", &["config", "user.email", "test@example.com"]);
    run_cmd(&repo, "git", &["config", "user.name", "Workflow Test"]);
    write_file(&repo.join("README.md"), "base\n");
    run_cmd(&repo, "git", &["add", "README.md"]);
    run_cmd(&repo, "git", &["commit", "-m", "base"]);
    run_cmd(
        &repo,
        "git",
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    GitFixture { _tmp: tmp, repo }
}

fn create_feature_worktree(fixture: &GitFixture) -> PathBuf {
    run_cmd(&fixture.repo, "git", &["branch", "feature"]);
    let worktree = fixture._tmp.path().join("feature-worktree");
    run_cmd(
        &fixture.repo,
        "git",
        &["worktree", "add", worktree.to_str().unwrap(), "feature"],
    );
    worktree
}

fn commit_feature_change(worktree: &Path) {
    write_file(&worktree.join("feature.txt"), "feature\n");
    run_cmd(worktree, "git", &["add", "feature.txt"]);
    run_cmd(worktree, "git", &["commit", "-m", "feature"]);
}

fn git_head(dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git rev-parse in {}: {e}", dir.display()));
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8 git head")
        .trim()
        .to_string()
}

fn fake_gh(tmp: &TempDir, body: &str, exit_code: i32) -> PathBuf {
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("create fake gh bin");
    let gh = bin_dir.join("gh");
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ {exit_code} -ne 0 ]; then
  echo "fake gh failure" >&2
  exit {exit_code}
fi
cat <<'JSON'
{body}
JSON
"#
    );
    fs::write(&gh, script).expect("write fake gh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod fake gh");
    }
    bin_dir
}

fn matching_pr_json(worktree: &Path, state: &str, merged_at: &str) -> String {
    serde_json::json!({
        "url": "https://github.com/owner/repo/pull/7",
        "number": 7,
        "state": state,
        "mergedAt": merged_at,
        "headRefName": "feature",
        "baseRefName": "main",
        "headRefOid": git_head(worktree),
        "headRepositoryOwner": { "login": "owner" },
        "headRepository": { "name": "repo" },
        "isCrossRepository": false,
        "statusCheckRollup": []
    })
    .to_string()
}

fn run_terminal_state(
    repo_path: &Path,
    worktree_path: Option<&Path>,
    branch_name: &str,
    pr_url: Option<&str>,
    fake_path: &Path,
) -> TerminalRun {
    let command = step_commands(&load_recipe("workflow-terminal-state"));
    let old_path = std::env::var("PATH").unwrap_or_default();
    let path = format!("{}:{old_path}", fake_path.display());
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .env("PATH", path)
        .env("REPO_PATH", repo_path)
        .env("BRANCH_NAME", branch_name)
        .env("BASE_REF", "main")
        .env("PR_NUMBER", "")
        .env("PR_URL", pr_url.unwrap_or(""))
        .env("GOAL_ALREADY_MET", "false");
    if let Some(worktree_path) = worktree_path {
        cmd.env("RECIPE_VAR_worktree_setup__worktree_path", worktree_path);
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("run workflow-terminal-state: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let json_line = stdout
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| {
            panic!("terminal-state emitted no JSON\nstdout:\n{stdout}\nstderr:\n{stderr}")
        });
    let json = serde_json::from_str(json_line)
        .unwrap_or_else(|e| panic!("parse terminal JSON `{json_line}`: {e}"));
    TerminalRun {
        success: output.status.success(),
        stdout,
        stderr,
        json,
    }
}

fn assert_contains_all(haystack: &str, needles: &[&str], context: &str) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "{context} must contain `{needle}`"
        );
    }
}

fn assert_ordered(haystack: &str, needles: &[&str], context: &str) {
    let mut previous = 0;
    for needle in needles {
        let position = haystack[previous..]
            .find(needle)
            .map(|offset| previous + offset)
            .unwrap_or_else(|| panic!("{context} must contain `{needle}` in order"));
        assert!(
            position >= previous,
            "{context} must place `{needle}` after earlier terminal checks"
        );
        previous = position;
    }
}

#[test]
fn terminal_state_recipe_exists_with_required_io_contract() {
    let recipe = load_recipe("workflow-terminal-state");
    let text = recipe_text("workflow-terminal-state");

    let input_names: Vec<_> = recipe
        .get("inputs")
        .and_then(Value::as_sequence)
        .expect("terminal-state recipe must declare inputs")
        .iter()
        .filter_map(|input| input.get("name").and_then(Value::as_str))
        .collect();

    for required_input in [
        "worktree_setup.worktree_path",
        "repo_path",
        "branch_name",
        "base_ref",
        "pr_number",
        "pr_url",
        "goal_already_met",
    ] {
        assert!(
            input_names.contains(&required_input),
            "workflow-terminal-state must declare input `{required_input}`"
        );
    }

    assert_contains_all(
        &text,
        &[
            "terminal_success",
            "terminal_state",
            "terminal_reason",
            "publish_status",
            "should_publish",
            "should_finalize",
            "should_run_ci_wait",
            "should_merge",
        ],
        "terminal-state recipe output contract",
    );
}

#[test]
fn terminal_state_probe_detects_outcomes_in_safe_fail_closed_order() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_ordered(
        &command,
        &[
            "git status --porcelain",
            "dirty worktree blocks terminal success",
            "mergedAt",
            "MERGED",
            "closed-unmerged",
            "CLOSED_OBSOLETE",
            "git diff --quiet",
            "NO_DIFF_SUCCESS",
            "FOLLOWUP_CREATED",
        ],
        "terminal-state detection order",
    );

    assert_contains_all(
        &command,
        &[
            "BLOCKED_CI",
            "FAILED_CLOSED_UNMERGED",
            "FAILED_MEANINGFUL_DIFF",
            "ambiguous",
            "exit 1",
        ],
        "terminal-state fail-closed semantics",
    );
}

#[test]
fn terminal_state_probe_validates_inputs_before_git_or_github_trust() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_contains_all(
        &command,
        &[
            "git rev-parse --is-inside-work-tree",
            "git check-ref-format --branch",
            "base_ref",
            "pr_number",
            "pr_url",
            "^[0-9][0-9]*$",
            "headRefName",
            "baseRefName",
            "headRefOid",
            "mergedAt",
        ],
        "terminal-state input validation",
    );

    let dirty_position = command
        .find("git status --porcelain")
        .expect("dirty worktree check must exist");
    for trusted_probe in ["gh_with_retry \"pr view\"", "git diff --quiet"] {
        let probe_position = command
            .find(trusted_probe)
            .unwrap_or_else(|| panic!("trusted probe `{trusted_probe}` must exist"));
        assert!(
            dirty_position < probe_position,
            "dirty worktree must be checked before `{trusted_probe}` can produce terminal success"
        );
    }
}

#[test]
fn terminal_state_probe_wraps_github_metadata_calls_with_bounded_retry() {
    let recipe = load_recipe("workflow-terminal-state");
    let command = step_commands(&recipe);

    assert_contains_all(
        &command,
        &[
            "sanitize_gh_stderr",
            "is_transient_gh_error",
            "gh_with_retry",
            "timeout 60 gh",
            "for attempt in 1 2 3",
            "retrying (${attempt}/3)",
            "gh_with_retry \"pr view\"",
            "gh_with_retry \"pr list\"",
            "unavailable PR metadata",
        ],
        "terminal-state GitHub adapter resilience",
    );

    assert!(
        !command.contains("gh pr view \"$pr_target\"")
            && !command.contains("gh pr list --head \"$BRANCH_INPUT\""),
        "terminal-state must not call gh metadata endpoints directly without retry/error handling"
    );
}

#[test]
fn default_workflow_propagates_terminal_state_context_between_phase_recipes() {
    let recipe = load_recipe("default-workflow");
    let context = recipe
        .get("context")
        .and_then(Value::as_mapping)
        .expect("default-workflow must declare a context mapping");

    for required_key in [
        "terminal_success",
        "terminal_state",
        "terminal_reason",
        "publish_status",
        "should_publish",
        "should_finalize",
        "should_run_ci_wait",
        "should_merge",
    ] {
        assert!(
            context.contains_key(Value::String(required_key.to_string())),
            "default-workflow context must propagate `{required_key}`"
        );
    }
}

#[test]
fn pr_review_phase_skips_stale_review_steps_after_terminal_success() {
    let recipe = load_recipe("workflow-pr-review");
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("workflow-pr-review must contain top-level steps");

    for step in steps {
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .expect("workflow-pr-review steps must have ids");
        let condition = step.get("condition").and_then(Value::as_str).unwrap_or("");
        assert!(
            condition.contains("terminal_success != 'true'")
                || condition.contains("terminal_state.terminal_success != 'true'")
                || condition.contains("should_finalize == 'true'"),
            "workflow-pr-review step `{id}` must be gated off for terminal_success=true"
        );
    }
}

#[test]
fn terminal_state_uses_worktree_checkout_for_meaningful_diff() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    commit_feature_change(&worktree);
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, "{}", 0);

    let run = run_terminal_state(&fixture.repo, Some(&worktree), "feature", None, &fake_path);

    assert!(
        run.success,
        "meaningful diff should continue workflow, not fail\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(run.json["terminal_success"], "false");
    assert_eq!(run.json["terminal_state"], "FOLLOWUP_CREATED");
    assert_eq!(run.json["branch_diff_status"], "has-diff");
}

#[test]
fn terminal_state_fails_when_repo_checkout_is_not_requested_branch() {
    let fixture = setup_repo();
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, "{}", 0);

    let run = run_terminal_state(&fixture.repo, None, "feature", None, &fake_path);

    assert!(!run.success, "wrong checkout must fail closed");
    assert_eq!(run.json["terminal_state"], "FAILED_WRONG_BRANCH");
}

#[test]
fn terminal_state_continues_for_dirty_target_worktree_without_terminal_success() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    write_file(&worktree.join("dirty.txt"), "uncommitted\n");
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, "{}", 0);

    let run = run_terminal_state(&fixture.repo, Some(&worktree), "feature", None, &fake_path);

    assert!(
        run.success,
        "dirty worktree should continue publish/finalize so pending work can be committed\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(run.json["terminal_success"], "false");
    assert_eq!(run.json["terminal_state"], "FOLLOWUP_CREATED");
    assert_eq!(run.json["should_publish"], "true");
    assert_eq!(run.json["should_finalize"], "true");
}

#[test]
fn terminal_state_rejects_stale_pr_head_sha() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    commit_feature_change(&worktree);
    let mut pr: JsonValue =
        serde_json::from_str(&matching_pr_json(&worktree, "OPEN", "")).expect("matching PR JSON");
    pr["headRefOid"] = JsonValue::String("0000000000000000000000000000000000000000".to_string());
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, &pr.to_string(), 0);

    let run = run_terminal_state(
        &fixture.repo,
        Some(&worktree),
        "feature",
        Some("https://github.com/owner/repo/pull/7"),
        &fake_path,
    );

    assert!(!run.success, "stale PR head SHA must fail closed");
    assert_eq!(run.json["terminal_state"], "FAILED_INVALID_INPUT");
    assert!(
        run.stderr.contains("stale PR metadata"),
        "stderr should explain stale PR metadata: {}",
        run.stderr
    );
}

#[test]
fn terminal_state_rejects_cross_repo_pr_url() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    commit_feature_change(&worktree);
    let mut pr: JsonValue =
        serde_json::from_str(&matching_pr_json(&worktree, "OPEN", "")).expect("matching PR JSON");
    pr["headRepositoryOwner"]["login"] = JsonValue::String("other".to_string());
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, &pr.to_string(), 0);

    let run = run_terminal_state(
        &fixture.repo,
        Some(&worktree),
        "feature",
        Some("https://github.com/other/repo/pull/7"),
        &fake_path,
    );

    assert!(!run.success, "cross-repo PR metadata must fail closed");
    assert_eq!(run.json["terminal_state"], "FAILED_INVALID_INPUT");
    assert!(
        run.stderr.contains("does not match current repo"),
        "stderr should explain repo identity mismatch: {}",
        run.stderr
    );
}

#[test]
fn terminal_state_rejects_closed_unmerged_pr_with_meaningful_diff() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    commit_feature_change(&worktree);
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, &matching_pr_json(&worktree, "CLOSED", ""), 0);

    let run = run_terminal_state(
        &fixture.repo,
        Some(&worktree),
        "feature",
        Some("https://github.com/owner/repo/pull/7"),
        &fake_path,
    );

    assert!(
        !run.success,
        "closed-unmerged PR with diff must fail closed"
    );
    assert_eq!(run.json["terminal_state"], "FAILED_CLOSED_UNMERGED");
}

#[test]
fn terminal_state_fails_closed_when_required_pr_metadata_is_unavailable() {
    let fixture = setup_repo();
    let worktree = create_feature_worktree(&fixture);
    commit_feature_change(&worktree);
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, "", 1);

    let run = run_terminal_state(
        &fixture.repo,
        Some(&worktree),
        "feature",
        Some("https://github.com/owner/repo/pull/7"),
        &fake_path,
    );

    assert!(
        !run.success,
        "explicit PR metadata failure must fail closed\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(run.json["terminal_state"], "FAILED_INVALID_INPUT");
    assert!(
        run.stderr.contains("unavailable PR metadata"),
        "stderr should explain unavailable metadata: {}",
        run.stderr
    );
}

#[test]
fn terminal_state_allows_git_only_no_diff_when_no_pr_dependency_exists() {
    let fixture = setup_repo();
    let fake_tmp = TempDir::new().expect("fake gh tempdir");
    let fake_path = fake_gh(&fake_tmp, "", 1);

    let run = run_terminal_state(&fixture.repo, None, "main", None, &fake_path);

    assert!(
        run.success,
        "clean no-diff branch can prove terminal success without PR metadata\nstdout:\n{}\nstderr:\n{}",
        run.stdout, run.stderr
    );
    assert_eq!(run.json["terminal_success"], "true");
    assert_eq!(run.json["terminal_state"], "NO_DIFF_SUCCESS");
}

#[test]
fn terminal_success_states_execute_runner_skip_gates_for_mutation_publish_ci_and_merge() {
    let publish = load_recipe("workflow-publish");
    let finalize = load_recipe("workflow-finalize");
    let tmp = TempDir::new().expect("tempdir");
    let marker = tmp.path().join("mutation-marker");

    for terminal_state in ["MERGED", "CLOSED_OBSOLETE", "NO_DIFF_SUCCESS"] {
        let recipe_path = tmp
            .path()
            .join(format!("terminal-gates-{terminal_state}.yaml"));
        let recipe = format!(
            r#"name: terminal-gates-{terminal_state}
steps:
  - id: terminal
    type: bash
    parse_json: true
    output: terminal_state
    command: |
      jq -nc --arg state "{terminal_state}" '{{terminal_success:"true",terminal_state:$state,publish_status:$state,should_publish:"false",should_finalize:"false",should_run_ci_wait:"false",should_merge:"false"}}'
  - id: publish-version
    type: bash
    condition: {publish_version_condition:?}
    command: |
      echo publish-version >> {marker:?}
  - id: publish-commit-push
    type: bash
    condition: {publish_commit_condition:?}
    command: |
      echo publish-commit-push >> {marker:?}
  - id: publish-create-pr
    type: bash
    condition: {publish_pr_condition:?}
    command: |
      echo publish-create-pr >> {marker:?}
  - id: publish-outside-in
    type: bash
    condition: {publish_outside_in_condition:?}
    command: |
      echo publish-outside-in >> {marker:?}
  - id: finalize-push-cleanup
    type: bash
    condition: {finalize_cleanup_condition:?}
    command: |
      echo finalize-push-cleanup >> {marker:?}
  - id: finalize-pr-ready
    type: bash
    condition: {finalize_ready_condition:?}
    command: |
      echo finalize-pr-ready >> {marker:?}
  - id: finalize-ci-wait
    type: bash
    condition: {finalize_ci_condition:?}
    command: |
      echo finalize-ci-wait >> {marker:?}
"#,
            terminal_state = terminal_state,
            marker = marker.display().to_string(),
            publish_version_condition = step_condition(&publish, "step-14-bump-version"),
            publish_commit_condition = step_condition(&publish, "step-15-commit-push"),
            publish_pr_condition = step_condition(&publish, "step-16-create-draft-pr"),
            publish_outside_in_condition = step_condition(&publish, "step-16b-outside-in-fix-loop"),
            finalize_cleanup_condition = step_condition(&finalize, "step-20b-push-cleanup"),
            finalize_ready_condition = step_condition(&finalize, "step-21-pr-ready"),
            finalize_ci_condition = step_condition(&finalize, "step-22-ensure-mergeable"),
        );
        write_file(&recipe_path, &recipe);

        let output = Command::new("recipe-runner-rs")
            .arg(&recipe_path)
            .arg("--output-format")
            .arg("json")
            .arg("-C")
            .arg(tmp.path())
            .output()
            .unwrap_or_else(|e| panic!("run recipe-runner-rs for {terminal_state}: {e}"));
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            output.status.success(),
            "terminal gate recipe for {terminal_state} must succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );

        let result: JsonValue = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("parse runner JSON: {e}\n{stdout}"));
        for skipped_step in [
            "publish-version",
            "publish-commit-push",
            "publish-create-pr",
            "publish-outside-in",
            "finalize-push-cleanup",
            "finalize-pr-ready",
            "finalize-ci-wait",
        ] {
            let status = result["step_results"]
                .as_array()
                .and_then(|steps| {
                    steps.iter().find_map(|step| {
                        (step["step_id"] == skipped_step).then(|| step["status"].as_str())
                    })
                })
                .flatten()
                .unwrap_or_else(|| panic!("missing step status for {skipped_step}: {stdout}"));
            assert_eq!(
                status, "skipped",
                "{skipped_step} must be skipped for terminal state {terminal_state}"
            );
        }
    }

    assert!(
        !marker.exists(),
        "terminal-success gate failure executed a mutation step: {}",
        fs::read_to_string(&marker).unwrap_or_default()
    );
}
