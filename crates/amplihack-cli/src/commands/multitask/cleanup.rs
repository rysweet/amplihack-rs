//! Cleanup by execution identity and verified ownership; Operations never loses its repository.
use super::{
    dispatch, plan,
    runtime::{self, Runtime},
};
use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::Path;
use std::process::Command;

pub(super) fn cleanup_merged(
    _base_dir: &Path,
    _state_dir: &Path,
    config_path: &str,
    dry_run: bool,
) -> Result<()> {
    let _lock = dispatch::lock()?;
    let roots = plan::repositories()?;
    let configs = dispatch::configuration(Path::new(config_path), "default-workflow", &roots)?;
    'leaf: for config in configs {
        let repo = roots
            .get(&config.repository)
            .context("unknown repository")?;
        let state = Runtime::load(config, repo)?;
        if state.lifecycle != "succeeded" {
            continue;
        }
        if let Some(pid) = state.process_scope.pid {
            ensure!(
                super::process_scope::process_start_metadata(pid)
                    .is_none_or(|start| start != state.process_scope.process_started_at),
                "owned process is still active"
            );
        }
        if state.config.classification != "Operations" {
            let result = state
                .result
                .as_ref()
                .context("missing durable completion")?;
            let deliveries: Vec<_> = result["outputs"]
                .as_object()
                .context("missing outputs")?
                .values()
                .filter(|o| o["kind"] == "delivery")
                .collect();
            // Investigations and answers have no merged delivery; retain their evidence.
            if deliveries.is_empty() {
                continue;
            }
            for output in deliveries {
                ensure!(
                    output["branch"] == state.config.branch,
                    "cleanup branch differs from the recorded plan"
                );
                let number = output["pull_request"]["number"]
                    .as_u64()
                    .context("cleanup requires a delivered PR")?;
                let out = Command::new("gh")
                    .args([
                        "pr",
                        "view",
                        &number.to_string(),
                        "--repo",
                        &state.config.repository,
                        "--json",
                        "state,headRefName,headRefOid",
                    ])
                    .current_dir(repo)
                    .output()?;
                ensure!(
                    out.status.success(),
                    "cannot verify merged PR before cleanup"
                );
                let pr: serde_json::Value = serde_json::from_slice(&out.stdout)?;
                if pr["state"] != "MERGED" {
                    continue 'leaf;
                }
                ensure!(
                    pr["headRefName"] == output["branch"] && pr["headRefOid"] == output["commit"],
                    "merged PR ownership mismatch"
                );
            }
        }
        runtime::owned(&state.artefact_dir, &runtime::base_dir())?;
        if state.config.classification == "Operations" {
            ensure!(
                !repo.starts_with(&state.artefact_dir),
                "cleanup would remove a repository"
            );
        }
        let targets = if state.config.classification == "Operations" {
            vec![
                state.artefact_dir.join("context.json"),
                state.artefact_dir.join("TASK.md"),
            ]
        } else {
            vec![state.work_dir.clone()]
        };
        for target in targets {
            runtime::owned(&target, &state.artefact_dir)?;
            if !target.exists() {
                continue;
            }
            if dry_run {
                println!("{} would remove {}", state.id(), target.display());
            } else if target.is_dir() {
                fs::remove_dir_all(&target)?;
            } else {
                fs::remove_file(&target)?;
            }
        }
    }
    Ok(())
}
