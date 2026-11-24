//! Checks on the v3 phase sub-recipes of `default-workflow`.
//!
//! Since #3129, `default-workflow` 4.0.0 no longer calls the nine phase
//! recipes (workflow-prep, workflow-worktree, workflow-design, workflow-tdd,
//! workflow-refactor-review, workflow-precommit-test, workflow-publish,
//! workflow-pr-review, workflow-finalize); it runs the build goal loop
//! (docs/reference/build-goal-loop.md). They stay for one release for runs
//! that loaded v3.
//!
//! What stays here: every phase recipe parses and has unique step ids, every
//! recipe keeps to the 400-line brick budget, step-03b is run against stub
//! inputs, and the checks over phase recipes this change does not edit. The
//! tests that pinned the wording, step ids or step order of recipes and tools
//! #3129 changes were removed under Ryan's direction of 23 Nov 2026: tests
//! check values and behaviour, never wording.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::{LazyLock, Mutex};
use tempfile::TempDir;

const BRICK_LIMIT: usize = 400;

const EXTRA_RECIPE_NAMES: &[&str] = &["default-workflow"];

static RECIPE_TEXTS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    PHASE_RECIPES
        .iter()
        .chain(EXTRA_RECIPE_NAMES)
        .map(|&name| {
            let path = recipe_path(name);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            (name, text)
        })
        .collect()
});

static RECIPES: LazyLock<HashMap<&'static str, Recipe>> = LazyLock::new(|| {
    RECIPE_TEXTS
        .iter()
        .map(|(&name, text)| {
            let path = recipe_path(name);
            let recipe = serde_yaml::from_str(text)
                .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
            (name, recipe)
        })
        .collect()
});

static WORKFLOW_PR_REVIEW_YAML: LazyLock<serde_yaml::Value> = LazyLock::new(|| {
    let name = "workflow-pr-review";
    let path = recipe_path(name);
    serde_yaml::from_str(recipe_text(name))
        .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
});

#[derive(Debug, Deserialize)]
struct Recipe {
    name: String,
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

fn recipes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(format!("{name}.yaml"))
}

fn recipe_text(name: &str) -> &'static str {
    RECIPE_TEXTS
        .get(name)
        .unwrap_or_else(|| panic!("uncached test recipe: {name}"))
}

fn load(name: &str) -> &'static Recipe {
    RECIPES
        .get(name)
        .unwrap_or_else(|| panic!("uncached test recipe: {name}"))
}

fn load_yaml(name: &str) -> &'static serde_yaml::Value {
    match name {
        "workflow-pr-review" => &WORKFLOW_PR_REVIEW_YAML,
        _ => panic!("uncached test YAML recipe: {name}"),
    }
}

/// The shell a bash step runs.
///
/// Issue #1619: a step whose body was extracted into
/// `amplifier-bundle/tools/` for the brick budget runs that file, so the body
/// every assertion below reads is the file's. The call-out is recognised by
/// the one line that names the script, and the script must exist: a step that
/// calls a file that is not there would otherwise read as a step with no
/// requirements at all.
fn step_command(recipe: &str, step_id: &str) -> &'static str {
    let inline = load(recipe)
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .command
        .as_deref()
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must be a bash step"));
    match extracted_body(inline) {
        Some(body) => body,
        None => inline,
    }
}

/// The text of the `amplifier-bundle/tools/` script a call-out names, or
/// `None` when the body is inline.
fn extracted_body(inline: &str) -> Option<&'static str> {
    static BODIES: LazyLock<Mutex<HashMap<String, &'static str>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let marker = "# The body of this step lives in amplifier-bundle/tools/";
    let line = inline
        .lines()
        .find(|line| line.trim_start().starts_with(marker))?;
    let name = line
        .trim_end()
        .trim_end_matches(',')
        .rsplit('/')
        .next()
        .expect("a call-out names its script");
    let mut bodies = BODIES.lock().expect("extracted body cache");
    if let Some(body) = bodies.get(name) {
        return Some(body);
    }
    let path = recipes_dir().join("..").join("tools").join(name);
    let text: &'static str = Box::leak(
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
            .into_boxed_str(),
    );
    bodies.insert(name.to_string(), text);
    Some(text)
}

