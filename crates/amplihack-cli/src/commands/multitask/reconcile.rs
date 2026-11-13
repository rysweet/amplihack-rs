//! Revision-bound, idempotent decisions for uncertain external outcomes.
use super::{completion, dispatch, identity::WorkstreamId, plan, runtime::Runtime};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

pub fn run(config: &Path, resolution: &Path) -> Result<()> {
    let _lock = dispatch::lock()?;
    let roots = plan::repositories()?;
    let entries = dispatch::configuration(config, "default-workflow", &roots)?;
    let decision: Value = serde_json::from_slice(&fs::read(resolution)?)?;
    ensure!(
        decision["schema_version"] == 1,
        "unsupported resolution schema"
    );
    let id: WorkstreamId = serde_json::from_value(decision["workstream_id"].clone())?;
    let _: WorkstreamId = serde_json::from_value(decision["resolution_id"].clone())?;
    let entry = entries
        .into_iter()
        .find(|e| e.workstream_id == Some(id))
        .context("resolution references an unknown leaf")?;
    ensure!(
        decision["repository"] == entry.repository,
        "resolution repository mismatch"
    );
    let mut state = Runtime::load(entry.clone(), &roots[&entry.repository])?;
    if let Some(previous) = state
        .resolutions
        .iter()
        .find(|r| r["decision"]["resolution_id"] == decision["resolution_id"])
    {
        ensure!(
            previous["decision"] == decision,
            "resolution_id reused with different contents"
        );
        println!("{}", previous["receipt"]);
        return Ok(());
    }
    ensure!(
        decision["expected_revision"].as_u64() == Some(state.revision),
        "stale resolution revision"
    );
    ensure!(
        ["reconciliation_required", "failed"].contains(&state.lifecycle.as_str()),
        "leaf is not eligible for reconciliation"
    );
    if let Some(pid) = state.process_scope.pid {
        let started = super::process_scope::process_start_metadata(pid);
        ensure!(
            started.is_none()
                || started.as_deref() != Some(&state.process_scope.process_started_at),
            "owned process is still active; stop it before reconciliation"
        );
    }
    completion::non_blank(&decision, "reason")?;
    completion::evidence(&decision)?;
    // File evidence is data, not an arbitrary path to execute. Verify repository and
    // attempt provenance before accepting a decision about an external mutation.
    for e in decision["evidence"].as_array().unwrap() {
        ensure!(
            e["source"] == "file",
            "resolution requires inspectable file evidence from the external service"
        );
        let evidence: Value = serde_json::from_slice(&fs::read(e["reference"].as_str().unwrap())?)?;
        ensure!(
            evidence["repository"] == state.config.repository,
            "resolution evidence repository mismatch"
        );
        if !decision["attempt_id"].is_null() {
            ensure!(
                evidence["attempt_id"] == decision["attempt_id"],
                "resolution evidence attempt mismatch"
            );
        }
        let expected = match decision["outcome"].as_str() {
            Some("unresolved") => "unknown",
            Some("succeeded") => "succeeded",
            Some("not_applied") => "not_applied",
            _ => bail!("unknown resolution outcome"),
        };
        ensure!(
            evidence["outcome"] == expected,
            "evidence does not establish the requested outcome"
        );
    }
    let outcome = decision["outcome"].as_str().context("resolution outcome")?;
    let attempt = if decision["attempt_id"].is_null() {
        None
    } else {
        let id: WorkstreamId = serde_json::from_value(decision["attempt_id"].clone())?;
        Some(id)
    };
    let mut tracking_index = None;
    let mut operation_index = None;
    if let Some(id) = attempt {
        tracking_index = state
            .tracking_attempts
            .iter()
            .position(|a| a["attempt_id"] == json!(id));
        operation_index = state
            .operation_attempts
            .iter()
            .position(|a| a["attempt_id"] == json!(id));
        ensure!(
            tracking_index.is_some() ^ operation_index.is_some(),
            "ambiguous or missing saved attempt"
        );
        let record = tracking_index
            .map(|i| &state.tracking_attempts[i])
            .or_else(|| operation_index.map(|i| &state.operation_attempts[i]))
            .unwrap();
        ensure!(
            record["workstream_id"] == json!(state.id())
                && record["repository"] == state.config.repository,
            "attempt ownership mismatch"
        );
        ensure!(
            ["prepared", "reconciliation_required", "failed"]
                .iter()
                .any(|s| record["status"] == *s),
            "attempt is already resolved"
        );
    }
    match outcome {
        "unresolved" => {
            ensure!(
                ["issue", "output", "result", "retry_authorized"]
                    .iter()
                    .all(|k| decision.get(k).is_none()),
                "unresolved decision must not carry a success or retry payload"
            );
            state.lifecycle = "reconciliation_required".into();
        }
        "succeeded" => {
            if let Some(i) = tracking_index {
                let issue = completion::issue_ref(&decision["issue"], &state.config.repository)?;
                state.config.issue = json!(issue.number);
                state.tracking_attempts[i]["issue"] = json!(issue);
                state.tracking_attempts[i]["status"] = json!("succeeded");
                state.lifecycle = "pending".into();
            } else if let Some(i) = operation_index {
                let name = state.operation_attempts[i]["output_name"]
                    .as_str()
                    .context("saved output name")?;
                ensure!(
                    decision["output_name"] == name,
                    "resolved output name mismatch"
                );
                let kind = state
                    .config
                    .required_outputs
                    .get(name)
                    .context("undeclared resolved output")?;
                completion::output(&decision["output"], kind, &state.config.repository)?;
                state.operation_attempts[i]["output"] = decision["output"].clone();
                state.operation_attempts[i]["status"] = json!("succeeded");
                state.lifecycle = "pending".into();
            } else {
                completion::validate(&decision["result"], &state.config)?;
                ensure!(
                    decision["result"]["outcome"] == "succeeded",
                    "resolution result must establish success"
                );
                state.result = Some(decision["result"].clone());
                state.lifecycle = "succeeded".into();
            }
        }
        "not_applied" => {
            let retry = decision["retry_authorized"]
                .as_bool()
                .context("not_applied requires Boolean retry_authorized")?;
            ensure!(
                ["issue", "output", "result"]
                    .iter()
                    .all(|k| decision.get(k).is_none()),
                "not_applied conflicts with a success payload"
            );
            if let Some(i) = tracking_index {
                state.tracking_attempts[i]["status"] =
                    json!(if retry { "not_applied" } else { "failed" });
            }
            if let Some(i) = operation_index {
                state.operation_attempts[i]["status"] =
                    json!(if retry { "not_applied" } else { "failed" });
            }
            state.lifecycle = if retry { "pending" } else { "failed" }.into();
        }
        _ => bail!("unknown resolution outcome"),
    }
    // Other unresolved attempts retain their barrier.
    if state
        .tracking_attempts
        .iter()
        .chain(&state.operation_attempts)
        .any(|a| a["status"] == "prepared" || a["status"] == "reconciliation_required")
    {
        state.lifecycle = "reconciliation_required".into();
    }
    let receipt = json!({"resolution_id": decision["resolution_id"], "workstream_id": state.id(),
        "repository": state.config.repository, "revision": state.revision + 1, "lifecycle": state.lifecycle});
    state
        .resolutions
        .push(json!({"decision": decision, "receipt": receipt}));
    state.save()?;
    println!("{receipt}");
    Ok(())
}
