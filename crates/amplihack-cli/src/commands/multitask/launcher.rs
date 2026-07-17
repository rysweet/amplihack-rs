//! Launcher generation and process spawning for workstreams.

use super::command_builder;
#[cfg(test)]
pub(super) use super::command_builder::VALID_DELEGATES;
pub(super) use super::command_builder::{detect_delegate, populate_workstream_scope};
use super::log_output::spawn_log_output_thread;
use super::models::{ProcessScope, Workstream};
use super::persistence::persist_state;
use super::process_scope::process_start_metadata;
use super::utils::set_executable;
use amplihack_types::hook_io::normalize_executable_script_line_endings;
use anyhow::{Context, Result};
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Write recipe-mode launcher scripts for a workstream.
pub(super) fn write_recipe_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    let resume_context = command_builder::build_resume_context(ws);
    let safe_context = serde_json::to_string(&resume_context)?;

    // Write context JSON so the launcher script can pass it via -c flags
    let context_json = ws.work_dir.join("context.json");
    fs::write(&context_json, &safe_context)?;

    let launcher_sh = ws.work_dir.join("launcher.sh");
    let launcher_content = command_builder::recipe_launcher_script(&ws.recipe);
    write_executable_script(&launcher_sh, &launcher_content)?;
    set_executable(&launcher_sh)?;

    let run_sh = ws.work_dir.join("run.sh");
    let run_content = command_builder::recipe_run_script(ws, delegate);
    write_executable_script(&run_sh, &run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

/// Write classic-mode launcher scripts for a workstream.
pub(super) fn write_classic_launcher(ws: &Workstream, delegate: &str) -> Result<()> {
    let task_md = ws.work_dir.join("TASK.md");
    fs::write(&task_md, command_builder::classic_task_markdown(ws))?;

    let run_sh = ws.work_dir.join("run.sh");
    let run_content = command_builder::classic_run_script(ws, delegate);
    write_executable_script(&run_sh, &run_content)?;
    set_executable(&run_sh)?;

    Ok(())
}

pub(super) fn write_executable_script(path: &std::path::Path, content: &str) -> Result<()> {
    let normalized = normalize_executable_script_line_endings(content);
    fs::write(path, normalized)
        .with_context(|| format!("write executable launcher script {}", path.display()))
}

/// Launch a single workstream subprocess.
pub(super) fn launch_workstream(
    ws: &mut Workstream,
    mode: &str,
    delegate: &str,
    processes: &mut HashMap<i64, Arc<Mutex<Option<Child>>>>,
) -> Result<()> {
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
    ws.process_scope = ProcessScope {
        pid: Some(child.id()),
        repository: ws.workstream_scope.repository.clone(),
        repo_root: ws.workstream_scope.repo_root.clone(),
        workdir: ws.workstream_scope.workdir.clone(),
        branch: ws.workstream_scope.branch.clone(),
        issue_id: ws.workstream_scope.issue_id.clone(),
        work_item_id: ws.workstream_scope.work_item_id.clone(),
        recipe_run_id: ws.workstream_scope.recipe_run_id.clone(),
        tree_id: ws.workstream_scope.tree_id.clone(),
        workstream_id: ws.workstream_scope.workstream_id.clone(),
        process_started_at: process_start_metadata(child.id())
            .unwrap_or_else(|| Utc::now().to_rfc3339()),
        recorded_at: Utc::now().to_rfc3339(),
    };

    println!("[{issue}] Launched PID {} ({mode} mode)", child.id());

    let child_arc = Arc::new(Mutex::new(Some(child)));
    processes.insert(issue, child_arc.clone());

    spawn_log_output_thread(child_arc, ws.log_file.clone(), issue);

    persist_state(ws)?;
    Ok(())
}
