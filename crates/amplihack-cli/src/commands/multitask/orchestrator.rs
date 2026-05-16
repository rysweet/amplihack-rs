//! Core orchestrator logic for parallel workstream execution.
//!
//! Launcher generation, state persistence helpers, and utility functions
//! live in sibling modules (`launcher`, `state`, `utils`).

use super::models::*;
use super::{launcher, state, utils};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::warn;

pub struct ParallelOrchestrator {
    repo_url: String,
    base_dir: PathBuf,
    state_dir: PathBuf,
    mode: String,
    workstreams: Vec<Workstream>,
    processes: HashMap<i64, Arc<Mutex<Option<Child>>>>,
    cleaned_up: std::collections::HashSet<i64>,
    freed_bytes: u64,
    default_max_runtime: u64,
    default_timeout_policy: String,
    default_branch: Option<String>,
}

impl ParallelOrchestrator {
    pub fn new(repo_url: &str, mode: &str) -> Self {
        Self {
            repo_url: repo_url.to_string(),
            base_dir: PathBuf::from(utils::default_base_dir()),
            state_dir: PathBuf::from(utils::default_base_dir()).join("state"),
            mode: mode.to_string(),
            workstreams: Vec::new(),
            processes: HashMap::new(),
            cleaned_up: std::collections::HashSet::new(),
            freed_bytes: 0,
            default_max_runtime: DEFAULT_MAX_RUNTIME,
            default_timeout_policy: DEFAULT_TIMEOUT_POLICY.to_string(),
            default_branch: None,
        }
    }

    pub fn set_default_max_runtime(&mut self, max_runtime: u64) {
        self.default_max_runtime = max_runtime;
    }

    pub fn set_default_timeout_policy(&mut self, policy: &str) {
        if policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY || policy == CONTINUE_PRESERVE_TIMEOUT_POLICY
        {
            self.default_timeout_policy = policy.to_string();
        } else {
            warn!("Invalid timeout policy {policy:?}; using default {DEFAULT_TIMEOUT_POLICY:?}");
        }
    }

