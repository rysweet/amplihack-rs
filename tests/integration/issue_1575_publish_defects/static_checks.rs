//! Static contract for #1575: recipe wiring, line budgets, and the security
//! properties listed in docs/reference/workflow-publish-helpers.md.

use regex::Regex;
use serde_yaml::Value;

use super::{helper_path, read, recipe_path};

const NEW_HELPERS: &[&str] = &[
    "workflow_sync_pr_head.sh",
    "workflow_stage_task_changes.sh",
    "workflow_commit_message.sh",
    "workflow_gh_auth_gate.sh",
];

fn publish() -> Value {
    serde_yaml::from_str(&read(&recipe_path("workflow-publish"))).expect("parse workflow-publish")
}

fn step<'a>(recipe: &'a Value, id: &str) -> &'a Value {
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .find(|s| s.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("workflow-publish must define step `{id}`"))
}

fn command(recipe: &Value, id: &str) -> String {
    step(recipe, id)["command"]
        .as_str()
        .unwrap_or_else(|| panic!("`{id}` must be a bash command step"))
        .to_string()
}

fn helper_text(name: &str) -> String {
    read(&helper_path(name))
}

/// Shell lines that are not comments and not echo/printf output.
fn code_lines(src: &str) -> impl Iterator<Item = &str> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && !l.starts_with("echo ") && !l.starts_with("printf "))
}

#[test]
fn new_helpers_exist_and_fit_the_brick_budget() {
    for name in NEW_HELPERS.iter().chain(["workflow_pr_ready.sh"].iter()) {
        let lines = helper_text(name).lines().count();
        assert!(lines <= 400, "{name} has {lines} lines (budget 400)");
    }
}

#[test]
fn publish_recipe_stays_within_399_lines() {
    let lines = read(&recipe_path("workflow-publish")).lines().count();
    assert!(
        lines <= 399,
        "workflow-publish.yaml has {lines} lines (budget 399)"
    );
}

