//! Scoped child commands, owned process groups, EOF drains and completion events.
use super::{
    identity::WorkstreamId,
    runtime::{self, Runtime},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

fn child_context(state: &Runtime, prerequisites: &[Value]) -> Value {
    let mut context = json!({"task_description": state.config.task, "repo_path": state.work_dir,
        "repository": state.config.repository, "workstream_id": state.id(),
        "classification": state.config.classification, "effective_recipe": state.config.recipe,
        "branch_name": state.config.branch,
        "workstream_state_file": state.state_file.clone(),
        "workstream_context_file": state.artefact_dir.join("context.json"),
        "workstream_progress_file": state.progress_sidecar,
        "workstream_result_file": state.artefact_dir.join("result.json"),
        "required_outputs": state.config.required_outputs, "prerequisite_results": prerequisites,
        "operation_attempts": state.operation_attempts, "run_id": state.run_id,
        "create_issue": "false"});
    if !state.config.issue.is_null() {
        context["issue_number"] = state.config.issue.clone();
    }
    if let Some(value) = &state.config.is_critical_code {
        context["is_critical_code"] = json!(value);
    }
    if let Some(legacy) = &state.legacy {
        if !legacy.checkpoint_id.is_empty() {
            context["resume_checkpoint"] = json!(legacy.checkpoint_id);
        }
        if !legacy.worktree_path.is_empty() {
            context["worktree_setup"] = json!({"worktree_path": legacy.worktree_path, "branch_name": legacy.branch, "created": false});
        }
    }
    // Applicable Development inputs cross the recipe boundary explicitly.
    if state.config.classification == "Development" {
        for key in [
            "strict_delivery",
            "no_merge",
            "should_merge",
            "allow_no_op",
            "existing_branch",
            "pr_number",
            "skip_pre_agent_validation",
        ] {
            if let Ok(value) = std::env::var(format!("RECIPE_VAR_{key}")) {
                context[key] = json!(value);
            }
        }
    }
    context
}

/// All model and task text travels in private files or literal arguments.
fn command(state: &Runtime, mode: &str, context: &Value) -> Result<Command> {
    let context_path = state.artefact_dir.join("context.json");
    runtime::atomic_json(&context_path, context)?;
    let mut cmd = Command::new("amplihack");
    if mode == "classic" && state.config.classification == "Operations" {
        let task_file = state.artefact_dir.join("TASK.md");
        let task = format!(
            "Execute this administrative leaf once in the declared repository. Follow its checklist and reuse recorded successful actions.\n\n{}\n\nContext: @{}\nReturn the completion JSON required by the context, with evidence for every declared output.\n",
            state.config.task,
            context_path.display()
        );
        runtime::atomic_json(&state.artefact_dir.join("task-context.json"), context)?;
        private_text(&task_file, task.as_bytes())?;
        let delegate = detect_delegate();
        let agent = delegate
            .split_whitespace()
            .nth(1)
            .context("missing delegate")?;
        cmd.args([agent, "--subprocess-safe", "--", "-p"])
            .arg(format!("@{}", task_file.display()));
    } else {
        cmd.args([
            "recipe",
            "run",
            state
                .config
                .recipe
                .as_deref()
                .context("missing trusted recipe")?,
            "--context-file",
        ])
        .arg(&context_path)
        .arg("--verbose");
    }
    cmd.current_dir(&state.work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // No stale tracking or whole-request context can bleed into a child leaf.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|s| {
            s.starts_with("RECIPE_VAR_")
                || [
                    "AMPLIHACK_WORKSTREAM_ISSUE",
                    "AMPLIHACK_CREATE_ISSUE",
                    "ISSUE_NUMBER",
                    "ISSUE_URL",
                ]
                .contains(&s)
        }) {
            cmd.env_remove(key);
        }
    }
    if let Some(profile) = std::env::var_os(crate::commands::profile::PROFILE_ENV) {
        let name = profile
            .to_str()
            .context("inherited Claude profile is not UTF-8")?;
        let name = crate::commands::profile::substitute_inherited_profile(name)
            .unwrap_or_else(|| name.to_owned());
        cmd.env(crate::commands::profile::PROFILE_ENV, name);
    }
    let depth = std::env::var("AMPLIHACK_SESSION_DEPTH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    cmd.env(
        "AMPLIHACK_SESSION_DEPTH",
        depth.saturating_add(1).to_string(),
    )
    .env("AMPLIHACK_TREE_ID", state.run_id.to_string())
    .env("AMPLIHACK_WORKSTREAM_ID", state.id().to_string())
    .env("AMPLIHACK_WORKSTREAM_STATE_FILE", state.state_file.clone())
    .env(
        "AMPLIHACK_WORKSTREAM_PROGRESS_FILE",
        &state.progress_sidecar,
    )
    .env("AMPLIHACK_REPO_PATH", &state.work_dir);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Each child owns a process group, so cancellation includes its descendants.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    Ok(cmd)
}

