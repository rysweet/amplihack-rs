//! Durable intent before the authenticated GitHub CLI boundary.
use super::{issue, models::Workstream, persistence::persist_state};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::{
    process::{Command, Output, Stdio},
    time::Duration,
};

fn github(ws: &Workstream, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("gh");
    command
        .args(args)
        .current_dir(&ws.work_dir)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null());
    // Never retry creation: even a timeout can follow a successful write.
    // Bound retained service output and omit raw stderr, which may contain secrets.
    crate::util::run_output_with_timeout_limited(command, Duration::from_secs(60), 1024 * 1024)
        .with_context(|| {
            format!(
                "[{}] tracking service call failed; intent {} retained for reconciliation before resuming",
                ws.key, ws.tracking_reference
            )
        })
}

pub(super) fn acquire(ws: &mut Workstream) -> Result<()> {
    if !ws.create_issue || ws.issue.is_some() {
        return Ok(());
    }
    let uncertain = !ws.tracking_reference.is_empty();
    if uncertain {
        ensure!(
            ws.tracking_reference
                .strip_prefix("amplihack-tracking:")
                .and_then(|reference| uuid::Uuid::parse_str(reference)
                    .ok()
                    .map(|id| id.to_string() == reference))
                == Some(true),
            "[{}] invalid saved tracking acquisition reference; reconcile saved state before resuming",
            ws.key
        );
    }
    if !uncertain {
        ws.tracking_reference = format!("amplihack-tracking:{}", uuid::Uuid::new_v4());
        ws.lifecycle_state = "tracking".into();
        persist_state(ws)?;
    }
    let number = if uncertain {
        let output = github(
            ws,
            &[
                "issue",
                "list",
                "--state",
                "all",
                "--search",
                &format!("{} in:body", ws.tracking_reference),
                "--json",
                "number,body",
                "--limit",
                "100",
            ],
        )?;
        ensure!(
            output.status.success(),
            "[{}] tracking reconciliation failed ({}); intent {} retained. Check GitHub authentication and service availability before resuming",
            ws.key,
            output.status,
            ws.tracking_reference
        );
        let candidates: Vec<Value> = serde_json::from_slice(&output.stdout).map_err(|_| {
            anyhow::anyhow!(
                "tracking reconciliation returned an invalid JSON issue list; intent retained"
            )
        })?;
        ensure!(
            candidates.len() < 100,
            "[{}] tracking reconciliation reached the search result limit; intent {} retained. Reconcile on GitHub before resuming",
            ws.key,
            ws.tracking_reference
        );
        // GitHub search can return approximate matches. Only the exact saved
        // marker is evidence that this acquisition created an issue.
        let marker = format!("<!-- {} -->", ws.tracking_reference);
        let matches: Vec<Value> = candidates
            .into_iter()
            .filter(|candidate| {
                candidate
                    .get("body")
                    .and_then(Value::as_str)
                    .is_some_and(|body| body.contains(&marker))
            })
            .collect();
        ensure!(
            matches.len() == 1,
            "[{}] unresolved tracking acquisition {}; expected exactly one matching issue, found {}. Reconcile on GitHub before resuming",
            ws.key,
            ws.tracking_reference,
            matches.len()
        );
        matches[0]
            .get("number")
            .cloned()
            .context("reconciled issue has no number")?
    } else {
        let body = format!("{}\n\n<!-- {} -->", ws.task, ws.tracking_reference);
        let output = github(
            ws,
            &[
                "issue",
                "create",
                "--title",
                &ws.description,
                "--body",
                &body,
            ],
        )?;
        if !output.status.success() {
            bail!(
                "[{}] issue acquisition failed or acknowledgement was lost ({}); intent retained for reconciliation: {}. Check GitHub authentication and service availability before resuming",
                ws.key,
                output.status,
                ws.tracking_reference
            );
        }
        Value::String(
            String::from_utf8(output.stdout)
                .context("tracking service returned invalid UTF-8; intent retained")?
                .trim()
                .to_owned(),
        )
    };
    ws.issue = Some(
        issue::parse_optional(&number)
            .map_err(|_| anyhow::anyhow!("tracking service returned an invalid issue number; intent retained for reconciliation"))?
            .context("tracking service returned no issue number")?
            .get(),
    );
    ws.lifecycle_state = "pending".into();
    persist_state(ws)
}
