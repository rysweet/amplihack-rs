//! Core orchestration logic for parallel workstream execution.

use anyhow::{Result, bail};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::models::*;

const DEFAULT_TIMEOUT_POLICY: &str = "interrupt-preserve";

/// Allowlist of valid delegate commands.
const VALID_DELEGATES: &[&str] = &[
    "amplihack claude",
    "amplihack copilot",
    "amplihack amplifier",
];

/// Run the multitask orchestrator.
pub fn run(
    config_path: &str,
    mode: &str,
    recipe: &str,
    max_runtime: u64,
    timeout_policy: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let config_content = fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("Cannot read config {config_path}: {e}"))?;

    let config: MultitaskConfig = serde_json::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Invalid config JSON: {e}"))?;

    let max_runtime = if max_runtime > 0 {
        max_runtime
    } else {
        config.max_runtime
    };
    let timeout_policy = timeout_policy.unwrap_or(DEFAULT_TIMEOUT_POLICY);
    let mode = if mode.is_empty() {
        config.mode.as_deref().unwrap_or("recipe")
    } else {
        mode
    };
    let recipe = if recipe.is_empty() {
        config.recipe.as_deref().unwrap_or("default-workflow")
    } else {
        recipe
    };

    println!("=== Multitask Orchestrator ===");
    println!(
        "Workstreams: {} | Mode: {mode} | Recipe: {recipe} | Max runtime: {max_runtime}s",
        config.workstreams.len()
    );
    println!("Timeout policy: {timeout_policy}");

    if dry_run {
        println!("\n[DRY RUN] Would execute:");
        for (i, ws) in config.workstreams.iter().enumerate() {
            println!(
                "  [{i}] branch={} issue={} task={}",
                ws.branch,
                ws.issue.unwrap_or(0),
                truncate(&ws.task, 80)
            );
        }
        return Ok(());
    }

    // Prepare base directory
    let base_dir = PathBuf::from("/tmp/amplihack-workstreams");
    fs::create_dir_all(&base_dir)?;

    // Build workstream states
    let states: Vec<WorkstreamState> = config
        .workstreams
        .iter()
        .enumerate()
        .map(|(i, ws)| WorkstreamState {
            id: format!("ws-{}", ws.issue.unwrap_or(i as u64)),
            config: ws.clone(),
            status: WorkstreamStatus::Pending,
            work_dir: None,
            pid: None,
            started_at: None,
            completed_at: None,
            exit_code: None,
        })
        .collect();

    let states = Arc::new(Mutex::new(states));
    let deadline = Instant::now() + Duration::from_secs(max_runtime);

    // Spawn workstreams
    let mut handles = Vec::new();
    let ws_count = states.lock().unwrap().len();

    for idx in 0..ws_count {
        let states = Arc::clone(&states);
        let base_dir = base_dir.clone();
        let recipe = recipe.to_string();
        let mode = mode.to_string();

        let handle = std::thread::spawn(move || {
            execute_workstream(idx, &states, &base_dir, &recipe, &mode, deadline);
        });
        handles.push(handle);
    }

    // Wait for all
    for handle in handles {
        let _ = handle.join();
    }

    // Report results
    let final_states = states.lock().unwrap();
    println!("\n=== Results ===");
    for ws in final_states.iter() {
        let status_icon = match ws.status {
            WorkstreamStatus::Completed => "\u{2713}",
            WorkstreamStatus::Running => "\u{23f3}",
            _ => "\u{2717}",
        };
        println!(
            "  {status_icon} {} [{}]: {}",
            ws.id, ws.status, ws.config.description
        );
    }

    let completed = final_states
        .iter()
        .filter(|ws| ws.status == WorkstreamStatus::Completed)
        .count();
    println!(
        "\nCompleted: {completed}/{} workstreams",
        final_states.len()
    );

    // Save state for resume
    let state_file = base_dir.join("state.json");
    let state_json = serde_json::to_string_pretty(&*final_states)?;
    fs::write(&state_file, state_json)?;
    println!("State saved to: {}", state_file.display());

    if completed < final_states.len() {
        bail!(
            "{} workstream(s) did not complete successfully",
            final_states.len() - completed
        );
    }

    Ok(())
}

