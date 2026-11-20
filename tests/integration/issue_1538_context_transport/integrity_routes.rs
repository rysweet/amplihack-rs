//! The five transports the integrity cases run their evidence over.
//!
//! Routes 0 and 1 put it in the context file, as an object and as an embedded
//! JSON string; routes 2 and 3 use the canonical `RECIPE_VAR_` root the same
//! two ways; route 4 uses the flat legacy aliases. Each builder also sets the
//! typed booleans a run would have to borrow a success from, so a case that
//! reaches a success has borrowed one.

use super::fixtures::*;
use serde_json::{Value, json};

pub(super) fn clean() -> Value {
    json!({"git":{"dirty_worktree":false},"tooling":{"missing":"","gh_required":false},
        "prior_terminal_state":{"terminal_state":""},
        "agent_outputs":{"hollow_success_signals":false}})
}

fn selected(evidence: Value, route: usize, noop: bool) -> Fixture {
    let implementation = json!({"implementation_completed":!noop,"terminal_no_op":noop});
    let verification = json!({"verification_completed":!noop});
    let f = Fixture::new()
        .env("FINALIZATION_EVIDENCE", clean().to_string())
        .env("FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE", "false")
        .env("FINALIZATION_EVIDENCE_TOOLING_MISSING", "")
        .env("FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED", "false")
        .env(
            "FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE",
            "",
        )
        .env(
            "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
            "false",
        )
        .env(
            "RECIPE_VAR_finalization_evidence__git__dirty_worktree",
            "false",
        )
        .env("IMPLEMENTATION_COMPLETED", "true")
        .env("VERIFICATION_COMPLETED", "true")
        .env("ALLOW_NO_OP", "true")
        .env("TERMINAL_NO_OP", "true");
    match route {
        0 | 1 => f
            .file(json!({"finalization_evidence":if route == 0 {evidence} else {json!(evidence.to_string())},
                "implementation_terminal_evidence":implementation,
                "verification_terminal_evidence":verification,"allow_no_op":noop}))
            .env("RECIPE_VAR_finalization_evidence", clean().to_string()),
        2 | 3 => f
            .env("RECIPE_VAR_finalization_evidence", if route == 2 {
                evidence.to_string()
            } else {
                json!(evidence.to_string()).to_string()
            })
            .env("RECIPE_VAR_implementation_terminal_evidence", implementation.to_string())
            .env("RECIPE_VAR_verification_terminal_evidence", verification.to_string())
            .env("RECIPE_VAR_allow_no_op", noop.to_string()),
        _ => unreachable!(),
    }
}

pub(super) fn fixture(evidence: Value, route: usize, noop: bool) -> Fixture {
    if route == 4 {
        return Fixture::new()
            .env("FINALIZATION_EVIDENCE", evidence.to_string())
            .env("FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE", "false")
            .env("FINALIZATION_EVIDENCE_TOOLING_MISSING", "")
            .env("FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED", "false")
            .env(
                "FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE",
                "",
            )
            .env(
                "FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS",
                "false",
            )
            .env(
                "IMPLEMENTATION_COMPLETED",
                if noop { "false" } else { "true" },
            )
            .env(
                "VERIFICATION_COMPLETED",
                if noop { "false" } else { "true" },
            )
            .env("TERMINAL_NO_OP", "true")
            .env("ALLOW_NO_OP", "true");
    }
    selected(evidence, route, noop)
}
