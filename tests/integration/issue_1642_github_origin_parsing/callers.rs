//! How each of the seven bundle callers is driven from a fixture repo.
//!
//! Every runner leaves the WORKFLOW_GITHUB_REPO_HELPER override unset, so the
//! caller must find `workflow_github_repo.sh` next to itself (SCRIPT_DIR for
//! standalone scripts, BASH_SOURCE for the two sourced libraries).

use super::fixture::{BRANCH, Fixture, Outcome, tool_path};

#[derive(Clone, Copy, Debug)]
pub enum Caller {
    PrReady,
    FinalStatusBranchLookup,
    FinalStatusScopedPr,
    PrScope,
    PublishPr,
    TerminalEvidence,
    PrSignal,
    Collect,
}

pub const ALL: &[Caller] = &[
    Caller::PrReady,
    Caller::FinalStatusBranchLookup,
    Caller::FinalStatusScopedPr,
    Caller::PrScope,
    Caller::PublishPr,
    Caller::TerminalEvidence,
    Caller::PrSignal,
    Caller::Collect,
];

pub fn run(fx: &Fixture, caller: Caller) -> Outcome {
    match caller {
        Caller::PrReady => {
            let mut cmd = fx.command("bash");
            cmd.arg(tool_path("workflow_pr_ready.sh"));
            fx.run(cmd)
        }
        Caller::FinalStatusBranchLookup | Caller::FinalStatusScopedPr => {
            let mut cmd = fx.command("bash");
            cmd.arg(tool_path("workflow_final_status.sh"))
                .env("REMOTE_HOST_TYPE", "github")
                .env("TASK_DESCRIPTION", "issue 1642 regression")
                .env("ISSUE_NUMBER", "1642");
            if matches!(caller, Caller::FinalStatusScopedPr) {
                // A known PR skips the branch lookup and goes through
                // validate_final_pr_scope, the other parse site.
                cmd.env("PR_URL", "https://github.com/o/r/pull/5")
                    .env("PR_NUMBER", "5");
            }
            fx.run(cmd)
        }
        Caller::PrScope => {
            let mut cmd = fx.command("bash");
            cmd.arg(tool_path("workflow_pr_scope.sh"))
                .args(["--base", "main"]);
            fx.run(cmd)
        }
        Caller::PublishPr => {
            // gh must be on PATH: `command -v gh` runs before the identity
            // check, so a missing gh would pass this test for the wrong reason.
            let mut cmd = fx.command("bash");
            cmd.arg(tool_path("workflow_publish_pr.sh"))
                .env("REMOTE_HOST_TYPE", "github")
                .env("ISSUE_NUMBER", "1642");
            fx.run(cmd)
        }
        Caller::TerminalEvidence => {
            let mut cmd = fx.command("bash");
            cmd.arg(tool_path("workflow_terminal_evidence.sh"))
                .arg("collect");
            fx.run(cmd)
        }
        Caller::PrSignal => {
            let mut cmd = fx.command("bash");
            cmd.arg("-c")
                .arg(r#"set -u; . "$1"; . "$2"; shift 2; finalization_open_pr_signal "$@""#)
                .arg("signal")
                .arg(tool_path("workflow_gh_retry.sh"))
                .arg(tool_path("workflow_finalization_pr_signal.sh"))
                .arg(&fx.repo)
                .arg(BRANCH);
            fx.run(cmd)
        }
        Caller::Collect => run_collect(fx, None),
    }
}

/// `workflow_agentic_finalization.sh collect`, optionally with a PR URL.
pub fn run_collect(fx: &Fixture, pr_url: Option<&str>) -> Outcome {
    let mut cmd = fx.command("bash");
    cmd.arg(tool_path("workflow_agentic_finalization.sh"))
        .arg("collect");
    if let Some(pr_url) = pr_url {
        cmd.env("PR_URL", pr_url);
    }
    fx.run(cmd)
}

/// `remote.github_remote` from collect's (pretty-printed) evidence JSON.
pub fn github_remote(outcome: &Outcome) -> String {
    let json: serde_json::Value = serde_json::from_str(outcome.stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "collect must emit one evidence JSON document: {e}\n{}",
            outcome.describe()
        )
    });
    match &json["remote"]["github_remote"] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        other => panic!("github_remote must be a boolean string, got {other}"),
    }
}

/// `repo` from terminal_evidence's collect JSON.
pub fn evidence_repo(outcome: &Outcome) -> String {
    let json = outcome
        .json()
        .unwrap_or_else(|| panic!("terminal evidence must emit JSON\n{}", outcome.describe()));
    json["repo"].as_str().unwrap_or_default().to_owned()
}
