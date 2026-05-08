//! Tests for issue #439: per-step `timeout:` lines on agent steps cause
//! spurious mid-thought aborts and must be removed from every recipe under
//! `amplifier-bundle/recipes/*.yaml`.
//!
//! These tests are intentionally written before the YAML cleanup lands (TDD).
//!
//! Contract enforced:
//!   1. No agent step in any `amplifier-bundle/recipes/*.yaml` may declare a
//!      per-step `timeout:` or `timeout_seconds:` field (at the step root or
//!      nested under `agent:`).
//!   2. A `timeout:` field is permitted ONLY on `bash:` steps that touch
//!      external network services (heuristic: command text contains `gh `,
//!      `curl`, `git fetch`, `git push`, `git pull`, `git clone`, or
//!      `git ls-remote`). When present, the value must be >= 1800 seconds —
//!      a generous availability floor, never a mid-thought abort gate.
//!   3. Recipe-level fields (e.g. `default_step_timeout:` at the document
//!      root) are explicitly NOT covered by this test — they act as global
//!      ceilings, not per-step gates, and `quality-loop.yaml` is allowed to
//!      keep its 1800s default.
//!   4. Every recipe file must still parse as valid YAML after the cleanup.
//!   5. The legacy `issue_449_step_02b_timeout.rs` test file (which asserted
//!      the *presence* of a per-step timeout that #439 removes) must be
//!      deleted — it is superseded by #439.

use std::path::{Path, PathBuf};

use serde_yaml::Value;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/amplihack-cli; walk up to repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn recipes_dir() -> PathBuf {
    repo_root().join("amplifier-bundle/recipes")
}

fn recipe_files() -> Vec<PathBuf> {
    let dir = recipes_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "expected at least one recipe yaml under {}",
        dir.display()
    );
    files
}

fn load_yaml(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn step_command(recipe_file: &str, step_id: &str) -> String {
    let path = recipes_dir().join(recipe_file);
    let recipe = load_yaml(&path);
    let steps = recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .unwrap_or_else(|| panic!("{}: expected steps sequence", path.display()));

    let step = steps
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(step_id))
        .unwrap_or_else(|| panic!("{}: missing step `{step_id}`", path.display()));

    step.get("command")
        .and_then(Value::as_str)
        .or_else(|| step.get("bash").and_then(Value::as_str))
        .or_else(|| {
            step.get("bash")
                .and_then(|bash| bash.get("command").and_then(Value::as_str))
        })
        .or_else(|| {
            step.get("bash")
                .and_then(|bash| bash.get("script").and_then(Value::as_str))
        })
        .unwrap_or_else(|| panic!("{}: step `{step_id}` has no shell command", path.display()))
        .to_string()
}

/// Returns true if the given bash command string looks like it talks to an
/// external network service that could legitimately hang indefinitely.
fn is_network_bash(cmd: &str) -> bool {
    let needles = [
        "gh ",
        "gh\n",
        "curl",
        "wget",
        "git fetch",
        "git push",
        "git pull",
        "git clone",
        "git ls-remote",
        "npm install",
        "npm i ",
        "pip install",
        "cargo fetch",
        "cargo install",
    ];
    needles.iter().any(|n| cmd.contains(n))
}

/// Walks every step in every recipe and yields `(file, step_id, step_value)`.
fn for_each_step<F: FnMut(&Path, &str, &Value)>(mut visit: F) {
    for file in recipe_files() {
        let recipe = load_yaml(&file);
        let Some(steps) = recipe.get("steps").and_then(Value::as_sequence) else {
            // Some files may not have a `steps:` sequence; skip them.
            continue;
        };
        for (idx, step) in steps.iter().enumerate() {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("<step #{idx}>"));
            visit(&file, &id, step);
        }
    }
}

/// (1) No agent step may declare a per-step timeout.
#[test]
fn no_agent_step_has_per_step_timeout() {
    let mut violations: Vec<String> = Vec::new();

    for_each_step(|file, id, step| {
        let is_agent_step = step.get("agent").is_some();
        if !is_agent_step {
            return;
        }

        // Check both step-root and nested `agent.*` for timeout fields.
        let root_timeout = step.get("timeout").or_else(|| step.get("timeout_seconds"));
        let agent_timeout = step
            .get("agent")
            .and_then(|a| a.get("timeout").or_else(|| a.get("timeout_seconds")));

        if let Some(v) = root_timeout {
            violations.push(format!(
                "{}: step `{id}` is an agent step with root-level `timeout`/`timeout_seconds: {v:?}`",
                file.display()
            ));
        }
        if let Some(v) = agent_timeout {
            violations.push(format!(
                "{}: step `{id}` has nested `agent.timeout`/`agent.timeout_seconds: {v:?}`",
                file.display()
            ));
        }
    });

    assert!(
        violations.is_empty(),
        "issue #439 violation — agent steps must not declare per-step timeouts:\n  {}",
        violations.join("\n  ")
    );
}

