//! TDD contracts for issue #672 and related workflow reliability fixes.
//!
//! These tests intentionally assert the desired final repository state before
//! implementation. They cover YAML/docs/version contracts that do not have a
//! narrow Rust API surface.

use serde_yaml::Value;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn read_repo_file(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(rel: &str) -> Value {
    let text = read_repo_file(rel);
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

fn find_step<'a>(recipe: &'a Value, step_id: &str) -> &'a Value {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must have steps")
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(step_id))
        .unwrap_or_else(|| panic!("step `{step_id}` not found"))
}

fn collect_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, extensions: &[&str], files: &mut Vec<PathBuf>) {
        for entry in
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extensions, files);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
            {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    walk(root, extensions, &mut files);
    files
}

#[test]
fn workspace_version_matches_latest_release_line() {
    let cargo_toml: toml::Value =
        toml::from_str(&read_repo_file("Cargo.toml")).expect("root Cargo.toml must parse");
    let version = cargo_toml
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .expect("workspace.package.version must exist");

    assert_eq!(
        version, "0.11.1",
        "workspace.package.version must match the latest release line"
    );
}

#[test]
fn install_flow_no_longer_mentions_dead_profile_management_module() {
    let install_mod = read_repo_file("crates/amplihack-cli/src/commands/install/mod.rs");

    assert!(
        !install_mod.contains("profile_management")
            && !install_mod.contains("Profile management unavailable"),
        "install flow must remove the obsolete Python profile_management availability check and warning"
    );
}

#[test]
fn known_safe_bundle_symlinks_are_handled_without_generic_warning() {
    let filesystem = read_repo_file("crates/amplihack-cli/src/commands/install/filesystem.rs");

    for expected in [
        "skills/docx/ooxml",
        "skills/pptx/ooxml",
        "skills/outside-in-testing",
    ] {
        assert!(
            filesystem.contains(expected),
            "copy logic must explicitly recognize known-safe bundled symlink path `{expected}`"
        );
    }
    assert!(
        filesystem.contains("known-safe") || filesystem.contains("known_safe"),
        "copy logic should distinguish known-safe bundle symlinks from arbitrary symlinks instead of warning for all symlinks"
    );
}

#[test]
fn tdd_checkpoint_formats_rust_workspace_before_staging() {
    let recipe = load_recipe("amplifier-bundle/recipes/workflow-tdd.yaml");
    let step = find_step(&recipe, "checkpoint-after-implementation");
    let command = step
        .get("command")
        .and_then(Value::as_str)
        .expect("checkpoint-after-implementation must have a command");

    let fmt_pos = command
        .find("cargo fmt --all")
        .expect("checkpoint-after-implementation must run `cargo fmt --all`");
    let add_pos = command
        .find("git add")
        .expect("checkpoint-after-implementation must stage changes");
    assert!(
        fmt_pos < add_pos,
        "`cargo fmt --all` must run before git add/commit so pre-commit hooks do not fail on unstaged formatting diffs"
    );
}

#[test]
fn finalize_pr_ready_step_is_defensive_and_bounded() {
    let recipe = load_recipe("amplifier-bundle/recipes/workflow-finalize.yaml");
    let step = find_step(&recipe, "step-21-pr-ready");
    let command = step
        .get("command")
        .and_then(Value::as_str)
        .expect("step-21-pr-ready must have a command");

    for required in [
        "command -v gh",
        "command -v jq",
        "gh auth status",
        "timeout",
        "gh pr view",
        "PR_NUMBER",
        "gh pr list",
        "git branch --show-current",
        "isDraft",
        "state",
    ] {
        assert!(
            command.contains(required),
            "step-21-pr-ready must include `{required}` handling for missing gh/auth/network/state edge cases"
        );
    }
    assert!(
        !command.contains("\ngh pr ready \"$PR_URL\""),
        "step-21-pr-ready must not call bare `gh pr ready \"$PR_URL\"`; it should be timeout-bounded and state-aware"
    );
    assert!(
        !command.contains("requires a git repo"),
        "step-21-pr-ready must not fail the whole finalize phase just because branch discovery cannot use a local git checkout"
    );
    assert!(
        command.contains("for pr_target in"),
        "step-21-pr-ready must iterate discovered PR targets instead of assuming a single PR_URL"
    );
    assert!(
        command.contains("no PR_URL, valid PR_NUMBER, or branch PR found"),
        "step-21-pr-ready must visibly skip when no relevant PR can be discovered"
    );
}