#[test]
fn sync_step_runs_first_in_pre_probe_mode() {
    let recipe = publish();
    let ids: Vec<&str> = recipe["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|s| s.get("id").and_then(Value::as_str))
        .collect();
    let sync = ids
        .iter()
        .position(|i| *i == "publish-sync-pr-head")
        .expect("sync step");
    let probe = ids
        .iter()
        .position(|i| *i == "publish-terminal-state")
        .expect("probe step");
    assert_eq!(
        sync + 1,
        probe,
        "publish-sync-pr-head must run immediately before the probe"
    );
    let cmd = command(&recipe, "publish-sync-pr-head");
    assert!(
        cmd.contains("workflow_sync_pr_head.sh") && cmd.contains("--mode pre-probe"),
        "{cmd}"
    );
    assert!(
        !cmd.contains("{{"),
        "no template expansion in shell lines\n{cmd}"
    );
    // #1528/#1575: the worktree path has no REPO_PATH fallback, so the step
    // can never push from the caller's own checkout.
    let path_line = cmd
        .lines()
        .find(|l| l.contains("WORKTREE_SETUP_WORKTREE_PATH:="))
        .expect("worktree path default");
    assert!(
        !path_line.contains("REPO_PATH"),
        "publish-sync-pr-head must not fall back to REPO_PATH\n{path_line}"
    );
    assert!(
        cmd.contains("is not a linked task worktree"),
        "publish-sync-pr-head must check for a linked task worktree\n{cmd}"
    );
}

#[test]
fn step15_warns_when_the_fallback_subject_is_used() {
    let cmd = command(&publish(), "step-15-commit-push");
    assert!(
        cmd.contains("WARNING: step-15: commit message helper returned no subject"),
        "{cmd}"
    );
}

#[test]
fn step15_uses_the_helpers_and_never_sweeps_with_git_add_all() {
    let cmd = command(&publish(), "step-15-commit-push");
    for needle in [
        "workflow_stage_task_changes.sh",
        "workflow_commit_message.sh",
        "workflow_sync_pr_head.sh",
        "--mode step15",
        "ALLOW_VERSION_BUMP=\"${RECIPE_VAR_allow_version_bump:-false}\"",
        "helper-missing:",
        "jq -nc --arg",
    ] {
        assert!(cmd.contains(needle), "step-15 must contain `{needle}`");
    }
    let add_all = Regex::new(r"git\s+add\s+(-A|--all|\.)(\s|;|$)").unwrap();
    for line in code_lines(&cmd) {
        assert!(!add_all.is_match(line), "step-15 must not run `{line}`");
    }
    assert!(
        !cmd.contains("{{"),
        "no template expansion in step-15 shell lines"
    );
    assert!(!cmd.contains("Implements %s") && !cmd.contains("Implementation as per design"));
    assert!(
        !cmd.contains("refuses to rebase divergent history"),
        "#978 block moved to the helper"
    );
}

#[test]
fn step15_and_sync_step_use_the_five_tier_lookup_in_order() {
    let recipe = publish();
    let tiers = [
        "${AMPLIHACK_HOME:-}",
        "${REPO_PATH:-}",
        "$(pwd)",
        "${HOME:-/root}/.copilot",
        "${HOME:-/root}/.amplihack",
    ];
    let sourced = Regex::new(r#"(^|\s)(\.|source)\s+"?\$(STAGE|MSG|SYNC)"#).unwrap();
    for id in ["publish-sync-pr-head", "step-15-commit-push"] {
        let cmd = command(&recipe, id);
        let mut at = 0;
        for tier in tiers {
            let found = cmd[at..]
                .find(tier)
                .unwrap_or_else(|| panic!("{id}: tier `{tier}` out of order"));
            at += found + tier.len();
        }
        assert!(
            !sourced.is_match(&cmd),
            "{id}: helpers found through the ladder are run with bash, never sourced"
        );
    }
}

#[test]
fn allow_version_bump_defaults_to_false_in_both_recipes() {
    for name in ["workflow-publish", "default-workflow"] {
        let recipe: Value = serde_yaml::from_str(&read(&recipe_path(name))).unwrap();
        let value = &recipe["context"]["allow_version_bump"];
        assert!(
            value.as_str() == Some("false"),
            "{name}.yaml context must declare allow_version_bump: \"false\", got {value:?}"
        );
    }
}

#[test]
fn no_forced_or_broad_pushes_anywhere() {
    let forbidden = Regex::new(
        r"(--force\b|--force-with-lease|--force-if-includes|--mirror|--all\b|--tags|--delete|--no-verify|\s-f\b|\s\+[A-Za-z]|\s\+refs/)",
    )
    .unwrap();
    let rebase = Regex::new(r"git\s+rebase").unwrap();
    let recipe = publish();
    let mut sources: Vec<(String, String)> = NEW_HELPERS
        .iter()
        .map(|n| (n.to_string(), helper_text(n)))
        .collect();
    for id in ["publish-sync-pr-head", "step-15-commit-push"] {
        sources.push((id.to_string(), command(&recipe, id)));
    }
    for (name, src) in &sources {
        for line in code_lines(src).filter(|l| l.contains("git push")) {
            assert!(
                !forbidden.is_match(line),
                "{name}: forbidden push form `{line}`"
            );
        }
        for line in code_lines(src).filter(|l| l.contains("rebase")) {
            assert!(
                !rebase.is_match(line),
                "{name}: must never run a rebase `{line}`"
            );
        }
    }
    let sync = helper_text("workflow_sync_pr_head.sh");
    assert!(sync.contains("HEAD:refs/heads/"), "explicit refspec only");
    assert!(
        sync.contains("FETCH_HEAD"),
        "remote commit comes from FETCH_HEAD (SEC-3)"
    );
    for env in [
        "GIT_TERMINAL_PROMPT=0",
        "GCM_INTERACTIVE=never",
        "timeout 120",
    ] {
        assert!(sync.contains(env), "sync helper must use `{env}`");
    }
}

#[test]
fn helpers_never_eval_and_the_gate_is_read_only() {
    let eval = Regex::new(r"(^|[;&|\s])eval(\s|$)").unwrap();
    let mutation =
        Regex::new(r"gh\s+auth\s+(switch|login|refresh|setup-git|token)|--show-token").unwrap();
    for name in NEW_HELPERS.iter().chain(["workflow_pr_ready.sh"].iter()) {
        let src = helper_text(name);
        for line in code_lines(&src) {
            assert!(!eval.is_match(line), "{name}: no eval `{line}`");
        }
        assert!(
            !mutation.is_match(&src),
            "{name} must never change or reveal gh credentials"
        );
    }
    assert!(
        !helper_text("workflow_commit_message.sh").contains("TASK_DESC"),
        "the message helper must never read the task description"
    );
}

#[test]
fn pr_ready_sources_the_gate_and_checks_permission_after_identity() {
    let src = helper_text("workflow_pr_ready.sh");
    assert!(
        src.contains("WORKFLOW_GH_AUTH_GATE_HELPER"),
        "override variable"
    );
    assert!(src.contains("workflow_gh_auth_gate.sh"));
    assert!(src.contains("gh_auth_gate || exit 1"));
    assert!(
        !src.contains("timeout 30 gh auth status"),
        "the single-shot auth check must be replaced by the gate"
    );
    let identity = src
        .find("[ -n \"$repo_identity\" ] || { echo \"ERROR: unable to determine current GitHub repo identity")
        .expect("top-level identity check");
    let gate = src
        .find("gh_push_permission_gate \"$repo_identity\" || exit 1")
        .expect("permission gate call");
    assert!(
        gate > identity,
        "permission gate must run after the identity check"
    );
    let gate_lib = helper_text("workflow_gh_auth_gate.sh");
    assert!(
        gate_lib.contains("ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness"),
        "the existing error string is a contract, dash included"
    );
    assert!(
        gate_lib.contains("10#"),
        "validated numbers are used as base-10 (SEC-2)"
    );
}

#[test]
fn staging_helper_reads_paths_nul_delimited_and_writes_safely() {
    let src = helper_text("workflow_stage_task_changes.sh");
    for needle in [
        "-z",
        "write-tree",
        "read-tree",
        "umask 077",
        "mktemp",
        "--git-path info/exclude",
        "apply --check",
    ] {
        assert!(src.contains(needle), "staging helper must use `{needle}`");
    }
    assert!(
        src.contains("--no-ext-diff") && src.contains("--no-textconv"),
        "no external diff drivers"
    );
}
