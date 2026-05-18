//! P1 workflow reliability TDD tests (issues #624, #596, #614, #581).
//!
//! Four workstreams, each with structural and/or runtime assertions:
//!
//! WS1 (#624): Verdict synonym mapping + unknown-verdict fail-safe
//! WS2 (#596): Verifier prompt prioritises git log over working tree
//! WS3 (#614): resolve-bundle-asset registers helper-path and hooks-dir
//! WS4 (#581): SKILL.md references Rust CLI, not Python lock_tool.py

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

// ───────────────────────────────────────────────────────────────────────────
// Shared helpers
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Recipe {
    #[serde(default)]
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct Step {
    id: String,
    #[serde(rename = "type", default)]
    step_type: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    output: Option<String>,
}

fn workflow_tdd_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("amplifier-bundle")
        .join("recipes")
        .join("workflow-tdd.yaml")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn load_recipe() -> Recipe {
    let path = workflow_tdd_path();
    let text = std::fs::read_to_string(&path).unwrap();
    serde_yaml::from_str(&text).unwrap()
}

fn step_by_id(id: &str) -> Step {
    let raw = std::fs::read_to_string(workflow_tdd_path()).unwrap();
    let val: serde_yaml::Value = serde_yaml::from_str(&raw).unwrap();
    for step in val.get("steps").unwrap().as_sequence().unwrap() {
        if step.get("id").and_then(|v| v.as_str()) == Some(id) {
            return serde_yaml::from_value(step.clone()).unwrap();
        }
    }
    panic!("step {id} not found in workflow-tdd.yaml")
}

fn enforce_verdict_command() -> String {
    step_by_id("step-08c-enforce-verdict")
        .command
        .expect("enforce-verdict must have a command")
}

struct GateRun {
    code: i32,
    stderr: String,
}

fn run_gate(verdict_json: &str, implementation: &str, allow_no_op: &str) -> GateRun {
    let cmd = enforce_verdict_command();
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.as_file_mut().write_all(cmd.as_bytes()).unwrap();

    let out = Command::new("bash")
        .arg(f.path())
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("VERDICT_JSON", verdict_json)
        .env("IMPLEMENTATION", implementation)
        .env("ALLOW_NO_OP", allow_no_op)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    GateRun {
        code: out.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WS1 — Issue #624: Verdict synonym mapping + unknown-verdict fail-safe
// ═══════════════════════════════════════════════════════════════════════════

mod ws1_synonym_mapping {
    use super::*;

    /// The enforce-verdict bash command must contain the synonym mapping block.
    #[test]
    fn enforce_verdict_command_contains_synonym_case_block() {
        let cmd = enforce_verdict_command();
        assert!(
            cmd.contains("VERIFIED") && cmd.contains("WORK_VERIFIED"),
            "enforce-verdict must contain synonym mapping from VERIFIED to WORK_VERIFIED"
        );
        assert!(
            cmd.contains("NO_WORK") && cmd.contains("HOLLOW_SUCCESS"),
            "enforce-verdict must map NO_WORK to HOLLOW_SUCCESS"
        );
        assert!(
            cmd.contains("INCONCLUSIVE") && cmd.contains("INSUFFICIENT_EVIDENCE"),
            "enforce-verdict must map INCONCLUSIVE to INSUFFICIENT_EVIDENCE"
        );
    }

    // ── Runtime: each synonym must map to the correct canonical verdict ──

    #[test]
    fn synonym_verified_maps_to_work_verified_exit_0() {
        let v = r#"{"verdict": "VERIFIED", "evidence": ["a"], "rationale": "ok"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(
            run.code, 0,
            "VERIFIED synonym must exit 0 (maps to WORK_VERIFIED): {}",
            run.stderr
        );
        assert!(
            run.stderr.contains("APPROVED") || run.stderr.contains("WORK_VERIFIED"),
            "VERIFIED should hit the WORK_VERIFIED branch: {}",
            run.stderr
        );
    }

    #[test]
    fn synonym_success_maps_to_work_verified_exit_0() {
        let v = r#"{"verdict": "SUCCESS", "evidence": ["a"], "rationale": "ok"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "SUCCESS synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_approved_maps_to_work_verified_exit_0() {
        let v = r#"{"verdict": "APPROVED", "evidence": ["a"], "rationale": "ok"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "APPROVED synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_pass_maps_to_work_verified_exit_0() {
        let v = r#"{"verdict": "PASS", "evidence": ["a"], "rationale": "ok"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "PASS synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_passed_maps_to_work_verified_exit_0() {
        let v = r#"{"verdict": "PASSED", "evidence": ["a"], "rationale": "ok"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "PASSED synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_failed_maps_to_hollow_success_exit_1() {
        let v = r#"{"verdict": "FAILED", "evidence": [], "rationale": "no"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(
            run.code, 1,
            "FAILED synonym must exit 1 (maps to HOLLOW_SUCCESS): {}",
            run.stderr
        );
    }

    #[test]
    fn synonym_no_work_maps_to_hollow_success_exit_1() {
        let v = r#"{"verdict": "NO_WORK", "evidence": [], "rationale": "empty"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 1, "NO_WORK synonym must exit 1: {}", run.stderr);
    }

    #[test]
    fn synonym_empty_maps_to_hollow_success_exit_1() {
        let v = r#"{"verdict": "EMPTY", "evidence": [], "rationale": "nothing"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 1, "EMPTY synonym must exit 1: {}", run.stderr);
    }

    #[test]
    fn synonym_no_artifacts_maps_to_hollow_success_exit_1() {
        let v = r#"{"verdict": "NO_ARTIFACTS", "evidence": [], "rationale": "none"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(
            run.code, 1,
            "NO_ARTIFACTS synonym must exit 1: {}",
            run.stderr
        );
    }

    #[test]
    fn synonym_inconclusive_maps_to_insufficient_evidence_exit_0() {
        let v = r#"{"verdict": "INCONCLUSIVE", "evidence": [], "rationale": "unsure"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(
            run.code, 0,
            "INCONCLUSIVE synonym must exit 0: {}",
            run.stderr
        );
        assert!(
            run.stderr.contains("WARN") || run.stderr.contains("INSUFFICIENT_EVIDENCE"),
            "INCONCLUSIVE should warn: {}",
            run.stderr
        );
    }

    #[test]
    fn synonym_unknown_maps_to_insufficient_evidence_exit_0() {
        let v = r#"{"verdict": "UNKNOWN", "evidence": [], "rationale": "dunno"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "UNKNOWN synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_unclear_maps_to_insufficient_evidence_exit_0() {
        let v = r#"{"verdict": "UNCLEAR", "evidence": [], "rationale": "maybe"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "UNCLEAR synonym must exit 0: {}", run.stderr);
    }

    #[test]
    fn synonym_partial_maps_to_insufficient_evidence_exit_0() {
        let v = r#"{"verdict": "PARTIAL", "evidence": ["x"], "rationale": "partial"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(run.code, 0, "PARTIAL synonym must exit 0: {}", run.stderr);
    }

    // ── Unknown verdict fail-safe ──

    #[test]
    fn completely_novel_verdict_string_failsafes_to_exit_0() {
        let v = r#"{"verdict": "LOOKS_GOOD_TO_ME", "evidence": [], "rationale": "lgtm"}"#;
        let run = run_gate(v, "files changed", "false");
        assert_eq!(
            run.code, 0,
            "Novel verdict must fail-safe to exit 0 (#624): {}",
            run.stderr
        );
        assert!(
            run.stderr.contains("fail-safe") || run.stderr.contains("unknown"),
            "Novel verdict must warn about fail-safe: {}",
            run.stderr
        );
    }

    #[test]
    fn unknown_verdict_never_exit_1() {
        // Regression: before #624, the `*) exit 1` default killed recipes
        // after PR was already opened.
        for verdict in &["MAYBE", "NEEDS_REVIEW", "PENDING", "LGTM", "OK"] {
            let v = format!(r#"{{"verdict": "{verdict}", "evidence": [], "rationale": "test"}}"#);
            let run = run_gate(&v, "files changed", "false");
            assert_eq!(
                run.code, 0,
                "Unknown verdict '{verdict}' must fail-safe to exit 0, not exit 1 (#624): {}",
                run.stderr
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WS2 — Issue #596: Verifier prompt prioritises git log over working tree
// ═══════════════════════════════════════════════════════════════════════════

mod ws2_verifier_prompt {
    use super::*;

    fn verifier_prompt() -> String {
        step_by_id("step-08c-work-verifier")
            .prompt
            .expect("work-verifier must have a prompt")
    }

    /// The verifier step must be an agentic step, not bash.
    #[test]
    fn work_verifier_is_agent_not_bash() {
        let step = step_by_id("step-08c-work-verifier");
        assert!(step.command.is_none(), "work-verifier must NOT be bash");
        assert!(step.agent.is_some(), "work-verifier must be an agent step");
    }

    /// Git log must be listed as PRIMARY evidence source.
    #[test]
    fn prompt_labels_git_log_as_primary() {
        let prompt = verifier_prompt();
        assert!(
            prompt.contains("PRIMARY"),
            "Verifier prompt must label git log evidence as PRIMARY (#596)"
        );
        // The PRIMARY label must appear near git log, not near working tree
        let primary_pos = prompt.find("PRIMARY").unwrap();
        let git_log_pos = prompt.find("git log").unwrap();
        let working_tree_pos = prompt.find("Working tree").unwrap_or(usize::MAX);
        assert!(
            (primary_pos as isize - git_log_pos as isize).unsigned_abs() < 200,
            "PRIMARY label must be near 'git log', not somewhere else"
        );
        assert!(
            primary_pos < working_tree_pos,
            "PRIMARY must appear before the working tree section"
        );
    }

    /// Working tree must be listed as SECONDARY evidence source.
    #[test]
    fn prompt_labels_working_tree_as_secondary() {
        let prompt = verifier_prompt();
        assert!(
            prompt.contains("SECONDARY"),
            "Verifier prompt must label working tree as SECONDARY (#596)"
        );
    }

    /// The prompt must explicitly state that a clean working tree after
    /// commit/push is CORRECT behavior (the core #596 regression).
    #[test]
    fn prompt_states_clean_worktree_is_correct_after_commit() {
        let prompt = verifier_prompt();
        let lower = prompt.to_lowercase();
        assert!(
            lower.contains("clean") && lower.contains("correct"),
            "Prompt must state clean working tree after commit is CORRECT (#596)"
        );
        assert!(
            prompt.contains("NOT evidence of no work")
                || prompt.contains("not evidence of no work")
                || prompt.contains("is NOT evidence"),
            "Prompt must explicitly say clean tree is NOT evidence of no work"
        );
    }

    /// The prompt must list git log before working tree in the numbered steps.
    #[test]
    fn git_log_listed_before_working_tree_in_investigation_steps() {
        let prompt = verifier_prompt();
        let git_log_pos = prompt.find("git log").expect("prompt must mention git log");
        let working_tree_pos = prompt
            .find("Working tree")
            .or_else(|| prompt.find("working tree"))
            .or_else(|| prompt.find("git status"))
            .expect("prompt must mention working tree / git status");
        assert!(
            git_log_pos < working_tree_pos,
            "git log (pos {git_log_pos}) must appear before working tree (pos {working_tree_pos})"
        );
    }

    /// No brittle git-diff-based hollow-success bash guard should remain.
    #[test]
    fn no_legacy_bash_hollow_success_guard() {
        let recipe = load_recipe();
        let ids: Vec<&str> = recipe.steps.iter().map(|s| s.id.as_str()).collect();
        assert!(
            !ids.contains(&"step-08c-implementation-no-op-guard"),
            "Legacy bash hollow-success guard must be removed (#596)"
        );
    }

    /// The verifier prompt must check PRs and merged PRs.
    #[test]
    fn prompt_checks_pr_status() {
        let prompt = verifier_prompt();
        assert!(
            prompt.contains("gh pr") || prompt.contains("pull request"),
            "Verifier must check PR status as evidence (#596)"
        );
        assert!(
            prompt.contains("merged") || prompt.contains("is:merged"),
            "Verifier must check for merged PRs (#596)"
        );
    }

    /// The verifier must check git log commits, not just working tree diffs.
    #[test]
    fn prompt_checks_commits_on_branch() {
        let prompt = verifier_prompt();
        assert!(
            prompt.contains("git log") && prompt.contains("origin/main"),
            "Verifier must use 'git log' with origin/main to check commits (#596)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WS3 — Issue #614: resolve-bundle-asset registers helper-path, hooks-dir
// ═══════════════════════════════════════════════════════════════════════════

mod ws3_asset_aliases {
    use super::*;

    fn resolve_bundle_asset_mod() -> String {
        let mod_path = workspace_root()
            .join("crates")
            .join("amplihack-cli")
            .join("src")
            .join("resolve_bundle_asset")
            .join("mod.rs");
        std::fs::read_to_string(&mod_path).unwrap()
    }

    /// The smart-orchestrator recipe references helper-path; the CLI must
    /// recognise it as a named asset.
    #[test]
    fn smart_orchestrator_helper_path_reference_has_matching_asset() {
        let so_path = workspace_root()
            .join("amplifier-bundle")
            .join("recipes")
            .join("smart-orchestrator.yaml");
        if so_path.exists() {
            let content = std::fs::read_to_string(&so_path).unwrap();
            if content.contains("helper-path") {
                let rust_src = resolve_bundle_asset_mod();
                assert!(
                    rust_src.contains(r#""helper-path""#),
                    "helper-path is referenced in smart-orchestrator.yaml but \
                     not registered in NAMED_ASSETS (#614)"
                );
            }
        }
    }

    /// The smart-orchestrator recipe references hooks-dir; the CLI must
    /// recognise it as a named asset.
    #[test]
    fn smart_orchestrator_hooks_dir_reference_has_matching_asset() {
        let so_path = workspace_root()
            .join("amplifier-bundle")
            .join("recipes")
            .join("smart-orchestrator.yaml");
        if so_path.exists() {
            let content = std::fs::read_to_string(&so_path).unwrap();
            if content.contains("hooks-dir") {
                let rust_src = resolve_bundle_asset_mod();
                assert!(
                    rust_src.contains(r#""hooks-dir""#),
                    "hooks-dir is referenced in smart-orchestrator.yaml but \
                     not registered in NAMED_ASSETS (#614)"
                );
            }
        }
    }

    /// NAMED_ASSETS must contain helper-path pointing to multitask-orchestrator.sh.
    #[test]
    fn named_assets_helper_path_targets_orchestrator() {
        let src = resolve_bundle_asset_mod();
        assert!(
            src.contains("helper-path") && src.contains("multitask-orchestrator"),
            "helper-path must point to multitask-orchestrator script (#614, #634)"
        );
    }

    /// NAMED_ASSETS must contain hooks-dir pointing to hooks/.
    #[test]
    fn named_assets_hooks_dir_targets_hooks_directory() {
        let src = resolve_bundle_asset_mod();
        assert!(
            src.contains("hooks-dir") && src.contains("hooks"),
            "hooks-dir must point to a hooks directory (#614)"
        );
    }

    /// The error message for unknown assets must list all registered names
    /// including the new ones.
    #[test]
    fn unknown_asset_error_lists_all_registered_names() {
        let src = resolve_bundle_asset_mod();
        for name in &["hooks-dir", "helper-path", "multitask-orchestrator"] {
            assert!(
                src.contains(&format!(r#""{name}""#)),
                "NAMED_ASSETS must include {name} (#614)"
            );
        }
    }

    /// run_cli must dispatch named assets (not fall through to raw path
    /// validation which would reject single-token names).
    #[test]
    fn run_cli_dispatches_helper_path_as_named_asset() {
        let src = resolve_bundle_asset_mod();
        assert!(
            src.contains(r#""helper-path""#),
            "helper-path must be in NAMED_ASSETS for run_cli dispatch (#614)"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// WS4 — Issue #581: SKILL.md must reference Rust CLI, not Python
// ═══════════════════════════════════════════════════════════════════════════

mod ws4_no_python_lock_tool {
    use super::*;

    fn fleet_copilot_skill_md() -> String {
        let path = workspace_root()
            .join("amplifier-bundle")
            .join("skills")
            .join("fleet-copilot")
            .join("SKILL.md");
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Cannot read fleet-copilot SKILL.md: {e}"))
    }

    /// SKILL.md must NOT reference the nonexistent Python lock tool.
    #[test]
    fn skill_md_does_not_reference_python_lock_tool() {
        let content = fleet_copilot_skill_md();
        assert!(
            !content.contains("lock_tool.py"),
            "SKILL.md must not reference lock_tool.py (#581)"
        );
        assert!(
            !content.contains("python .claude/tools"),
            "SKILL.md must not reference python .claude/tools (#581)"
        );
    }

    /// SKILL.md must reference the Rust CLI `amplihack lock` command.
    #[test]
    fn skill_md_references_amplihack_lock_command() {
        let content = fleet_copilot_skill_md();
        assert!(
            content.contains("amplihack lock"),
            "SKILL.md must reference `amplihack lock` (#581)"
        );
    }

    /// No SKILL.md across the entire skills directory should reference
    /// the Python lock_tool.py file.
    #[test]
    fn no_skill_md_references_python_lock_tool() {
        let skills_dir = workspace_root().join("amplifier-bundle").join("skills");
        if !skills_dir.exists() {
            return;
        }
        fn check_dir(dir: &std::path::Path) {
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => return,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    check_dir(&path);
                } else if path.file_name().and_then(|f| f.to_str()) == Some("SKILL.md") {
                    let content = std::fs::read_to_string(&path).unwrap_or_default();
                    assert!(
                        !content.contains("lock_tool.py"),
                        "{}: must not reference lock_tool.py (#581)",
                        path.display()
                    );
                }
            }
        }
        check_dir(&skills_dir);
    }
}