/// (2) Bash-step `timeout:` is permitted only for network-touching commands
/// and must be >= 1800 seconds.
#[test]
fn bash_step_timeouts_are_network_only_and_at_least_1800() {
    let mut violations: Vec<String> = Vec::new();

    for_each_step(|file, id, step| {
        let Some(bash) = step.get("bash") else {
            return;
        };

        let timeout = step
            .get("timeout")
            .or_else(|| step.get("timeout_seconds"))
            .or_else(|| bash.get("timeout"))
            .or_else(|| bash.get("timeout_seconds"));

        let Some(t) = timeout else {
            return; // No timeout — fine.
        };

        let secs = t
            .as_u64()
            .or_else(|| t.as_i64().and_then(|v| u64::try_from(v).ok()))
            .unwrap_or_else(|| {
                panic!(
                    "{}: step `{id}` has non-integer timeout {t:?}",
                    file.display()
                )
            });

        let cmd = bash
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| {
                bash.get("command")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                bash.get("script")
                    .and_then(Value::as_str)
                    .map(|s| s.to_string())
            })
            .unwrap_or_default();

        if !is_network_bash(&cmd) {
            violations.push(format!(
                "{}: bash step `{id}` has timeout={secs}s but command does not touch the network — remove it",
                file.display()
            ));
        } else if secs < 1800 {
            violations.push(format!(
                "{}: bash step `{id}` is network-touching but timeout={secs}s < 1800s floor",
                file.display()
            ));
        }
    });

    assert!(
        violations.is_empty(),
        "issue #439 violation — bash-step timeouts must be network-only and >= 1800s:\n  {}",
        violations.join("\n  ")
    );
}

/// (4) Every recipe must still parse as valid YAML.
#[test]
fn every_recipe_parses() {
    for file in recipe_files() {
        let _ = load_yaml(&file);
    }
}

/// (5) The legacy issue-449 timeout-presence test must be deleted as part of
/// the #439 cleanup — it asserts the exact behavior that #439 reverses.
#[test]
fn legacy_issue_449_test_is_removed() {
    let path = repo_root().join("crates/amplihack-cli/tests/issue_449_step_02b_timeout.rs");
    assert!(
        !path.exists(),
        "expected `{}` to be deleted (issue #439 supersedes #449)",
        path.display()
    );
}

/// (3) Sanity: `quality-loop.yaml` keeps its recipe-level `default_step_timeout`
/// (this is explicitly OUT of scope for #439).
#[test]
fn quality_loop_keeps_recipe_level_default_step_timeout() {
    let path = recipes_dir().join("quality-loop.yaml");
    if !path.exists() {
        return; // Recipe not present in this checkout — nothing to assert.
    }
    let recipe = load_yaml(&path);
    let v = recipe
        .get("default_step_timeout")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| {
            panic!(
                "{}: expected top-level `default_step_timeout: <int>` to remain (recipe-level ceiling, not a per-step gate)",
                path.display()
            )
        });
    assert!(
        v >= 1800,
        "{}: default_step_timeout={v} should be >= 1800s availability floor",
        path.display()
    );
}

