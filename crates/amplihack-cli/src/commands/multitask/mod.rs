//! Native parallel workstream orchestrator.
//!
//! Executes multiple independent development workstreams in parallel, each in
//! its own subprocess with isolated working directories. Supports recipe-based
//! and classic execution modes, per-workstream timeout budgets, state
//! persistence for resumption, and automatic cleanup of completed workstreams.

mod cleanup;
mod command_builder;
#[cfg(test)]
mod command_builder_tests;
mod default_branch;
mod dispatch;
mod execution;
mod identity;
pub(crate) mod issue;
#[cfg(test)]
mod issue_1570_launcher;
#[cfg(test)]
mod issue_1642_repo_identity;
#[cfg(test)]
mod issue_1966_criticality;
#[cfg(test)]
mod issue_2014_launcher_env;
#[cfg(test)]
mod issue_2553_delivery;
#[cfg(test)]
mod issue_2553_identity;
#[cfg(test)]
mod issue_2553_recovery;
mod launcher;
#[cfg(test)]
mod launcher_tests;
#[cfg(test)]
mod log_output;
#[cfg(test)]
mod model_tests;
mod models;
mod orchestrator;
mod ownership;
mod persistence;
pub(crate) mod plan;
mod process_scope;
#[cfg(test)]
mod process_scope_tests;
mod report;
mod state;
#[cfg(test)]
mod state_tests;
mod tracking;
mod utils;
pub(crate) mod waves;

use anyhow::{Context, Result};
use std::path::Path;

/// Run parallel workstreams from a JSON config file.
pub fn run_multitask(
    config_path: &str,
    mode: &str,
    recipe: &str,
    max_runtime: Option<u64>,
    timeout_policy: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let config_text = std::fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read config file: {config_path}"))?;

    let input: serde_json::Value = serde_json::from_str(&config_text)
        .with_context(|| format!("Failed to parse config file: {config_path}"))?;
    let items = plan::normalise(&input, recipe)?;

    let repo_url = detect_repo_url();

    let mut orch = orchestrator::ParallelOrchestrator::new(&repo_url, mode);

    if let Some(max_rt) = max_runtime {
        orch.set_default_max_runtime(max_rt);
    }
    if let Some(policy) = timeout_policy {
        orch.set_default_timeout_policy(policy);
    }

    if dry_run {
        println!("Dry-run: {} workstreams would be launched", items.len());
        for (i, item) in items.iter().enumerate() {
            let key = models::derive_key(item, i + 1)
                .with_context(|| format!("workstream {} on branch {:?}", i + 1, item.branch))?;
            println!(
                "  [{key}] {} (recipe: {})",
                item.description_or_default(),
                item.recipe.as_deref().unwrap_or(recipe)
            );
        }
        return Ok(());
    }

    let _lock = ownership::lock(Path::new(&utils::default_base_dir()))?;
    orch.setup()?;
    ownership::persist_plan(&items, &Path::new(&utils::default_base_dir()).join("state"))?;
    // Validate and claim every identity before any external action.
    for (i, item) in items.iter().enumerate() {
        orch.register(i + 1, item, recipe)?;
    }
    orch.prepare_all()?;
    let running = orch.running_flag();
    orch.monitor(running)?;
    let report = orch.report();
    println!("{report}");

    Ok(())
}

/// Clean up workstreams with merged PRs.
pub fn run_cleanup(config_path: &str, dry_run: bool) -> Result<()> {
    let repo_url = detect_repo_url();
    let orch = orchestrator::ParallelOrchestrator::new(&repo_url, "recipe");
    orch.cleanup_merged(config_path, dry_run)
}

/// Show status of existing workstreams.
pub fn run_status(base_dir: Option<&str>) -> Result<()> {
    let base = base_dir
        .map(|s| s.to_string())
        .unwrap_or_else(utils::default_base_dir);
    let state_dir = Path::new(&base).join("state");

    if !state_dir.exists() {
        println!(
            "No workstream state directory found at {}",
            state_dir.display()
        );
        return Ok(());
    }

    let mut found = false;
    for entry in std::fs::read_dir(&state_dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !name.starts_with("ws-")
            || !name.ends_with(".json")
            || name.contains(".progress.")
            || name.contains(".owner.")
        {
            continue;
        }
        if let Some(state) = persistence::read_state(&path)? {
            let key = if state.key.is_empty() {
                name.trim_end_matches(".json").to_owned()
            } else {
                state.key
            };
            let issue = state
                .issue
                .map(|number| format!(" issue=#{number}"))
                .unwrap_or_default();
            println!(
                "[{key}]{issue} {:20} step={:30} branch={}",
                state.lifecycle_state, state.current_step, state.branch
            );
            found = true;
        }
    }
    if !found {
        println!("No workstream state files found.");
    }
    Ok(())
}

fn detect_repo_url() -> String {
    let mut cmd = amplihack_git::command();
    cmd.args(["remote", "get-url", "origin"]);
    cmd.output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            std::env::var("AMPLIHACK_REPO_PATH").unwrap_or_else(|_| {
                let cwd = std::env::current_dir()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|error| {
                        eprintln!("WARNING: cannot resolve execution directory: {error}");
                        ".".into()
                    });
                eprintln!("WARNING: no git remote 'origin'; using local path: {cwd}");
                cwd
            })
        })
}
