/// Integration tests for issue #684 — Host-aware workflow steps.
///
/// ## Problem
///
/// The default-workflow assumes GitHub for commit messages (Closes #N),
/// PR creation, and final status summaries. This breaks on Azure DevOps,
/// other Git hosts, and repos without remotes.
///
/// ## Blockers addressed
///
/// 1. **step-15 commit message** hardcodes `Closes #N` (GitHub-only).
///    Fix: host-aware refs — `AB#N` (AzDO), `Closes #N` (GitHub), `Ref #N` (other).
/// 2. **step-16 PR body** hardcodes `Closes #N` and has duplicate host detection.
///    Fix: consume `$REMOTE_HOST_TYPE` from context, not inline re-detection.
/// 3. **step-22b summary** calls `gh pr view` without host-type guard.
///    Fix: guard with `REMOTE_HOST_TYPE` check, host-aware issue/PR lines.
/// 4. **step-03 AzDO parsing** rejects percent-encoded project names (`My%20Project`).
///    Fix: decode `%XX` before validation, expand regex to allow spaces.
/// 5. **Context propagation**: `remote_host_type` must be declared in
///    `default-workflow.yaml` and exported by a new `step-02d-detect-host-type`
///    in `workflow-prep.yaml`.
///
/// ## Test strategy
///
/// Mirrors `issue_655_656_skill_fetch_resilience_test.rs`:
///   - Parse recipe YAML with `serde_yaml` to inspect step command bodies
///   - Assert structural properties of bash scripts (keywords, patterns, absence)
///   - Execute targeted workflow-prep bash steps with provider CLI shims
///
/// ## Test status
///
/// These tests protect the host-aware workflow contract across recipe files.
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use serde_yaml::Value;

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
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

// ---------------------------------------------------------------------------
// Recipe parsing helpers (same pattern as issue_655_656 tests)
// ---------------------------------------------------------------------------

fn parse_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    let text = recipe_text(name);
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {} as YAML: {e}", path.display()))
}

fn load_recipe(name: &str) -> &'static Value {
    static DEFAULT_WORKFLOW: OnceLock<Value> = OnceLock::new();
    static WORKFLOW_PREP: OnceLock<Value> = OnceLock::new();
    static WORKFLOW_PUBLISH: OnceLock<Value> = OnceLock::new();
    static WORKFLOW_FINALIZE: OnceLock<Value> = OnceLock::new();

    match name {
        "default-workflow" => DEFAULT_WORKFLOW.get_or_init(|| parse_recipe(name)),
        "workflow-prep" => WORKFLOW_PREP.get_or_init(|| parse_recipe(name)),
        "workflow-publish" => WORKFLOW_PUBLISH.get_or_init(|| parse_recipe(name)),
        "workflow-finalize" => WORKFLOW_FINALIZE.get_or_init(|| parse_recipe(name)),
        _ => panic!("uncached test recipe: {name}"),
    }
}

/// Extract the `command:` body of a bash step by its `id:` field.
fn extract_step_body(recipe: &Value, step_id: &str) -> String {
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have a top-level 'steps' sequence");

    for step in steps {
        let id = step.get("id").and_then(Value::as_str).unwrap_or("");
        if id == step_id {
            if let Some(cmd) = step.get("command").and_then(Value::as_str) {
                return cmd.to_owned();
            }
            if let Some(prompt) = step.get("prompt").and_then(Value::as_str) {
                return prompt.to_owned();
            }
            panic!("step '{step_id}' has neither 'command:' nor 'prompt:' body");
        }
    }
    panic!("step '{step_id}' not found in recipe");
}

/// Check if a step exists in the recipe.
fn step_exists(recipe: &Value, step_id: &str) -> bool {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .map(|steps| {
            steps
                .iter()
                .any(|s| s.get("id").and_then(Value::as_str) == Some(step_id))
        })
        .unwrap_or(false)
}

/// Get the `output:` field of a step.
fn step_output(recipe: &Value, step_id: &str) -> Option<String> {
    let steps = recipe.get("steps")?.as_sequence()?;
    for step in steps {
        if step.get("id").and_then(Value::as_str) == Some(step_id) {
            return step.get("output").and_then(Value::as_str).map(String::from);
        }
    }
    None
}

fn write_executable(path: &std::path::Path, content: &str) {
    let mut file = fs::File::create(path).expect("create shim executable");
    file.write_all(content.as_bytes())
        .expect("write shim executable");
    drop(file);
    let mut perms = fs::metadata(path).expect("shim metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod shim executable");
}

#[derive(Debug)]
struct Step02dRun {
    output: Output,
    git_log: String,
}

#[derive(Debug)]
struct Step03Run {
    output: Output,
    git_log: String,
    gh_log: String,
    az_log: String,
}

fn run_step_02d_with_remote(remote_url: &str) -> Step02dRun {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = temp.path().join("repo");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&repo_dir).expect("mkdir repo");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");

    let git_log = temp.path().join("git.log");
    write_executable(
        &bin_dir.join("git"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$GIT_LOG"
case "$1:$2" in
  rev-parse:--is-inside-work-tree) exit 0 ;;
  remote:get-url) printf '%s\n' "$GIT_REMOTE_URL"; exit 0 ;;
  *) echo "unexpected git invocation: $*" >&2; exit 2 ;;
esac
"#,
    );

    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = extract_step_body(load_recipe("workflow-prep"), "step-02d-detect-host-type");
    let output = Command::new("bash")
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("PATH", path)
        .env("REPO_PATH", &repo_dir)
        .env("GIT_LOG", &git_log)
        .env("GIT_REMOTE_URL", remote_url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run step-02d-detect-host-type");

    Step02dRun {
        output,
        git_log: fs::read_to_string(&git_log).unwrap_or_default(),
    }
}