/// (6) The full default-workflow chain (top-level recipe + all 9 sub-recipes)
/// must have zero YAML `timeout:` fields at any level (step root, `agent.*`,
/// or `bash.*`).  Agent steps intentionally run to natural completion; the
/// only `timeout` appearances allowed are shell-command uses *inside* bash
/// script text (e.g. `timeout 60 gh pr list …`), which this test does not
/// inspect.
///
/// This enforces the NO-TIMEOUT POLICY documented at the top of
/// `amplifier-bundle/recipes/default-workflow.yaml`.
#[test]
fn default_workflow_chain_has_no_yaml_timeout_fields() {
    const CHAIN: &[&str] = &[
        "default-workflow",
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

    let dir = recipes_dir();
    let mut violations: Vec<String> = Vec::new();

    for name in CHAIN {
        let path = dir.join(format!("{name}.yaml"));
        if !path.exists() {
            // Sub-recipe not present in this checkout — skip rather than fail
            // so the test can run in a partial checkout environment.
            continue;
        }
        let recipe = load_yaml(&path);
        let Some(steps) = recipe.get("steps").and_then(Value::as_sequence) else {
            continue;
        };
        for (idx, step) in steps.iter().enumerate() {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("<step #{idx}>"));

            for field in ["timeout", "timeout_seconds"] {
                if step.get(field).is_some() {
                    violations.push(format!(
                        "{name}.yaml: step `{id}` has YAML `{field}:` at step root"
                    ));
                }
                if step.get("agent").and_then(|a| a.get(field)).is_some() {
                    violations.push(format!(
                        "{name}.yaml: step `{id}` has YAML `agent.{field}:`"
                    ));
                }
                if let Some(bash) = step.get("bash") {
                    if bash.get(field).is_some() {
                        violations
                            .push(format!("{name}.yaml: step `{id}` has YAML `bash.{field}:`"));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "default-workflow NO-TIMEOUT POLICY violation — no YAML timeout fields \
         permitted in the default-workflow recipe chain:\n  {}\n\n\
         Agent steps must run to natural completion. If you need to guard a \
         network bash command, use the shell `timeout` command inside the \
         script text, not a YAML `timeout:` field.",
        violations.join("\n  ")
    );
}

const SAFE_COMMIT_STEPS: &[(&str, &str)] = &[
    ("workflow-tdd.yaml", "checkpoint-after-implementation"),
    (
        "workflow-refactor-review.yaml",
        "checkpoint-after-review-feedback",
    ),
    ("workflow-pr-review.yaml", "step-18c-push-feedback-changes"),
    ("workflow-finalize.yaml", "step-20b-push-cleanup"),
];

const PUSHING_SAFE_COMMIT_STEPS: &[(&str, &str)] = &[
    ("workflow-pr-review.yaml", "step-18c-push-feedback-changes"),
    ("workflow-finalize.yaml", "step-20b-push-cleanup"),
];

/// Issue #573: all remaining direct commit paths in the default workflow must
/// use explicit, visible pre-commit handling rather than a bare `git commit`.
#[test]
fn default_workflow_direct_commit_paths_use_safe_pre_commit_handling() {
    let mut violations = Vec::new();

    for (recipe, step_id) in SAFE_COMMIT_STEPS {
        let cmd = step_command(recipe, step_id);

        for required in [
            "set -euo pipefail",
            "git add -A",
            "git diff --cached --name-only",
            "commit_output_file=$(mktemp",
            "if git commit",
            "commit_rc=$?",
            "ERROR: git commit failed",
            "cat \"$commit_output_file\"",
            "git --no-pager diff --cached --name-status",
            "git --no-pager status --short",
            "exit \"$commit_rc\"",
            "No staged changes",
        ] {
            if !cmd.contains(required) {
                violations.push(format!("{recipe}: step `{step_id}` missing `{required}`"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "issue #573 violation — direct commit paths must surface hook/commit failures:\n  {}",
        violations.join("\n  ")
    );
}

/// Issue #573: safe commit handling must never bypass or hide Git hook
/// failures, and must not introduce timeout/sleep behavior.
#[test]
fn default_workflow_safe_commit_paths_do_not_bypass_hooks_or_add_timeouts() {
    let mut violations = Vec::new();

    for (recipe, step_id) in SAFE_COMMIT_STEPS {
        let cmd = step_command(recipe, step_id);
        for forbidden in ["--no-verify", "|| true", "timeout ", "\ntimeout", "sleep "] {
            if cmd.contains(forbidden) {
                violations.push(format!(
                    "{recipe}: step `{step_id}` contains forbidden `{forbidden}`"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "issue #573 violation — safe commit paths must preserve hooks and no-timeout policy:\n  {}",
        violations.join("\n  ")
    );
}

/// Issue #573: push-capable feedback/cleanup paths must publish the active
/// branch/upstream instead of assuming `main` or requiring an existing upstream.
#[test]
fn default_workflow_feedback_and_cleanup_pushes_are_branch_aware() {
    let mut violations = Vec::new();

    for (recipe, step_id) in PUSHING_SAFE_COMMIT_STEPS {
        let cmd = step_command(recipe, step_id);

        for required in [
            "current_branch=$(git branch --show-current)",
            "git rev-parse --abbrev-ref '@{u}'",
            "git push -u origin \"$current_branch\"",
            "ERROR: current branch is empty",
            "push_output_file=$(mktemp",
            "ERROR: git push failed",
            "exit \"$push_rc\"",
        ] {
            if !cmd.contains(required) {
                violations.push(format!("{recipe}: step `{step_id}` missing `{required}`"));
            }
        }

        for forbidden in ["origin/main", "main...HEAD", " main "] {
            if cmd.contains(forbidden) {
                violations.push(format!(
                    "{recipe}: step `{step_id}` contains branch assumption `{forbidden}`"
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "issue #573 violation — feedback/cleanup push paths must be active-branch aware:\n  {}",
        violations.join("\n  ")
    );
}
