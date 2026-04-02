use super::*;

#[derive(Debug, Clone)]
pub(super) struct DryRunSession {
    pub(super) vm_name: String,
    pub(super) session_name: String,
}

#[derive(Debug, Clone)]
pub(super) struct ScoutDiscovery {
    pub(super) all_vm_count: usize,
    pub(super) running_vm_count: usize,
    pub(super) sessions: Vec<DiscoveredSession>,
}

#[derive(Debug, Clone)]
pub(super) struct DiscoveredSession {
    pub(super) vm_name: String,
    pub(super) session_name: String,
    pub(super) status: AgentStatus,
    pub(super) cached_tmux_capture: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SessionAction {
    SendInput,
    Wait,
    Escalate,
    MarkComplete,
    Restart,
}

impl SessionAction {
    pub(super) fn all() -> [Self; 5] {
        [
            SessionAction::SendInput,
            SessionAction::Wait,
            SessionAction::Escalate,
            SessionAction::MarkComplete,
            SessionAction::Restart,
        ]
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            SessionAction::SendInput => "send_input",
            SessionAction::Wait => "wait",
            SessionAction::Escalate => "escalate",
            SessionAction::MarkComplete => "mark_complete",
            SessionAction::Restart => "restart",
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            SessionAction::SendInput => SessionAction::Wait,
            SessionAction::Wait => SessionAction::Escalate,
            SessionAction::Escalate => SessionAction::MarkComplete,
            SessionAction::MarkComplete => SessionAction::Restart,
            SessionAction::Restart => SessionAction::SendInput,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SessionDecision {
    pub(super) session_name: String,
    pub(super) vm_name: String,
    pub(super) action: SessionAction,
    #[serde(default)]
    pub(super) input_text: String,
    #[serde(default)]
    pub(super) reasoning: String,
    pub(super) confidence: f64,
}

impl SessionDecision {
    pub(super) fn summary(&self) -> String {
        let mut lines = vec![
            format!("  Session: {}/{}", self.vm_name, self.session_name),
            format!("  Action: {}", self.action.as_str()),
            format!("  Confidence: {:.0}%", self.confidence * 100.0),
            format!("  Reasoning: {}", self.reasoning),
        ];
        if !self.input_text.is_empty() {
            lines.push(format!(
                "  Input: \"{}\"",
                truncate_chars(&self.input_text.replace('\n', "\\n"), 100)
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone)]
pub(super) struct SessionContext {
    pub(super) vm_name: String,
    pub(super) session_name: String,
    pub(super) tmux_capture: String,
    pub(super) transcript_summary: String,
    pub(super) working_directory: String,
    pub(super) git_branch: String,
    pub(super) repo_url: String,
    pub(super) agent_status: AgentStatus,
    pub(super) files_modified: Vec<String>,
    pub(super) pr_url: String,
    pub(super) task_prompt: String,
    pub(super) project_priorities: String,
    pub(super) health_summary: String,
    pub(super) project_name: String,
    pub(super) project_objectives: Vec<ProjectObjective>,
}

impl SessionContext {
    pub(super) fn new(
        vm_name: &str,
        session_name: &str,
        task_prompt: &str,
        project_priorities: &str,
    ) -> Result<Self> {
        validate_vm_name(vm_name)?;
        validate_session_name(session_name)?;
        Ok(Self {
            vm_name: vm_name.to_string(),
            session_name: session_name.to_string(),
            tmux_capture: String::new(),
            transcript_summary: String::new(),
            working_directory: String::new(),
            git_branch: String::new(),
            repo_url: String::new(),
            agent_status: AgentStatus::Unknown,
            files_modified: Vec::new(),
            pr_url: String::new(),
            task_prompt: task_prompt.to_string(),
            project_priorities: project_priorities.to_string(),
            health_summary: String::new(),
            project_name: String::new(),
            project_objectives: Vec::new(),
        })
    }

    pub(super) fn to_prompt_context(&self) -> String {
        let mut parts = vec![
            format!("VM: {}, Session: {}", self.vm_name, self.session_name),
            format!("Status: {}", self.agent_status.as_str()),
        ];
        if !self.repo_url.is_empty() {
            parts.push(format!("Repo: {}", self.repo_url));
        }
        if !self.git_branch.is_empty() {
            parts.push(format!("Branch: {}", self.git_branch));
        }
        if !self.task_prompt.is_empty() {
            parts.push(format!("Original task: {}", self.task_prompt));
        }
        if !self.pr_url.is_empty() {
            parts.push(format!("PR: {}", self.pr_url));
        }
        if !self.files_modified.is_empty() {
            parts.push(format!(
                "Files modified: {}",
                self.files_modified.join(", ")
            ));
        }
        if !self.transcript_summary.is_empty() {
            parts.push(format!(
                "\nSession transcript (early + recent messages):\n{}",
                self.transcript_summary
            ));
        }
        parts.push("\nCurrent terminal output (full scrollback):".to_string());
        parts.push(if self.tmux_capture.is_empty() {
            "(empty)".to_string()
        } else {
            self.tmux_capture.clone()
        });
        if !self.health_summary.is_empty() {
            parts.push(format!("\nVM health: {}", self.health_summary));
        }
        if !self.project_name.is_empty() {
            parts.push(format!("\nProject: {}", self.project_name));
            let open = self
                .project_objectives
                .iter()
                .filter(|objective| objective.state == "open")
                .collect::<Vec<_>>();
            if !open.is_empty() {
                parts.push("Open objectives:".to_string());
                for objective in open {
                    parts.push(format!("  - #{}: {}", objective.number, objective.title));
                }
            }
        }
        if !self.project_priorities.is_empty() {
            parts.push(format!("\nProject priorities: {}", self.project_priorities));
        }
        parts.join("\n")
    }

    pub(super) fn appears_dead(&self) -> bool {
        self.tmux_capture.trim().is_empty() && self.transcript_summary.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
pub(super) struct SessionAnalysis {
    pub(super) context: SessionContext,
    pub(super) decision: SessionDecision,
    pub(super) diagnostic: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) enum NativeReasonerBackend {
    None,
    Claude(PathBuf),
}

impl NativeReasonerBackend {
    pub(super) fn detect(requested: &str) -> Result<Self> {
        match requested {
            "auto" | "anthropic" | "claude" => Ok(find_reasoner_binary()
                .map(NativeReasonerBackend::Claude)
                .unwrap_or(NativeReasonerBackend::None)),
            "copilot" | "litellm" => bail!(
                "native fleet reasoner backend `{requested}` is not implemented yet; use the default Claude backend"
            ),
            other => bail!("unknown fleet reasoner backend: {other}"),
        }
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            NativeReasonerBackend::None => "heuristic",
            NativeReasonerBackend::Claude(_) => "claude",
        }
    }

    pub(super) fn complete(&self, prompt: &str) -> Result<String> {
        match self {
            NativeReasonerBackend::None => {
                bail!("no native reasoner backend available")
            }
            NativeReasonerBackend::Claude(path) => {
                let mut cmd = Command::new(path);
                cmd.stdin(Stdio::null());
                cmd.args(["--dangerously-skip-permissions", "-p", prompt]);
                let mut env_builder = EnvBuilder::new()
                    .with_amplihack_session_id()
                    .with_session_tree_context()
                    .with_amplihack_vars()
                    .with_agent_binary("claude")
                    .with_amplihack_home()
                    .with_asset_resolver()
                    .set("AMPLIHACK_NONINTERACTIVE", "1");
                if let Ok(current_dir) = env::current_dir() {
                    env_builder = env_builder.with_project_graph_db(&current_dir)?;
                }
                env_builder.apply_to_command(&mut cmd);
                let output = run_output_with_timeout(cmd, SCOUT_REASONER_TIMEOUT)?;
                if !output.status.success() {
                    bail!(
                        "reasoner command failed: {}",
                        truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                    );
                }
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            }
        }
    }
}
