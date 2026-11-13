//! Durable intent before the authenticated GitHub CLI boundary.
use super::{issue, models::Workstream, persistence::persist_state};
use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use std::process::Command;

pub(super) fn acquire(ws: &mut Workstream) -> Result<()> {
    if !ws.create_issue || ws.issue.is_some() {
        return Ok(());
    }
    let uncertain = !ws.tracking_reference.is_empty();
    if !uncertain {
        ws.tracking_reference = format!("amplihack-tracking:{}", uuid::Uuid::new_v4());
        ws.lifecycle_state = "tracking".into();
        persist_state(ws)?;
    }
    let number = if uncertain {
        let output = Command::new("gh")
            .args([
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
            ])
            .current_dir(&ws.work_dir)
            .output()
            .context("reconcile uncertain issue acquisition")?;
        ensure!(
            output.status.success(),
            "[{}] tracking reconciliation failed: {}",
            ws.key,
            String::from_utf8_lossy(&output.stderr)
        );
        let matches: Vec<Value> = serde_json::from_slice(&output.stdout)?;
        ensure!(
            matches.len() == 1,
            "[{}] unresolved tracking acquisition {}; expected exactly one matching issue, found {}. Reconcile on GitHub before resuming",
            ws.key,
            ws.tracking_reference,
            matches.len()
        );
        ensure!(
            matches[0]
                .get("body")
                .and_then(Value::as_str)
                .is_some_and(|body| body.contains(&format!("<!-- {} -->", ws.tracking_reference))),
            "[{}] reconciled issue does not carry the exact acquisition reference",
            ws.key
        );
        matches[0]
            .get("number")
            .cloned()
            .context("reconciled issue has no number")?
    } else {
        let body = format!("{}\n\n<!-- {} -->", ws.task, ws.tracking_reference);
        let output = Command::new("gh")
            .args([
                "issue",
                "create",
                "--title",
                &ws.description,
                "--body",
                &body,
            ])
            .current_dir(&ws.work_dir)
            .output()
            .context("acquire explicitly requested tracking")?;
        if !output.status.success() {
            bail!(
                "[{}] issue acquisition failed or acknowledgement was lost ({}); intent retained for reconciliation: {}",
                ws.key,
                ws.tracking_reference,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Value::String(String::from_utf8(output.stdout)?.trim().to_owned())
    };
    ws.issue = Some(
        issue::parse_optional(&number)?
            .context("tracking service returned no issue number")?
            .get(),
    );
    ws.lifecycle_state = "pending".into();
    persist_state(ws)
}