fn execute_workstream(
    idx: usize,
    states: &Arc<Mutex<Vec<WorkstreamState>>>,
    base_dir: &Path,
    recipe: &str,
    mode: &str,
    deadline: Instant,
) {
    let (id, branch, task) = {
        let mut s = states.lock().unwrap();
        s[idx].status = WorkstreamStatus::Running;
        s[idx].started_at = Some(now_epoch());
        (
            s[idx].id.clone(),
            s[idx].config.branch.clone(),
            s[idx].config.task.clone(),
        )
    };

    let work_dir = base_dir.join(&id);

    // Clone repo for isolation
    let repo_path = std::env::var("AMPLIHACK_REPO_PATH")
        .or_else(|_| std::env::var("CLAUDE_PROJECT_DIR"))
        .unwrap_or_else(|_| ".".to_string());

    if let Err(e) = setup_workstream_dir(&work_dir, &repo_path, &branch) {
        eprintln!("[{id}] Setup failed: {e}");
        let mut s = states.lock().unwrap();
        s[idx].status = WorkstreamStatus::FailedTerminal;
        s[idx].completed_at = Some(now_epoch());
        return;
    }

    {
        let mut s = states.lock().unwrap();
        s[idx].work_dir = Some(work_dir.to_string_lossy().to_string());
    }

    // Build command
    let delegate =
        std::env::var("AMPLIHACK_DELEGATE").unwrap_or_else(|_| "amplihack claude".to_string());

    if !VALID_DELEGATES.contains(&delegate.as_str()) {
        eprintln!("[{id}] Invalid delegate: {delegate}");
        let mut s = states.lock().unwrap();
        s[idx].status = WorkstreamStatus::FailedTerminal;
        s[idx].completed_at = Some(now_epoch());
        return;
    }

    let parts: Vec<&str> = delegate.split_whitespace().collect();
    let (cmd, args) = (parts[0], &parts[1..]);

    let mut command = Command::new(cmd);
    command
        .args(args)
        .arg("--dangerously-skip-permissions")
        .arg("--print")
        .arg(&task)
        .current_dir(&work_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Set env vars for session tree
    command.env("AMPLIHACK_WORKSTREAM_ID", &id);
    if mode == "recipe" {
        command.env("AMPLIHACK_RECIPE", recipe);
    }

    let result = command.spawn();

    match result {
        Ok(mut child) => {
            if let Ok(pid) = child.id().try_into() {
                let mut s = states.lock().unwrap();
                s[idx].pid = Some(pid);
            }

            // Tail output
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    if Instant::now() >= deadline {
                        eprintln!("[{id}] Timeout reached, killing process");
                        let _ = child.kill();
                        let mut s = states.lock().unwrap();
                        s[idx].status = WorkstreamStatus::TimedOutResumable;
                        s[idx].completed_at = Some(now_epoch());
                        return;
                    }
                    if let Ok(line) = line {
                        println!("[{id}] {line}");
                    }
                }
            }

            match child.wait() {
                Ok(exit) => {
                    let mut s = states.lock().unwrap();
                    s[idx].exit_code = exit.code();
                    s[idx].completed_at = Some(now_epoch());
                    if exit.success() {
                        s[idx].status = WorkstreamStatus::Completed;
                    } else {
                        s[idx].status = WorkstreamStatus::FailedResumable;
                    }
                }
                Err(e) => {
                    eprintln!("[{id}] Wait failed: {e}");
                    let mut s = states.lock().unwrap();
                    s[idx].status = WorkstreamStatus::FailedTerminal;
                    s[idx].completed_at = Some(now_epoch());
                }
            }
        }
        Err(e) => {
            eprintln!("[{id}] Spawn failed: {e}");
            let mut s = states.lock().unwrap();
            s[idx].status = WorkstreamStatus::FailedTerminal;
            s[idx].completed_at = Some(now_epoch());
        }
    }
}