#[test]
fn amplifier_bundle_docs_reference_rust_recipe_runner_not_python_runner() {
    let bundle = repo_root().join("amplifier-bundle");
    let mut stale = Vec::new();
    for path in collect_files(&bundle, &["md", "yaml"]) {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for forbidden in [
            "Python controls step execution",
            "recipe_runner.py",
            "python recipe runner",
            "cargo install --git https://github.com/rysweet/amplihack-recipe-runner",
        ] {
            if content
                .to_ascii_lowercase()
                .contains(&forbidden.to_ascii_lowercase())
            {
                stale.push(format!(
                    "{} contains stale recipe-runner reference `{forbidden}`",
                    path.strip_prefix(repo_root()).unwrap_or(&path).display()
                ));
            }
        }
    }

    assert!(
        stale.is_empty(),
        "amplifier-bundle docs/recipes must reference the supported Rust `amplihack recipe run ...` / recipe-runner-rs path, not removed Python runner docs:\n{}",
        stale.join("\n")
    );
}

#[test]
fn recipe_files_with_git_commands_disable_interactive_pagers() {
    let recipes = repo_root().join("amplifier-bundle/recipes");
    let mut offenders = Vec::new();

    for path in collect_files(&recipes, &["yaml"]) {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let file_has_pager_default = content.contains("GIT_PAGER=cat");
        for (idx, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            let mentions_git_command = trimmed.contains("git ")
                || trimmed.contains("git -C ")
                || trimmed.contains("$(git ")
                || trimmed.contains("`git ");
            let line_is_safe = trimmed.contains("git --no-pager")
                || trimmed.contains("GIT_PAGER=cat")
                || trimmed.contains("PAGER=cat");
            if mentions_git_command && !file_has_pager_default && !line_is_safe {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(repo_root()).unwrap_or(&path).display(),
                    idx + 1,
                    trimmed
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "recipe git commands must use `git --no-pager` or set GIT_PAGER=cat/PAGER=cat to avoid blocking agent subprocesses:\n{}",
        offenders
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn issue_749_prompts_and_dev_orchestrator_docs_state_development_routing_invariant() {
    let required = "Development classification always routes to `default-workflow`; model-provided recipe fields do not override that invariant.";

    for rel in [
        "amplifier-bundle/recipes/smart-classify-route.yaml",
        "amplifier-bundle/recipes/smart-execute-routing.yaml",
        "amplifier-bundle/skills/dev-orchestrator/SKILL.md",
        "amplifier-bundle/skills/dev-orchestrator/reference.md",
    ] {
        let content = read_repo_file(rel);
        assert!(
            content.contains(required),
            "{rel} must explicitly state the Development routing invariant"
        );
    }
}

#[test]
fn issue_749_prompts_and_dev_orchestrator_docs_state_per_workstream_hybrid_routing() {
    let required =
        "Hybrid decompositions route each workstream by its own normalized classification.";

    for rel in [
        "amplifier-bundle/recipes/smart-classify-route.yaml",
        "amplifier-bundle/recipes/smart-execute-routing.yaml",
        "amplifier-bundle/skills/dev-orchestrator/SKILL.md",
        "amplifier-bundle/skills/dev-orchestrator/reference.md",
    ] {
        let content = read_repo_file(rel);
        assert!(
            content.contains(required),
            "{rel} must document per-workstream routing for hybrid decompositions"
        );
    }
}

#[test]
fn issue_749_routing_hook_prompt_reinforces_development_invariant() {
    let content = read_repo_file("crates/amplihack-hooks/src/routing_prompt.txt");

    assert!(
        content.contains(
            "Development classification always routes to `default-workflow`; model-provided recipe fields do not override that invariant."
        ),
        "routing hook prompt must reinforce the Development routing invariant"
    );
}
