//! TDD contracts for issue #672 and related workflow reliability fixes.
//!
//! These tests intentionally assert the desired final repository state before
//! implementation. They cover YAML/docs/version contracts that do not have a
//! narrow Rust API surface.

use amplihack_cli::commands::orch::build_workstreams_config_to_tempfile;
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

fn build_workstream_entries(decomposition: serde_json::Value) -> Vec<serde_json::Value> {
    let path = build_workstreams_config_to_tempfile(&decomposition.to_string())
        .expect("workstreams config must build");
    let path = PathBuf::from(path);
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    std::fs::remove_file(&path).unwrap_or_else(|e| panic!("remove {}: {e}", path.display()));
    serde_json::from_str(&body).expect("workstreams config must be JSON array")
}

fn recipes(entries: &[serde_json::Value]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| {
            entry
                .get("recipe")
                .and_then(serde_json::Value::as_str)
                .expect("each workstream entry must have a string recipe")
        })
        .collect()
}

fn step_command<'a>(recipe: &'a Value, step_id: &str) -> &'a str {
    find_step(recipe, step_id)
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("step `{step_id}` must contain a command"))
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

fn assert_contains_all(label: &str, content: &str, required: &[&str]) {
    for needle in required {
        assert!(content.contains(needle), "{label} must contain `{needle}`");
    }
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
        version, "0.11.2",
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

#[test]
fn issue_672_workflow_reliability_normalizes_development_recipes() {
    let entries = build_workstream_entries(serde_json::json!({
        "task_type": "Development",
        "workstreams": [
            {
                "name": "missing-recipe",
                "classification": "Development",
                "description": "Add tests for the routing normalizer."
            },
            {
                "name": "empty-recipe",
                "classification": "Development",
                "description": "Fix blank generated recipe output.",
                "recipe": ""
            },
            {
                "name": "wrong-recipe",
                "classification": "Development",
                "description": "Implement Development behavior misrouted by the LLM.",
                "recipe": "investigation-workflow"
            },
            {
                "name": "correct-recipe",
                "classification": "Development",
                "description": "Keep already-correct Development routes stable.",
                "recipe": "default-workflow"
            }
        ]
    }));

    assert_eq!(
        recipes(&entries),
        vec![
            "default-workflow",
            "default-workflow",
            "default-workflow",
            "default-workflow",
        ],
        "Development-classified workstreams must route to default-workflow for missing, empty, wrong, and already-correct recipe values"
    );
}

#[test]
fn issue_672_workflow_reliability_normalizes_parallel_development_workstreams() {
    let entries = build_workstream_entries(serde_json::json!({
        "task_type": "Development",
        "workstreams": [
            {
                "name": "rust-routing",
                "classification": "Development",
                "description": "Implement deterministic Rust routing normalization.",
                "recipe": "investigation-workflow"
            },
            {
                "name": "recipe-coverage",
                "classification": "Development",
                "description": "Update recipe contract coverage.",
                "recipe": "consensus-workflow"
            },
            {
                "name": "prompt-guidance",
                "classification": "Development",
                "description": "Align routing prompt guidance.",
                "recipe": "qa-workflow"
            }
        ]
    }));

    assert_eq!(
        recipes(&entries),
        vec!["default-workflow", "default-workflow", "default-workflow"],
        "parallel Development decompositions must not leak non-Development recipes into orch run configs"
    );
}

#[test]
fn issue_672_workflow_reliability_preserves_non_development_routes() {
    let entries = build_workstream_entries(serde_json::json!({
        "task_type": "Development",
        "workstreams": [
            {
                "name": "investigation-route",
                "classification": "Investigation",
                "description": "Preserve the investigation route.",
                "recipe": "investigation-workflow"
            },
            {
                "name": "qa-route",
                "classification": "Q&A",
                "description": "Preserve the Q&A route.",
                "recipe": "qa-workflow"
            },
            {
                "name": "consensus-route",
                "classification": "Consensus",
                "description": "Preserve the consensus route.",
                "recipe": "consensus-workflow"
            }
        ]
    }));

    assert_eq!(
        recipes(&entries),
        vec![
            "investigation-workflow",
            "qa-workflow",
            "consensus-workflow",
        ],
        "non-Development workstreams with explicit recipes must not be coerced to default-workflow"
    );
}

#[test]
fn issue_672_workflow_reliability_development_paths_use_default_workflow() {
    let routing = load_recipe("amplifier-bundle/recipes/smart-execute-routing.yaml");

    for step_id in [
        "execute-single-round-1-development",
        "execute-single-fallback-blocked-development",
        "adaptive-execute-development",
    ] {
        let step = find_step(&routing, step_id);
        assert_eq!(
            step.get("type").and_then(Value::as_str),
            Some("recipe"),
            "{step_id} must be a recipe step"
        );
        assert_eq!(
            step.get("recipe").and_then(Value::as_str),
            Some("default-workflow"),
            "{step_id} must execute default-workflow for Development routing"
        );
    }
}

#[test]
fn issue_672_workflow_reliability_adaptive_gap_selects_default_workflow_for_development() {
    let routing = load_recipe("amplifier-bundle/recipes/smart-execute-routing.yaml");
    let step = find_step(&routing, "detect-execution-gap");
    let command = step
        .get("command")
        .and_then(Value::as_str)
        .expect("detect-execution-gap must have command text");

    assert!(
        command.contains("default-workflow"),
        "adaptive recovery must have a default-workflow route for Development gaps"
    );
    assert!(
        command.contains("investigation-workflow"),
        "adaptive recovery must preserve the Investigation route"
    );
    assert!(
        command.contains("grep -qi \"investigation\"")
            || command.contains("grep -qi 'investigation'"),
        "adaptive recovery must choose investigation-workflow only for Investigation task types"
    );
}

#[test]
fn issue_672_workflow_reliability_parallel_path_runs_normalized_workstreams_config() {
    let routing = load_recipe("amplifier-bundle/recipes/smart-execute-routing.yaml");
    let create_step = find_step(&routing, "create-workstreams-config");
    let launch_step = find_step(&routing, "launch-parallel-round-1");

    let create_command = create_step
        .get("command")
        .and_then(Value::as_str)
        .expect("create-workstreams-config must have command text");
    assert!(
        create_command.contains("amplihack orch helper build-workstreams-config"),
        "parallel routing must build workstream configs through the Rust helper that enforces recipe normalization"
    );

    let launch_command = launch_step
        .get("command")
        .and_then(Value::as_str)
        .expect("launch-parallel-round-1 must have command text");
    assert!(
        launch_command.contains("orch run -- \"$WS_FILE\""),
        "parallel routing must execute the normalized workstreams file, not reconstruct recipes inline"
    );
}

#[test]
fn issue_780_launcher_and_provenance_use_external_runtime_root_contract() {
    let launcher = read_repo_file("crates/amplihack-launcher/src/launcher_core.rs");
    let provenance = read_repo_file("crates/amplihack-workflows/src/provenance.rs");
    let combined = format!("{launcher}\n{provenance}");

    assert_contains_all(
        "shared runtime-root resolver",
        &combined,
        &[
            "AMPLIHACK_RUNTIME_ROOT",
            "XDG_RUNTIME_DIR",
            "/tmp/amplihack-runtime",
            "locks",
            "reflection",
            "logs",
            "metrics",
            "provenance",
        ],
    );
    assert!(
        !launcher.contains(".join(\".claude\").join(\"runtime\")"),
        "launcher must not create generated runtime state under the target worktree .claude/runtime"
    );
    assert!(
        !provenance.contains(".join(\".claude\").join(\"runtime\")"),
        "provenance must write under the shared runtime root, not under base_dir/.claude/runtime"
    );
}

#[test]
fn issue_780_workflow_worktree_exports_one_runtime_root_to_child_flows() {
    let recipe = load_recipe("amplifier-bundle/recipes/workflow-worktree.yaml");
    let command = step_command(&recipe, "step-04-setup-worktree");

    assert_contains_all(
        "workflow-worktree runtime root setup",
        command,
        &[
            "AMPLIHACK_RUNTIME_ROOT",
            "export AMPLIHACK_RUNTIME_ROOT",
            "XDG_RUNTIME_DIR",
            "/tmp/amplihack-runtime",
        ],
    );
    assert!(
        !command.contains("$WORKTREE_PATH/.claude/runtime")
            && !command.contains("${WORKTREE_PATH}/.claude/runtime"),
        "workflow-worktree must not seed runtime output inside the task worktree"
    );
}

#[test]
fn issue_780_lifecycle_recipes_preflight_runtime_artifacts_before_sensitive_operations() {
    for (recipe_rel, step_id, sensitive_operation) in [
        (
            "amplifier-bundle/recipes/workflow-tdd.yaml",
            "checkpoint-after-implementation",
            "git add",
        ),
        (
            "amplifier-bundle/recipes/workflow-refactor-review.yaml",
            "checkpoint-after-review-feedback",
            "git add",
        ),
        (
            "amplifier-bundle/recipes/workflow-pr-review.yaml",
            "step-18c-push-feedback-changes",
            "git add",
        ),
        (
            "amplifier-bundle/recipes/workflow-publish.yaml",
            "step-15-commit-push",
            "git add",
        ),
        (
            "amplifier-bundle/recipes/workflow-publish.yaml",
            "step-16-create-draft-pr",
            "workflow_publish_pr.sh",
        ),
        (
            "amplifier-bundle/recipes/workflow-finalize.yaml",
            "step-20a-artifact-guard",
            "amplihack hygiene artifact-guard",
        ),
        (
            "amplifier-bundle/recipes/workflow-finalize.yaml",
            "step-20b-push-cleanup",
            "git add",
        ),
        (
            "amplifier-bundle/recipes/workflow-finalize.yaml",
            "step-22b-final-status",
            "workflow_final_status.sh",
        ),
    ] {
        let recipe = load_recipe(recipe_rel);
        let command = step_command(&recipe, step_id);
        let preflight = command
            .find("preflight_known_workflow_runtime_artifacts")
            .unwrap_or_else(|| {
                panic!("{recipe_rel}:{step_id} must preflight known workflow runtime artifacts")
            });
        let sensitive = command.find(sensitive_operation).unwrap_or_else(|| {
            panic!(
                "{recipe_rel}:{step_id} must contain sensitive operation `{sensitive_operation}`"
            )
        });

        assert!(
            command.contains("workflow_runtime_artifacts.sh"),
            "{recipe_rel}:{step_id} must source the narrow runtime-artifact helper"
        );
        assert!(
            preflight < sensitive,
            "{recipe_rel}:{step_id} must preflight known runtime artifacts before `{sensitive_operation}`"
        );
    }
}
