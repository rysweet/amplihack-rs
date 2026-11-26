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
                "\nSession transcript (most recent messages, bounded):\n{}",
                bounded_transcript(&self.transcript_summary)
            ));
        }
        parts.push("\nCurrent terminal output, recent scrollback (bounded):".to_string());
        parts.push(if self.tmux_capture.is_empty() {
            "(empty)".to_string()
        } else {
            bounded_scrollback(&self.tmux_capture)
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

/// Issue #1482: where the Claude reasoner gets its root-sandbox decision;
/// [`amplihack_utils::root_sandbox::detect`] outside tests.
pub(super) type RootSandboxDecision = fn() -> amplihack_utils::root_sandbox::SkipPermissionsEnv;

#[derive(Debug, Clone)]
pub(super) enum NativeReasonerBackend {
    None,
    /// The heuristic reasoner, chosen because the run is on the named CLI and
    /// the only native backend starts `claude` (issue #1983).
    StaysOn(String),
    Claude(PathBuf, RootSandboxDecision),
}

impl NativeReasonerBackend {
    /// `requested` is the backend name; `auto` follows the run's CLI.
    pub(super) fn detect(requested: &str) -> Result<Self> {
        let run = crate::env_builder::active_agent_binary_resolution();
        let observed = (!run.source.is_inferred()).then_some(run.binary.as_str());
        Self::detect_for(requested, observed)
    }

    /// [`Self::detect`] with the run's CLI passed in: `observed` is the binary
    /// the resolver named when it was observed (an explicit
    /// `AMPLIHACK_AGENT_BINARY`, the configured `agent.binary`, or a live
    /// session marker) and `None` when it was inferred from a launcher
    /// context file or the built-in default.
    ///
    /// Issue #1983: a run stays on the CLI it started with. The only native
    /// backend starts `claude`, so under `auto` a run observed on another CLI
    /// gets the heuristic reasoner rather than a `claude` it did not ask for.
    /// Naming `claude` (or `anthropic`) is the explicit request that starts
    /// `claude` anyway. When nothing observed a CLI, `auto` keeps using
    /// `claude` when it is installed, as before.
    pub(super) fn detect_for(requested: &str, observed: Option<&str>) -> Result<Self> {
        match requested {
            "auto" => match observed {
                Some(cli) if cli != "claude" => Ok(NativeReasonerBackend::StaysOn(cli.to_string())),
                _ => Ok(Self::claude_if_installed()),
            },
            "anthropic" | "claude" => Ok(Self::claude_if_installed()),
            "copilot" | "litellm" => bail!(
                "native fleet reasoner backend `{requested}` is not implemented yet; use the default Claude backend"
            ),
            other => bail!("unknown fleet reasoner backend: {other}"),
        }
    }

    fn claude_if_installed() -> Self {
        find_reasoner_binary()
            .map(|path| NativeReasonerBackend::Claude(path, amplihack_utils::root_sandbox::detect))
            .unwrap_or(NativeReasonerBackend::None)
    }

    pub(super) fn label(&self) -> &'static str {
        match self {
            NativeReasonerBackend::None | NativeReasonerBackend::StaysOn(_) => "heuristic",
            NativeReasonerBackend::Claude(..) => "claude",
        }
    }

    /// The line the caller shows once where the user sees it (stderr for the
    /// fleet commands, the notice panel in the TUI): an automatic
    /// `IS_SANDBOX=1` for this backend's `claude` (issue #1482), or why the
    /// heuristic reasoner runs instead of `claude` (issue #1983).
    pub(super) fn root_sandbox_notice(&self) -> Option<String> {
        match self {
            NativeReasonerBackend::None => None,
            NativeReasonerBackend::StaysOn(cli) => Some(format!(
                "fleet reasoner: this run is on {cli} and the native reasoner only starts \
                 claude, so the heuristic reasoner is used; run `amplihack fleet dry-run \
                 --backend claude` to ask for claude explicitly"
            )),
            NativeReasonerBackend::Claude(_, decision) => decision().notice(),
        }
    }

    /// Test-only: production calls the `_with` form directly (issue #1765).
    #[cfg(test)]
    pub(super) fn complete(&self, prompt: &str) -> Result<String> {
        self.complete_with(prompt, &|_| {})
    }

    /// `complete` with `configure` applied to the `claude` child after
    /// its environment is built and before it starts, so a test can give the
    /// child its own environment without changing the test process's
    /// (issue #1765).
    pub(super) fn complete_with(
        &self,
        prompt: &str,
        configure: &dyn Fn(&mut Command),
    ) -> Result<String> {
        match self {
            NativeReasonerBackend::None => {
                bail!("no native reasoner backend available")
            }
            NativeReasonerBackend::StaysOn(cli) => {
                bail!(
                    "this run is on {cli} and no native reasoner starts {cli}; claude is not started"
                )
            }
            NativeReasonerBackend::Claude(path, decision) => {
                let mut cmd = reasoner_command(path, &decision())?;
                // This starts a Claude Code session, so the child is told so
                // the way every launcher tells its child: tagged
                // `session:claude`, with every other CLI's markers removed. A
                // reasoner run from a Copilot session used to hand `claude`
                // an untagged instruction beside the inherited `COPILOT_CLI`
                // (crusty review of #1490 at 960eaacb).
                let mut env_builder = EnvBuilder::new()
                    .with_amplihack_session_id()
                    .with_session_tree_context()
                    .with_amplihack_vars()
                    .with_launched_agent_binary("claude")
                    .with_amplihack_home()
                    .with_asset_resolver()
                    .set("AMPLIHACK_NONINTERACTIVE", "1");
                if let Ok(current_dir) = env::current_dir() {
                    env_builder = env_builder.with_project_graph_db(&current_dir)?;
                }
                env_builder.apply_to_command(&mut cmd);
                configure(&mut cmd);
                // Issue #1570: the prompt goes to `claude -p` on stdin, never argv.
                let output = run_output_with_timeout_stdin(
                    cmd,
                    SCOUT_REASONER_TIMEOUT,
                    Some(prompt.as_bytes().to_vec()),
                )?;
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

/// The reasoner's `claude` invocation with the root-sandbox `decision` applied
/// (issue #1482): `IS_SANDBOX=1` on the child when amplihack enables it, or an
/// error when Claude Code would refuse the flag. The notice is shown once by
/// the caller through [`NativeReasonerBackend::root_sandbox_notice`], not per
/// call. The prompt is not part of the argv: [`NativeReasonerBackend::complete`]
/// writes it to the child's stdin (issue #1570).
pub(super) fn reasoner_command(
    path: &Path,
    decision: &amplihack_utils::root_sandbox::SkipPermissionsEnv,
) -> Result<Command> {
    let mut cmd = Command::new(path);
    cmd.args([amplihack_utils::root_sandbox::SKIP_PERMISSIONS_FLAG, "-p"]);
    decision.apply_quietly(&mut cmd)?;
    Ok(cmd)
}