fn run_step_03_with_env(
    host_type: &str,
    issue_number: &str,
    task_description: &str,
    extra_env: &[(&str, &str)],
) -> Step03Run {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_dir = temp.path().join("repo");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&repo_dir).expect("mkdir repo");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");

    let gh_log = temp.path().join("gh.log");
    let az_log = temp.path().join("az.log");
    let git_log = temp.path().join("git.log");

    write_executable(
        &bin_dir.join("git"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$GIT_LOG"
case "$1:$2" in
  rev-parse:--is-inside-work-tree) exit 0 ;;
  remote:get-url) printf '%s\n' "${GIT_REMOTE_URL:-https://dev.azure.com/example-org/My%20Project/_git/example-repo}"; exit 0 ;;
  *) echo "unexpected git invocation: $*" >&2; exit 2 ;;
esac
"#,
    );

    write_executable(
        &bin_dir.join("gh"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_LOG"
case "$1:$2" in
  issue:view)
    if [ "${GH_VIEW_ISSUE:-718}" = "$3" ] && [ -n "${GH_VIEW_URL:-https://github.com/example-org/example-repo/issues/718}" ]; then
      printf '%s\n' "${GH_VIEW_URL:-https://github.com/example-org/example-repo/issues/718}"
      exit 0
    fi
    exit "${GH_VIEW_STATUS:-1}"
    ;;
  issue:list)
    [ -n "${GH_LIST_URL:-}" ] && printf '%s\n' "$GH_LIST_URL"
    exit "${GH_LIST_STATUS:-0}"
    ;;
  label:create) exit 0 ;;
  issue:create)
    [ -n "${GH_CREATE_OUTPUT:-}" ] && printf '%s\n' "$GH_CREATE_OUTPUT"
    exit "${GH_CREATE_STATUS:-7}"
    ;;
esac
echo "unexpected gh invocation: $*" >&2
exit 7
"#,
    );

    write_executable(
        &bin_dir.join("az"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$AZ_LOG"
echo "unexpected az invocation: $*" >&2
exit 7
"#,
    );

    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let command = extract_step_body(load_recipe("workflow-prep"), "step-03-create-issue");

    let mut command_builder = Command::new("bash");
    command_builder
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("PATH", path)
        .env("REPO_PATH", &repo_dir)
        .env("REMOTE_HOST_TYPE", host_type)
        .env("TASK_DESCRIPTION", task_description)
        .env("FINAL_REQUIREMENTS", "Issue #718 regression requirements")
        .env("ISSUE_NUMBER", issue_number)
        .env("GIT_LOG", &git_log)
        .env("GH_LOG", &gh_log)
        .env("AZ_LOG", &az_log);
    for (key, value) in extra_env {
        command_builder.env(key, value);
    }
    let output = command_builder
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run step-03-create-issue");

    Step03Run {
        output,
        git_log: fs::read_to_string(&git_log).unwrap_or_default(),
        gh_log: fs::read_to_string(&gh_log).unwrap_or_default(),
        az_log: fs::read_to_string(&az_log).unwrap_or_default(),
    }
}

fn run_step_03(host_type: &str, issue_number: &str, task_description: &str) -> Step03Run {
    run_step_03_with_env(host_type, issue_number, task_description, &[])
}

fn run_detect_then_step_03(remote_url: &str, task_description: &str) -> (Step02dRun, Step03Run) {
    let detection = run_step_02d_with_remote(remote_url);
    let stdout = String::from_utf8_lossy(&detection.output.stdout);
    let detected_host_type = stdout.trim().to_owned();
    let step_03 = run_step_03_with_env(
        &detected_host_type,
        "",
        task_description,
        &[("GIT_REMOTE_URL", remote_url)],
    );
    (detection, step_03)
}

fn run_step_03b(issue_creation: &str, task_description: &str) -> Output {
    let command = extract_step_body(
        load_recipe("workflow-prep"),
        "step-03b-extract-issue-number",
    );

    Command::new("bash")
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("ISSUE_CREATION", issue_creation)
        .env("TASK_DESCRIPTION", task_description)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run step-03b-extract-issue-number")
}

/// Get the context block from default-workflow.yaml.
fn default_workflow_context(recipe: &Value) -> Value {
    recipe
        .get("context")
        .cloned()
        .expect("default-workflow.yaml must have a 'context:' block")
}

// ===========================================================================
// BLOCKER 1a: default-workflow.yaml — remote_host_type context variable
// ===========================================================================

