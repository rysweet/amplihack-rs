//! Issue #1740: the structure of `amplifier-bundle/recipes/quick-change.yaml`.
//!
//! One agent session, the repository's checks, a diff-limit step decided by
//! bash, an optional hand-over to `default-workflow`, then commit, push and
//! PR, and nothing after the PR. These tests parse the live YAML and the
//! shared tool script; no recipe runs.

use std::path::PathBuf;

use serde_yaml::Value;
use sha2::{Digest, Sha256};

const STEPS: &[&str] = &[
    "quick-change-worktree",
    "quick-change-record-base",
    "quick-change-implement",
    "quick-change-run-checks",
    "quick-change-diff-limits",
    "quick-change-write-handover",
    "quick-change-handover",
    "quick-change-commit",
    "quick-change-push",
    "quick-change-create-pr",
];

const STEP_ID_BANNED: &[&str] = &[
    "merge",
    "finaliz",
    "finalis",
    "auto-drive",
    "autodrive",
    "version",
    "bump",
];

const COMMAND_BANNED: &[&str] = &[
    "--force",
    "--mirror",
    "gh pr merge",
    "--admin",
    "set -x",
    "eval ",
    "bash -c \"$",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("repo root")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn recipe() -> Value {
    serde_yaml::from_str(&read("amplifier-bundle/recipes/quick-change.yaml"))
        .expect("quick-change.yaml must parse")
}

fn steps(r: &Value) -> Vec<Value> {
    r["steps"].as_sequence().expect("steps").clone()
}

fn step(r: &Value, id: &str) -> Value {
    steps(r)
        .into_iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("step {id} missing"))
}

fn command(r: &Value, id: &str) -> String {
    step(r, id)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("{id} must have a command"))
        .to_string()
}

fn condition(r: &Value, id: &str) -> String {
    step(r, id)["condition"].as_str().unwrap_or("").to_string()
}

fn is_agent(s: &Value) -> bool {
    s["type"].as_str() == Some("agent") || s.get("agent").is_some()
}

#[test]
fn steps_are_present_in_order() {
    let r = recipe();
    let ids: Vec<String> = steps(&r)
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, STEPS);
    assert_eq!(r["name"].as_str(), Some("quick-change"));
}

#[test]
fn exactly_one_agent_step_and_it_is_the_builder() {
    let r = recipe();
    let agents: Vec<Value> = steps(&r).into_iter().filter(is_agent).collect();
    assert_eq!(agents.len(), 1, "quick-change must run one agent session");
    assert_eq!(agents[0]["id"].as_str(), Some("quick-change-implement"));
    assert_eq!(agents[0]["agent"].as_str(), Some("amplihack:core:builder"));
}

#[test]
fn create_pr_is_last_and_no_step_merges_or_finalises() {
    let r = recipe();
    let all = steps(&r);
    assert_eq!(
        all.last().unwrap()["id"].as_str(),
        Some("quick-change-create-pr")
    );
    for s in &all {
        let id = s["id"].as_str().unwrap();
        for banned in STEP_ID_BANNED {
            assert!(!id.contains(banned), "step {id} looks like a {banned} step");
        }
        if let Some(sub) = s["recipe"].as_str() {
            assert!(
                ["workflow-worktree", "default-workflow"].contains(&sub),
                "unexpected sub-recipe {sub}"
            );
        }
    }
}

#[test]
fn no_step_declares_a_timeout_or_templates_into_bash() {
    let r = recipe();
    for s in steps(&r) {
        let id = s["id"].as_str().unwrap();
        assert!(
            s.get("timeout").is_none(),
            "{id} must not declare timeout (#439)"
        );
        if let Some(cmd) = s["command"].as_str() {
            assert!(
                !cmd.contains("{{"),
                "{id}: untrusted values only via RECIPE_VAR_*"
            );
            for banned in COMMAND_BANNED {
                assert!(!cmd.contains(banned), "{id} must not use {banned:?}");
            }
        }
    }
}

#[test]
fn diff_limits_step_always_runs_and_outputs_the_verdict() {
    let r = recipe();
    let s = step(&r, "quick-change-diff-limits");
    assert!(
        s.get("condition").is_none(),
        "no condition may skip the diff limits"
    );
    assert_eq!(s["output"].as_str(), Some("quick_change_verdict"));
    let cmd = command(&r, "quick-change-diff-limits");
    assert!(cmd.contains("quick_change_diff_limits.sh"));
    assert!(
        cmd.contains("--base"),
        "the base must be pinned, not recomputed"
    );
}