fn setup_workstream_dir(work_dir: &Path, repo_path: &str, branch: &str) -> Result<()> {
    if work_dir.exists() {
        // Resume existing workstream
        return Ok(());
    }

    // Clone via git worktree or cp
    let output = Command::new("git")
        .args(["worktree", "add", "--detach"])
        .arg(work_dir)
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        // Fall back to simple clone
        let output = Command::new("git")
            .args(["clone", "--depth=1", "--branch", branch, repo_path])
            .arg(work_dir)
            .output()?;

        if !output.status.success() {
            bail!(
                "Failed to clone repo: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    // Checkout branch
    let _ = Command::new("git")
        .args(["checkout", "-B", branch])
        .current_dir(work_dir)
        .output();

    Ok(())
}

/// Clean up completed workstream directories.
pub fn cleanup(config_path: &str, dry_run: bool) -> Result<()> {
    let base_dir = PathBuf::from("/tmp/amplihack-workstreams");
    let state_file = base_dir.join("state.json");

    let states: Vec<WorkstreamState> = if state_file.exists() {
        let content = fs::read_to_string(&state_file)?;
        serde_json::from_str(&content)?
    } else if Path::new(config_path).exists() {
        println!("No saved state found, nothing to clean up");
        return Ok(());
    } else {
        println!("No config or state found");
        return Ok(());
    };

    let eligible: Vec<&WorkstreamState> = states
        .iter()
        .filter(|ws| {
            matches!(
                ws.status,
                WorkstreamStatus::Completed
                    | WorkstreamStatus::FailedTerminal
                    | WorkstreamStatus::Abandoned
            )
        })
        .collect();

    if eligible.is_empty() {
        println!("No eligible workstreams to clean up");
        return Ok(());
    }

    for ws in &eligible {
        if let Some(ref dir) = ws.work_dir {
            let path = Path::new(dir);
            if dry_run {
                println!("[DRY RUN] Would remove: {dir}");
            } else if path.exists() {
                // Try git worktree remove first
                let _ = Command::new("git")
                    .args(["worktree", "remove", "--force", dir])
                    .output();
                if path.exists() {
                    fs::remove_dir_all(path)?;
                }
                println!("Removed: {dir}");
            }
        }
    }

    Ok(())
}

/// Show status of workstreams.
pub fn status(base_dir: Option<&str>) -> Result<()> {
    let dir = PathBuf::from(base_dir.unwrap_or("/tmp/amplihack-workstreams"));
    let state_file = dir.join("state.json");

    if !state_file.exists() {
        println!(
            "No active workstreams found (no state file at {})",
            state_file.display()
        );
        return Ok(());
    }

    let content = fs::read_to_string(&state_file)?;
    let states: Vec<WorkstreamState> = serde_json::from_str(&content)?;

    println!("=== Workstream Status ===");
    for ws in &states {
        let icon = match ws.status {
            WorkstreamStatus::Completed => "\u{2713}",
            WorkstreamStatus::Running => "\u{23f3}",
            WorkstreamStatus::Pending => "\u{2022}",
            _ => "\u{2717}",
        };
        let duration = match (ws.started_at, ws.completed_at) {
            (Some(start), Some(end)) => format!(" ({:.0}s)", end - start),
            (Some(start), None) => format!(" ({:.0}s elapsed)", now_epoch() - start),
            _ => String::new(),
        };
        println!(
            "  {icon} {} [{}]{duration}: {}",
            ws.id, ws.status, ws.config.description
        );
    }

    let total = states.len();
    let completed = states
        .iter()
        .filter(|w| w.status == WorkstreamStatus::Completed)
        .count();
    let running = states
        .iter()
        .filter(|w| w.status == WorkstreamStatus::Running)
        .count();
    let failed = total
        - completed
        - running
        - states
            .iter()
            .filter(|w| w.status == WorkstreamStatus::Pending)
            .count();

    println!("\nTotal: {total} | Completed: {completed} | Running: {running} | Failed: {failed}");

    Ok(())
}

fn now_epoch() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate("this is a very long string that needs truncation", 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn workstream_status_display() {
        assert_eq!(format!("{}", WorkstreamStatus::Completed), "completed");
        assert_eq!(format!("{}", WorkstreamStatus::Running), "running");
        assert_eq!(format!("{}", WorkstreamStatus::Pending), "pending");
        assert_eq!(
            format!("{}", WorkstreamStatus::FailedResumable),
            "failed_resumable"
        );
    }

    #[test]
    fn multitask_config_deserializes() {
        let json = r#"{
            "workstreams": [
                {
                    "branch": "feat/test",
                    "description": "Test workstream",
                    "task": "Do something",
                    "issue": 123
                }
            ]
        }"#;
        let config: MultitaskConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.workstreams.len(), 1);
        assert_eq!(config.workstreams[0].branch, "feat/test");
        assert_eq!(config.workstreams[0].issue, Some(123));
        assert_eq!(config.max_runtime, 7200);
    }

    #[test]
    fn workstream_state_round_trip() {
        let state = WorkstreamState {
            id: "ws-1".to_string(),
            config: WorkstreamConfig {
                issue: Some(1),
                branch: "feat/test".to_string(),
                description: "Test".to_string(),
                task: "Do it".to_string(),
                priority: None,
                depends_on: vec![],
            },
            status: WorkstreamStatus::Completed,
            work_dir: Some("/tmp/test".to_string()),
            pid: Some(12345),
            started_at: Some(1000.0),
            completed_at: Some(2000.0),
            exit_code: Some(0),
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: WorkstreamState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, "ws-1");
        assert_eq!(restored.status, WorkstreamStatus::Completed);
        assert_eq!(restored.exit_code, Some(0));
    }
}
