//! tests/integration/artifact_guard_workflow_tests.rs
//!
//! Structural contracts for issue #755 workflow integration.
//!
//! Every recipe that performs broad staging must invoke Artifact Guard first so
//! generated/runtime artifacts cannot be swept into a default workflow commit
//! or PR publication path.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipes_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle").join("recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(name)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    serde_yaml::from_str(&read(&path)).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing recipe step `{id}`"))
}

fn step_text<'a>(recipe: &'a Value, id: &str, field: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get(field).and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step `{id}` must contain a {field}"))
}

fn recipe_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(recipes_dir()).expect("read recipes dir") {
        let path = entry.expect("recipe dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn publish_runs_artifact_guard_before_commit_push_and_pr_publication_paths() {
    let recipe = load_recipe("workflow-publish.yaml");
    let commit_index = step_index(&recipe, "step-15-commit-push");
    let publish_index = step_index(&recipe, "step-16-create-draft-pr");
    let fix_loop_index = step_index(&recipe, "step-16b-outside-in-fix-loop");

    let guard_index = steps(&recipe)
        .iter()
        .position(|step| {
            step.get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    command.contains("amplihack hygiene artifact-guard")
                        && command.contains("--mode pre-publish")
                })
        })
        .expect("workflow-publish must include an artifact guard command step");

    assert!(
        guard_index < commit_index && guard_index < publish_index && guard_index < fix_loop_index,
        "artifact guard must run before broad staging, PR creation, and outside-in fix publication"
    );
}

#[test]
fn finalize_runs_artifact_guard_before_cleanup_commit_or_final_push() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let final_cleanup_index = step_index(&recipe, "step-20-final-cleanup");
    let guard_index = step_index(&recipe, "step-20a-artifact-guard");
    let push_cleanup_index = step_index(&recipe, "step-20b-push-cleanup");
    let guard_command = step_text(&recipe, "step-20a-artifact-guard", "command");

    assert!(
        guard_command.contains("amplihack hygiene artifact-guard")
            && guard_command.contains("--mode pre-publish"),
        "workflow-finalize step-20a must invoke Artifact Guard in pre-publish mode"
    );
    assert!(
        final_cleanup_index < guard_index && guard_index < push_cleanup_index,
        "artifact guard must run after agent cleanup and before workflow-finalize cleanup commit/push"
    );
}

#[test]
fn finalize_push_cleanup_guards_immediately_before_broad_staging() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let command = step_text(&recipe, "step-20b-push-cleanup", "command");
    let guard = command
        .find("amplihack hygiene artifact-guard --repo . --mode pre-publish")
        .expect("step-20b must guard the exact repo before broad staging");
    let add = command
        .find("git add -A")
        .expect("step-20b currently performs broad staging");
    let between = &command[guard..add];

    assert!(
        guard < add,
        "step-20b must invoke Artifact Guard before git add -A"
    );
    assert!(
        between
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("amplihack hygiene artifact-guard")
                    && !trimmed.starts_with("git add -A")
            })
            .count()
            == 0,
        "Artifact Guard must be the deterministic final gate immediately before git add -A; intervening lines:\n{between}"
    );
}

#[test]
fn finalize_artifact_guard_failures_are_not_silenced() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let guard_commands: Vec<&str> = steps(&recipe)
        .iter()
        .filter_map(|step| step.get("command").and_then(Value::as_str))
        .filter(|command| command.contains("amplihack hygiene artifact-guard"))
        .collect();

    assert!(
        !guard_commands.is_empty(),
        "workflow-finalize must invoke Artifact Guard"
    );
    for command in guard_commands {
        for line in command
            .lines()
            .filter(|line| line.contains("amplihack hygiene artifact-guard"))
        {
            assert!(
                !line.contains("|| true")
                    && !line.contains("|| :")
                    && !line.contains("2>/dev/null"),
                "Artifact Guard failures must fail loudly, not be hidden: `{line}`"
            );
        }
    }
}

