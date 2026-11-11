//! Runner-side result authority is independent of child-authored summaries.
use super::{
    authorization::InvocationAuthorization, outcome::reduce_outcome, records::*,
    validation::validate_candidate,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

/// Re-validate the final structured observations with the captured invocation.
/// Child command/recipe success is not a readiness or merge claim.
pub(crate) fn report(
    auth: &InvocationAuthorization,
    context: &serde_json::Map<String, Value>,
    expected_pr: Option<&str>,
) -> Value {
    match already_merged(auth, context, expected_pr).and_then(|entry| match entry {
        Some(outcome) => Ok(outcome),
        None => accepted_outcome(auth, context, expected_pr),
    }) {
        Ok(outcome) => {
            serde_json::to_value(outcome).expect("serializable typed auto-drive outcome")
        }
        Err(error) => {
            let mut stop_reason = "dependency_failure";
            let mut blockers = vec![format!(
                "accepted finalization evidence unavailable: {error:#}"
            )];
            for key in ["crusty_loop_result", "merge_loop_result"] {
                if let Some(value) = context.get(key) {
                    let parsed = match value {
                        Value::String(text) if text.is_empty() => continue,
                        Value::String(text) => match serde_json::from_str::<Value>(text) {
                            Ok(value) => value,
                            Err(error) => {
                                blockers.push(format!("unreadable {key}: {error}"));
                                continue;
                            }
                        },
                        value => value.clone(),
                    };
                    if let Some(result) = parsed.get("loop_result").and_then(Value::as_str) {
                        stop_reason = match result {
                            "BUDGET_EXHAUSTED" => "budget_exhausted",
                            "USAGE_LIMIT" => "usage_limit",
                            "ORPHANED" => "orphaned",
                            "TERMINAL_POLICY_REFUSAL" => "terminal_refusal",
                            "STUCK" => "no_progress",
                            _ => stop_reason,
                        };
                    }
                }
            }
            json!({"schema_version":1,"run_id":auth.run_id(),"readiness":"incomplete","action":"not_requested","platform_state":"unknown","stop_reason":stop_reason,"blockers":blockers})
        }
    }
}
fn accepted_outcome(
    auth: &InvocationAuthorization,
    context: &serde_json::Map<String, Value>,
    expected_pr: Option<&str>,
) -> Result<Outcome> {
    let raw = context
        .get("finalization_observations")
        .context("no finalization observations")?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Envelope {
        candidate: CandidateRecord,
        finalization: FinalizationObservation,
        completion: LoopCompletion,
    }
    // Deserialize raw strings directly: duplicate fields must remain visible to serde.
    let envelope: Envelope = match raw {
        Value::String(text) => {
            ensure!(
                text.len() <= super::validation::MAX_RECORD_BYTES,
                "finalization exceeds size limit"
            );
            serde_json::from_str(text)?
        }
        value => serde_json::from_value(value.clone())?,
    };
    if let Some(pr) = expected_pr.filter(|p| !p.is_empty()) {
        ensure!(
            pr.parse::<u64>()? == envelope.candidate.binding.pr_number,
            "finalization changed the explicitly selected PR"
        );
    }
    if envelope.candidate.readiness == Readiness::MergeReady {
        let directory = context
            .get("autodrive_state_dir")
            .and_then(Value::as_str)
            .context("private state directory missing from finalization")?;
        super::completion::verify(auth, std::path::Path::new(directory), &envelope.candidate)?;
    }
    let accepted = validate_candidate(auth, &envelope.candidate.binding, &envelope.candidate)?;
    reduce_outcome(
        auth,
        &accepted,
        &envelope.finalization,
        &envelope.completion,
    )
}

fn already_merged(
    auth: &InvocationAuthorization,
    context: &serde_json::Map<String, Value>,
    expected_pr: Option<&str>,
) -> Result<Option<Outcome>> {
    let Some(raw) = context.get("build_result") else {
        return Ok(None);
    };
    let value: Value = match raw {
        Value::String(s) if s.is_empty() => return Ok(None),
        Value::String(s) => serde_json::from_str(s).context("unreadable run-entry preparation")?,
        value => value.clone(),
    };
    if value["merged_before_run"] != true {
        return Ok(None);
    }
    ensure!(
        value["pr_state"] == "MERGED"
            && value["entry_evidence"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty()),
        "already-merged preparation requires concrete platform confirmation"
    );
    let binding: Binding = serde_json::from_value(value["binding"].clone())
        .context("already-merged PR identity missing")?;
    super::validation::validate_binding(&binding)?;
    auth.validate_selection(&binding)?;
    ensure!(
        binding.run_id == auth.run_id()
            && auth
                .policy_provenance()
                .is_none_or(|p| p == binding.policy_provenance),
        "already-merged preparation belongs to another invocation or policy"
    );
    if let Some(pr) = expected_pr.filter(|p| !p.is_empty()) {
        ensure!(
            pr.parse::<u64>()? == binding.pr_number,
            "already-merged preparation changed the explicitly selected PR"
        );
    }
    Ok(Some(Outcome {
        schema_version: 1,
        binding,
        readiness: Readiness::Incomplete,
        action: Action::AlreadyMerged,
        platform_state: PlatformState::Merged,
        stop_reason: StopReason::Completed,
        blockers: Vec::new(),
        attempts: serde_json::from_value(
            value.get("attempts").cloned().unwrap_or_else(|| json!([])),
        )
        .context("invalid preparation attempts")?,
        failure_cause: None,
    }))
}
