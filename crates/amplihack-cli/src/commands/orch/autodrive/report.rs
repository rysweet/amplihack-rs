//! Runner-side result authority is independent of child-authored summaries.
use super::{
    authorization::InvocationAuthorization,
    helper::{Envelope, validate_reduction},
    records::*,
    state::EvidenceStore,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

/// Re-validate the final structured observations with the captured invocation.
/// Child command/recipe success is not a readiness or merge claim.
pub(crate) fn report(
    auth: &InvocationAuthorization,
    context: &serde_json::Map<String, Value>,
    expected_pr: Option<&str>,
    working_dir: &std::path::Path,
) -> Value {
    match already_merged(auth, context, expected_pr).and_then(|entry| match entry {
        Some(outcome) => Ok(outcome),
        None => accepted_outcome(auth, context, expected_pr, working_dir),
    }) {
        Ok(outcome) => {
            serde_json::to_value(outcome).expect("serializable typed auto-drive outcome")
        }
        Err(error) => {
            let mut stop_reason = "dependency_failure";
            let failure_cause = format!("{error:#}");
            let mut blockers = vec![format!(
                "accepted finalization evidence unavailable: {failure_cause}"
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
                            "WAITING" => "waiting",
                            _ => stop_reason,
                        };
                        if result == "WAITING"
                            && let Some(blocker) = waiting_blocker(&parsed)
                        {
                            blockers.push(blocker);
                        }
                    }
                }
            }
            json!({"schema_version":1,"run_id":auth.run_id(),"readiness":"incomplete","action":"not_requested","platform_state":"unknown","stop_reason":stop_reason,"blockers":blockers,"failure_cause":failure_cause})
        }
    }
}
/// The closed `wait_target` forms, as `autodrive_round_evidence.sh` checks
/// them (#2472): a CI or mergeability wait at a full SHA, a rate-limit reset,
/// or a named read.
static WAIT_TARGET: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"^((ci|mergeability)@[0-9a-f]{40}|ratelimit-reset@([0-9]{1,12}|unknown)|read@(pr|pr-head|pr-number|checks|reviews|threads))$",
    )
    .expect("static pattern")
});

/// `waiting: <wait_target>` for a WAITING loop result. A present but
/// invalid target is named invalid and never repeated; an absent one adds
/// nothing.
fn waiting_blocker(result: &Value) -> Option<String> {
    let target = result.get("wait_target")?;
    Some(match target.as_str() {
        Some(t) if WAIT_TARGET.is_match(t) => format!("waiting: {t}"),
        _ => "waiting: <invalid target>".to_string(),
    })
}

fn accepted_outcome(
    auth: &InvocationAuthorization,
    context: &serde_json::Map<String, Value>,
    expected_pr: Option<&str>,
    working_dir: &std::path::Path,
) -> Result<Outcome> {
    let raw = context
        .get("finalization_observations")
        .context("no finalization observations")?;
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
    // The reducer's output carries pointers, never authority by itself. Replay
    // its complete checks and compare the private outcome from successful
    // reduction before reporting any readiness or action.
    // #2329: evidence is opened only from the directory the runner assigned
    // and bound. A directory named by the child is compared lexically and
    // never joined onto the working directory or resolved.
    let directory = auth
        .state_dir()
        .context("no runner-assigned state directory is bound to this invocation")?;
    let preparation = context.get("build_result").map(context_value).transpose()?;
    require_assigned_state_dir(
        "autodrive_state_dir",
        context.get("autodrive_state_dir"),
        directory,
    )?;
    require_assigned_state_dir(
        "build_result.state_dir",
        preparation.as_ref().and_then(|v| v.get("state_dir")),
        directory,
    )?;
    let store = EvidenceStore::create(directory)?;
    let receipt = context
        .get("autodrive_finalization")
        .filter(|v| !matches!(v, Value::String(s) if s.is_empty()))
        .map(context_value)
        .transpose()?;
    let assessment = context
        .get("accepted_readiness")
        .map(context_value)
        .transpose()?;
    let accepted_path = assessment
        .as_ref()
        .and_then(|v| v["accepted"].as_str())
        .or_else(|| {
            receipt
                .as_ref()
                .and_then(|v| v["accepted_snapshot"].as_str())
        })
        .context("deterministic reduction has no accepted snapshot")?;
    let repo = receipt
        .as_ref()
        .and_then(|v| v["repo"].as_str())
        .or_else(|| preparation.as_ref().and_then(|v| v["repo"].as_str()))
        .or_else(|| context.get("repo_path").and_then(Value::as_str))
        .context("deterministic reduction has no local worktree")?;
    let outcome = validate_reduction(
        auth,
        &store,
        directory,
        &working_dir.join(repo),
        std::path::Path::new(accepted_path),
        &envelope,
    )?;
    // Validate first even when the child reducer failed: retain its concrete
    // rejection (for example a missing QA artifact), not just absent output.
    let receipt = receipt.context("successful deterministic reduction missing")?;
    let path = receipt["outcome_snapshot"]
        .as_str()
        .context("successful deterministic reduction has no outcome snapshot")?;
    let path = std::path::Path::new(path);
    ensure!(
        path.parent()
            .context("outcome snapshot has no parent")?
            .canonicalize()?
            == directory.canonicalize()?,
        "outcome snapshot lies outside state"
    );
    let stored: Outcome = serde_json::from_slice(
        &store.read_private(
            path.file_name()
                .and_then(|s| s.to_str())
                .context("invalid outcome snapshot name")?,
        )?,
    )?;
    ensure!(
        stored == outcome,
        "reported outcome differs from deterministic reduction"
    );
    Ok(outcome)
}

fn context_value(raw: &Value) -> Result<Value> {
    match raw {
        Value::String(text) => {
            ensure!(
                text.len() <= super::validation::MAX_RECORD_BYTES,
                "reduction receipt exceeds size limit"
            );
            Ok(serde_json::from_str(text)?)
        }
        value => Ok(value.clone()),
    }
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

/// #2329 (V2): a state directory reported by the child must equal the
/// runner-assigned one by `Path` equality: no join, canonicalisation or
/// symlink resolution. An absent value or an empty string counts as absent;
/// any other JSON type is a mismatch. The message names only the runner's own
/// path, escaped, never the child's value.
fn require_assigned_state_dir(
    key: &str,
    reported: Option<&Value>,
    assigned: &std::path::Path,
) -> Result<()> {
    match reported {
        None => Ok(()),
        Some(Value::String(text)) if text.is_empty() => Ok(()),
        Some(Value::String(text)) if std::path::Path::new(text) == assigned => Ok(()),
        Some(_) => {
            anyhow::bail!("{key} differs from the runner-assigned state directory {assigned:?}")
        }
    }
}

#[cfg(test)]
#[path = "report_waiting_tests.rs"]
mod waiting_tests;