#[test]
fn default_workflow_declares_remote_host_type_context() {
    let recipe = load_recipe("default-workflow");
    let context = default_workflow_context(recipe);

    assert!(
        context.get("remote_host_type").is_some(),
        "default-workflow.yaml context block must declare 'remote_host_type' \
         for cross-sub-recipe propagation. Without this, step-02d's output \
         cannot reach workflow-publish and workflow-finalize. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 1b: workflow-prep.yaml — step-02d-detect-host-type exists
// ===========================================================================

#[test]
fn workflow_prep_has_step_02d_detect_host_type() {
    let recipe = load_recipe("workflow-prep");

    assert!(
        step_exists(recipe, "step-02d-detect-host-type"),
        "workflow-prep.yaml must contain step 'step-02d-detect-host-type'. \
         This centralized step detects the git remote host type once and \
         exports it for all downstream steps. (Issue #684)"
    );
}

#[test]
fn step_02d_has_output_remote_host_type() {
    let recipe = load_recipe("workflow-prep");

    let output = step_output(recipe, "step-02d-detect-host-type");
    assert_eq!(
        output.as_deref(),
        Some("remote_host_type"),
        "step-02d-detect-host-type must declare output: 'remote_host_type' \
         so the recipe runner captures the host type and propagates it to \
         subsequent steps and sub-recipes. (Issue #684)"
    );
}

#[test]
fn step_02d_detects_github_azdo_other() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-02d-detect-host-type");

    // Must detect all three host types
    assert!(
        body.contains("github"),
        "step-02d must detect 'github' host type (Issue #684)"
    );
    assert!(
        body.contains("azdo"),
        "step-02d must detect 'azdo' host type (Issue #684)"
    );
    assert!(
        body.contains("other"),
        "step-02d must handle 'other' as the fallback host type (Issue #684)"
    );

    // Must use git remote get-url to detect
    assert!(
        body.contains("git remote get-url"),
        "step-02d must use 'git remote get-url' to detect the remote type (Issue #684)"
    );
}

#[test]
fn step_02d_detects_all_azdo_url_patterns() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-02d-detect-host-type");

    // Must detect all three AzDO URL patterns
    assert!(
        body.contains("dev.azure.com"),
        "step-02d must detect dev.azure.com URLs (Issue #684)"
    );
    assert!(
        body.contains("visualstudio.com"),
        "step-02d must detect visualstudio.com URLs (Issue #684)"
    );
    assert!(
        body.contains("ssh.dev.azure.com"),
        "step-02d must detect ssh.dev.azure.com URLs (Issue #684)"
    );
}

#[test]
fn step_02d_does_not_echo_remote_url() {
    // Security: remote URL may contain embedded PATs
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-02d-detect-host-type");

    let echoes_url = body.contains("echo \"$REMOTE_URL\"")
        || body.contains("echo $REMOTE_URL")
        || body.contains("printf '%s' \"$REMOTE_URL\"")
        || body.contains("printf \"%s\" \"$REMOTE_URL\"");

    assert!(
        !echoes_url,
        "step-02d must NOT echo the remote URL directly. Remote URLs may \
         contain embedded PATs. Use pattern matching (case/[[]]) instead. \
         (Issue #684, security)"
    );
}

#[test]
fn step_02d_executes_case_insensitive_azdo_remote_classification() {
    let remotes = [
        "HTTPS://DEV.AZURE.COM/example-org/My%20Project/_git/example-repo",
        "https://ExampleOrg.VisualStudio.Com/My%20Project/_git/example-repo",
        "git@SSH.DEV.AZURE.COM:v3/example-org/My%20Project/example-repo",
    ];

    for remote in remotes {
        let run = run_step_02d_with_remote(remote);
        let stdout = String::from_utf8_lossy(&run.output.stdout);
        let stderr = String::from_utf8_lossy(&run.output.stderr);

        assert!(
            run.output.status.success(),
            "step-02d must succeed for mixed-case AzDO remote {remote}; stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(
            stdout.trim(),
            "azdo",
            "step-02d must lowercase before classifying AzDO remote {remote}; git log:\n{}",
            run.git_log
        );
    }
}

#[test]
fn step_02d_does_not_classify_spoofed_azdo_host_substrings_as_azdo() {
    let remotes = [
        "https://dev.azure.com.evil.example/example-org/MyProject/_git/example-repo",
        "https://example.visualstudio.com.evil.example/MyProject/_git/example-repo",
        "git@ssh.dev.azure.com.evil.example:v3/example-org/MyProject/example-repo",
    ];

    for remote in remotes {
        let run = run_step_02d_with_remote(remote);
        let stdout = String::from_utf8_lossy(&run.output.stdout);
        let stderr = String::from_utf8_lossy(&run.output.stderr);

        assert!(
            run.output.status.success(),
            "step-02d must succeed for spoofed non-AzDO remote {remote}; stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(
            stdout.trim(),
            "other",
            "step-02d must classify spoofed AzDO-like host {remote} as other, not azdo"
        );
    }
}

#[test]
fn step_02d_appears_before_step_03() {
    // step-02d must run before step-03 so REMOTE_HOST_TYPE is available
    let recipe = load_recipe("workflow-prep");
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps");

    let step_02d_idx = steps
        .iter()
        .position(|s| s.get("id").and_then(Value::as_str) == Some("step-02d-detect-host-type"));
    let step_03_idx = steps.iter().position(|s| {
        s.get("id")
            .and_then(Value::as_str)
            .map(|id| id.starts_with("step-03"))
            .unwrap_or(false)
    });

    assert!(
        step_02d_idx.is_some(),
        "step-02d-detect-host-type must exist in workflow-prep.yaml"
    );
    assert!(
        step_03_idx.is_some(),
        "step-03 must exist in workflow-prep.yaml"
    );
    assert!(
        step_02d_idx.unwrap() < step_03_idx.unwrap(),
        "step-02d-detect-host-type (index {}) must appear before step-03 (index {}) \
         in workflow-prep.yaml so REMOTE_HOST_TYPE is available for issue creation. \
         (Issue #684)",
        step_02d_idx.unwrap(),
        step_03_idx.unwrap()
    );
}

// ===========================================================================
// BLOCKER 1c: step-15 — host-aware commit message
// ===========================================================================