struct StepRun {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

struct Step03bRunner {
    _shim_dir: TempDir,
    path: String,
}

impl Step03bRunner {
    fn new() -> Self {
        let shim_dir = TempDir::new().expect("create step-03b gh shim tempdir");
        let gh_shim = shim_dir.path().join("gh");
        fs::write(&gh_shim, "#!/usr/bin/env bash\nexit 0\n")
            .unwrap_or_else(|e| panic!("write {}: {e}", gh_shim.display()));
        let mut permissions = fs::metadata(&gh_shim)
            .unwrap_or_else(|e| panic!("stat {}: {e}", gh_shim.display()))
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh_shim, permissions)
            .unwrap_or_else(|e| panic!("chmod {}: {e}", gh_shim.display()));

        let path = match std::env::var("PATH") {
            Ok(existing) if !existing.is_empty() => {
                format!("{}:{existing}", shim_dir.path().display())
            }
            _ => shim_dir.path().display().to_string(),
        };

        Self {
            _shim_dir: shim_dir,
            path,
        }
    }

    fn run_extract_issue_number(&self, issue_creation: &str, task_description: &str) -> StepRun {
        let output = Command::new("bash")
            .arg("-c")
            .arg(step_command(
                "workflow-prep",
                "step-03b-extract-issue-number",
            ))
            .env("RECIPE_VAR_issue_creation", issue_creation)
            .env("TASK_DESCRIPTION", task_description)
            .env("PATH", &self.path)
            .output()
            .expect("run step-03b-extract-issue-number command");

        StepRun {
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

fn step_prompt(recipe: &str, step_id: &str) -> &'static str {
    load(recipe)
        .steps
        .iter()
        .find(|step| step.id == step_id)
        .unwrap_or_else(|| panic!("{recipe}.yaml missing step {step_id}"))
        .prompt
        .as_deref()
        .unwrap_or_else(|| panic!("{recipe}.yaml step {step_id} must have a prompt"))
}

const PHASE_RECIPES: &[&str] = &[
    "workflow-prep",
    "workflow-worktree",
    "workflow-design",
    "workflow-tdd",
    "workflow-refactor-review",
    "workflow-precommit-test",
    "workflow-publish",
    "workflow-pr-review",
    "workflow-finalize",
];

const PR_REVIEW_STEP_INVENTORY: &[&str] = &[
    "step-17a-testing-evidence-gate",
    "step-17b-reviewer-agent",
    "step-17c-security-review",
    "step-17d-philosophy-guardian",
    "step-17e-address-blocking-issues",
    "step-17f-verification-gate",
    "step-18a-analyze-feedback",
    "step-18b-implement-feedback",
    "step-18c-push-feedback-changes",
    "step-18c-push-recovery",
    "step-18d-respond-to-comments",
    "step-18e-verification-gate",
    "step-19a-philosophy-check",
    "step-19b-patterns-check",
    "step-19c-zero-bs-verification",
    "step-19d-verification-gate",
];

/// Every phase sub-recipe must parse and have a non-empty step list.
#[test]
fn every_phase_subrecipe_loads_and_has_steps() {
    for name in PHASE_RECIPES {
        let r = load(name);
        assert_eq!(&r.name, name, "recipe.name must match filename");
        assert!(!r.steps.is_empty(), "{name} must declare at least one step");
    }
}

/// No step ID may appear twice anywhere across the decomposition. Duplicates
/// would silently shadow context outputs.
#[test]
fn no_duplicate_step_ids_across_subrecipes() {
    let mut seen: HashSet<String> = HashSet::new();
    let mut dups: Vec<String> = Vec::new();
    for name in PHASE_RECIPES {
        for step in &load(name).steps {
            if !seen.insert(step.id.clone()) {
                dups.push(step.id.clone());
            }
        }
    }
    assert!(dups.is_empty(), "duplicate step IDs found: {dups:?}");
}

#[test]
fn step_13_does_not_mandate_uvx_or_remote_git_install_flow() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let forbidden_phrases = [
        "Get the remote URL (for uvx testing)",
        "git remote get-url origin",
        "Pass the PR branch name and repository URL so tests run against the actual branch",
        "uvx --from git+<remote-url>@<branch-name>",
        "uvx --from git+...@<branch>",
        "via `uvx --from git+...`",
    ];

    for phrase in forbidden_phrases {
        assert!(
            !prompt.contains(phrase),
            "Step 13 must not present `{phrase}` as the universal outside-in validation path"
        );
    }
}

#[test]
fn step_13_requires_agentic_toolchain_detection_before_validation_selection() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let lower = prompt.to_ascii_lowercase();

