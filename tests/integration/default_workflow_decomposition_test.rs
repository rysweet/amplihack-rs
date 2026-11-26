//! Checks on the v3 phase sub-recipes of `default-workflow`.
//!
//! Since #3129, `default-workflow` 4.0.0 no longer calls the nine phase
//! recipes (workflow-prep, workflow-worktree, workflow-design, workflow-tdd,
//! workflow-refactor-review, workflow-precommit-test, workflow-publish,
//! workflow-pr-review, workflow-finalize); it runs the build goal loop
//! (docs/reference/build-goal-loop.md). They stay for one release for runs
//! that loaded v3.
//!
//! What stays here: every phase recipe parses and has unique step ids,
//! step-03b is run against stub inputs, step-18c's pre-commit bypass stays
//! scoped to its one commit, and the zero-BS transport runs. The tests that
//! read prompt or step text, pinned step order, or enforced the line budget,
//! timeouts or sleeps by reading text were removed under Ryan's direction of
//! 23 Nov 2026: tests check values and behaviour, never wording.

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

/// A guard on the hook boundary: step-18c may skip a missing pre-commit
/// configuration only for its one commit, and only when a stale hook file is
/// present.
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

#[path = "zero_bs_transport/mod.rs"]
mod zero_bs_transport;