fn private_text(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!path.is_symlink(), "private file is a symlink");
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

fn drain(mut input: impl Read, log: Arc<Mutex<File>>) -> Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let count = input.read(&mut buf)?;
        if count == 0 {
            break;
        }
        log.lock()
            .map_err(|_| anyhow::anyhow!("log writer poisoned"))?
            .write_all(&buf[..count])?;
        if captured.len() < 16 * 1024 * 1024 {
            let keep = count.min(16 * 1024 * 1024 - captured.len());
            captured.extend_from_slice(&buf[..keep]);
        }
    }
    Ok(captured)
}

pub(super) enum Event {
    Completed(WorkstreamId, Result<(i32, Vec<u8>)>),
    Interrupted,
}

pub(super) struct Active {
    pid: u32,
    started: Option<String>,
    scope: super::models::WorkstreamScope,
    process: super::models::ProcessScope,
}
impl Active {
    pub(super) fn cancel(&self) -> Result<()> {
        use super::process_scope::{
            CurrentWorkflowScope, ProcessScopeConfig, snapshot_for_pid, validate_process_scope,
        };
        let scope = &self.scope;
        let current = CurrentWorkflowScope {
            repository: scope.repository.clone(),
            repo_root: scope.repo_root.clone(),
            workdir: scope.workdir.clone(),
            branch: scope.branch.clone(),
            issue_id: scope.issue_id.clone(),
            work_item_id: scope.work_item_id.clone(),
            recipe_run_id: scope.recipe_run_id.clone(),
            tree_id: scope.tree_id.clone(),
            workstream_id: scope.workstream_id.clone(),
        };
        let snapshot = snapshot_for_pid(self.pid, std::path::Path::new(&scope.workdir));
        let validation = validate_process_scope(
            &current,
            scope,
            &self.process,
            &snapshot,
            &ProcessScopeConfig {
                max_age_seconds: i64::MAX,
            },
        );
        if !validation.is_valid() {
            tracing::debug!(
                "leaving unowned or completed process alone: {}",
                validation.reason()
            );
            return Ok(());
        }
        // Identity alone is not authority: only our live group with its launch evidence.
        #[cfg(unix)]
        {
            if self.started.is_some()
                && super::process_scope::process_start_metadata(self.pid) == self.started
            {
                let group = unsafe { libc::getpgid(self.pid as i32) };
                if group == self.pid as i32 && unsafe { libc::kill(-group, libc::SIGTERM) } != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error.into());
                    }
                }
            }
        }
        Ok(())
    }
}

pub(super) struct Children(pub HashMap<WorkstreamId, Active>);
impl Drop for Children {
    fn drop(&mut self) {
        for child in self.0.values() {
            if let Err(e) = child.cancel() {
                tracing::error!("cannot cancel owned child: {e:#}");
            }
        }
    }
}