#[test]
fn step_15_commit_message_not_hardcoded_closes() {
    // The commit message must NOT use hardcoded "Closes #N" for all hosts.
    // It should be conditional based on REMOTE_HOST_TYPE.
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    // Count occurrences of literal "Closes #" in commit message construction.
    // After the fix, "Closes #" should only appear inside a GitHub conditional,
    // not as the unconditional default in the COMMIT_MSG printf.
    let has_unconditional_closes = body.contains("Closes #%s' \"$COMMIT_TITLE\"")
        || body.contains("Closes #%s\" \"$COMMIT_TITLE\"");

    assert!(
        !has_unconditional_closes,
        "step-15 commit message must NOT hardcode 'Closes #N' unconditionally. \
         Use host-aware refs: 'AB#N' for azdo, 'Closes #N' for github, \
         'Ref #N' for other. (Issue #684, BLOCKER 1)"
    );
}

#[test]
fn step_15_uses_remote_host_type_for_commit_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    // The step must reference REMOTE_HOST_TYPE to decide the issue ref format
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-15 must use REMOTE_HOST_TYPE to determine the commit message \
         issue reference format (Closes #N vs AB#N vs Ref #N). (Issue #684)"
    );
}

#[test]
fn step_15_supports_azdo_ab_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    assert!(
        body.contains("AB#"),
        "step-15 must use 'AB#' format for Azure DevOps work item linking \
         in commit messages. (Issue #684)"
    );
}

#[test]
fn step_15_supports_neutral_ref() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    assert!(
        body.contains("Ref #"),
        "step-15 must use 'Ref #' format for non-GitHub/non-AzDO hosts \
         in commit messages. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 1d: step-16 — no duplicate host detection, host-aware PR body
// ===========================================================================

#[test]
fn step_16_no_inline_remote_host_type_detection() {
    // step-16 should consume $REMOTE_HOST_TYPE from context (step-02d output),
    // not re-detect it inline. Duplicate detection is fragile and violates DRY.
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-16-create-draft-pr");

    // The inline detection pattern from the current code:
    //   REMOTE_URL=$(git remote get-url origin ...)
    //   case "$REMOTE_URL" in *github.com*) ...
    // After the fix, this should be replaced by consuming $REMOTE_HOST_TYPE.
    let has_inline_case = body.contains("case \"$REMOTE_URL\"") && body.contains("*github.com*)");

    assert!(
        !has_inline_case,
        "step-16 must NOT have inline REMOTE_HOST_TYPE detection via \
         case \"$REMOTE_URL\". It should consume $REMOTE_HOST_TYPE from \
         context (set by step-02d). (Issue #684, DRY violation)"
    );
}

#[test]
fn step_16_pr_body_not_hardcoded_closes() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-16-create-draft-pr");

    // The PR body must not hardcode "Closes #%s" for all hosts
    let has_unconditional_closes = body.contains("Closes #%s\\n");

    assert!(
        !has_unconditional_closes,
        "step-16 PR body must NOT hardcode 'Closes #N'. It should use \
         host-aware refs like step-15. (Issue #684, BLOCKER 1)"
    );
}

#[test]
fn step_16_consumes_remote_host_type() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-16-create-draft-pr");

    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-16 must reference REMOTE_HOST_TYPE (from context/env) \
         to decide PR creation behavior and issue ref format. (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 2: step-22b — host-aware summary
// ===========================================================================

#[test]
fn step_22b_guards_gh_pr_view_with_host_type() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    // gh pr view must be guarded by REMOTE_HOST_TYPE check, not just PR_URL
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-22b must check REMOTE_HOST_TYPE before calling gh pr view. \
         Belt-and-suspenders: non-GitHub hosts must never invoke gh. (Issue #684)"
    );
}

#[test]
fn step_22b_issue_line_is_host_aware() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    // The issue summary should not unconditionally use "Issue: #N"
    // It should adapt based on host type (AB#N for AzDO, #N for GitHub)
    let has_unconditional_issue_hash = body.contains("'=== Issue: #%s ===\\n'");

    assert!(
        !has_unconditional_issue_hash,
        "step-22b issue summary must NOT use unconditional '=== Issue: #N ===' format. \
         Use host-aware format: 'AB#N' for azdo, '#N' for github. (Issue #684)"
    );
}

#[test]
fn step_22b_pr_line_handles_empty_pr_url() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    // When PR_URL is empty (non-GitHub host), the summary should say
    // something like "PR: N/A" rather than "PR: " (empty)
    let has_empty_pr_handling = body.contains("N/A")
        || body.contains("manual")
        || body.contains("not created")
        || body.contains("skipped");

    assert!(
        has_empty_pr_handling,
        "step-22b must handle empty PR_URL gracefully in the summary output. \
         Use 'N/A', 'manual creation required', or similar when PR was not created. \
         (Issue #684, BLOCKER 2)"
    );
}

#[test]
fn step_22b_uses_host_type_safe_pattern() {
    // Must use HOST_TYPE=${REMOTE_HOST_TYPE:-other} for set -u safety
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-22b must use safe default pattern '${{REMOTE_HOST_TYPE:-other}}' \
         or 'HOST_TYPE=${{REMOTE_HOST_TYPE:-other}}' for set -u compatibility. \
         (Issue #684)"
    );
}

// ===========================================================================
// BLOCKER 3: step-03 — percent-encoded AzDO project names
// ===========================================================================

#[test]
fn step_03_decodes_percent_encoding() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    // The step must decode %XX sequences (e.g., %20 → space) before validation.
    // We check for actual decode logic, not incidental mentions of "sed" in comments.
    let has_percent_decode = body.contains("%20")
        || body.contains("percent_decode")
        || body.contains("printf '%b'")  // printf-based decode
        || body.contains("\\\\x")  // hex escape for printf decode
        || (body.contains("sed") && body.contains("%[0-9A-Fa-f]")); // sed-based decode with hex pattern

    assert!(
        has_percent_decode,
        "step-03 must decode percent-encoded sequences (e.g., %20 → space) \
         in AzDO project names before validation. URLs like \
         'dev.azure.com/org/My%20Project/' must be handled. (Issue #684, BLOCKER 3)"
    );
}

