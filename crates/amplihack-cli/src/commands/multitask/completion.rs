//! Completion contracts shared by dispatch and guarded reconciliation.
use super::{issue::IssueRef, models::WorkstreamConfig};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::path::Path;

pub fn non_blank(value: &Value, field: &str) -> Result<()> {
    ensure!(
        value
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty()),
        "missing or blank {field}"
    );
    Ok(())
}

pub fn evidence(value: &Value) -> Result<()> {
    let items = value
        .get("evidence")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .context("non-empty evidence is required")?;
    for item in items {
        for field in ["source", "reference", "observation"] {
            non_blank(item, field)?;
        }
        if item["source"] == "file" {
            let path = Path::new(item["reference"].as_str().unwrap());
            ensure!(
                path.is_absolute() && path.is_file() && !path.is_symlink(),
                "evidence file must be an inspectable regular file"
            );
        }
    }
    Ok(())
}

pub fn issue_ref(value: &Value, repository: &str) -> Result<IssueRef> {
    let issue: IssueRef =
        serde_json::from_value(value.clone()).context("repository-scoped issue reference")?;
    ensure!(issue.repository == repository, "issue repository mismatch");
    Ok(issue)
}

pub fn output(value: &Value, kind: &str, repository: &str) -> Result<()> {
    ensure!(
        value["kind"] == kind,
        "output kind mismatch: expected {kind}"
    );
    evidence(value)?;
    match kind {
        "answer" => {
            non_blank(value, "text")?;
            ensure!(
                value["destination"] == "chat" || value["destination"] == "issue",
                "invalid answer destination"
            );
            if value["destination"] == "issue" {
                issue_ref(&value["issue"], repository)?;
            }
        }
        "issue" => {
            issue_ref(&value["issue"], repository)?;
        }
        "operation" => {
            non_blank(value, "action")?;
            issue_ref(&value["issue"], repository)?;
        }
        "delivery" => {
            if value["terminal_outcome"] != "no_change" {
                for field in ["branch", "commit"] {
                    non_blank(value, field)?;
                }
                issue_ref(&value["pull_request"], repository)?;
            }
        }
        _ => anyhow::bail!("unrecognised output kind {kind:?}"),
    }
    Ok(())
}

pub fn validate(result: &Value, config: &WorkstreamConfig) -> Result<()> {
    ensure!(
        result["schema_version"] == 1,
        "unsupported completion schema_version"
    );
    ensure!(
        result["workstream_id"] == serde_json::to_value(config.workstream_id)?,
        "completion producer mismatch"
    );
    ensure!(
        result["repository"] == config.repository,
        "completion repository mismatch"
    );
    ensure!(
        result["classification"] == config.classification,
        "completion classification mismatch"
    );
    ensure!(
        (result["effective_recipe"] == config.recipe.as_deref().unwrap_or_default()
            || (config.recipe.as_deref() == Some("quick-change")
                && result["effective_recipe"] == "default-workflow"
                && result["handover_from"] == "quick-change")),
        "completion route mismatch"
    );
    let outcome = result["outcome"]
        .as_str()
        .context("completion outcome is required")?;
    ensure!(
        ["succeeded", "failed", "reconciliation_required"].contains(&outcome),
        "unknown completion outcome"
    );
    if outcome != "succeeded" {
        non_blank(result, "diagnostic")?;
    }
    let outputs = result["outputs"]
        .as_object()
        .context("completion outputs must be an object")?;
    for (name, value) in outputs {
        let kind = config
            .required_outputs
            .get(name)
            .with_context(|| format!("undeclared output {name}"))?;
        output(value, kind, &config.repository).with_context(|| format!("output {name}"))?;
    }
    if outcome == "succeeded" {
        for name in config.required_outputs.keys() {
            ensure!(outputs.contains_key(name), "missing required output {name}");
        }
    }
    Ok(())
}