#[test]
fn every_recipe_with_git_add_all_invokes_guard_first() {
    let mut missing = Vec::new();

    for path in recipe_files() {
        let text = read(&path);
        let broad_add_positions: Vec<usize> = ["git add -A", "git add --all", "git add ."]
            .iter()
            .flat_map(|needle| text.match_indices(needle).map(|(index, _)| index))
            .collect();
        if broad_add_positions.is_empty() {
            continue;
        }

        let guard_position = text.find("amplihack hygiene artifact-guard");
        for add_position in broad_add_positions {
            if !guard_position.is_some_and(|guard| guard < add_position) {
                missing.push(format!(
                    "{}: missing artifact guard before broad staging at byte {add_position}",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(&path)
                        .display()
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "all broad staging recipes must run Artifact Guard first:\n{}",
        missing.join("\n")
    );
}

#[test]
fn recipe_commands_with_guarded_broad_staging_fail_loudly() {
    let mut unsafe_commands = Vec::new();

    for path in recipe_files() {
        let recipe = serde_yaml::from_str::<Value>(&read(&path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        for step in steps(&recipe) {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let Some(command) = step.get("command").and_then(Value::as_str) else {
                continue;
            };
            let Some(guard_position) = command.find("amplihack hygiene artifact-guard") else {
                continue;
            };
            let broad_add_after_guard =
                ["git add -A", "git add --all", "git add ."]
                    .iter()
                    .any(|needle| {
                        command
                            .match_indices(needle)
                            .any(|(add_position, _)| guard_position < add_position)
                    });
            if broad_add_after_guard && !command.contains("set -euo pipefail") {
                unsafe_commands.push(format!(
                    "{}:{id} guards broad staging but lacks `set -euo pipefail`",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(&path)
                        .display()
                ));
            }
        }
    }

    assert!(
        unsafe_commands.is_empty(),
        "guard failures must stop broad staging:\n{}",
        unsafe_commands.join("\n")
    );
}

#[test]
fn publish_and_finalize_do_not_inline_shell_delete_artifacts_as_remediation() {
    for recipe_name in ["workflow-publish.yaml", "workflow-finalize.yaml"] {
        let text = read(&recipe_path(recipe_name));
        for forbidden in [
            "rm -rf node_modules",
            "rm -rf dist",
            "rm -rf .claude/runtime",
            "git clean -fdx",
            "git reset --hard",
        ] {
            assert!(
                !text.contains(forbidden),
                "{recipe_name} must fail with remediation, not silently delete artifacts via `{forbidden}`"
            );
        }
    }
}

#[test]
fn guard_command_uses_structured_cli_arguments_not_shell_interpolation() {
    for recipe_name in ["workflow-publish.yaml", "workflow-finalize.yaml"] {
        let recipe = load_recipe(recipe_name);
        let commands: Vec<&str> = steps(&recipe)
            .iter()
            .filter_map(|step| step.get("command").and_then(Value::as_str))
            .filter(|command| command.contains("amplihack hygiene artifact-guard"))
            .collect();

        assert!(
            !commands.is_empty(),
            "{recipe_name} must invoke amplihack hygiene artifact-guard"
        );
        for command in commands {
            assert!(
                command.contains("--repo .") || command.contains("--repo \"$REPO_PATH\""),
                "{recipe_name} guard must pass an explicit repo path; command was:\n{command}"
            );
            assert!(
                command.contains("--mode pre-publish"),
                "{recipe_name} guard must use pre-publish mode before PR/finalize publication; command was:\n{command}"
            );
            assert!(
                !command.contains("eval ") && !command.contains("sh -c"),
                "{recipe_name} guard must not be invoked through eval/sh -c; command was:\n{command}"
            );
        }
    }
}

#[test]
fn workflow_publish_outside_in_fix_loop_guards_its_inline_git_add_all() {
    let recipe = load_recipe("workflow-publish.yaml");
    let prompt = step_text(&recipe, "step-16b-outside-in-fix-loop", "prompt");
    let guard = prompt
        .find("amplihack hygiene artifact-guard")
        .expect("outside-in fix loop must guard its inline git add -A");
    let add = prompt
        .find("git add -A")
        .expect("outside-in fix loop currently contains broad staging");

    assert!(
        guard < add,
        "outside-in fix loop must invoke artifact guard immediately before broad staging"
    );
}
