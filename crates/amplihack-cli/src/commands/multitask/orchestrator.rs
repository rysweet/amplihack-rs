//! Core orchestrator logic for parallel workstream execution.
//!
//! Port of `multitask/orchestrator.py` — manages workstream lifecycle,
//! subprocess spawning, output tailing, timeout enforcement, state persistence,
//! and reporting.

use super::models::*;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Default base directory for workstream artifacts.
pub fn default_base_dir() -> String {
    format!(
        "{}/amplihack-workstreams",
        std::env::temp_dir().to_string_lossy()
    )
}

/// Valid delegate commands for subprocess execution.
const VALID_DELEGATES: &[&str] = &[
    "amplihack claude",
    "amplihack copilot",
    "amplihack amplifier",
];

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
            base_dir: PathBuf::from(default_base_dir()),
            state_dir: PathBuf::from(default_base_dir()).join("state"),
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
        if policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY
            || policy == CONTINUE_PRESERVE_TIMEOUT_POLICY
        {
            self.default_timeout_policy = policy.to_string();
        } else {
            warn!(
                "Invalid timeout policy {policy:?}; using default {DEFAULT_TIMEOUT_POLICY:?}"
            );
        }
    }

    pub fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(true))
    }

    pub fn setup(&self) -> Result<()> {
        fs::create_dir_all(&self.base_dir)
            .with_context(|| format!("Failed to create base dir: {}", self.base_dir.display()))?;
        fs::create_dir_all(&self.state_dir).with_context(|| {
            format!("Failed to create state dir: {}", self.state_dir.display())
        })?;

        // Check disk space
        self.check_disk_space(5.0)?;
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

        // Apply runtime overrides
        ws.max_runtime = config.max_runtime.unwrap_or(self.default_max_runtime);
        ws.timeout_policy = config
            .timeout_policy
            .clone()
            .unwrap_or_else(|| self.default_timeout_policy.clone());

        // Load any saved state
        let saved = load_state(&ws.state_file);
        if let Some(ref state) = saved {
            self.apply_saved_state(&mut ws, state);
        }

        let reuse_existing =
            saved.is_some() && !ws.cleanup_eligible && ws.work_dir.exists();

        if !reuse_existing && ws.work_dir.exists() {
            let _ = fs::remove_dir_all(&ws.work_dir);
        }

        let default_branch = self.resolve_default_branch();

        if reuse_existing {
            println!("[{}] Reusing preserved work dir {}", issue, ws.work_dir.display());
        } else {
            println!(
                "[{}] Cloning default branch '{}' from remote...",
                issue, default_branch
            );
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
                bail!("[{issue}] git clone failed with exit code {}", status);
            }
        }

        fs::create_dir_all(&ws.work_dir)?;

        // Write execution files
        if self.mode == "recipe" {
            self.write_recipe_launcher(&ws)?;
        } else {
            self.write_classic_launcher(&ws)?;
        }

        persist_state(&ws)?;
        self.workstreams.push(ws);
        Ok(())
    }

    pub fn launch_all(&mut self) -> Result<()> {
        let delegate = self.detect_delegate();
        let count = self.workstreams.len();

        for i in 0..count {
            self.launch_workstream(i, &delegate)?;
        }

        println!(
            "\n{count} workstreams launched in parallel ({} mode)",
            self.mode
        );
        Ok(())
    }

    fn launch_workstream(&mut self, idx: usize, delegate: &str) -> Result<()> {
        let ws = &mut self.workstreams[idx];
        let issue = ws.issue;

        let mut env_vars: HashMap<String, String> = std::env::vars().collect();
        env_vars.insert("AMPLIHACK_DELEGATE".to_string(), delegate.to_string());

        let run_sh = ws.work_dir.join("run.sh");
        let child = Command::new("bash")
            .arg(&run_sh)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&ws.work_dir)
            .envs(&env_vars)
            .spawn()
            .with_context(|| format!("[{issue}] Failed to launch workstream"))?;

        ws.pid = Some(child.id());
        ws.start_time = Some(Instant::now());
        ws.end_time = None;
        ws.exit_code = None;
        ws.lifecycle_state = "running".to_string();
        ws.cleanup_eligible = false;
        ws.attempt += 1;

        println!("[{issue}] Launched PID {} ({} mode)", child.id(), self.mode);

        let child_arc = Arc::new(Mutex::new(Some(child)));
        self.processes.insert(issue, child_arc.clone());

        // Spawn output tailing thread
        let log_file = ws.log_file.clone();
        let max_log_bytes: u64 = std::env::var("AMPLIHACK_MAX_LOG_BYTES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100 * 1024 * 1024);

        thread::spawn(move || {
            let mut child_guard = child_arc.lock().unwrap();
            if let Some(ref mut child) = *child_guard {
                if let Some(stdout) = child.stdout.take() {
                    drop(child_guard);
                    tail_output(stdout, &log_file, issue, max_log_bytes);
                }
            }
        });

        persist_state(&self.workstreams[idx])?;
        Ok(())
    }

    pub fn monitor(&mut self, running: Arc<AtomicBool>) -> Result<()> {
        let check_interval = Duration::from_secs(10);
        let start = Instant::now();

        while running.load(Ordering::Relaxed) {
            // Enforce per-workstream timeouts
            self.enforce_timeouts();

            let status = self.get_status();
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
                if !self.cleaned_up.contains(&ws.issue) && ws.exit_code.is_some() {
                    if Workstream::derive_cleanup_eligible(&ws.lifecycle_state) {
                        self.cleanup_workstream_dir(ws);
                    }
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
            self.cleanup_running();
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
                    ws.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".to_string())
                )
            };

            lines.push(format!("\n[{}] {}", ws.issue, ws.description));
            lines.push(format!("  Branch:    {}", ws.branch));
            lines.push(format!("  Status:    {status}"));
            lines.push(format!("  Lifecycle: {}", ws.lifecycle_state));
            lines.push(format!("  Runtime:   {runtime}"));
            lines.push(format!(
                "  Checkpoint: {}",
                if ws.checkpoint_id.is_empty() { "n/a" } else { &ws.checkpoint_id }
            ));
            lines.push(format!(
                "  Worktree: {}",
                if ws.worktree_path.is_empty() { "n/a" } else { &ws.worktree_path }
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
        let config_text = fs::read_to_string(config_path)
            .with_context(|| format!("Failed to read config: {config_path}"))?;
        let items: Vec<WorkstreamConfig> = serde_json::from_str(&config_text)?;

        let mut deleted_count = 0u32;
        let mut freed_bytes = 0u64;

        for item in &items {
            let issue = item.issue_id();
            let safe_id = sanitize_id(&issue.to_string());
            let work_dir = self.base_dir.join(format!("ws-{issue}"));
            let state_file = self.state_dir.join(format!("ws-{safe_id}.json"));

            // Check if PR is merged via gh CLI
            let is_merged = Command::new("gh")
                .args(["pr", "view", &item.branch, "--json", "state", "-q", ".state"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .is_some_and(|s| s.trim() == "MERGED");

            if !is_merged {
                continue;
            }

            if work_dir.exists() {
                let dir_size = dir_size_bytes(&work_dir);
                if dry_run {
                    println!(
                        "[{issue}] Would delete work dir ({:.0}MB)",
                        dir_size as f64 / (1024.0 * 1024.0)
                    );
                } else {
                    let _ = fs::remove_dir_all(&work_dir);
                    let _ = fs::remove_file(&state_file);
                    println!(
                        "[{issue}] Deleted work dir ({:.0}MB freed)",
                        dir_size as f64 / (1024.0 * 1024.0)
                    );
                }
                freed_bytes += dir_size;
                deleted_count += 1;
            }
        }

        let freed_gb = freed_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        println!(
            "\n{}Summary:\n  Workstreams {}deleted: {deleted_count}\n  Disk space {}freed: {freed_gb:.2}GB",
            if dry_run { "DRY RUN " } else { "" },
            if dry_run { "would be " } else { "" },
            if dry_run { "would be " } else { "" },
        );

        if dry_run && deleted_count > 0 {
            println!("\nRun without --dry-run to actually delete these workstreams.");
        }

        Ok(())
    }

    // --- Private helpers ---

    fn get_status(&mut self) -> WorkstreamStatus {
        let mut status = WorkstreamStatus::default();

        for ws in &mut self.workstreams {
            let proc = self.processes.get(&ws.issue);

            let maybe_code = proc.and_then(|arc| {
                let mut guard = arc.lock().unwrap();
                guard.as_mut().and_then(|child| child.try_wait().ok().flatten().map(|s| s.code().unwrap_or(-1)))
            });

            if let Some(code) = maybe_code {
                finalize_workstream(ws, code);
                if ws.lifecycle_state == "completed" {
                    status.completed.push(ws.issue);
                } else {
                    status.failed.push(ws.issue);
                }
            } else if proc.is_some() && ws.exit_code.is_none() {
                ws.lifecycle_state = "running".to_string();
                status.running.push(ws.issue);
            } else if ws.exit_code == Some(0) || ws.lifecycle_state == "completed" {
                status.completed.push(ws.issue);
            } else {
                status.failed.push(ws.issue);
            }
        }

        status
    }

    fn enforce_timeouts(&mut self) {
        for ws in &mut self.workstreams {
            if ws.lifecycle_state != "running" {
                continue;
            }
            let Some(start) = ws.start_time else {
                continue;
            };
            let elapsed = start.elapsed().as_secs();
            if elapsed < ws.max_runtime {
                continue;
            }

            let issue = ws.issue;
            println!(
                "[{issue}] Timed out after {}s, marking timed_out_resumable",
                ws.max_runtime
            );

            if ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY {
                if let Some(proc_arc) = self.processes.get(&issue) {
                    let mut guard = proc_arc.lock().unwrap();
                    if let Some(ref mut child) = *guard {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
            }

            ws.lifecycle_state = "timed_out_resumable".to_string();
            ws.end_time = Some(Instant::now());
            let _ = persist_state(ws);
        }
    }

    fn cleanup_running(&mut self) {
        for ws in &mut self.workstreams {
            if ws.lifecycle_state != "running" {
                continue;
            }
            let issue = ws.issue;
            if let Some(proc_arc) = self.processes.get(&issue) {
                let mut guard = proc_arc.lock().unwrap();
                if let Some(ref mut child) = *guard {
                    println!("[{issue}] Terminating PID {}...", ws.pid.unwrap_or(0));
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            ws.lifecycle_state = "interrupted_resumable".to_string();
            ws.end_time = Some(Instant::now());
            let _ = persist_state(ws);
        }
    }

    fn cleanup_workstream_dir(&self, ws: &Workstream) {
        if !ws.work_dir.exists() {
            return;
        }
        let dir_size = dir_size_bytes(&ws.work_dir);
        let _ = fs::remove_dir_all(&ws.work_dir);
        let freed_mb = dir_size as f64 / (1024.0 * 1024.0);
        println!(
            "[{}] Cleaned up work dir ({freed_mb:.0}MB freed, log preserved at {})",
            ws.issue,
            ws.log_file.display()
        );
    }

    fn detect_delegate(&self) -> String {
        if let Ok(delegate) = std::env::var("AMPLIHACK_DELEGATE") {
            if VALID_DELEGATES.contains(&delegate.as_str()) {
                return delegate;
            }
            warn!(
                "AMPLIHACK_DELEGATE={delegate:?} is not valid. Using default."
            );
        }
        // Default to claude
        "amplihack claude".to_string()
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

    fn write_recipe_launcher(&self, ws: &Workstream) -> Result<()> {
        let resume_context = self.build_resume_context(ws);
        let safe_recipe = serde_json::to_string(&ws.recipe)?;
        let safe_context = serde_json::to_string(&resume_context)?;

        let launcher_py = ws.work_dir.join("launcher.py");
        let launcher_content = format!(
            r#"#!/usr/bin/env python3
"""Workstream launcher - Rust recipe runner execution."""
import sys
import json
import logging
from pathlib import Path

repo_root = Path(__file__).resolve().parent
src_path = repo_root / "src"
if src_path.exists():
    sys.path.insert(0, str(src_path))

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
)

try:
    from amplihack.recipes import run_recipe_by_name
except ImportError:
    print("ERROR: amplihack package not importable.")
    sys.exit(2)

user_context = json.loads({safe_context})
result = run_recipe_by_name(
    {safe_recipe},
    user_context=user_context,
    progress=True,
)

print()
print("=" * 60)
print("RECIPE EXECUTION RESULTS")
print("=" * 60)
for sr in result.step_results:
    print(f"  [{{sr.status.value:>9}}] {{sr.step_id}}")
print(f"\nOverall: {{'SUCCESS' if result.success else 'FAILED'}}")
sys.exit(0 if result.success else 1)
"#
        );
        fs::write(&launcher_py, launcher_content)?;
        set_executable(&launcher_py)?;

        let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let tree_id = std::env::var("AMPLIHACK_TREE_ID")
            .unwrap_or_else(|_| format!("{:08x}", rand_u32()));
        let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string());
        let max_sessions =
            std::env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string());
        let delegate = self.detect_delegate();

        let run_sh = ws.work_dir.join("run.sh");
        let run_content = format!(
            r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
export AMPLIHACK_DELEGATE='{delegate}'
export AMPLIHACK_WORKSTREAM_ISSUE='{issue}'
export AMPLIHACK_WORKSTREAM_PROGRESS_FILE='{progress_file}'
export AMPLIHACK_WORKSTREAM_STATE_FILE='{state_file}'
export AMPLIHACK_WORKTREE_PATH='{worktree_path}'
exec python3 -u launcher.py
"#,
            work_dir = ws.work_dir.display(),
            depth = depth + 1,
            issue = ws.issue,
            progress_file = ws.progress_file.display(),
            state_file = ws.state_file.display(),
            worktree_path = ws.worktree_path,
        );
        fs::write(&run_sh, run_content)?;
        set_executable(&run_sh)?;

        Ok(())
    }

    fn write_classic_launcher(&self, ws: &Workstream) -> Result<()> {
        let task_md = ws.work_dir.join("TASK.md");
        fs::write(
            &task_md,
            format!(
                "# Issue #{}\n\n{}\n\nFollow DEFAULT_WORKFLOW.md autonomously. \
                 NO QUESTIONS. Work through Steps 0-22. Create PR when complete.",
                ws.issue, ws.task
            ),
        )?;

        let depth: u32 = std::env::var("AMPLIHACK_SESSION_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let tree_id = std::env::var("AMPLIHACK_TREE_ID")
            .unwrap_or_else(|_| format!("{:08x}", rand_u32()));
        let max_depth = std::env::var("AMPLIHACK_MAX_DEPTH").unwrap_or_else(|_| "3".to_string());
        let max_sessions =
            std::env::var("AMPLIHACK_MAX_SESSIONS").unwrap_or_else(|_| "10".to_string());
        let delegate = self.detect_delegate();

        let run_sh = ws.work_dir.join("run.sh");
        let run_content = format!(
            r#"#!/bin/bash
cd '{work_dir}'
export AMPLIHACK_TREE_ID='{tree_id}'
export AMPLIHACK_SESSION_DEPTH='{depth}'
export AMPLIHACK_MAX_DEPTH='{max_depth}'
export AMPLIHACK_MAX_SESSIONS='{max_sessions}'
{delegate} --subprocess-safe -- -p "@TASK.md Execute task autonomously following DEFAULT_WORKFLOW.md. NO QUESTIONS. Work through all steps. Create PR when complete."
"#,
            work_dir = ws.work_dir.display(),
            depth = depth + 1,
        );
        fs::write(&run_sh, run_content)?;
        set_executable(&run_sh)?;

        Ok(())
    }

    fn build_resume_context(&self, ws: &Workstream) -> HashMap<String, serde_json::Value> {
        let mut ctx = HashMap::new();
        ctx.insert(
            "task_description".to_string(),
            serde_json::Value::String(ws.task.clone()),
        );
        ctx.insert(
            "repo_path".to_string(),
            serde_json::Value::String(".".to_string()),
        );
        ctx.insert(
            "issue_number".to_string(),
            serde_json::json!(ws.issue),
        );
        ctx.insert(
            "workstream_state_file".to_string(),
            serde_json::Value::String(ws.state_file.to_string_lossy().to_string()),
        );
        ctx.insert(
            "workstream_progress_file".to_string(),
            serde_json::Value::String(ws.progress_file.to_string_lossy().to_string()),
        );
        if !ws.resume_checkpoint.is_empty() {
            ctx.insert(
                "resume_checkpoint".to_string(),
                serde_json::Value::String(ws.resume_checkpoint.clone()),
            );
        }
        if !ws.worktree_path.is_empty() {
            ctx.insert(
                "worktree_setup".to_string(),
                serde_json::json!({
                    "worktree_path": ws.worktree_path,
                    "branch_name": ws.branch,
                    "created": false,
                }),
            );
        }
        ctx
    }

    fn apply_saved_state(&self, ws: &mut Workstream, state: &PersistedState) {
        if !state.branch.is_empty() {
            ws.branch = state.branch.clone();
        }
        if !state.description.is_empty() {
            ws.description = state.description.clone();
        }
        if !state.lifecycle_state.is_empty() {
            ws.lifecycle_state = state.lifecycle_state.clone();
        }
        ws.cleanup_eligible = state.cleanup_eligible
            || Workstream::derive_cleanup_eligible(&ws.lifecycle_state);
        if !state.worktree_path.is_empty() {
            ws.worktree_path = state.worktree_path.clone();
        }
        if !state.checkpoint_id.is_empty() {
            ws.checkpoint_id = state.checkpoint_id.clone();
            ws.resume_checkpoint = state.checkpoint_id.clone();
        }
        if !state.current_step.is_empty() {
            ws.last_step = state.current_step.clone();
        }
        ws.attempt = state.attempt;
        if let Some(max_rt) = state.max_runtime {
            ws.max_runtime = max_rt;
        }
        if let Some(ref policy) = state.timeout_policy {
            ws.timeout_policy = policy.clone();
        }
        ws.pid = state.last_pid;
        if let Some(code) = state.last_exit_code {
            ws.exit_code = Some(code);
        }
    }

    fn check_disk_space(&self, min_free_gb: f64) -> Result<()> {
        // Use statvfs to check available space
        let path_cstr = std::ffi::CString::new(self.base_dir.to_string_lossy().as_bytes())?;
        unsafe {
            let mut stat: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(path_cstr.as_ptr(), &mut stat) == 0 {
                let free_bytes = stat.f_bavail as u64 * stat.f_frsize as u64;
                let free_gb = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                if free_gb < min_free_gb {
                    bail!(
                        "Insufficient disk space: {free_gb:.1}GB free, need {min_free_gb}GB minimum"
                    );
                }
                debug!("Disk space check: {free_gb:.1}GB available");
            }
        }
        Ok(())
    }
}

/// Status snapshot for all workstreams.
#[derive(Default)]
struct WorkstreamStatus {
    running: Vec<i64>,
    completed: Vec<i64>,
    failed: Vec<i64>,
}

/// Finalize a workstream after its process exits.
fn finalize_workstream(ws: &mut Workstream, exit_code: i32) {
    if ws.end_time.is_none() {
        ws.end_time = Some(Instant::now());
    }
    ws.exit_code = Some(exit_code);

    if ws.lifecycle_state == "interrupted_resumable"
        || (ws.lifecycle_state == "timed_out_resumable"
            && ws.timeout_policy == INTERRUPT_PRESERVE_TIMEOUT_POLICY)
    {
        let _ = persist_state(ws);
        return;
    }

    if exit_code == 0 {
        ws.lifecycle_state = "completed".to_string();
    } else if !ws.checkpoint_id.is_empty() {
        ws.lifecycle_state = "failed_resumable".to_string();
    } else {
        ws.lifecycle_state = "failed_terminal".to_string();
    }
    ws.cleanup_eligible = Workstream::derive_cleanup_eligible(&ws.lifecycle_state);
    let _ = persist_state(ws);
}

/// Load persisted state from a JSON file.
fn load_state(state_file: &Path) -> Option<PersistedState> {
    fs::read_to_string(state_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// Persist workstream state to its JSON state file.
fn persist_state(ws: &Workstream) -> Result<()> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    let existing = load_state(&ws.state_file);
    let created_at = existing
        .as_ref()
        .and_then(|s| {
            if s.created_at.is_empty() {
                None
            } else {
                Some(s.created_at.clone())
            }
        })
        .unwrap_or_else(|| now.clone());

    let state = PersistedState {
        issue: ws.issue,
        branch: ws.branch.clone(),
        description: ws.description.clone(),
        task: ws.task.clone(),
        recipe: ws.recipe.clone(),
        lifecycle_state: ws.lifecycle_state.clone(),
        cleanup_eligible: ws.cleanup_eligible,
        attempt: ws.attempt,
        last_pid: ws.pid,
        last_exit_code: ws.exit_code,
        current_step: if ws.last_step.is_empty() {
            "unknown".to_string()
        } else {
            ws.last_step.clone()
        },
        checkpoint_id: ws.checkpoint_id.clone(),
        work_dir: ws.work_dir.to_string_lossy().to_string(),
        worktree_path: ws.worktree_path.clone(),
        log_file: ws.log_file.to_string_lossy().to_string(),
        progress_sidecar: ws.progress_file.to_string_lossy().to_string(),
        max_runtime: Some(ws.max_runtime),
        timeout_policy: Some(ws.timeout_policy.clone()),
        created_at,
        updated_at: now,
        resume_context: existing.and_then(|s| s.resume_context),
    };

    let json = serde_json::to_string_pretty(&state)?;
    atomic_write(&ws.state_file, json.as_bytes())?;
    Ok(())
}

/// Tail subprocess output to a log file and prefixed stdout.
fn tail_output(stdout: impl std::io::Read, log_file: &Path, issue_id: i64, max_log_bytes: u64) {
    let reader = BufReader::new(stdout);
    let mut log_bytes_written: u64 = 0;

    // Open log file with 0o600 permissions
    let log_fd = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(log_file);

    let mut log_writer = log_fd.ok();

    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line_bytes = line.len() as u64 + 1; // +1 for newline

        if log_bytes_written < max_log_bytes {
            if log_bytes_written + line_bytes <= max_log_bytes {
                if let Some(ref mut w) = log_writer {
                    let _ = writeln!(w, "{line}");
                    let _ = w.flush();
                }
                log_bytes_written += line_bytes;
            }
        }

        // Prefix output to stdout
        println!("[ws:{issue_id}] {line}");
    }
}

/// Atomically write data to a file (write to tmp, then rename).
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp_path)?;
    file.write_all(data)?;
    file.flush()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

/// Set a file as executable (chmod +x).
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

/// Calculate total size of a directory tree.
fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if path.is_dir() {
                total += dir_size_bytes(&path);
            }
        }
    }
    total
}

/// Simple pseudo-random u32 using time-based seed (no external crate needed).
fn rand_u32() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_nanos() & 0xFFFF_FFFF) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_id() {
        assert_eq!(sanitize_id("123"), "123");
        assert_eq!(sanitize_id("hello-world"), "hello-world");
        assert_eq!(sanitize_id("path/../../etc"), "path_______etc");
        assert_eq!(sanitize_id("a b c"), "a_b_c");
    }

    #[test]
    fn test_workstream_config_parsing() {
        let json = r#"[
            {
                "issue": 42,
                "branch": "feat/test",
                "description": "Test workstream",
                "task": "Do something",
                "recipe": "default-workflow"
            }
        ]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].issue_id(), 42);
        assert_eq!(configs[0].branch, "feat/test");
        assert_eq!(configs[0].description_or_default(), "Test workstream");
    }

    #[test]
    fn test_workstream_config_string_issue() {
        let json = r#"[{"issue": "99", "branch": "b", "task": "t"}]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].issue_id(), 99);
    }

    #[test]
    fn test_workstream_config_default_description() {
        let json = r#"[{"issue": 7, "branch": "b", "task": "t"}]"#;
        let configs: Vec<WorkstreamConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs[0].description_or_default(), "Issue #7");
    }

    #[test]
    fn test_persisted_state_roundtrip() {
        let state = PersistedState {
            issue: 42,
            branch: "feat/test".to_string(),
            lifecycle_state: "completed".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: PersistedState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.issue, 42);
        assert_eq!(parsed.branch, "feat/test");
        assert_eq!(parsed.lifecycle_state, "completed");
    }

    #[test]
    fn test_derive_cleanup_eligible() {
        assert!(Workstream::derive_cleanup_eligible("completed"));
        assert!(Workstream::derive_cleanup_eligible("failed_terminal"));
        assert!(Workstream::derive_cleanup_eligible("abandoned"));
        assert!(!Workstream::derive_cleanup_eligible("running"));
        assert!(!Workstream::derive_cleanup_eligible("pending"));
        assert!(!Workstream::derive_cleanup_eligible("failed_resumable"));
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_MAX_RUNTIME, 7200);
        assert_eq!(DEFAULT_TIMEOUT_POLICY, "interrupt-preserve");
    }

    #[test]
    fn test_valid_delegates() {
        assert!(VALID_DELEGATES.contains(&"amplihack claude"));
        assert!(VALID_DELEGATES.contains(&"amplihack copilot"));
        assert!(VALID_DELEGATES.contains(&"amplihack amplifier"));
        assert!(!VALID_DELEGATES.contains(&"rm -rf /"));
    }

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
    fn test_atomic_write() {
        let dir = std::env::temp_dir().join("amplihack-test-atomic-write");
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("test.json");
        atomic_write(&file, b"hello world").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
        let _ = fs::remove_dir_all(&dir);
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