    for required in ["outside-in", "detect", "toolchain", "validation strategy"] {
        assert!(
            lower.contains(required),
            "Step 13 must preserve outside-in validation while requiring agents to detect project languages/toolchains and select a validation strategy; missing `{required}`"
        );
    }

    assert!(
        lower.contains("choose") || lower.contains("select"),
        "Step 13 must delegate validation pattern selection to the acting agent"
    );
    assert!(
        lower.contains("record") && lower.contains("chosen"),
        "Step 13 must require evidence of the chosen validation strategy, not just raw command output"
    );
}

#[test]
fn step_13_covers_major_toolchain_examples_without_making_uvx_global() {
    let prompt = step_prompt("workflow-precommit-test", "step-13-local-testing");
    let lower = prompt.to_ascii_lowercase();
    let toolchain_examples = [
        ("Rust/Cargo", lower.contains("cargo")),
        ("Node/npm", lower.contains("npm")),
        (
            "Python/uv/uvx",
            lower.contains("python") && lower.contains("uv") && lower.contains("uvx"),
        ),
        (
            "Go",
            lower.contains("go test")
                || lower.contains("go run")
                || lower.contains("golang")
                || prompt.contains("Go/"),
        ),
        (".NET", lower.contains("dotnet") || prompt.contains(".NET")),
    ];

    for (toolchain, present) in toolchain_examples {
        assert!(
            present,
            "Step 13 must include a generalized outside-in validation example for {toolchain}"
        );
    }

    assert!(
        lower.contains("python") && lower.find("python") < lower.rfind("uvx"),
        "Step 13 may mention uvx only as a Python/uv-specific validation option"
    );
}

/// Brick rule: every phase sub-recipe must be ≤ 400 lines. The composer is
/// already tiny by construction; checking it too costs nothing.
#[test]
fn every_phase_subrecipe_under_400_lines() {
    let mut violations: Vec<(String, usize)> = Vec::new();
    for name in PHASE_RECIPES
        .iter()
        .chain(std::iter::once(&"default-workflow"))
    {
        let lines = recipe_text(name).lines().count();
        if lines >= BRICK_LIMIT {
            violations.push((name.to_string(), lines));
        }
    }
    assert!(
        violations.is_empty(),
        "brick rule violation (must be <{BRICK_LIMIT} physical lines): {violations:?}"
    );
}

#[test]
fn workflow_pr_review_phase_contract_is_strict_and_ordered() {
    let line_count = recipe_text("workflow-pr-review").lines().count();
    assert!(
        line_count < BRICK_LIMIT,
        "workflow-pr-review.yaml is {line_count} lines; PR review phase bricks must be <{BRICK_LIMIT} physical lines"
    );

    let step_ids: Vec<String> = load("workflow-pr-review")
        .steps
        .iter()
        .map(|step| step.id.clone())
        .collect();
    assert_eq!(
        step_ids, PR_REVIEW_STEP_INVENTORY,
        "workflow-pr-review must preserve every PR review step from 17a through 19d in order"
    );
}

