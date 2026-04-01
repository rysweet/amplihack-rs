use super::*;

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(super) struct AdoptedSession {
    pub(super) vm_name: String,
    pub(super) session_name: String,
    pub(super) inferred_repo: String,
    pub(super) inferred_branch: String,
    pub(super) inferred_task: String,
    pub(super) inferred_pr: String,
    pub(super) working_directory: String,
    pub(super) agent_type: String,
    pub(super) adopted_at: Option<String>,
    pub(super) task_id: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct SessionAdopter {
    pub(super) azlin_path: PathBuf,
}

impl SessionAdopter {
    pub(super) fn new(azlin_path: PathBuf) -> Self {
        Self { azlin_path }
    }

    pub(super) fn discover_sessions(&self, vm_name: &str) -> Vec<AdoptedSession> {
        let discover_command = self.build_discover_command();
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args([
            "connect",
            vm_name,
            "--no-tmux",
            "--yes",
            "--",
            &discover_command,
        ]);

        match run_output_with_timeout(cmd, SUBPROCESS_TIMEOUT) {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains("===SESSION:") || stdout.contains("===DONE===") {
                    return self.parse_discovery_output(vm_name, &stdout);
                }
                if !output.status.success() {
                    return Vec::new();
                }
                self.parse_discovery_output(vm_name, &stdout)
            }
            Err(_) => Vec::new(),
        }
    }

    pub(super) fn adopt_sessions<B: FleetAdoptionBackend>(
        &self,
        vm_name: &str,
        backend: &mut B,
        sessions: Option<&[String]>,
    ) -> Result<Vec<AdoptedSession>> {
        let discovered = self.discover_sessions(vm_name);
        self.adopt_discovered_sessions(vm_name, discovered, backend, sessions)
    }

    pub(super) fn adopt_discovered_sessions<B: FleetAdoptionBackend>(
        &self,
        vm_name: &str,
        mut discovered: Vec<AdoptedSession>,
        backend: &mut B,
        sessions: Option<&[String]>,
    ) -> Result<Vec<AdoptedSession>> {
        if let Some(session_names) = sessions {
            discovered.retain(|session| {
                session_names
                    .iter()
                    .any(|name| name == &session.session_name)
            });
        }

        let mut adopted = Vec::new();
        for mut session in discovered {
            backend.record_adopted_session(vm_name, &mut session)?;
            adopted.push(session);
        }

        Ok(adopted)
    }

    pub(super) fn build_discover_command(&self) -> String {
        [
            r##"for session in $(tmux list-sessions -F "#{session_name}" 2>/dev/null); do "##,
            r#"echo "===SESSION:$session==="; "#,
            r##"CWD=$(tmux display-message -t "$session" -p "#{pane_current_path}" 2>/dev/null); "##,
            r#"echo "CWD:$CWD"; "#,
            r##"CMD=$(tmux display-message -t "$session" -p "#{pane_current_command}" 2>/dev/null); "##,
            r#"echo "CMD:$CMD"; "#,
            r#"if [ -n "$CWD" ] && [ -d "$CWD/.git" ]; then "#,
            r#"BRANCH=$(cd "$CWD" && git branch --show-current 2>/dev/null); "#,
            r#"REMOTE=$(cd "$CWD" && git remote get-url origin 2>/dev/null); "#,
            r#"echo "BRANCH:$BRANCH"; "#,
            r#"echo "REPO:$REMOTE"; "#,
            r#"fi; "#,
            r#"echo "PANE_START"; "#,
            r#"tmux capture-pane -t "$session" -p -S -5 2>/dev/null | tail -5; "#,
            r#"echo "PANE_END"; "#,
            r#"done; "#,
            r#"echo "===DONE===""#,
        ]
        .concat()
    }

    pub(super) fn parse_discovery_output(&self, vm_name: &str, output: &str) -> Vec<AdoptedSession> {
        let mut sessions = Vec::new();
        let mut current: Option<AdoptedSession> = None;

        for raw_line in output.lines() {
            let line = raw_line.trim();

            if line.starts_with("===SESSION:") && line.ends_with("===") {
                if let Some(session) = current.take() {
                    sessions.push(session);
                }
                let session_name = &line["===SESSION:".len()..line.len() - "===".len()];
                if session_name.is_empty() || session_name.starts_with('(') {
                    continue;
                }
                if validate_session_name(session_name).is_err() {
                    continue;
                }
                current = Some(AdoptedSession {
                    vm_name: vm_name.to_string(),
                    session_name: session_name.to_string(),
                    ..Default::default()
                });
                continue;
            }

            let Some(session) = current.as_mut() else {
                continue;
            };

            if let Some(value) = line.strip_prefix("CWD:") {
                session.working_directory = value.to_string();
            } else if let Some(value) = line.strip_prefix("CMD:") {
                let command = value.to_ascii_lowercase();
                if command.contains("claude") || command.contains("node") {
                    session.agent_type = "claude".to_string();
                } else if command.contains("amplifier") {
                    session.agent_type = "amplifier".to_string();
                } else if command.contains("copilot") {
                    session.agent_type = "copilot".to_string();
                }
            } else if let Some(value) = line.strip_prefix("BRANCH:") {
                session.inferred_branch = value.to_string();
            } else if let Some(value) = line.strip_prefix("REPO:") {
                session.inferred_repo = value.to_string();
            } else if let Some(value) = line.strip_prefix("PR:") {
                session.inferred_pr = value.to_string();
            } else if let Some(value) = line.strip_prefix("LAST_MSG:")
                && session.inferred_task.is_empty()
            {
                session.inferred_task = value.to_string();
            }
        }

        if let Some(session) = current {
            sessions.push(session);
        }

        sessions
    }
}

