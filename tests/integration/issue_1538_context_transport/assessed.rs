//! A finalization run as the transports carry it once `assess-finalization`
//! has decided its terminal state (#2343, #2809).
//!
//! The validator reports that decision. The typed completion booleans travel
//! beside it and are reported, but they decide nothing. The transport cases in
//! this directory therefore carry a decision, and the facts it was made on,
//! through the transport they exercise, and assert what arrives. A run that
//! carries any `RECIPE_VAR_*` is on the canonical tier, where the flat legacy
//! aliases are not read, so the decision and its evidence travel together.

use super::fixtures::Fixture;
use crate::issue_1641_finalization_open_pr::builders::assessment;
use serde_json::{Value, json};

pub const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

/// The facts the outcome read records. The run branch is pushed at `HEAD`; a
/// non-empty `pr` adds an open, green pull request of that number at that head.
pub fn recorded_outcome(pr: &str) -> Value {
    let mut outcome = json!({
        "schema_version": 2, "branch": "fixture/branch", "local_head": HEAD,
        "remote": "rysweet/fixture", "remote_head": HEAD, "unread": "",
        "pr_number": "", "pr_url": "", "pr_state": "", "pr_head": "", "checks": "none",
    });
    if !pr.is_empty() {
        outcome["pr_number"] = json!(pr);
        outcome["pr_url"] = json!(format!("https://github.com/rysweet/fixture/pull/{pr}"));
        outcome["pr_state"] = json!("OPEN");
        outcome["pr_head"] = json!(HEAD);
        outcome["checks"] = json!("green");
    }
    outcome
}

/// The facts as the validator quotes them in `evidence_used`, for a run with
/// no pull request.
pub fn facts_line() -> String {
    format!(
        "outcome.branch=fixture/branch,outcome.local_head={HEAD},outcome.remote_head={HEAD},\
         outcome.pr=none,outcome.pr_state=unread,outcome.pr_head=none,outcome.checks=none"
    )
}

/// `finalization_evidence` for a clean run, with its outcome recorded.
pub fn clean_evidence(pr: &str) -> Value {
    json!({"git": {"dirty_worktree": "false"}, "outcome": recorded_outcome(pr)})
}

/// The answer `assess-finalization` wrote: `state` for pull request `pr`
/// (empty for none), as the one object the transports store.
pub fn decided(state: &str, pr: &str) -> Value {
    serde_json::from_str(&assessment(state, "none", pr).1)
        .expect("the builder writes one JSON object")
}

/// `context` for a run whose outcome was recorded and then decided as `state`.
pub fn assessed(mut context: Value, state: &str, pr: &str) -> Value {
    context["finalization_evidence"] = clean_evidence(pr);
    context["finalization_assessment"] = decided(state, pr);
    context
}

/// The typed completion records of a run whose implementation and verification
/// both succeeded (`positive`) or both did not.
pub fn completion(positive: bool) -> Value {
    json!({
        "implementation_terminal_evidence": {
            "implementation_completed": positive, "terminal_no_op": false
        },
        "verification_terminal_evidence": {"verification_completed": positive},
        "allow_no_op": false
    })
}

/// `f` with `state` decided over the transport it already uses: inside its
/// context file when it has one, else at the canonical root.
pub fn carry(f: Fixture, state: &str) -> Fixture {
    let path = f.temp.path().join("context.json");
    if !path.exists() {
        return f.env(
            "RECIPE_VAR_finalization_assessment",
            decided(state, "").to_string(),
        );
    }
    let mut context: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    context["finalization_assessment"] = decided(state, "");
    f.file(context)
}