#[test]
fn workflow_pr_review_has_no_recipe_level_timeouts_or_foreground_long_sleep() {
    let raw = recipe_text("workflow-pr-review");
    let parsed = load_yaml("workflow-pr-review");
    let top_level = parsed
        .as_mapping()
        .expect("workflow-pr-review.yaml must parse as a YAML mapping");

    for forbidden in ["timeout", "timeout_seconds", "default_step_timeout"] {
        assert!(
            !top_level.contains_key(serde_yaml::Value::String(forbidden.to_string())),
            "workflow-pr-review.yaml must not define recipe-level `{forbidden}`"
        );
    }

    let foreground_sleeps: Vec<&str> = raw
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("sleep ")
                || trimmed.contains("&& sleep ")
                || trimmed.contains("; sleep ")
                || trimmed.contains(" sleep $")
        })
        .collect();
    assert!(
        foreground_sleeps.is_empty(),
        "workflow-pr-review.yaml must not use foreground sleeps: {foreground_sleeps:?}"
    );
}

#[test]
fn workflow_pr_review_fails_loud_for_required_worktree_context() {
    let push_command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    let zero_bs_command = step_command("workflow-pr-review", "step-19c-zero-bs-verification");

    // step-18c: early-stage — MUST keep hard-fail on missing worktree
    assert!(
        push_command.contains("set -euo pipefail"),
        "step-18c must fail loudly on shell errors"
    );
    assert!(
        push_command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
        "step-18c must require worktree_setup.worktree_path instead of inventing a default"
    );

    // step-19c: late-stage — MUST be resilient (issue #647)
    // It can function from repo_path or cwd; hard-fail is wrong here.
    assert!(
        zero_bs_command.contains("set -euo pipefail"),
        "step-19c must still fail loudly on shell errors (only cd target selection is resilient)"
    );
    assert!(
        zero_bs_command.contains("WARNING"),
        "step-19c must emit WARNING on stderr when falling back from missing worktree"
    );
    assert!(
        zero_bs_command.contains("REPO_PATH"),
        "step-19c must fall back to REPO_PATH when worktree is unavailable"
    );
    assert!(
        !zero_bs_command.contains("cd \"${WORKTREE_SETUP_WORKTREE_PATH:?"),
        "step-19c must NOT hard-fail on cd into missing worktree (issue #647)"
    );

    // step-18c specific assertions preserved from original
    let hidden_git_failures: Vec<&str> = push_command
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("git ") && trimmed.contains("|| true")
        })
        .collect();
    assert!(
        hidden_git_failures.is_empty(),
        "step-18c must not hide git failures with `|| true`: {hidden_git_failures:?}"
    );
    assert!(
        push_command.contains("git commit failed") && push_command.contains("exit \"$commit_rc\""),
        "step-18c must surface git commit failures and exit with the original status"
    );
    assert!(
        push_command.contains("git push failed") && push_command.contains("exit \"$push_rc\""),
        "step-18c must surface git push failures and exit with the original status"
    );
}

#[test]
fn workflow_pr_review_input_description_reflects_resilient_step_19c() {
    // Issue #647: step-19c is now resilient, so the input description should
    // only list step-18c as requiring the worktree path.
    let text = recipe_text("workflow-pr-review");
    assert!(
        text.contains("Required by") && text.contains("step-18c"),
        "worktree_setup.worktree_path input description must mention step-18c"
    );
    assert!(
        !text.contains("step-19c-zero-bs-verification that cd into the worktree"),
        "worktree_setup.worktree_path input description must not claim step-19c requires cd into worktree (issue #647)"
    );
}

#[test]
fn workflow_pr_review_scopes_pre_commit_allow_no_config_to_stale_hook_case() {
    let command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    assert_eq!(
        command.matches("PRE_COMMIT_ALLOW_NO_CONFIG").count(),
        1,
        "PRE_COMMIT_ALLOW_NO_CONFIG must only appear at the scoped git commit callsite"
    );
    assert!(
        command.contains("pre_commit_hook=\"$(git rev-parse --git-path hooks/pre-commit)\""),
        "step-18c must inspect the repository-local pre-commit hook path"
    );
    assert!(
        command.contains("[ -f \"$pre_commit_hook\" ]"),
        "PRE_COMMIT_ALLOW_NO_CONFIG is allowed only for a stale pre-commit hook file"
    );
    assert!(
        command.contains("[ ! -f .pre-commit-config.yaml ]"),
        "PRE_COMMIT_ALLOW_NO_CONFIG is allowed only when .pre-commit-config.yaml is missing"
    );
    assert!(
        command.contains("PRE_COMMIT_ALLOW_NO_CONFIG=1 git commit \"$@\"")
            && command.contains("elif git commit \"$@\"")
            && command.contains("commit_with_pre_commit_guard -m \"address review feedback\""),
        "PRE_COMMIT_ALLOW_NO_CONFIG must be scoped to the single git commit invocation"
    );
    assert!(
        command.contains("address review feedback")
            && command.contains("Implemented reviewer suggestions")
            && command.contains("fixed identified issues")
            && command.contains("updated per security review")
            && command.contains("addressed philosophy compliance items"),
        "step-18c must preserve the review-feedback commit subject and body"
    );
}

