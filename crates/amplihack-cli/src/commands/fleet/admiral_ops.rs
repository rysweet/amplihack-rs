use super::*;

/// Issue #1570: stage 1 of `start_agent`. The remote shell saves its stdin
/// (the prompt) in a private temporary file and prints the file's path.
/// `'%s\n'` must stay single-quoted, or the path is printed ending in `n`.
const START_AGENT_STAGE1: &str = r#"umask 077; f=$(mktemp); cat > "$f"; printf '%s\n' "$f""#;

/// The path printed by stage 1, if its last non-empty line is one absolute
/// path made only of `[A-Za-z0-9._/-]` and free of `..`.
fn validated_remote_prompt_path(printed: &str) -> Option<&str> {
    // The remote shell may print banner text (a `.bashrc` echo, a message of
    // the day) before the path, and stage 1 always prints the path last. The
    // character check below, not the line count, is what keeps the path
    // safe to place in the stage 2 command.
    let path = printed
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .rfind(|line| !line.is_empty())?;
    let valid = path.len() > 1
        && path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
    valid.then_some(path)
}

impl FleetAdmiral {
    pub(super) fn execute_action(&mut self, action: &DirectorAction) -> Result<String> {
        match action.action_type {
            ActionType::StartAgent => self.start_agent(action),
            ActionType::MarkComplete => self.mark_complete(action),
            ActionType::MarkFailed => self.mark_failed(action),
            ActionType::ReassignTask => self.reassign_task(action),
            ActionType::PropagateAuth => self.propagate_auth(action),
            ActionType::StopAgent | ActionType::Report => {
                Ok(format!("Unknown action: {}", action.action_type.as_str()))
            }
        }
    }