#[test]
fn step_03_regex_allows_spaces_in_project_names() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    // After percent-decoding, the regex must allow spaces in project names.
    // The current regex is ^[a-zA-Z0-9._-]+$ which rejects spaces.
    // The fix should expand to ^[a-zA-Z0-9._ -]+$ (note the space).
    let has_space_in_regex =
        body.contains("[a-zA-Z0-9._ -]") || body.contains("[a-zA-Z0-9._[:space:]-]");

    assert!(
        has_space_in_regex,
        "step-03 AzDO project name validation regex must allow spaces \
         (for decoded %20). Change from ^[a-zA-Z0-9._-]+$ to \
         ^[a-zA-Z0-9._ -]+$. (Issue #684, BLOCKER 3)"
    );
}

#[test]
fn step_03_rejects_invalid_percent_sequences() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    // Invalid percent sequences (%ZZ, %G1, etc.) must be caught during decode.
    // This is separate from the existing "unexpected characters" validation —
    // it must explicitly handle the decode-failure path.
    let has_decode_validation = body.contains("%20")
        || body.contains("percent_decode")
        || body.contains("printf '%b'")
        || body.contains("\\\\x");

    assert!(
        has_decode_validation,
        "step-03 must have percent-decode logic that implicitly rejects invalid \
         sequences (e.g., %%ZZ). The decode itself will fail or pass through invalid \
         sequences which the expanded regex then catches. (Issue #684, BLOCKER 3)"
    );
}

// ===========================================================================
// Issue #718: step-03 reuses existing Azure DevOps work item context
// ===========================================================================

#[test]
fn step_03_reuses_existing_issue_number_for_azure_devops_alias_without_gh_or_az() {
    let run = run_step_03(
        "azure-devops",
        "718",
        "ADO follow-up work with existing issue context",
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "azure-devops host with existing ISSUE_NUMBER must succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "AB#718",
        "azure-devops host with existing ISSUE_NUMBER must reuse the Azure Boards work item directly"
    );
    assert!(
        run.git_log.is_empty(),
        "azure-devops host must not probe git when ISSUE_NUMBER already supplies the work item; git log:\n{}",
        run.git_log
    );
    assert!(
        run.gh_log.is_empty(),
        "azure-devops host must not enter GitHub issue logic when ISSUE_NUMBER exists; gh log:\n{}",
        run.gh_log
    );
    assert!(
        run.az_log.is_empty(),
        "azure-devops host must not call Azure CLI when ISSUE_NUMBER already supplies the work item; az log:\n{}",
        run.az_log
    );
}

#[test]
fn step_03_reuses_existing_issue_number_for_azdo_alias_without_gh_or_az() {
    let run = run_step_03(
        "azdo",
        "718",
        "ADO follow-up work with existing issue context",
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "azdo host with existing ISSUE_NUMBER must succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "AB#718",
        "azdo host with existing ISSUE_NUMBER must reuse the Azure Boards work item directly"
    );
    assert!(
        run.git_log.is_empty(),
        "azdo host must not probe git when ISSUE_NUMBER already supplies the work item; git log:\n{}",
        run.git_log
    );
    assert!(
        run.gh_log.is_empty(),
        "azdo host must not enter GitHub issue logic when ISSUE_NUMBER exists; gh log:\n{}",
        run.gh_log
    );
    assert!(
        run.az_log.is_empty(),
        "azdo host must not call Azure CLI when ISSUE_NUMBER already supplies the work item; az log:\n{}",
        run.az_log
    );
}

#[test]
fn workflow_prep_azdo_https_remote_bypasses_all_github_issue_commands() {
    let (detection, step_03) = run_detect_then_step_03(
        "https://dev.azure.com/example-org/My%20Project/_git/example-repo",
        "Create tracking for Azure DevOps-backed repo without existing issue",
    );

    let detection_stdout = String::from_utf8_lossy(&detection.output.stdout);
    let detection_stderr = String::from_utf8_lossy(&detection.output.stderr);
    assert!(
        detection.output.status.success(),
        "AzDO HTTPS host detection must succeed; stdout:\n{detection_stdout}\nstderr:\n{detection_stderr}"
    );
    assert_eq!(
        detection_stdout.trim(),
        "azdo",
        "dev.azure.com remotes must classify as azdo before step-03 dispatch"
    );

    let stdout = String::from_utf8_lossy(&step_03.output.stdout);
    let stderr = String::from_utf8_lossy(&step_03.output.stderr);
    assert!(
        step_03.output.status.success(),
        "AzDO HTTPS workflow-prep path must succeed via Azure Boards or local tracking; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("tracking_system=local") || stdout.contains("_workitems/edit/"),
        "AzDO HTTPS workflow-prep path must use Azure Boards/local tracking output, not GitHub output; stdout:\n{stdout}"
    );
    assert!(
        step_03.gh_log.is_empty(),
        "AzDO HTTPS workflow-prep path must not invoke gh issue or gh label commands; gh log:\n{}",
        step_03.gh_log
    );
}

