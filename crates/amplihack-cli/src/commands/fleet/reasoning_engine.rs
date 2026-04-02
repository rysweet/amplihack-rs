use super::*;

#[derive(Debug, Clone)]
pub(super) struct FleetSessionReasoner {
    pub(super) azlin_path: PathBuf,
    pub(super) backend: NativeReasonerBackend,
    pub(super) decisions: Vec<SessionDecision>,
}

impl FleetSessionReasoner {
    pub(super) fn new(azlin_path: PathBuf, backend: NativeReasonerBackend) -> Self {
        Self {
            azlin_path,
            backend,
            decisions: Vec::new(),
        }
    }

    pub(super) fn backend_label(&self) -> &'static str {
        self.backend.label()
    }

    pub(super) fn reason_about_session(
        &mut self,
        vm_name: &str,
        session_name: &str,
        task_prompt: &str,
        project_priorities: &str,
        cached_tmux_capture: Option<&str>,
    ) -> Result<SessionAnalysis> {
        let context = gather_session_context(
            &self.azlin_path,
            vm_name,
            session_name,
            task_prompt,
            project_priorities,
            cached_tmux_capture,
        )?;
        let (decision, diagnostic) = self.reason(&context);
        self.decisions.push(decision.clone());
        Ok(SessionAnalysis {
            context,
            decision,
            diagnostic,
        })
    }

    pub(super) fn reason(&self, context: &SessionContext) -> (SessionDecision, Option<String>) {
        if context.agent_status == AgentStatus::Thinking {
            return (
                SessionDecision {
                    session_name: context.session_name.clone(),
                    vm_name: context.vm_name.clone(),
                    action: SessionAction::Wait,
                    input_text: String::new(),
                    reasoning: "Agent is actively thinking/processing -- do not interrupt"
                        .to_string(),
                    confidence: 1.0,
                },
                None,
            );
        }
        if context.appears_dead()
            || matches!(
                context.agent_status,
                AgentStatus::Unknown | AgentStatus::NoSession | AgentStatus::Unreachable
            )
        {
            return (
                SessionDecision {
                    session_name: context.session_name.clone(),
                    vm_name: context.vm_name.clone(),
                    action: SessionAction::Wait,
                    input_text: String::new(),
                    reasoning: "Session is empty or unreachable; no intervention taken".to_string(),
                    confidence: CONFIDENCE_UNKNOWN,
                },
                None,
            );
        }
        if context.agent_status == AgentStatus::Completed {
            return (
                SessionDecision {
                    session_name: context.session_name.clone(),
                    vm_name: context.vm_name.clone(),
                    action: SessionAction::MarkComplete,
                    input_text: String::new(),
                    reasoning: "Session output indicates completion".to_string(),
                    confidence: CONFIDENCE_COMPLETION,
                },
                None,
            );
        }

        let prompt = format!(
            "{}\n\n{}\n\nRespond with JSON only.",
            SESSION_REASONER_SYSTEM_PROMPT,
            context.to_prompt_context()
        );
        match self.backend.complete(&prompt) {
            Ok(response_text) => {
                if let Some(decision) = parse_reasoner_response(&response_text, context) {
                    return (decision, None);
                }
                (
                    heuristic_decision(context),
                    Some(format!(
                        "Native {} reasoner returned an invalid response; showing a heuristic proposal instead.",
                        self.backend.label()
                    )),
                )
            }
            Err(error) => (
                heuristic_decision(context),
                Some(format!(
                    "Native {} reasoner failed: {}. Showing a heuristic proposal instead.",
                    self.backend.label(),
                    error
                )),
            ),
        }
    }

    pub(super) fn execute_decision(&self, decision: &SessionDecision) -> Result<()> {
        validate_vm_name(&decision.vm_name)?;
        validate_session_name(&decision.session_name)?;

        match decision.action {
            SessionAction::SendInput => {
                if decision.confidence < MIN_CONFIDENCE_SEND {
                    bail!(
                        "send_input suppressed because confidence {:.2} is below {:.2}",
                        decision.confidence,
                        MIN_CONFIDENCE_SEND
                    );
                }
                if decision.input_text.is_empty() {
                    bail!("send_input requires non-empty input_text");
                }
                if is_dangerous_input(&decision.input_text) {
                    bail!("send_input blocked because it matched the dangerous-input policy");
                }
                let safe_session = shell_single_quote(&decision.session_name);
                for line in decision.input_text.split('\n') {
                    let command = format!(
                        "tmux send-keys -t {safe_session} {} Enter",
                        shell_single_quote(line)
                    );
                    let mut cmd = Command::new(&self.azlin_path);
                    cmd.args([
                        "connect",
                        &decision.vm_name,
                        "--no-tmux",
                        "--yes",
                        "--",
                        &command,
                    ]);
                    let output = run_output_with_timeout(cmd, Duration::from_secs(30))?;
                    if !output.status.success() {
                        bail!(
                            "send_input failed: {}",
                            truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                        );
                    }
                }
                Ok(())
            }
            SessionAction::Restart => {
                if decision.confidence < MIN_CONFIDENCE_RESTART {
                    bail!(
                        "restart suppressed because confidence {:.2} is below {:.2}",
                        decision.confidence,
                        MIN_CONFIDENCE_RESTART
                    );
                }
                let command = format!(
                    "tmux send-keys -t {} C-c C-c",
                    shell_single_quote(&decision.session_name)
                );
                let mut cmd = Command::new(&self.azlin_path);
                cmd.args([
                    "connect",
                    &decision.vm_name,
                    "--no-tmux",
                    "--yes",
                    "--",
                    &command,
                ]);
                let output = run_output_with_timeout(cmd, Duration::from_secs(30))?;
                if !output.status.success() {
                    bail!(
                        "restart failed: {}",
                        truncate_chars(String::from_utf8_lossy(&output.stderr).trim(), 200)
                    );
                }
                Ok(())
            }
            SessionAction::Wait | SessionAction::Escalate | SessionAction::MarkComplete => Ok(()),
        }
    }

    pub(super) fn dry_run_report(&self) -> String {
        let mut counts = BTreeMap::<String, usize>::new();
        for decision in &self.decisions {
            *counts
                .entry(decision.action.as_str().to_string())
                .or_insert(0) += 1;
        }

        let mut lines = vec![
            format!(
                "Fleet Admiral Dry Run -- {} sessions analyzed",
                self.decisions.len()
            ),
            String::new(),
            "Summary:".to_string(),
        ];
        for (action, count) in counts {
            lines.push(format!("  {action}: {count}"));
        }
        lines.push(String::new());
        for decision in &self.decisions {
            lines.push(decision.summary());
            lines.push(String::new());
        }
        lines.join("\n")
    }
}