#[test]
fn handover_runs_default_workflow_on_the_quick_branch() {
    let r = recipe();
    for id in ["quick-change-write-handover", "quick-change-handover"] {
        assert!(condition(&r, id).contains("handover"), "{id}");
    }
    let s = step(&r, "quick-change-handover");
    assert_eq!(s["type"].as_str(), Some("recipe"));
    assert_eq!(s["recipe"].as_str(), Some("default-workflow"));
    let branch = s["context"]["existing_branch"].as_str().unwrap_or("");
    assert!(branch.contains("worktree_setup.branch_name"), "{branch}");
    assert!(command(&r, "quick-change-write-handover").contains("quick-change-handover.patch"));
}

#[test]
fn publish_steps_run_only_on_proceed() {
    let r = recipe();
    for id in [
        "quick-change-commit",
        "quick-change-push",
        "quick-change-create-pr",
    ] {
        let c = condition(&r, id);
        assert!(
            c.contains("proceed") && !c.contains("handover"),
            "{id}: {c}"
        );
    }
}

#[test]
fn publish_steps_use_files_retries_and_verification() {
    let r = recipe();
    let commit = command(&r, "quick-change-commit");
    assert!(
        commit.contains("git commit") && commit.contains(" -F "),
        "#1570: -F"
    );
    let guard = commit
        .find("artifact-guard")
        .expect("artifact guard before commit");
    let identity = commit.find("user.email").expect("commit identity");
    let commit_at = commit.find("git commit").unwrap();
    assert!(
        guard < identity && identity < commit_at,
        "guard, identity, then commit"
    );

    let push = command(&r, "quick-change-push");
    assert!(push.contains("refs/heads/") && push.contains("verify_pushed_ref.sh"));
    for banned in ["--all", "--tags", "--delete", "+refs/"] {
        assert!(!push.contains(banned), "push must not use {banned:?}");
    }

    let pr = command(&r, "quick-change-create-pr");
    for needle in [
        "--body-file",
        "workflow_gh_retry.sh",
        "--draft",
        "gh pr create",
    ] {
        assert!(pr.contains(needle), "create-pr must contain {needle:?}");
    }
    assert!(
        !pr.contains("--auto"),
        "create-pr must not enable auto-merge"
    );
    for tool in ["verify_pushed_ref.sh", "workflow_gh_retry.sh"] {
        assert!(
            repo_root()
                .join("amplifier-bundle/tools")
                .join(tool)
                .is_file()
        );
    }
}

#[test]
fn recursion_limits_and_line_budget() {
    let r = recipe();
    assert_eq!(r["recursion"]["max_depth"].as_u64(), Some(6));
    assert_eq!(r["recursion"]["max_total_steps"].as_u64(), Some(140));
    let n = read("amplifier-bundle/recipes/quick-change.yaml")
        .lines()
        .count();
    assert!(
        n <= 300,
        "quick-change.yaml is {n} lines; the target is 300"
    );
}

#[test]
fn manifest_entry_matches_the_recipe_hash() {
    let manifest: serde_json::Value =
        serde_json::from_str(&read("amplifier-bundle/recipes/_recipe_manifest.json")).unwrap();
    let body =
        std::fs::read(repo_root().join("amplifier-bundle/recipes/quick-change.yaml")).unwrap();
    let hex: String = Sha256::digest(&body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(manifest["quick-change"].as_str(), Some(&hex[..16]));
}

#[test]
fn common_preamble_never_resolves_tools_from_the_worktree() {
    let common = read("amplifier-bundle/tools/quick_change_common.sh");
    assert!(common.contains("set -euo pipefail"));
    assert!(common.contains("GIT_PAGER=cat"));
    assert!(!common.contains("$(pwd)") && !common.contains("${PWD") && !common.contains("$PWD"));
    let r = recipe();
    for s in steps(&r) {
        if s["type"].as_str() == Some("bash") {
            let id = s["id"].as_str().unwrap();
            assert!(
                s["command"]
                    .as_str()
                    .unwrap()
                    .contains("quick_change_common.sh"),
                "{id} must source quick_change_common.sh"
            );
        }
    }
}
