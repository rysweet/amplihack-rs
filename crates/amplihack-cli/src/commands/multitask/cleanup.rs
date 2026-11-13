use super::models::{Workstream, derive_key};
use super::utils::{dir_size_bytes, remove_dir_within};
use super::{ownership, persistence, plan};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;
use tracing::warn;

pub(super) fn cleanup_merged(
    base_dir: &Path,
    state_dir: &Path,
    config_path: &str,
    dry_run: bool,
) -> Result<()> {
    let config_text = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config: {config_path}"))?;
    let input = serde_json::from_str(&config_text)?;
    let items = plan::normalise(&input, "default-workflow")?;
    let _lock = ownership::lock(base_dir)?;
    let mut deleted_count = 0u32;
    let mut freed_bytes = 0u64;

    for (i, item) in items.iter().enumerate() {
        // The same derivation `Workstream::new` used, so the directory this
        // deletes is the one that workstream actually owns (issue #2553, SR2).
        let key = match derive_key(item, i + 1) {
            Ok(key) => key,
            Err(e) => {
                warn!(
                    branch = %item.branch,
                    error = %format!("{e:#}"),
                    "skipping a workstream whose identity could not be derived"
                );
                continue;
            }
        };
        let mut ws = Workstream::new(
            key.clone(),
            item.issue()?.map(|n| n.get()),
            item.branch.clone(),
            item.description_or_default(),
            item.task.clone(),
            item.recipe.clone().unwrap(),
            base_dir,
            state_dir,
        );
        if item.classification.as_deref() == Some("Operations") {
            ws.work_dir = std::env::current_dir()?.canonicalize()?;
        }
        ws.source_repository = ownership::RepositoryIdentity::current()?;
        ws.classification = item.classification.clone().unwrap();
        ws.dependencies = item.dependencies.clone();
        ws.create_issue = item.create_issue;
        let Some(saved) = persistence::read_state(&ws.state_file)? else {
            continue;
        };
        ownership::validate_saved(&ws, &saved)?;
        ownership::verify(&ws)?;
        anyhow::ensure!(
            Workstream::derive_cleanup_eligible(&saved.lifecycle_state) && saved.last_pid.is_none(),
            "[{key}] cleanup requires a terminal, reaped execution"
        );
        let work_dir = ws.artefact_dir.clone();

        // `gh` takes this as an argv element, so there is no shell to inject
        // into, but a branch beginning with `-` would parse as a flag. An
        // Operations workstream may carry an unusual or empty branch.
        if item.classification.as_deref() != Some("Operations")
            && (item.branch.is_empty() || item.branch.starts_with('-'))
        {
            warn!(
                branch = %item.branch,
                "skipping a workstream whose branch name is empty or option-like"
            );
            continue;
        }

        let mut gh_cmd = Command::new("gh");
        gh_cmd.args([
            "pr",
            "view",
            &item.branch,
            "--json",
            "state",
            "-q",
            ".state",
        ]);
        let is_merged = if item.classification.as_deref() == Some("Operations") {
            saved.lifecycle_state == "completed"
        } else {
            let output = gh_cmd
                .output()
                .context("inspect pull request before cleanup")?;
            anyhow::ensure!(
                output.status.success(),
                "[{key}] cannot establish merged-PR cleanup eligibility ({}); check GitHub authentication and service availability",
                output.status
            );
            String::from_utf8(output.stdout)?.trim() == "MERGED"
        };
        if !is_merged || !work_dir.exists() {
            continue;
        }

        let dir_size = dir_size_bytes(&work_dir);
        if dry_run {
            println!(
                "[{key}] Would delete work dir ({:.0}MB)",
                dir_size as f64 / (1024.0 * 1024.0)
            );
        } else {
            if !remove_dir_within(base_dir, &work_dir) {
                continue;
            }
            // Keep completion and results so a resumed plan cannot repeat external actions.
            println!(
                "[{key}] Deleted work dir ({:.0}MB freed)",
                dir_size as f64 / (1024.0 * 1024.0)
            );
        }
        freed_bytes += dir_size;
        deleted_count += 1;
    }

    let freed_gb = freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    let prefix = if dry_run { "DRY RUN " } else { "" };
    let modal = if dry_run { "would be " } else { "" };
    println!(
        "\n{prefix}Summary:\n  Workstreams {modal}deleted: {deleted_count}\n  Disk space {modal}freed: {freed_gb:.2}GB"
    );

    if dry_run && deleted_count > 0 {
        println!("\nRun without --dry-run to actually delete these workstreams.");
    }
    Ok(())
}
