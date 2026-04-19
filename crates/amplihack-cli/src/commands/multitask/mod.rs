//! Native parallel workstream orchestrator (port of `multitask/orchestrator.py`).
//!
//! Executes multiple independent development workstreams in parallel, each in
//! its own subprocess with isolated working directories. Supports recipe-based
//! and classic execution modes, per-workstream timeout budgets, state
//! persistence for resumption, and automatic cleanup of completed workstreams.

mod models;
mod orchestrator;

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

    let items: Vec<models::WorkstreamConfig> = serde_json::from_str(&config_text)
        .with_context(|| format!("Failed to parse config file: {config_path}"))?;

    if items.is_empty() {
        eprintln!("No workstreams defined in {config_path}");
        return Ok(());
    }

    let repo_url = detect_repo_url();

    let mut orch = orchestrator::ParallelOrchestrator::new(&repo_url, mode);

    if let Some(max_rt) = max_runtime {
        orch.set_default_max_runtime(max_rt);
    }
    if let Some(policy) = timeout_policy {
        orch.set_default_timeout_policy(policy);
    }

    orch.setup()?;

    for item in &items {
        orch.add(item, recipe)?;
    }

    if dry_run {
        println!("Dry-run: {} workstreams would be launched", items.len());
        for item in &items {
            println!(
                "  [{}] {} (recipe: {})",
                item.issue,
                item.description_or_default(),
                item.recipe.as_deref().unwrap_or(recipe)
            );
        }
        return Ok(());
    }

    // Install signal handler for graceful shutdown
    let running = orch.running_flag();
    let r = running.clone();
    ctrlc_flag(&r);

    orch.launch_all()?;
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
        .unwrap_or_else(orchestrator::default_base_dir);
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
        if path.extension().is_some_and(|e| e == "json")
            && !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".progress."))
        {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    if let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) {
                        let issue = state.get("issue").and_then(|v| v.as_i64()).unwrap_or(0);
                        let lifecycle = state
                            .get("lifecycle_state")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let step = state
                            .get("current_step")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let branch = state.get("branch").and_then(|v| v.as_str()).unwrap_or("");
                        println!("[{issue}] {lifecycle:20} step={step:30} branch={branch}");
                        found = true;
                    }
                }
                Err(_) => continue,
            }
        }
    }

    if !found {
        println!("No workstream state files found.");
    }

    Ok(())
}

fn detect_repo_url() -> String {
    std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::env::var("AMPLIHACK_REPO_PATH").unwrap_or_else(|_| {
                let cwd = std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| ".".to_string());
                eprintln!("WARNING: No git remote 'origin'; using local path: {cwd}");
                cwd
            })
        })
}

fn ctrlc_flag(flag: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    let f = flag.clone();
    // Best-effort: if signal registration fails, Ctrl-C will kill the process
    // immediately instead of triggering graceful shutdown.
    let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, f);
}