#[test]
fn workflow_pr_review_zero_bs_scan_covers_tracked_and_untracked_without_recursive_walks() {
    let command = step_command("workflow-pr-review", "step-19c-zero-bs-verification");
    assert!(
        command.contains("git grep -n -E") && command.contains("record_scan 'git grep'"),
        "step-19c must use fail-loud git grep scans for repository verification"
    );
    assert!(
        command.contains("git ls-files --others --exclude-standard -z")
            && command.contains("while IFS= read -r -d '' file")
            && command.contains("grep -n -E -H -a --null -- \"$pattern\" \"${files[@]}\"")
            && command.contains("record_scan 'untracked grep'"),
        "step-19c must restore fail-loud untracked-file scanning"
    );
    assert!(
        command.contains("'*.rs'")
            && command.contains("'*.md'")
            && command.contains("'*.yaml'")
            && command.contains("'*.yml'"),
        "step-19c TODO/FIXME scanning must include source, config, and docs globs"
    );
    assert!(
        !command.contains("grep -r"),
        "step-19c must avoid repeated recursive filesystem grep scans"
    );
}

// ==========================================================================
// Step-03b, run: the issue reference it extracts from step-03's output.
// ==========================================================================

#[test]
fn workflow_prep_step_03b_local_tracking_propagates_reference_without_coercing_to_number() {
    // #815/#804: well-formed local tracking has no real GitHub/AzDO issue, so
    // step-03b must propagate the non-numeric local reference (never the bare
    // embedded number, which could become a `Closes #123` for an unrelated
    // issue) and never abort the workflow.
    let cases = [
        (
            "local tracking system",
            "tracking_system=local\ntracking_reference=local-123\ntracking_issue=local-123\nissue_creation=local-tracking\n",
            "local-123",
        ),
        (
            "local-* reference",
            "tracking_reference=local-123\ntracking_issue=local-123\nissue_creation=local-tracking\n",
            "local-123",
        ),
        (
            "legacy local-tracking reference",
            "tracking_reference=local-tracking:123\ntracking_issue=local-tracking:123\n",
            "local-tracking:123",
        ),
    ];

    let runner = Step03bRunner::new();
    for (name, issue_creation, expected) in cases {
        let run = runner.run_extract_issue_number(issue_creation, "");
        assert!(
            run.status.success(),
            "{name} must skip numeric extraction and exit successfully; stderr:\n{}",
            run.stderr
        );
        assert_eq!(
            run.stdout, expected,
            "{name} must propagate the local tracking reference verbatim"
        );
        assert_ne!(
            run.stdout, "123",
            "{name} must not coerce a local ID to the bare embedded number"
        );
    }
}

#[test]
fn workflow_prep_step_03b_rejects_malformed_local_tracking_metadata() {
    let cases = [
        (
            "local markers without a reference",
            "tracking_system=local\nissue_creation=local-tracking\nissue_number=763\n",
        ),
        (
            "local reference with shell metacharacter",
            "tracking_reference=local-123;rm\ntracking_issue=local-123;rm\nissue_number=763\n",
        ),
    ];

    let runner = Step03bRunner::new();
    for (name, issue_creation) in cases {
        let run = runner.run_extract_issue_number(issue_creation, "Fallback #999");
        assert!(
            !run.status.success(),
            "{name} must fail instead of coercing malformed local metadata; stdout:\n{}\nstderr:\n{}",
            run.stdout,
            run.stderr
        );
        assert_eq!(
            run.stdout, "",
            "{name} must not emit a numeric issue number from malformed local metadata"
        );
        assert!(
            run.stderr
                .contains("local tracking metadata missing valid local reference"),
            "{name} must explain the malformed local metadata; stderr:\n{}",
            run.stderr
        );
    }
}