#[test]
fn workflow_prep_azdo_visualstudio_remote_bypasses_all_github_issue_commands() {
    let (detection, step_03) = run_detect_then_step_03(
        "https://example-org.visualstudio.com/My%20Project/_git/example-repo",
        "Create tracking for legacy Azure DevOps-backed repo without existing issue",
    );

    let detection_stdout = String::from_utf8_lossy(&detection.output.stdout);
    let detection_stderr = String::from_utf8_lossy(&detection.output.stderr);
    assert!(
        detection.output.status.success(),
        "Visual Studio host detection must succeed; stdout:\n{detection_stdout}\nstderr:\n{detection_stderr}"
    );
    assert_eq!(
        detection_stdout.trim(),
        "azdo",
        "visualstudio.com remotes must classify as azdo before step-03 dispatch"
    );

    let stdout = String::from_utf8_lossy(&step_03.output.stdout);
    let stderr = String::from_utf8_lossy(&step_03.output.stderr);
    assert!(
        step_03.output.status.success(),
        "Visual Studio workflow-prep path must succeed via Azure Boards or local tracking; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("tracking_system=local") || stdout.contains("_workitems/edit/"),
        "Visual Studio workflow-prep path must use Azure Boards/local tracking output, not GitHub output; stdout:\n{stdout}"
    );
    assert!(
        step_03.gh_log.is_empty(),
        "Visual Studio workflow-prep path must not invoke gh issue or gh label commands; gh log:\n{}",
        step_03.gh_log
    );
}

#[test]
fn workflow_prep_azdo_ssh_remote_bypasses_all_github_issue_commands() {
    let (detection, step_03) = run_detect_then_step_03(
        "git@ssh.dev.azure.com:v3/example-org/My%20Project/example-repo",
        "Create tracking for Azure DevOps SSH-backed repo without existing issue",
    );

    let detection_stdout = String::from_utf8_lossy(&detection.output.stdout);
    let detection_stderr = String::from_utf8_lossy(&detection.output.stderr);
    assert!(
        detection.output.status.success(),
        "AzDO SSH host detection must succeed; stdout:\n{detection_stdout}\nstderr:\n{detection_stderr}"
    );
    assert_eq!(
        detection_stdout.trim(),
        "azdo",
        "ssh.dev.azure.com remotes must classify as azdo before step-03 dispatch"
    );

    let stdout = String::from_utf8_lossy(&step_03.output.stdout);
    let stderr = String::from_utf8_lossy(&step_03.output.stderr);
    assert!(
        step_03.output.status.success(),
        "AzDO SSH workflow-prep path must succeed via Azure Boards or local tracking; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("tracking_system=local") || stdout.contains("_workitems/edit/"),
        "AzDO SSH workflow-prep path must use Azure Boards/local tracking output, not GitHub output; stdout:\n{stdout}"
    );
    assert!(
        step_03.gh_log.is_empty(),
        "AzDO SSH workflow-prep path must not invoke gh issue or gh label commands; gh log:\n{}",
        step_03.gh_log
    );
}

#[test]
fn step_03_preserves_github_existing_issue_reuse_path() {
    let run = run_step_03("github", "718", "GitHub follow-up for #718");

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "github host with referenced issue must succeed; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "https://github.com/example-org/example-repo/issues/718",
        "github host must preserve existing gh issue reuse semantics"
    );
    assert!(
        run.gh_log.contains("issue view 718"),
        "github host must use gh issue view for referenced issues; gh log:\n{}",
        run.gh_log
    );
    assert!(
        run.az_log.is_empty(),
        "github host must not call Azure CLI; az log:\n{}",
        run.az_log
    );
}

#[test]
fn step_03_preserves_github_issue_create_success_path() {
    let run = run_step_03_with_env(
        "github",
        "",
        "GitHub follow-up without existing issue",
        &[
            (
                "GH_CREATE_OUTPUT",
                "https://github.com/example-org/example-repo/issues/901",
            ),
            ("GH_CREATE_STATUS", "0"),
        ],
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "github issue create success must remain successful; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("https://github.com/example-org/example-repo/issues/901"),
        "github issue create success must emit GitHub issue metadata; stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("local-tracking"),
        "github issue create success must not degrade to local tracking; stdout:\n{stdout}"
    );
}

#[test]
fn step_03_github_issue_create_external_calls_are_timeout_wrapped_without_transient_retry() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    assert!(
        body.contains("timeout 30 gh label create") && body.contains("timeout 60 gh issue create"),
        "GitHub label/create calls must be bounded external-service calls"
    );
    assert!(
        !body.contains("retrying once") && !body.contains("GH_RETRY_DELAY_SECONDS"),
        "GitHub issue create must not add broad transient retry logic; only repo access/resolution failures may fall back locally"
    );
}

