//! Steps that look up the PR by branch use the remote branch name (#1762
//! R11, R12, D9, D10, AC11). Each test runs from an alias worktree made as
//! step 4's create pass makes it, with the fake gh answering only for
//! `--head <B>`:
//! - finalization's open-PR check (`workflow_agentic_finalization.sh
//!   validate`, which covers the open_pr, pr_signal and collect sites);
//! - workflow_final_status.sh's branch lookup and its scope check;
//! - the terminal-state probe's branch check, with a control that must still
//!   fail.

use serde_json::json;

use super::fixture::*;

const SCOPE_STUB: &str = r#"#!/usr/bin/env bash
# Stub PR scope helper: logs its arguments and accepts the PR it is given.
printf '%s\n' "$*" >> "$GH_FIXTURE_DIR/scope-calls.log"
url="" number=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --pr-url) url="${2:-}"; shift 2 ;;
    --pr-number) number="${2:-}"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"url":"%s","number":%s}\n' "${url:-https://github.com/acme/widget/pull/7}" "${number:-7}"
"#;

fn assert_looked_up_remote_branch(fx: &Fx, run: &Run, why: &str) {
    let calls = fx.gh_calls();
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("pr list ") && names_head(c, BRANCH)),
        "{why}: the PR must be looked up with --head {BRANCH}, got {calls:?}\n{}",
        run.describe()
    );
    fx.assert_gh_never_names_alias(run, why);
}

/// AC11 (i). With `branch_name` unset the run branch comes from the checkout
/// (open_pr.sh:48); with it set the checked-out branch is still compared with
/// it (pr_signal.sh:161). Both must see <B>.
#[test]
fn open_pr_check_looks_up_remote_branch() {
    for named in [false, true] {
        let why = if named {
            "branch_name=<B>"
        } else {
            "branch_name unset"
        };
        let fx = Fx::new();
        fx.on_pushed_branch();
        let wt = fx.alias_worktree(&alias(1), BRANCH);
        fx.answer_open_pr(&fx.head(&wt));
        let mut command = fx.bash(&wt);
        command
            .arg(tool_path("workflow_agentic_finalization.sh"))
            .arg("validate")
            .env("REPO_PATH", wt.as_os_str())
            .env("FINALIZER_STEP_STATUS", "ok")
            .env("TERMINAL_STATE_PUBLISH_STATUS", "existing-open-pr");
        if named {
            command.env("BRANCH_NAME", BRANCH);
        }

        let run = run(&mut command);

        assert_looked_up_remote_branch(&fx, &run, why);
        assert_eq!(
            run.json()["terminal_state"],
            "EXISTING_OPEN_PR",
            "{why}: the open PR on <B> is this run's PR\n{}",
            run.describe()
        );
    }
}

/// AC11 (ii): the #1596 branch lookup (final_status.sh:381) and the scope
/// check after it (:194) name <B>.
#[test]
fn final_status_branch_lookup_uses_remote_branch() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let wt = fx.alias_worktree(&alias(1), BRANCH);
    fx.answer_open_pr(&fx.head(&wt));
    let scope = fx.root.join("bin/scope-helper");
    write_exec(&scope, SCOPE_STUB);
    let wt_str = wt.to_str().unwrap();
    let setup = json!({"worktree_path": wt_str, "branch_name": BRANCH, "local_branch": alias(1)})
        .to_string();
    let mut command = fx.bash(&wt);
    command
        .arg(tool_path("workflow_final_status.sh"))
        .env("REMOTE_HOST_TYPE", "github")
        .env("RECIPE_VAR_worktree_setup", setup)
        .env("RECIPE_VAR_worktree_setup__worktree_path", wt_str)
        .env("RECIPE_VAR_worktree_setup__branch_name", BRANCH)
        .env("RECIPE_VAR_issue_number", "1762")
        .env("TASK_DESCRIPTION", "issue 1762 final status")
        .env("WORKFLOW_PR_SCOPE_HELPER", scope.as_os_str())
        .env("GH_RETRY_TMPDIR", fx.root.join("scratch"))
        .env("GH_RETRY_MAX_TRANSIENT", "1")
        .env("GH_RETRY_MAX_RL_WINDOWS", "0");

    let run = run(&mut command);

    assert_looked_up_remote_branch(&fx, &run, "final status");
    assert!(
        fx.gh_calls().iter().any(|c| c.starts_with("pr list ")
            && names_head(c, BRANCH)
            && c.contains("--state open")),
        "the #1596 lookup is `pr list --head <B> --state open`\n{}",
        run.describe()
    );
    assert!(
        run.stderr
            .contains(&format!("resolved open PR {PR_URL} for branch {BRANCH} (")),
        "the open PR on <B> is resolved\n{}",
        run.describe()
    );
    let scope_calls = fx.scope_calls();
    assert!(
        scope_calls.iter().any(|c| names_head(c, BRANCH)),
        "the scope check names <B> as --head, got {scope_calls:?}\n{}",
        run.describe()
    );
    assert!(
        scope_calls.iter().all(|c| !c.contains("--wt-")),
        "the scope check never names the alias, got {scope_calls:?}"
    );
}

fn probe(fx: &Fx, worktree: &std::path::Path) -> Run {
    let wt = worktree.to_str().unwrap();
    let mut command = fx.bash(worktree);
    command
        .arg("-c")
        .arg(step_text(
            "workflow-terminal-state",
            "probe-terminal-state",
            "command",
        ))
        .env("RECIPE_VAR_repo_path", &fx.caller)
        .env(
            "RECIPE_VAR_worktree_setup",
            json!({"worktree_path": wt, "branch_name": BRANCH}).to_string(),
        )
        .env("RECIPE_VAR_branch_name", BRANCH)
        .env("RECIPE_VAR_base_ref", "main")
        .env("RECIPE_VAR_pr_number", "")
        .env("RECIPE_VAR_pr_url", "")
        .env("RECIPE_VAR_goal_already_met", "false")
        .env("RECIPE_VAR_terminal_fail_mode", "report")
        .env("RECIPE_VAR_accept_pr_head_ahead", "false");
    run(&mut command)
}

/// AC11 (iii): an alias of <B> satisfies branch_name=<B>; an alias-shaped
/// branch whose upstream is not <B> still fails the check.
#[test]
fn terminal_state_probe_accepts_alias() {
    let fx = Fx::new();
    fx.on_pushed_branch();
    let wt = fx.alias_worktree(&alias(1), BRANCH);

    let run = probe(&fx, &wt);

    assert_ne!(
        run.json()["terminal_state"],
        "FAILED_WRONG_BRANCH",
        "an alias of <B> is on <B>\n{}",
        run.describe()
    );
    assert!(
        !run.stderr.contains("FAILED_WRONG_BRANCH"),
        "{}",
        run.describe()
    );
    fx.assert_gh_never_names_alias(&run, "terminal-state probe");

    // Control: same shape, but tracking origin/main.
    let other = fx.alias_worktree(&alias(2), "main");
    let run = probe(&fx, &other);
    assert_eq!(
        run.json()["terminal_state"],
        "FAILED_WRONG_BRANCH",
        "a branch that only looks like an alias is not <B>\n{}",
        run.describe()
    );
}