#[test]
fn workflow_prep_step_03b_remote_references_keep_numeric_extraction_behavior() {
    let cases = [
        (
            "github issue URL",
            "https://github.com/owner/repo/issues/456",
            "456",
        ),
        (
            "github pull request URL",
            "https://github.com/owner/repo/pull/789",
            "789",
        ),
        (
            "azure devops work item URL",
            "https://dev.azure.com/org/project/_workitems/edit/321",
            "321",
        ),
        ("azure boards shorthand", "AB#654", "654"),
    ];

    let runner = Step03bRunner::new();
    for (name, issue_creation, expected) in cases {
        let run = runner.run_extract_issue_number(issue_creation, "");
        assert!(
            run.status.success(),
            "{name} must still extract a numeric issue/work-item id; stderr:\n{}",
            run.stderr
        );
        assert_eq!(
            run.stdout, expected,
            "{name} must preserve existing numeric extraction behavior"
        );
    }
}

// ==========================================================================
// Issue #684: mid-stage steps that stage nothing may fall back to REPO_PATH
// when the worktree variable is not propagated from sub-recipe context
// (step-04). Steps that stage and commit may not (issue #1528).
// ==========================================================================

#[test]
fn mid_stage_read_only_steps_fall_back_to_repo_path() {
    // step-16 creates the PR from already-pushed work and stages nothing, so a
    // REPO_PATH fallback cannot commit in the launch checkout. It still
    // hard-fails (via :?) when REPO_PATH is also empty.
    let command = step_command("workflow-publish", "step-16-create-draft-pr");
    // #1561: the fallback is the recipe's own repo_path, never an inherited
    // bare REPO_PATH, and only when worktree_setup is unset (exit 3); a
    // present but rejected path stops the step.
    assert!(
        command.contains(
            r#"|| { [ $? -eq 3 ] || exit 1; WORKTREE_SETUP_WORKTREE_PATH="${RECIPE_VAR_repo_path:-}"; }"#
        ),
        "workflow-publish/step-16-create-draft-pr must fall back to repo_path when worktree_setup is unset"
    );
    assert!(
        command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
        "workflow-publish/step-16-create-draft-pr must hard-fail when worktree AND REPO_PATH are both empty"
    );
}

#[test]
fn mid_stage_committing_steps_never_fall_back_to_repo_path() {
    // Issue #1528: REPO_PATH is the launch checkout, where the run worktrees
    // sit under the ignored `worktrees/`. A step that stages and commits must
    // run only at the top of the linked task worktree, and must hard-fail
    // rather than skip, because a skipped commit loses work silently.
    let cases = [
        ("workflow-publish", "step-15-commit-push"),
        ("workflow-tdd", "checkpoint-after-implementation"),
        (
            "workflow-refactor-review",
            "checkpoint-after-review-feedback",
        ),
    ];
    for (recipe, step_id) in cases {
        let command = step_command(recipe, step_id);
        assert!(
            !command.contains("worktree_path:-${REPO_PATH"),
            "{recipe}/{step_id} must not fall back to $REPO_PATH (#1528)"
        );
        assert!(
            command.contains("WORKTREE_SETUP_WORKTREE_PATH:?"),
            "{recipe}/{step_id} must hard-fail when the worktree path is empty"
        );
    }
}

#[test]
fn step_18c_does_not_fall_back_to_repo_path() {
    // step-18c is early-stage and must keep hard-fail (per test
    // workflow_pr_review_fails_loud_for_required_worktree_context).
    // It must NOT have a REPO_PATH fallback in the default chain.
    let command = step_command("workflow-pr-review", "step-18c-push-feedback-changes");
    assert!(
        !command.contains(":-${REPO_PATH"),
        "step-18c must NOT fall back to REPO_PATH — it requires the worktree"
    );
}

#[path = "zero_bs_transport/mod.rs"]
mod zero_bs_transport;
