//! TDD tests for issue #537.
//!
//! Contract:
//! - smart-orchestrator dry-run is usable from a non-git working directory.
//! - smart-orchestrator dry-run still succeeds from a git working directory.
//! - investigation-workflow is git-optional and dry-runs successfully outside git.
//! - direct recipe git commands are either strict with a loud precondition or
//!   optional with an explicit visible skip.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|path| path.join("amplifier-bundle/recipes").is_dir())
        .map(Path::to_path_buf)
        .expect("workspace must contain amplifier-bundle/recipes")
}

fn recipes_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle/recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(name)
}

fn amplihack_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

fn run_recipe_from(cwd: &Path, recipe: &Path, extra_args: &[&str]) -> Output {
    let mut command = Command::new(amplihack_bin());
    command
        .current_dir(cwd)
        .env("AMPLIHACK_HOME", workspace_root())
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env_remove("CLAUDECODE")
        .arg("recipe")
        .arg("run")
        .arg(recipe)
        .args(extra_args);

    command.output().expect("failed to run amplihack binary")
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_not_raw_git_128(output: &Output) {
    let combined = combined_output(output);
    assert!(
        !combined.contains("exit 128")
            && !combined.contains("exited with 128")
            && !combined.contains("exit status: 128")
            && !combined.contains("fatal: not a git repository"),
        "missing git context must not leak as raw git exit-128/fatal output.\n{combined}"
    );
}

#[test]
fn smart_orchestrator_dry_run_from_non_git_dir_succeeds_or_reports_structured_git_error() {
    let non_git = tempfile::tempdir().expect("create non-git tempdir");

    let output = run_recipe_from(
        non_git.path(),
        &recipe_path("smart-orchestrator.yaml"),
        &[
            "--dry-run",
            "-c",
            "task_description=hello",
            "-c",
            "repo_path=.",
        ],
    );

    assert_not_raw_git_128(&output);
    if output.status.success() {
        return;
    }

    let combined = combined_output(&output);
    assert!(
        combined.contains("requires a git repo")
            && combined.contains("git init")
            && combined.contains("rerun from a checkout"),
        "non-git smart-orchestrator failure must be structured and actionable, \
         not broad top-level git validation.\n{combined}"
    );
}

#[test]
fn smart_orchestrator_dry_run_from_git_dir_succeeds() {
    let git_dir = tempfile::tempdir().expect("create git tempdir");
    let init = Command::new("git")
        .arg("init")
        .arg("--quiet")
        .current_dir(git_dir.path())
        .output()
        .expect("run git init");
    assert!(
        init.status.success(),
        "git init failed:\n{}",
        combined_output(&init)
    );

    let output = run_recipe_from(
        git_dir.path(),
        &recipe_path("smart-orchestrator.yaml"),
        &[
            "--dry-run",
            "-c",
            "task_description=hello",
            "-c",
            "repo_path=.",
        ],
    );

    assert!(
        output.status.success(),
        "smart-orchestrator dry-run must succeed inside a git repo.\n{}",
        combined_output(&output)
    );
}

#[test]
fn investigation_workflow_dry_run_from_non_git_dir_succeeds() {
    let non_git = tempfile::tempdir().expect("create non-git tempdir");

    let output = run_recipe_from(
        non_git.path(),
        &recipe_path("investigation-workflow.yaml"),
        &[
            "--dry-run",
            "-c",
            "task_description=hello",
            "-c",
            "investigation_question=hello",
            "-c",
            "repo_path=.",
        ],
    );

    assert!(
        output.status.success(),
        "investigation-workflow must be runnable from a non-git directory.\n{}",
        combined_output(&output)
    );
}

#[test]
fn git_optional_entry_recipes_do_not_require_git_repo_context_validation() {
    for recipe in ["smart-orchestrator.yaml", "investigation-workflow.yaml"] {
        let value = load_yaml(&recipe_path(recipe));
        let repo_validation = value
            .get("context_validation")
            .and_then(|validation| validation.get("repo_path"))
            .and_then(Value::as_str);

        assert_ne!(
            repo_validation,
            Some("git_repo"),
            "{recipe} must not reject non-git directories at top-level validation; \
             git requirements belong at git-dependent step boundaries"
        );
    }
}

#[test]
fn direct_git_recipe_steps_are_guarded_or_visibly_skipped() {
    let mut violations = Vec::new();

    for recipe in all_recipe_files() {
        let yaml = load_yaml(&recipe);
        for step in steps(&yaml) {
            let Some(command) = step.get("command").and_then(Value::as_str) else {
                continue;
            };
            if !contains_git_command(command) {
                continue;
            }

            let step_id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let guarded = command.contains("git rev-parse --is-inside-work-tree")
                || command.contains("git -C")
                    && command.contains("rev-parse --is-inside-work-tree");
            let loud = command.contains("requires a git repo")
                && command.contains("git init")
                && command.contains("rerun from a checkout");
            let visible_skip = command.contains("[skip] not a git repo");

            if !(guarded && (loud || visible_skip)) {
                violations.push(format!(
                    "{} step {step_id} uses git without an explicit strict \
                     precondition or visible optional skip",
                    recipe.file_name().unwrap().to_string_lossy()
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "issue #537 requires every direct recipe git command to be guarded:\n{}",
        violations.join("\n")
    );
}

#[test]
fn investigation_recipe_family_has_no_direct_git_operations() {
    let mut offenders = Vec::new();
    for recipe in all_recipe_files() {
        let name = recipe.file_name().unwrap().to_string_lossy();
        if !(name.starts_with("investigation-") || name.starts_with("qa-")) {
            continue;
        }

        let yaml = load_yaml(&recipe);
        for step in steps(&yaml) {
            let Some(command) = step.get("command").and_then(Value::as_str) else {
                continue;
            };
            if contains_git_command(command) {
                let step_id = step
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("<unknown>");
                offenders.push(format!("{name} step {step_id}"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "investigation/Q&A recipes must run from any directory without direct git operations:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn git_required_workflow_recipes_document_loud_git_preconditions() {
    for recipe in [
        "default-workflow.yaml",
        "workflow-prep.yaml",
        "workflow-worktree.yaml",
        "workflow-tdd.yaml",
        "workflow-publish.yaml",
    ] {
        let source = fs::read_to_string(recipe_path(recipe)).expect("read recipe");
        assert!(
            source.contains("requires a git repo")
                && source.contains("git init")
                && source.contains("rerun from a checkout"),
            "{recipe} must fail loudly before strict git-dependent work with: \
             \"requires a git repo ... either `git init` or rerun from a checkout\""
        );
    }
}

#[test]
fn operations_git_telemetry_uses_visible_non_git_skip() {
    let source =
        fs::read_to_string(recipe_path("smart-execute-routing.yaml")).expect("read routing recipe");
    let routing = load_yaml(&recipe_path("smart-execute-routing.yaml"));
    let step = step_command(&routing, "ops-file-change-check")
        .expect("ops-file-change-check command must exist");

    assert!(
        step.contains("git rev-parse --is-inside-work-tree"),
        "ops-file-change-check must probe git availability before git diff/ls-files"
    );
    assert!(
        step.contains("[skip] not a git repo"),
        "optional operations git telemetry must visibly skip outside git"
    );
    assert!(
        !source.contains("cd \"$REPO_PATH\" 2>/dev/null || true"),
        "optional git telemetry must not hide a failed cd with `|| true`"
    );
}

fn all_recipe_files() -> Vec<PathBuf> {
    let mut files = fs::read_dir(recipes_dir())
        .expect("read recipes dir")
        .map(|entry| entry.expect("read recipe entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("yaml"))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn load_yaml(path: &Path) -> Value {
    let body = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    serde_yaml::from_str(&body)
        .unwrap_or_else(|err| panic!("failed to parse {} as YAML: {err}", path.display()))
}

fn steps(recipe: &Value) -> Vec<&Value> {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .map(|steps| steps.iter().collect())
        .unwrap_or_default()
}

fn step_command<'a>(recipe: &'a Value, step_id: &str) -> Option<&'a str> {
    steps(recipe).into_iter().find_map(|step| {
        (step.get("id").and_then(Value::as_str) == Some(step_id))
            .then(|| step.get("command").and_then(Value::as_str))
            .flatten()
    })
}

fn contains_git_command(command: &str) -> bool {
    command.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#')
            && (trimmed.starts_with("git ")
                || trimmed.contains(" git ")
                || trimmed.contains(" git -C ")
                || trimmed.contains("$(git ")
                || trimmed.contains("! git "))
    })
}