    pub fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    pub fn setup(&self) -> Result<()> {
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("Failed to create base dir: {}", self.base_dir.display()))?;
        fs::create_dir_all(&self.state_dir)
            .with_context(|| format!("Failed to create state dir: {}", self.state_dir.display()))?;
        utils::check_disk_space(&self.base_dir, 5.0)?;
        Ok(())
    }

    pub fn add(&mut self, config: &WorkstreamConfig, default_recipe: &str) -> Result<()> {
        let issue = config.issue_id();
        if issue <= 0 {
            bail!("Invalid issue number: {}", config.issue);
        }

        let recipe = config
            .recipe
            .as_deref()
            .unwrap_or(default_recipe)
            .to_string();

        let mut ws = Workstream::new(
            issue,
            config.branch.clone(),
            config.description_or_default(),
            config.task.clone(),
            recipe,
            &self.base_dir,
            &self.state_dir,
        );

        ws.max_runtime = config.max_runtime.unwrap_or(self.default_max_runtime);
        ws.timeout_policy = config
            .timeout_policy
            .clone()
            .unwrap_or_else(|| self.default_timeout_policy.clone());

        let saved = state::load_state(&ws.state_file);
        if let Some(ref s) = saved {
            state::apply_saved_state(&mut ws, s);
        }

        let reuse_existing = saved.is_some() && !ws.cleanup_eligible && ws.work_dir.exists();

        if !reuse_existing && ws.work_dir.exists() {
            let _ = fs::remove_dir_all(&ws.work_dir);
        }

        let default_branch = self.resolve_default_branch();

        if reuse_existing {
            println!(
                "[{}] Reusing preserved work dir {}",
                issue,
                ws.work_dir.display()
            );
        } else {
            println!("[{issue}] Cloning default branch '{default_branch}' from remote...");
            let status = Command::new("git")
                .args([
                    "clone",
                    "--depth=1",
                    &format!("--branch={default_branch}"),
                    &self.repo_url,
                    &ws.work_dir.to_string_lossy(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .with_context(|| format!("[{issue}] Failed to spawn git clone"))?;

            if !status.success() {
                bail!("[{issue}] git clone failed with exit code {status}");
            }
        }

        fs::create_dir_all(&ws.work_dir)?;

        let delegate = launcher::detect_delegate();
        if self.mode == "recipe" {
            launcher::write_recipe_launcher(&ws, &delegate)?;
        } else {
            launcher::write_classic_launcher(&ws, &delegate)?;
        }

        state::persist_state(&ws)?;
        self.workstreams.push(ws);
        Ok(())
    }

    pub fn launch_all(&mut self) -> Result<()> {
        let delegate = launcher::detect_delegate();
        let count = self.workstreams.len();

        for i in 0..count {
            let ws = &mut self.workstreams[i];
            launcher::launch_workstream(ws, &self.mode, &delegate, &mut self.processes)?;
        }

        println!(
            "\n{count} workstreams launched in parallel ({} mode)",
            self.mode
        );
        Ok(())
    }

    pub fn monitor(&mut self, running: Arc<AtomicBool>) -> Result<()> {
        let check_interval = Duration::from_secs(10);
        let start = Instant::now();

        while running.load(Ordering::Relaxed) {
            state::enforce_timeouts(&mut self.workstreams, &self.processes);

            let status = state::get_status(&mut self.workstreams, &self.processes);
            let elapsed = start.elapsed().as_secs();
            let now = Utc::now().format("%H:%M:%S");

            println!(
                "\n[{now}] Status (elapsed: {elapsed}s):\n  \
                 Running:   {} {:?}\n  \
                 Completed: {} {:?}\n  \
                 Failed:    {} {:?}",
                status.running.len(),
                status.running,
                status.completed.len(),
                status.completed,
                status.failed.len(),
                status.failed,
            );

            // Emit JSONL heartbeat to stderr
            let heartbeat = serde_json::json!({
                "type": "heartbeat",
                "ts": Utc::now().timestamp_millis() as f64 / 1000.0,
                "elapsed_s": elapsed,
                "summary": {
                    "running": status.running.len(),
                    "completed": status.completed.len(),
                    "failed": status.failed.len(),
                    "total": self.workstreams.len(),
                },
            });
            eprintln!("{}", serde_json::to_string(&heartbeat).unwrap_or_default());

            // Auto-cleanup completed workstream directories
            for ws in &self.workstreams {
                if !self.cleaned_up.contains(&ws.issue)
                    && ws.exit_code.is_some()
                    && Workstream::derive_cleanup_eligible(&ws.lifecycle_state)
                {
                    self.cleanup_workstream_dir(ws);
                }
            }

            if status.running.is_empty() {
                break;
            }

            thread::sleep(check_interval);
        }

        // If interrupted, terminate remaining
        if !running.load(Ordering::Relaxed) {
            println!("\nInterrupted! Cleaning up workstreams...");
            state::cleanup_running(&mut self.workstreams, &self.processes);
        }

        Ok(())
    }

    pub fn report(&self) -> String {
        let mut lines = vec![
            String::new(),
            "=".repeat(70),
            "PARALLEL WORKSTREAM REPORT".to_string(),
            format!("Mode: {}", self.mode),
            "=".repeat(70),
        ];

        let mut succeeded = 0u32;
        let mut failed = 0u32;

        for ws in &self.workstreams {
            let runtime = ws
                .runtime_seconds()
                .map(|s| format!("{s:.0}s"))
                .unwrap_or_else(|| "N/A".to_string());

            let status = if ws.exit_code == Some(0) {
                succeeded += 1;
                "OK".to_string()
            } else {
                failed += 1;
                format!(
                    "FAILED (exit {})",
                    ws.exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "?".to_string())
                )
            };

            lines.push(format!("\n[{}] {}", ws.issue, ws.description));
            lines.push(format!("  Branch:    {}", ws.branch));
            lines.push(format!("  Status:    {status}"));
            lines.push(format!("  Lifecycle: {}", ws.lifecycle_state));
            lines.push(format!("  Runtime:   {runtime}"));
            lines.push(format!(
                "  Checkpoint: {}",
                if ws.checkpoint_id.is_empty() {
                    "n/a"
                } else {
                    &ws.checkpoint_id
                }
            ));
            lines.push(format!(
                "  Worktree: {}",
                if ws.worktree_path.is_empty() {
                    "n/a"
                } else {
                    &ws.worktree_path
                }
            ));
            lines.push(format!("  Log:       {}", ws.log_file.display()));
            lines.push(format!("  Cleanup eligible: {}", ws.cleanup_eligible));
        }

        let freed_gb = self.freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        lines.push(String::new());
        lines.push("-".repeat(70));
        lines.push(format!(
            "Total: {} | Succeeded: {succeeded} | Failed: {failed}",
            self.workstreams.len()
        ));
        lines.push(String::new());
        lines.push("DISK MANAGEMENT:".to_string());
        lines.push(format!(
            "  Auto-cleaned: {} workstream dirs ({freed_gb:.2}GB freed)",
            self.cleaned_up.len()
        ));
        lines.push(format!(
            "  Log files preserved at: {}/log-*.txt",
            self.base_dir.display()
        ));
        lines.push("=".repeat(70));

        let report_text = lines.join("\n");

        // Write report to file
        let report_file = self.base_dir.join("REPORT.md");
        if let Err(e) = fs::write(&report_file, &report_text) {
            warn!("Failed to write report to {}: {e}", report_file.display());
        } else {
            println!("Report saved to: {}", report_file.display());
        }

        report_text
    }

    pub fn cleanup_merged(&self, config_path: &str, dry_run: bool) -> Result<()> {
        state::cleanup_merged(&self.base_dir, &self.state_dir, config_path, dry_run)
    }

    fn cleanup_workstream_dir(&self, ws: &Workstream) {
        if !ws.work_dir.exists() {
            return;
        }
        let dir_size = utils::dir_size_bytes(&ws.work_dir);
        let _ = fs::remove_dir_all(&ws.work_dir);
        let freed_mb = dir_size as f64 / (1024.0 * 1024.0);
        println!(
            "[{}] Cleaned up work dir ({freed_mb:.0}MB freed, log preserved at {})",
            ws.issue,
            ws.log_file.display()
        );
    }

    fn resolve_default_branch(&mut self) -> String {
        if let Some(ref branch) = self.default_branch {
            return branch.clone();
        }

        let branch = Command::new("git")
            .args(["ls-remote", "--symref", &self.repo_url, "HEAD"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|out| {
                out.lines()
                    .find(|l| l.starts_with("ref: refs/heads/"))
                    .and_then(|l| l.strip_prefix("ref: refs/heads/"))
                    .and_then(|l| l.split('\t').next())
                    .map(|s| s.trim().to_string())
            })
            .unwrap_or_else(|| "main".to_string());

        self.default_branch = Some(branch.clone());
        branch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orchestrator_construction() {
        let orch = ParallelOrchestrator::new("https://github.com/test/repo", "recipe");
        assert_eq!(orch.mode, "recipe");
        assert_eq!(orch.default_max_runtime, DEFAULT_MAX_RUNTIME);
        assert!(orch.workstreams.is_empty());
    }

    #[test]
    fn test_set_timeout_policy() {
        let mut orch = ParallelOrchestrator::new(".", "recipe");
        orch.set_default_timeout_policy("continue-preserve");
        assert_eq!(orch.default_timeout_policy, "continue-preserve");

        // Invalid policy should be rejected
        orch.set_default_timeout_policy("invalid-policy");
        assert_eq!(orch.default_timeout_policy, "continue-preserve");
    }

    #[test]
    fn test_report_empty() {
        let orch = ParallelOrchestrator::new(".", "recipe");
        let report = orch.report();
        assert!(report.contains("PARALLEL WORKSTREAM REPORT"));
        assert!(report.contains("Mode: recipe"));
        assert!(report.contains("Total: 0 | Succeeded: 0 | Failed: 0"));
    }
}
