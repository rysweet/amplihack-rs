//! #1714 case 2, crusty round 4: the terminal-state probe that
//! `workflow-finalize` runs first (`finalize-terminal-state`, report mode)
//! must agree with the open-PR check about "is this the same PR?".
//!
//! Before this, the probe accepted only a PR head that is an ancestor of the
//! local HEAD (#1467). On the mistt #138 history, with the merge of main
//! already on the PR before finalization starts (another checkout, or
//! GitHub's "Update branch" button), it reported `FAILED_INVALID_INPUT`
//! ("stale PR metadata"), and the validator failed closed on that prior state
//! before its open-PR check ran.
//!
//! These tests run the real probe step from `workflow-terminal-state.yaml`
//! with the inputs `workflow-finalize` gives it, then feed its JSON to the
//! real validator as `terminal_state`, as the recipe runner does. The PR head
//! M exists only on the fixture's private remote, so the probe has to fetch
//! it. They check that:
//! - green checks end `EXISTING_OPEN_PR`, named or discovered;
//! - pending checks end `FAILED_IMPLEMENTATION`, handed off only in auto-drive;
//! - a failing check is `BLOCKED_CI` from the probe, handed off;
//! - a diverged head, a rewound remote, and a probe run without
//!   `accept_pr_head_ahead` (as `workflow-publish` runs it) still end
//!   `FAILED_INVALID_INPUT`, which is never handed off (`probe_stale.rs`).

use std::process::Command;

use serde_json::{Value, json};

use super::ancestor::{
    FLAG, drop_pushed_branch, has_object, merge_main_on_pushed, origin_remote, pending, prune,
};
use super::fixture::*;
use crate::issue_1641_finalization_open_pr::fixture::assert_failed_implementation;

/// One `gh pr view`/`pr list` answer that satisfies both the probe's scope
/// helper and the open-PR check.
fn full_view(pr_head: &str, rollup: Value) -> Value {
    let mut view = open_pr(rollup);
    let extra = json!({
        "headRefOid": pr_head, "baseRefName": "main", "mergedAt": null,
        "headRepositoryOwner": {"login": "rysweet"}, "headRepository": {"name": "fixture"},
        "title": "run work", "body": "", "createdAt": "2026-10-01T00:00:00Z",
    });
    for (k, v) in extra.as_object().expect("object") {
        view[k] = v.clone();
    }
    view
}

pub(super) fn answer(fx: &Fixture, pr_head: &str, rollup: Value) {
    let view = full_view(pr_head, rollup);
    fx.gh_view(&view);
    fx.gh_list(&json!([view]));
}

/// The mistt #138 history with M only on the remote: the local HEAD is L,
/// origin/<branch> last held L (this checkout's own push), and M is not in
/// the local object store.
pub(super) fn merged_on_remote_before_finalize() -> (Fixture, String, String) {
    let fx = Fixture::new();
    let (local, pr_head) = merge_main_on_pushed(&fx);
    origin_remote(&fx);
    fx.git(&["push", "-q", "-u", "origin", BRANCH]);
    fx.git(&[
        "push",
        "-q",
        "origin",
        &format!("pushed:refs/heads/{BRANCH}"),
    ]);
    // That push moved origin/<branch> to M; this checkout never saw M.
    fx.git(&[
        "update-ref",
        &format!("refs/remotes/origin/{BRANCH}"),
        &local,
    ]);
    drop_pushed_branch(&fx);
    prune(&fx, &pr_head);
    (fx, local, pr_head)
}

/// Run the probe step as `workflow-finalize` runs it. `accept_ahead` is the
/// `accept_pr_head_ahead` input; `named` passes the PR URL.
pub(super) fn probe(fx: &Fixture, named: bool, accept_ahead: bool) -> Outcome {
    let command = step(
        &recipe_yaml("workflow-terminal-state"),
        "probe-terminal-state",
    )["command"]
        .as_str()
        .expect("probe is a bash step")
        .to_owned();
    let mut bash = Command::new("bash");
    fx.isolate(&mut bash);
    bash.arg("-c")
        .arg(command)
        .env("AMPLIHACK_HOME", workspace_root())
        .env("RECIPE_VAR_repo_path", fx.repo_path())
        .env("RECIPE_VAR_worktree_setup", "")
        .env("RECIPE_VAR_branch_name", BRANCH)
        .env("RECIPE_VAR_base_ref", "main")
        .env("RECIPE_VAR_pr_number", "")
        .env("RECIPE_VAR_pr_url", if named { PR_URL } else { "" })
        .env("RECIPE_VAR_goal_already_met", "false")
        .env("RECIPE_VAR_terminal_fail_mode", "report")
        .env(
            "RECIPE_VAR_accept_pr_head_ahead",
            if accept_ahead { "true" } else { "false" },
        );
    let output = bash.output().expect("run the probe");
    Outcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        output,
    }
}