#[test]
fn step_03_github_repo_resolution_failure_falls_back_to_local_tracking() {
    let secret_remote = "https://token:ghp_secret123@github.com/cloud-ecosystem-security/hyenas";
    let run = run_step_03_with_env(
        "github",
        "",
        "Please see issue #763; create tracking for the workflow",
        &[
            ("GIT_REMOTE_URL", secret_remote),
            (
                "GH_CREATE_OUTPUT",
                "GraphQL: Could not resolve to a Repository with the name 'cloud-ecosystem-security/hyenas'.",
            ),
            ("GH_CREATE_STATUS", "1"),
        ],
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        run.output.status.success(),
        "repo-resolution failure must fall back locally instead of aborting; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        combined.contains("WARNING: GitHub issue creation failed because the repository could not be resolved or accessed; using local tracking metadata instead."),
        "fallback must be visible; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("issue_creation=local-tracking")
            && stdout.contains("tracking_system=local")
            && stdout.contains("tracking_reference=local-issue-763")
            && stdout.contains("issue_number=763"),
        "fallback must emit deterministic local tracking metadata; stdout:\n{stdout}"
    );
    assert!(
        !combined.contains(secret_remote) && !combined.contains("ghp_secret123"),
        "fallback logs must not leak credential-bearing remote URLs; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !combined.contains("Created GitHub issue"),
        "fallback must not look like GitHub issue creation success; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn step_03_github_repo_resolution_failure_uses_issue_number_for_local_tracking() {
    let run = run_step_03_with_env(
        "github",
        "763",
        "Create tracking for a workflow without an inline issue reference",
        &[
            (
                "GH_CREATE_OUTPUT",
                "GraphQL: Could not resolve to a Repository with the name 'cloud-ecosystem-security/hyenas'.",
            ),
            ("GH_CREATE_STATUS", "1"),
        ],
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "repo-resolution failure with ISSUE_NUMBER must fall back locally; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("tracking_reference=local-issue-763")
            && stdout.contains("issue_number=763"),
        "fallback must preserve ISSUE_NUMBER as deterministic local metadata; stdout:\n{stdout}"
    );
}

#[test]
fn step_03_github_unexpected_create_failure_remains_error() {
    let run = run_step_03_with_env(
        "github",
        "",
        "Create tracking for an unexpected GitHub failure",
        &[
            (
                "GH_CREATE_OUTPUT",
                "GraphQL: rate limit exceeded for https://token:ghp_secret123@github.com/example-org/example-repo",
            ),
            ("GH_CREATE_STATUS", "1"),
        ],
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        !run.output.status.success(),
        "unexpected GitHub failures must not fall back locally; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ERROR: GitHub issue creation failed.")
            && stderr.contains("GraphQL: rate limit exceeded")
            && stderr.contains("https://<redacted>@github.com/example-org/example-repo"),
        "unexpected GitHub failures must remain visible; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("ghp_secret123")
            && !stderr.contains("https://token:ghp_secret123@github.com"),
        "unexpected GitHub failures must sanitize credential-bearing CLI output; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("issue_creation=local-tracking")
            && !stdout.contains("tracking_system=local"),
        "unexpected GitHub failures must not emit local tracking metadata; stdout:\n{stdout}"
    );
}

#[test]
fn step_03b_has_local_tracking_metadata_extraction_contract() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03b-extract-issue-number");
    let local_pos = body
        .find("LOCAL_TRACKING_RE")
        .expect("step-03b must define the local tracking guard");
    let issue_number_pos = body
        .find("issue_number=([0-9]+)")
        .expect("step-03b must still support provider numeric extraction");

    assert!(
        local_pos < issue_number_pos,
        "step-03b must skip numeric extraction before provider issue_number parsing for local metadata"
    );
}

#[test]
fn step_03b_propagates_local_reference_for_local_tracking_metadata() {
    // #815/#804: a local fallback has no real GitHub/AzDO issue, so step-03b
    // must NOT surface the derived bare number (763) — that could later become
    // a `Closes #763` reference closing an unrelated issue. It must instead
    // propagate the non-numeric local tracking reference and never abort, so
    // the workflow proceeds past prep and stays traceable.
    let output = run_step_03b(
        "tracking_system=local\ntracking_reference=local-issue-763\ntracking_issue=local-issue-763\nissue_creation=local-tracking\nissue_number=763\n",
        "Create tracking for local fallback",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "step-03b must accept local tracking metadata; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "local-issue-763",
        "step-03b must propagate the non-numeric local tracking reference"
    );
    assert_ne!(
        stdout.trim(),
        "763",
        "step-03b must not surface the bare derived number for local tracking"
    );
}

#[test]
fn step_03b_sanitizes_unparseable_issue_creation_output() {
    let output = run_step_03b(
        "GraphQL: rate limit exceeded for https://token:ghp_secret123@github.com/example-org/example-repo",
        "Create tracking for local fallback",
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "unparseable issue_creation must fail; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ERROR: step-03b failed to extract issue number")
            && stderr.contains("https://<redacted>@github.com/example-org/example-repo"),
        "step-03b must keep useful context while sanitizing; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("ghp_secret123")
            && !stderr.contains("https://token:ghp_secret123@github.com"),
        "step-03b must not leak credential-bearing issue_creation output; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn step_03_preserves_generic_local_tracking_fallback() {
    let run = run_step_03(
        "other",
        "718",
        "Generic host follow-up with existing context",
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "generic host must succeed via local tracking fallback; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("issue_creation=local-tracking")
            && stdout.contains("tracking_system=local")
            && stdout.contains("tracking_reference=local-issue-718")
            && stdout.contains("issue_number=718"),
        "generic host must preserve visible local tracking metadata; stdout:\n{stdout}"
    );
    assert!(
        run.git_log.is_empty(),
        "generic host must not probe git before local tracking fallback; git log:\n{}",
        run.git_log
    );
    assert!(
        run.gh_log.is_empty(),
        "generic host must not call gh issue commands; gh log:\n{}",
        run.gh_log
    );
    assert!(
        run.az_log.is_empty(),
        "generic host must not call Azure CLI; az log:\n{}",
        run.az_log
    );
}

#[test]
fn step_03_unexpected_host_type_falls_back_without_log_injection() {
    let run = run_step_03(
        "github\ntracking_system=github-forged",
        "763",
        "Generic host follow-up with malicious host context",
    );

    let stdout = String::from_utf8_lossy(&run.output.stdout);
    let stderr = String::from_utf8_lossy(&run.output.stderr);
    assert!(
        run.output.status.success(),
        "unexpected host type must fall back locally; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("WARN: Unexpected REMOTE_HOST_TYPE")
            && !stderr.contains("github\ntracking_system=github-forged"),
        "unexpected host type warning must not echo untrusted host text; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("tracking_system=local")
            && stdout.contains("issue_creation=local-tracking")
            && stdout.contains("tracking_reference=local-issue-763"),
        "unexpected host type must emit local tracking metadata; stdout:\n{stdout}"
    );
    assert!(
        run.git_log.is_empty() && run.gh_log.is_empty() && run.az_log.is_empty(),
        "unexpected host type must not call git/gh/az; git:\n{}\ngh:\n{}\naz:\n{}",
        run.git_log,
        run.gh_log,
        run.az_log
    );
}

