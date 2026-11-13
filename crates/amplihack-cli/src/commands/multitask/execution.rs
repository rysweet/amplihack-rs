//! Typed child invocation and private leaf context.
use super::{models::Workstream, ownership, persistence::persist_state, tracking, utils};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    fs,
    process::{Child, Command, Stdio},
    time::Instant,
};

fn context(ws: &Workstream, results: Value) -> Value {
    let mut context = json!({"task_description": ws.task, "repo_path": ws.work_dir,
        "workstream_state_file": ws.state_file, "workstream_progress_file": ws.progress_file,
        "prerequisite_results": results, "create_issue": "false"});
    if context["prerequisite_results"]
        .as_object()
        .is_some_and(|results| !results.is_empty())
    {
        context["task_description"] = json!(format!(
            "{}\n\nPrerequisite results by workstream key:\n{}",
            ws.task, context["prerequisite_results"]
        ));
    }
    if ws.classification == "Q&A" {
        context["question"] = context["task_description"].clone();
    }
    if let Some(issue) = ws.issue {
        context["issue_number"] = json!(issue);
    }
    if let Some(value) = &ws.is_critical_code {
        context["is_critical_code"] = json!(value);
    }
    if !ws.resume_checkpoint.is_empty() {
        context["resume_checkpoint"] = json!(ws.resume_checkpoint);
    }
    if !ws.worktree_path.is_empty() {
        context["worktree_setup"] =
            json!({"worktree_path": ws.worktree_path, "branch_name": ws.branch, "created": false});
    }
    if ws.classification == "Development" || ws.classification == "Consensus" {
        context["branch_name"] = json!(ws.branch);
        for key in [
            "strict_delivery",
            "no_merge",
            "should_merge",
            "allow_no_op",
            "existing_branch",
            "pr_number",
            "skip_pre_agent_validation",
            "auto_init_repo",
        ] {
            if let Ok(value) = std::env::var(format!("RECIPE_VAR_{key}")) {
                context[key] = json!(value);
            }
        }
    }
    context
}

fn private_output(path: &std::path::Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

pub(super) fn launch(ws: &mut Workstream, mode: &str, prerequisites: Value) -> Result<Child> {
    ownership::verify(ws)?;
    tracking::acquire(ws)?;
    let path = ws.artefact_dir.join("context.json");
    utils::atomic_write(&path, &serde_json::to_vec(&context(ws, prerequisites))?)?;
    // Remove an old sidecar before a new attempt; stale successful prose is not evidence.
    match fs::remove_file(&ws.progress_file) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    let mut command = Command::new("amplihack");
    if mode == "classic" {
        let delegate = super::launcher::detect_delegate();
        let agent = delegate
            .split_whitespace()
            .nth(1)
            .context("missing agent delegate")?;
        let task = if ws.classification == "Operations" {
            format!(
                "Carry out this Operations checklist directly. Do not start a workflow, clone, branch or create a PR.\n{}\nContext: @{}\nFinish with OPERATIONS_RESULT: <completed|partial|failed> — <outcome>",
                ws.task,
                path.display()
            )
        } else {
            super::command_builder::classic_task_markdown(ws)
        };
        let task_path = ws.artefact_dir.join("TASK.md");
        utils::atomic_write(&task_path, task.as_bytes())?;
        command
            .args([agent, "--subprocess-safe", "--", "-p"])
            .arg(format!("@{}", task_path.display()));
    } else {
        command
            .args(["recipe", "run", &ws.recipe, "--context-file"])
            .arg(&path)
            .args(["--format", "json", "--no-detach"]);
    }
    // Tracking and recipe context belong to this leaf, never to the parent request.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|k| {
            k.starts_with("RECIPE_VAR_")
                || ["AMPLIHACK_WORKSTREAM_ISSUE", "AMPLIHACK_CREATE_ISSUE"].contains(&k)
        }) {
            command.env_remove(key);
        }
    }
    if let Some(issue) = ws.issue {
        command.env("AMPLIHACK_WORKSTREAM_ISSUE", issue.to_string());
    }
    let (tree, depth, max_depth, max_sessions) = super::command_builder::session_tree_env(ws);
    command
        .env("AMPLIHACK_TREE_ID", tree)
        .env("AMPLIHACK_SESSION_DEPTH", (depth + 1).to_string())
        .env("AMPLIHACK_MAX_DEPTH", max_depth)
        .env("AMPLIHACK_MAX_SESSIONS", max_sessions);
    for key in [
        crate::commands::profile::PROFILE_ENV,
        crate::commands::profile::ROOT_ENV,
    ] {
        if let Some(raw) = std::env::var_os(key).filter(|value| !value.is_empty()) {
            let value = raw.to_str().with_context(|| {
                format!("{key} is not UTF-8; cannot preserve the pinned profile")
            })?;
            let value = if key == crate::commands::profile::PROFILE_ENV {
                crate::commands::profile::substitute_inherited_profile(value)
                    .unwrap_or_else(|| value.to_owned())
            } else {
                value.to_owned()
            };
            command.env(key, value);
        }
    }
    command
        .env("AMPLIHACK_WORKSTREAM_STATE_FILE", &ws.state_file)
        .env("AMPLIHACK_WORKSTREAM_PROGRESS_FILE", &ws.progress_file)
        .env("AMPLIHACK_DELEGATE", super::launcher::detect_delegate());
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let log = options.open(&ws.log_file)?;
    command
        .current_dir(&ws.work_dir)
        .stdin(Stdio::null())
        .stdout(private_output(&ws.artefact_dir.join("output.json"))?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    // Persist the launch intent before the child can perform external actions.
    ws.lifecycle_state = "launching".into();
    ws.attempt += 1;
    persist_state(ws)?;
    let mut child = command
        .spawn()
        .with_context(|| format!("[{}] launch {}", ws.key, ws.recipe))?;
    ws.pid = Some(child.id());
    ws.start_time = Some(Instant::now());
    ws.lifecycle_state = "running".into();
    ws.process_scope = super::models::ProcessScope {
        pid: ws.pid,
        repository: ws.workstream_scope.repository.clone(),
        repo_root: ws.workstream_scope.repo_root.clone(),
        workdir: ws.workstream_scope.workdir.clone(),
        branch: ws.workstream_scope.branch.clone(),
        issue_id: ws.issue.map(|n| n.to_string()).unwrap_or_default(),
        work_item_id: ws.key.to_string(),
        recipe_run_id: ws.workstream_scope.recipe_run_id.clone(),
        tree_id: ws.workstream_scope.tree_id.clone(),
        workstream_id: ws.workstream_scope.workstream_id.clone(),
        process_started_at: super::process_scope::process_start_metadata(child.id())
            .unwrap_or_default(),
        recorded_at: chrono::Utc::now().to_rfc3339(),
    };

    if let Err(error) = persist_state(ws) {
        child
            .kill()
            .context("stop child after persistence failure")?;
        child
            .wait()
            .context("reap child after persistence failure")?;
        return Err(error);
    }
    Ok(child)
}