/// The probe's JSON line, as the runner stores it in `terminal_state`.
pub(super) fn probe_json(outcome: &Outcome) -> Value {
    let line = outcome
        .stdout
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("the probe printed no JSON\n{}", outcome.describe()));
    serde_json::from_str(line).expect("probe JSON")
}

/// The validator after the probe, with the probe's result as `terminal_state`.
pub(super) fn validate_after(fx: &Fixture, probe: &Value, handoff: bool) -> Outcome {
    let repo = fx.repo_path();
    let probe = probe.to_string();
    let mut envs = vec![
        ("RECIPE_VAR_repo_path", repo.as_str()),
        ("RECIPE_VAR_branch_name", BRANCH),
        ("RECIPE_VAR_pr_url", PR_URL),
        ("RECIPE_VAR_terminal_state", probe.as_str()),
        ("RECIPE_VAR_finalizer_step_status", r#"{"status":"ok"}"#),
    ];
    if handoff {
        envs.push(FLAG);
    }
    validate(fx, &envs)
}

fn assert_probe_continues(outcome: &Outcome, why: &str) -> Value {
    let json = probe_json(outcome);
    assert_eq!(
        json["terminal_state"],
        "FOLLOWUP_CREATED",
        "{why}\n{}",
        outcome.describe()
    );
    assert!(
        outcome
            .stderr
            .contains("a fast-forward of it; same PR (#1714)"),
        "{why}: the probe must say why it accepted the head\n{}",
        outcome.describe()
    );
    json
}

pub(super) fn assert_stale(outcome: &Outcome, why: &str) -> Value {
    let json = probe_json(outcome);
    assert_eq!(
        json["terminal_state"],
        "FAILED_INVALID_INPUT",
        "{why}\n{}",
        outcome.describe()
    );
    json
}

// The case crusty named: green checks end EXISTING_OPEN_PR, whether the probe
// is given the PR URL (scope helper, named mode) or discovers the PR.
#[test]
fn a_merge_already_on_the_pr_before_finalize_ends_existing_open_pr() {
    for named in [true, false] {
        let (fx, local, pr_head) = merged_on_remote_before_finalize();
        answer(&fx, &pr_head, green_rollup());
        let p = probe(&fx, named, true);
        let json = assert_probe_continues(&p, &format!("named={named}"));
        assert!(has_object(&fx, &pr_head), "the probe fetched M");
        assert_eq!(fx.head(), local, "the probe never moves the local HEAD");
        assert_eq!(
            fx.git(&["rev-parse", &format!("refs/remotes/origin/{BRANCH}")])
                .trim(),
            local,
            "the probe's fetch writes no remote-tracking ref"
        );
        let outcome = validate_after(&fx, &json, false);
        assert!(outcome.success(), "named={named}\n{}", outcome.describe());
        assert_eq!(outcome.field("terminal_state"), "EXISTING_OPEN_PR");
        assert_eq!(outcome.field("pr_number"), "7");
        fx.assert_gh_reads_only();
    }
}

// CI not reported and no verification evidence of this run's own:
// FAILED_IMPLEMENTATION, handed to the loops only inside auto-drive.
#[test]
fn pending_checks_end_failed_implementation_handed_off_only_in_auto_drive() {
    let (fx, _, pr_head) = merged_on_remote_before_finalize();
    answer(&fx, &pr_head, pending());
    let json = assert_probe_continues(&probe(&fx, true, true), "pending");

    let outcome = validate_after(&fx, &json, true);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "FAILED_IMPLEMENTATION");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
    assert_eq!(outcome.field("pr_number"), "7");

    let outcome = validate_after(&fx, &json, false);
    assert_failed_implementation(&outcome, "standalone");
    assert_no_handoff_key(&outcome, "standalone");
}

// A red check on a head that holds the run's work is BLOCKED_CI from the
// probe itself, and handed off.
#[test]
fn a_failing_check_is_blocked_ci_from_the_probe_and_handed_off() {
    let (fx, _, pr_head) = merged_on_remote_before_finalize();
    answer(
        &fx,
        &pr_head,
        json!([check_run("COMPLETED", json!("FAILURE"))]),
    );
    let json = probe_json(&probe(&fx, true, true));
    assert_eq!(json["terminal_state"], "BLOCKED_CI");
    let outcome = validate_after(&fx, &json, true);
    assert!(outcome.success(), "{}", outcome.describe());
    assert_eq!(outcome.field("terminal_state"), "BLOCKED_CI");
    assert_eq!(outcome.field("handoff"), "merge_ready_loop");
}