    pub(super) fn start_agent(&mut self, action: &DirectorAction) -> Result<String> {
        let Some(task) = action.task.as_ref() else {
            return Ok("ERROR: No task provided".to_string());
        };
        let Some(vm_name) = action.vm_name.as_deref() else {
            return Ok("ERROR: No VM name provided".to_string());
        };
        let session_name = action.session_name.as_deref().unwrap_or("fleet-session");
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;

        if !matches!(
            task.agent_command.as_str(),
            "claude" | "amplifier" | "copilot"
        ) {
            return Ok(format!(
                "ERROR: Invalid agent command: {:?}",
                task.agent_command
            ));
        }
        // Issue #1570: only `auto` has a prompt-file form.
        if task.agent_mode != "auto" {
            return Ok(format!("ERROR: Invalid agent mode: {:?}", task.agent_mode));
        }
        if task.max_turns == 0 || task.max_turns > 1000 {
            return Ok(format!("ERROR: Invalid max_turns: {:?}", task.max_turns));
        }

        // Stage 1: the prompt travels on stdin into a private remote file.
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", START_AGENT_STAGE1]);
        let output = run_output_with_timeout_stdin(
            cmd,
            SUBPROCESS_TIMEOUT,
            Some(task.prompt.as_bytes().to_vec()),
        )?;
        if !output.status.success() {
            return Ok(format!(
                "ERROR: Failed to send prompt to {vm_name}: {}",
                sanitize_external_error_detail(String::from_utf8_lossy(&output.stderr).trim(), 200)
            ));
        }
        let printed = String::from_utf8_lossy(&output.stdout);
        let Some(remote_path) = validated_remote_prompt_path(&printed) else {
            return Ok(format!(
                "ERROR: Invalid remote prompt file path returned by {vm_name}"
            ));
        };

        // Stage 2: only validated values reach the command; the prompt never does.
        // Every value is free of quotes, `$`, backquotes and `\`, so the
        // single quotes inside the double-quoted send-keys argument are safe.
        let session = shell_single_quote(session_name);
        let path = shell_single_quote(remote_path);
        let setup_cmd = format!(
            "tmux new-session -d -s {session} && tmux send-keys -t {session} \
             \"amplihack {} --auto --max-turns {} --prompt-file {path}; rm -f -- {path}\" C-m",
            shell_single_quote(&task.agent_command),
            shell_single_quote(&task.max_turns.to_string()),
        );
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &setup_cmd]);
        let stage2 = run_output_with_timeout_stdin(cmd, SUBPROCESS_TIMEOUT, None);
        if let Ok(output) = &stage2
            && output.status.success()
        {
            self.task_queue
                .mark_task_running(&task.id, vm_name, session_name)?;
            return Ok(format!("Agent started: {session_name} on {vm_name}"));
        }

        self.remove_remote_prompt_file(vm_name, remote_path);
        match stage2 {
            Ok(output) => Ok(format!(
                "ERROR: Failed to start agent: {}",
                sanitize_external_error_detail(String::from_utf8_lossy(&output.stderr).trim(), 200,)
            )),
            Err(error) => Err(error),
        }
    }

    /// One clean-up call after a failed stage 2; a failure is only logged.
    fn remove_remote_prompt_file(&self, vm_name: &str, remote_path: &str) {
        let rm_cmd = format!("rm -f -- {}", shell_single_quote(remote_path));
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &rm_cmd]);
        let removed = run_output_with_timeout_stdin(cmd, SUBPROCESS_TIMEOUT, None)
            .is_ok_and(|output| output.status.success());
        if !removed {
            warn!("Failed to remove remote prompt file {remote_path}");
        }
    }

    pub(super) fn mark_complete(&mut self, action: &DirectorAction) -> Result<String> {
        if let Some(task) = action.task.as_ref() {
            self.task_queue
                .mark_task_complete(&task.id, "Detected as completed by observer")?;
        }
        Ok("Task marked complete".to_string())
    }

    pub(super) fn mark_failed(&mut self, action: &DirectorAction) -> Result<String> {
        if let Some(task) = action.task.as_ref() {
            self.task_queue.mark_task_failed(&task.id, &action.reason)?;
        }
        Ok(format!("Task marked failed: {}", action.reason))
    }

    pub(super) fn reassign_task(&mut self, action: &DirectorAction) -> Result<String> {
        let (Some(task), Some(vm_name), Some(session_name)) = (
            action.task.as_ref(),
            action.vm_name.as_deref(),
            action.session_name.as_deref(),
        ) else {
            return Ok("ERROR: Missing task/vm/session for reassignment".to_string());
        };
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;

        let kill_cmd = format!(
            "tmux kill-session -t {} 2>/dev/null || true",
            shell_single_quote(session_name)
        );
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &kill_cmd]);
        if let Err(e) = run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
            warn!(
                "Failed to kill old session '{}': {}; zombie agent may persist",
                session_name, e
            );
        }

        self.task_queue.requeue_task(&task.id)?;
        Ok("Stuck agent killed, task requeued".to_string())
    }

    pub(super) fn propagate_auth(&mut self, action: &DirectorAction) -> Result<String> {
        let Some(vm_name) = action.vm_name.as_deref() else {
            return Ok("ERROR: No VM specified".to_string());
        };
        let results = self
            .auth
            .propagate_all(vm_name, &["github".into(), "azure".into(), "claude".into()]);
        let success = results.iter().filter(|result| result.success).count();
        Ok(format!(
            "Auth propagated: {success}/{} services",
            results.len()
        ))
    }

    pub(super) fn learn(&mut self, results: &[(DirectorAction, String)]) {
        for (_action, outcome) in results {
            self.stats.actions += 1;
            if outcome.starts_with("ERROR") {
                self.stats.failures += 1;
            } else {
                self.stats.successes += 1;
            }
        }
    }

    pub(super) fn adopt_all_sessions(&mut self) -> Result<usize> {
        self.fleet_state.refresh();
        let adopter = SessionAdopter::new(self.azlin_path.clone());
        let mut total = 0usize;
        for vm in self.fleet_state.managed_vms() {
            if !vm.is_running() {
                continue;
            }
            total += adopter
                .adopt_sessions(&vm.name, &mut self.task_queue, None)?
                .len();
        }
        Ok(total)
    }

    pub(super) fn write_coordination_files(&self) -> Result<()> {
        let mut grouped = BTreeMap::<String, Vec<&FleetTask>>::new();
        for task in self.task_queue.active_tasks() {
            if task.repo_url.is_empty() {
                continue;
            }
            grouped.entry(task.repo_url.clone()).or_default().push(task);
        }

        fs::create_dir_all(&self.coordination_dir)
            .with_context(|| format!("failed to create {}", self.coordination_dir.display()))?;
        for (repo_url, tasks) in grouped {
            if tasks.len() < 2 {
                continue;
            }
            let safe_key = repo_url
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or("coordination")
                .trim_end_matches(".git");
            let path = self.coordination_dir.join(format!("{safe_key}.json"));
            let payload = serde_json::json!({
                "repo": repo_url,
                "active_agents": tasks.iter().map(|task| serde_json::json!({
                    "task_id": task.id,
                    "prompt": task.prompt,
                    "vm": task.assigned_vm,
                    "session": task.assigned_session,
                })).collect::<Vec<_>>(),
                "updated_at": now_isoformat(),
            });
            let bytes = serde_json::to_vec_pretty(&payload)
                .context("failed to serialize coordination file")?;
            let mut temp =
                tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
                    .with_context(|| {
                        format!("failed to create temp file for {}", path.display())
                    })?;
            temp.write_all(&bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
            // SEC-PERM: tempfile guarantees 0o600 on Unix (O_CREAT with mode 0600, unaffected by umask > 0o177)
            temp.persist(&path)
                .map_err(|err| err.error)
                .with_context(|| format!("failed to persist {}", path.display()))?;
        }
        Ok(())
    }
}