pub(super) fn launch(
    state: &mut Runtime,
    mode: &str,
    prerequisites: &[Value],
    tx: mpsc::Sender<Event>,
) -> Result<Active> {
    let context = child_context(state, prerequisites);
    // Fence even a crash between spawn and recording the process. No automatic replay.
    state.lifecycle = "running".into();
    state.save()?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let log = Arc::new(Mutex::new(options.open(&state.log_file)?));
    let mut child = command(state, mode, &context)?
        .spawn()
        .context("launch leaf")?;
    let pid = child.id();
    let started = super::process_scope::process_start_metadata(pid);
    state.process_scope = super::models::ProcessScope {
        pid: Some(pid),
        repository: state.config.repository.clone(),
        repo_root: state.work_dir.to_string_lossy().into_owned(),
        workdir: state.work_dir.to_string_lossy().into_owned(),
        branch: state.config.branch.clone(),
        issue_id: if state.config.issue.is_null() {
            String::new()
        } else {
            state.config.issue.to_string()
        },
        work_item_id: state.id().to_string(),
        workstream_id: state.id().to_string(),
        recipe_run_id: state.run_id.to_string(),
        tree_id: state.run_id.to_string(),
        process_started_at: started.clone().unwrap_or_default(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(error) = state.save() {
        child.kill()?;
        child.wait()?;
        return Err(error);
    }
    let stdout = child.stdout.take().context("missing child stdout")?;
    let stderr = child.stderr.take().context("missing child stderr")?;
    let id = state.id();
    let out_log = log.clone();
    let err_log = log.clone();
    thread::spawn(move || {
        let out = thread::spawn(move || drain(stdout, out_log));
        let err = thread::spawn(move || drain(stderr, err_log));
        let status = child.wait();
        // Join both drains even after an error, before signalling completion.
        let out = out
            .join()
            .map_err(|_| anyhow::anyhow!("stdout drain panicked"))
            .and_then(|v| v);
        let err = err
            .join()
            .map_err(|_| anyhow::anyhow!("stderr drain panicked"))
            .and_then(|v| v);
        let completion = (|| {
            let status = status?;
            let bytes = out?;
            err?;
            log.lock()
                .map_err(|_| anyhow::anyhow!("log poisoned"))?
                .sync_all()?;
            Ok((status.code().unwrap_or(-1), bytes))
        })();
        if tx.send(Event::Completed(id, completion)).is_err() {
            tracing::error!("dispatcher closed before child {id} completed");
        }
    });
    let process = state.process_scope.clone();
    let scope = super::models::WorkstreamScope {
        repository: process.repository.clone(),
        repo_root: process.repo_root.clone(),
        workdir: process.workdir.clone(),
        branch: process.branch.clone(),
        issue_id: process.issue_id.clone(),
        work_item_id: process.work_item_id.clone(),
        recipe: state.config.recipe.clone().unwrap_or_default(),
        recipe_run_id: process.recipe_run_id.clone(),
        tree_id: process.tree_id.clone(),
        workstream_id: process.workstream_id.clone(),
        started_at: process.recorded_at.clone(),
        ..Default::default()
    };
    Ok(Active {
        pid,
        started,
        scope,
        process,
    })
}

pub(super) fn detect_delegate() -> String {
    const VALID: &[&str] = &[
        "amplihack claude",
        "amplihack copilot",
        "amplihack amplifier",
    ];
    match std::env::var("AMPLIHACK_DELEGATE") {
        Ok(value) if VALID.contains(&value.as_str()) => value,
        Ok(value) => {
            tracing::warn!("invalid delegate {value:?}; using amplihack claude");
            "amplihack claude".into()
        }
        Err(_) => "amplihack claude".into(),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;

    #[test]
    fn issue_2553_cancellation_checks_ownership_without_requiring_issue_or_branch() {
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new("/bin/cat");
        command
            .env_clear()
            .env("HOME", home.path())
            .current_dir(home.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"ready\n").unwrap();
        let mut ready = String::new();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        let id = WorkstreamId::new().to_string();
        let dir = super::super::process_scope::normalize_path(home.path());
        let started = super::super::process_scope::process_start_metadata(child.id()).unwrap();
        let process = super::super::models::ProcessScope {
            pid: Some(child.id()),
            repository: "example/operations".into(),
            repo_root: dir.clone(),
            workdir: dir.clone(),
            work_item_id: id.clone(),
            workstream_id: id.clone(),
            recipe_run_id: "owned-run".into(),
            tree_id: "owned-tree".into(),
            process_started_at: started.clone(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            ..Default::default()
        };
        let scope = super::super::models::WorkstreamScope {
            repository: process.repository.clone(),
            repo_root: dir.clone(),
            workdir: dir,
            recipe: "operations-direct".into(),
            work_item_id: id.clone(),
            workstream_id: id,
            recipe_run_id: process.recipe_run_id.clone(),
            tree_id: process.tree_id.clone(),
            ..Default::default()
        };
        let mut active = Active {
            pid: child.id(),
            started: Some(started),
            scope,
            process,
        };
        active.scope.repository = "example/foreign".into();
        active.cancel().unwrap();
        input.write_all(b"still-owned\n").unwrap();
        ready.clear();
        output.read_line(&mut ready).unwrap();
        assert_eq!(ready, "still-owned\n");
        active.scope.repository = active.process.repository.clone();
        active.cancel().unwrap();
        assert!(!child.wait().unwrap().success());
    }
}
