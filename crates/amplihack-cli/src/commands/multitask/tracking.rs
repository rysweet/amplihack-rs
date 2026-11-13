//! Explicit tracking adapter. A prepared mutation is never automatically replayed.
use super::{completion, github, identity::WorkstreamId, issue::IssueRef, runtime::Runtime};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

pub fn resolve(state: &mut Runtime, prerequisites: &[Value]) -> Result<()> {
    if !state.config.issue.is_null() || !state.config.tracking.required {
        return Ok(());
    }
    if let Some(binding) = &state.config.tracking.from_result {
        let producer = prerequisites
            .iter()
            .find(|p| p["workstream_id"] == json!(binding.workstream_id))
            .context("tracking prerequisite is not ready")?;
        let issue = completion::issue_ref(
            &producer["outputs"][&binding.output]["issue"],
            &state.config.repository,
        )?;
        state.config.issue = json!(issue.number);
        return state.save();
    }
    ensure!(
        state.tracking_attempts.is_empty()
            || state
                .tracking_attempts
                .iter()
                .all(|a| a["status"] == "not_applied"),
        "tracking attempt already exists; reconciliation is required"
    );
    let title = state
        .config
        .tracking
        .title
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .context("required tracking title is missing")?
        .to_owned();
    let body = state
        .config
        .tracking
        .body
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .context("required tracking body is missing")?
        .to_owned();
    state.lifecycle = "tracking".into();
    state.tracking_attempts.push(json!({"schema_version": 1, "attempt_id": WorkstreamId::new(),
        "workstream_id": state.id(), "repository": state.config.repository, "revision": state.revision + 1,
        "purpose": "tracking", "request": {"title": title, "body": body}, "status": "prepared"}));
    state.save()?;
    let creation = (|| -> Result<IssueRef> {
        let mut command = github::command(&state.work_dir);
        command.args([
            "issue",
            "create",
            "--repo",
            &state.config.repository,
            "--title",
            &title,
            "--body-file",
            "-",
        ]);
        let output = github::execute(&mut command, Some(&body), &state.artefact_dir)?;
        github::created_issue(&output, &state.config.repository)
    })();
    match creation {
        Ok(issue) => {
            state.config.issue = json!(issue.number);
            let last = state.tracking_attempts.last_mut().unwrap();
            last["status"] = json!("succeeded");
            last["issue"] = json!(issue);
            state.lifecycle = "pending".into();
            state.save()
        }
        Err(error) => {
            // A CLI failure cannot prove the service did not accept the mutation.
            state.lifecycle = "reconciliation_required".into();
            state.diagnostic = format!(
                "tracking creation for {} requires reconciliation: {error:#}",
                state.config.repository
            );
            let last = state.tracking_attempts.last_mut().unwrap();
            last["status"] = json!("reconciliation_required");
            last["diagnostic"] = json!(state.diagnostic);
            state.save()?;
            Err(error).context("tracking creation failed; no child launched and no automatic retry")
        }
    }
}
