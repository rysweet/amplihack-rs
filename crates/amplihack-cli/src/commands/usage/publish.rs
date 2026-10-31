//! Explicit GitHub publication using the CLI's existing authenticated runner.
use super::{
    models::{Manifest, Report},
    report::summary,
};
use crate::commands::pr::GhRunner;
use anyhow::{Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::Write;

fn gh(runner: &impl GhRunner, args: &[&str]) -> Result<String> {
    let out = runner
        .run_gh(args)
        .map_err(|_| anyhow::anyhow!("usage publication: hosting command failed"))?;
    if !out.success {
        bail!("usage publication: hosting command rejected; check gh authentication and PR access");
    }
    Ok(out.stdout)
}
fn json(runner: &impl GhRunner, args: &[&str]) -> Result<Value> {
    serde_json::from_str(&gh(runner, args)?)
        .map_err(|_| anyhow::anyhow!("usage publication: invalid hosting response"))
}

/// One marker per workstream/PR, updated only on the authenticated user's comment.
/// Callers serialize publications for a workstream (the hosting API has no atomic upsert).
pub fn publish(m: &Manifest, r: &Report, pr: u64, runner: &impl GhRunner) -> Result<&'static str> {
    super::manifest::validate(m)?;
    if !m.completed
        || !m.prs.contains(&pr)
        || m.workstream.is_none()
        || m.repository.is_none()
        || m.branch.is_none()
    {
        bail!(
            "publication requires completed=true and explicit workstream/repository/branch/PR identity"
        );
    }
    let repo = m.repository.as_deref().unwrap();
    let pr_text = pr.to_string();
    let view = json(
        runner,
        &[
            "pr",
            "view",
            &pr_text,
            "--repo",
            repo,
            "--json",
            "headRefName,isDraft,state",
        ],
    )?;
    if view["headRefName"].as_str() != m.branch.as_deref()
        || view["isDraft"].as_bool() != Some(false)
        || !matches!(view["state"].as_str(), Some("OPEN" | "MERGED"))
    {
        bail!("publication requires a ready or merged PR with the manifest branch");
    }
    let user = json(runner, &["api", "user"])?;
    let Some(uid) = user["id"].as_u64() else {
        bail!("hosting user identity unavailable");
    };
    let hash = format!(
        "{:x}",
        Sha256::digest(m.workstream.as_deref().unwrap().as_bytes())
    );
    let marker = format!("<!-- amplihack-usage:v1:{hash}:pr-{pr} -->");
    let body = format!("{marker}\n{}", summary(r));
    // GitHub comments have a 65536-character limit. Refuse rather than silently truncate completeness.
    if body.len() > 60000 {
        bail!("usage summary too large for a PR comment; reduce manifest scope");
    }
    let endpoint = format!("repos/{repo}/issues/{pr}/comments");
    let pages = json(runner, &["api", &endpoint, "--paginate", "--slurp"])?;
    let Some(pages) = pages.as_array() else {
        bail!("invalid hosting comment pages");
    };
    let mut matches = Vec::new();
    for page in pages {
        let Some(comments) = page.as_array() else {
            bail!("invalid hosting comment page");
        };
        for c in comments {
            if c["body"]
                .as_str()
                .is_some_and(|b| b.starts_with(&format!("{marker}\n")))
            {
                if c["user"]["id"].as_u64() != Some(uid) {
                    bail!(
                        "usage marker belongs to another publisher; use the original hosting identity"
                    );
                }
                matches.push(c);
            }
        }
    }
    if matches.len() > 1 {
        bail!("duplicate usage markers require manual reconciliation");
    }
    if matches
        .first()
        .is_some_and(|c| c["body"].as_str() == Some(body.as_str()))
    {
        return Ok("unchanged");
    }
    let (method, target) = if let Some(c) = matches.first() {
        let Some(id) = c["id"].as_u64() else {
            bail!("invalid hosting comment identity");
        };
        ("PATCH", format!("repos/{repo}/issues/comments/{id}"))
    } else {
        ("POST", endpoint)
    };
    // Keep the body out of shell interpolation and command arguments.
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| anyhow::anyhow!("cannot create publication body"))?;
    file.write_all(body.as_bytes())
        .map_err(|_| anyhow::anyhow!("cannot write publication body"))?;
    let field = format!("body=@{}", file.path().display());
    gh(
        runner,
        &["api", &target, "--method", method, "--field", &field],
    )?;
    Ok(if method == "POST" {
        "created"
    } else {
        "updated"
    })
}