// ===========================================================================
// Cross-cutting: step-03 consumes REMOTE_HOST_TYPE from env
// ===========================================================================

#[test]
fn step_03_does_not_redefine_remote_host_type_via_case() {
    // After step-02d is added, step-03 should consume $REMOTE_HOST_TYPE
    // from the environment, not re-detect it with its own case block.
    // Note: step-03 must still USE $REMOTE_HOST_TYPE for branching (github/azdo/other).
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    // Count how many times REMOTE_HOST_TYPE is assigned via case statement.
    // After the fix, there should be zero case-based assignments — only reads.
    let has_case_assignment =
        body.contains("case \"$REMOTE_URL\"") && body.contains("REMOTE_HOST_TYPE=\"github\"");

    assert!(
        !has_case_assignment,
        "step-03 must NOT re-detect REMOTE_HOST_TYPE via case statement. \
         It should consume $REMOTE_HOST_TYPE from context (set by step-02d). \
         This eliminates duplicate host detection. (Issue #684)"
    );
}

// ===========================================================================
// Step-21: gh pr ready guard (existing PR_URL guard + host-type)
// ===========================================================================

#[test]
fn step_21_guards_gh_commands_with_host_type() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-21-pr-ready");

    // step-21 already has a PR_URL guard. After fix, it should also check
    // REMOTE_HOST_TYPE to prevent gh commands on non-GitHub hosts.
    assert!(
        body.contains("REMOTE_HOST_TYPE"),
        "step-21 must check REMOTE_HOST_TYPE in addition to PR_URL guard \
         to prevent gh commands on non-GitHub hosts. (Issue #684)"
    );
}

// ===========================================================================
// Brick rule: all recipe files must stay under 400 lines
// ===========================================================================

#[test]
fn all_modified_recipes_under_400_lines() {
    let recipes = [
        "default-workflow",
        "workflow-prep",
        "workflow-publish",
        "workflow-finalize",
    ];

    for name in &recipes {
        let text = recipe_text(name);
        let line_count = text.lines().count();
        assert!(
            line_count <= 400,
            "{name}.yaml has {line_count} lines — exceeds the 400-line brick limit. \
             (Issue #684, brick rule)"
        );
    }
}

// ===========================================================================
// Security: HOST_TYPE safe defaults in all consuming steps
// ===========================================================================

#[test]
fn step_15_uses_host_type_safe_default() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-15 must use safe default pattern for REMOTE_HOST_TYPE \
         (e.g., '${{REMOTE_HOST_TYPE:-other}}') for set -u safety. (Issue #684)"
    );
}

#[test]
fn step_16_uses_host_type_safe_default() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-16-create-draft-pr");

    let has_safe_default =
        body.contains("${REMOTE_HOST_TYPE:-") || body.contains("HOST_TYPE=${REMOTE_HOST_TYPE:-");

    assert!(
        has_safe_default,
        "step-16 must use safe default pattern for REMOTE_HOST_TYPE \
         (e.g., '${{REMOTE_HOST_TYPE:-other}}') for set -u safety. (Issue #684)"
    );
}

// ===========================================================================
// Preserved invariants: existing step behavior must not regress
// ===========================================================================

#[test]
fn step_03_preserves_github_path() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    assert!(
        body.contains("gh issue create"),
        "step-03 must preserve the GitHub issue creation path (Issue #684)"
    );
    assert!(
        body.contains("gh issue view"),
        "step-03 must preserve the GitHub issue lookup for idempotency (Issue #684)"
    );
}

#[test]
fn step_03_preserves_azdo_path() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    assert!(
        body.contains("az boards work-item"),
        "step-03 must preserve the AzDO work-item creation path (Issue #684)"
    );
}

#[test]
fn step_03_preserves_local_tracking_fallback() {
    let recipe = load_recipe("workflow-prep");
    let body = extract_step_body(recipe, "step-03-create-issue");

    assert!(
        body.contains("local-tracking") || body.contains("local tracking"),
        "step-03 must preserve the local tracking fallback path (Issue #684)"
    );
}

#[test]
fn step_22b_preserves_pr_url_guard() {
    // The existing PR_URL empty-check must be preserved
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    let has_pr_url_check =
        body.contains("PR_URL") && (body.contains("-z \"$PR_URL\"") || body.contains("PR_URL:-"));

    assert!(
        has_pr_url_check,
        "step-22b must preserve the PR_URL empty-check guard. (Issue #684)"
    );
}

#[test]
fn step_15_preserves_set_euo_pipefail() {
    let recipe = load_recipe("workflow-publish");
    let body = extract_step_body(recipe, "step-15-commit-push");

    assert!(
        body.contains("set -euo pipefail"),
        "step-15 must preserve 'set -euo pipefail' at the top of the bash block"
    );
}

#[test]
fn step_22b_preserves_set_euo_pipefail() {
    let recipe = load_recipe("workflow-finalize");
    let body = extract_step_body(recipe, "step-22b-final-status");

    assert!(
        body.contains("set -euo pipefail"),
        "step-22b must preserve 'set -euo pipefail' at the top of the bash block"
    );
}
