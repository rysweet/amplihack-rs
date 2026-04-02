use super::*;

#[derive(Debug, Clone)]
pub(super) struct FleetObserver {
    pub(super) azlin_path: PathBuf,
    pub(super) capture_lines: usize,
    pub(super) previous_captures: BTreeMap<String, String>,
    pub(super) last_change_time: BTreeMap<String, std::time::Instant>,
    pub(super) stuck_threshold_seconds: f64,
}

impl FleetObserver {
    pub(super) fn new(azlin_path: PathBuf) -> Self {
        Self {
            azlin_path,
            capture_lines: DEFAULT_CAPTURE_LINES,
            previous_captures: BTreeMap::new(),
            last_change_time: BTreeMap::new(),
            stuck_threshold_seconds: DEFAULT_STUCK_THRESHOLD_SECONDS,
        }
    }

    pub(super) fn observe_session(
        &mut self,
        vm_name: &str,
        session_name: &str,
    ) -> Result<ObservationResult> {
        let pane_content = self.capture_pane(vm_name, session_name);
        let Some(pane_content) = pane_content else {
            return Ok(ObservationResult {
                session_name: session_name.to_string(),
                status: AgentStatus::Unknown,
                last_output_lines: Vec::new(),
                confidence: 0.0,
                matched_pattern: String::new(),
            });
        };

        let lines = pane_content
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let (status, confidence, pattern) = self.classify_output(&lines, vm_name, session_name);
        Ok(ObservationResult {
            session_name: session_name.to_string(),
            status,
            last_output_lines: lines,
            confidence,
            matched_pattern: pattern,
        })
    }

    pub(super) fn observe_all(
        &self,
        sessions: &[TmuxSessionInfo],
    ) -> Result<Vec<ObservationResult>> {
        let mut observer = self.clone();
        sessions
            .iter()
            .map(|session| observer.observe_session(&session.vm_name, &session.session_name))
            .collect()
    }

    pub(super) fn capture_pane(&self, vm_name: &str, session_name: &str) -> Option<String> {
        if validate_vm_name(vm_name).is_err() || validate_session_name(session_name).is_err() {
            return None;
        }

        let session_name = shell_single_quote(session_name);
        let command = format!(
            "tmux capture-pane -t {session_name} -p -S -{} 2>/dev/null",
            self.capture_lines
        );
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", &command]);
        match run_output_with_timeout(cmd, TMUX_LIST_TIMEOUT) {
            Ok(output) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).into_owned())
            }
            Ok(_) | Err(_) => None,
        }
    }

    pub(super) fn classify_output(
        &mut self,
        lines: &[String],
        vm_name: &str,
        session_name: &str,
    ) -> (AgentStatus, f64, String) {
        if lines.is_empty() {
            return (AgentStatus::Unknown, 0.0, String::new());
        }

        let combined = lines.join("\n");
        let key = format!("{vm_name}:{session_name}");
        let inferred = infer_agent_status(&combined);

        match inferred {
            AgentStatus::Completed => {
                return (
                    AgentStatus::Completed,
                    CONFIDENCE_COMPLETION,
                    "completion_detected".to_string(),
                );
            }
            AgentStatus::Error => {
                return (
                    AgentStatus::Error,
                    CONFIDENCE_ERROR,
                    "error_detected".to_string(),
                );
            }
            AgentStatus::WaitingInput => {
                return (
                    AgentStatus::WaitingInput,
                    CONFIDENCE_RUNNING,
                    "waiting_input_detected".to_string(),
                );
            }
            AgentStatus::Thinking => {
                self.last_change_time
                    .insert(key.clone(), std::time::Instant::now());
                self.previous_captures.insert(key, combined.clone());
                return (
                    AgentStatus::Thinking,
                    CONFIDENCE_THINKING,
                    "thinking_detected".to_string(),
                );
            }
            AgentStatus::Idle => {
                self.previous_captures.insert(key, combined);
                return (
                    AgentStatus::Idle,
                    CONFIDENCE_IDLE,
                    "idle_detected".to_string(),
                );
            }
            AgentStatus::Shell => {
                self.previous_captures.insert(key, combined);
                return (
                    AgentStatus::Shell,
                    CONFIDENCE_ERROR,
                    "shell_prompt".to_string(),
                );
            }
            AgentStatus::Unknown
            | AgentStatus::NoSession
            | AgentStatus::Unreachable
            | AgentStatus::Running
            | AgentStatus::Stuck => {}
        }

        if let Some(pattern) = first_matching_pattern(COMPLETION_PATTERNS, &combined, false) {
            return (AgentStatus::Completed, CONFIDENCE_COMPLETION, pattern);
        }
        if let Some(pattern) = first_matching_pattern(ERROR_PATTERNS, &combined, false) {
            return (AgentStatus::Error, CONFIDENCE_ERROR, pattern);
        }
        if let Some(pattern) = first_matching_pattern(RUNNING_PATTERNS, &combined, false) {
            self.last_change_time
                .insert(key.clone(), std::time::Instant::now());
            self.previous_captures.insert(key, combined);
            return (AgentStatus::Running, CONFIDENCE_RUNNING, pattern);
        }
        if let Some(pattern) = first_matching_pattern(WAITING_PATTERNS, &combined, true) {
            return (AgentStatus::WaitingInput, CONFIDENCE_RUNNING, pattern);
        }

        let now = std::time::Instant::now();
        if let Some(previous) = self.previous_captures.get(&key) {
            if previous == &combined {
                let last_change = self.last_change_time.get(&key).copied().unwrap_or(now);
                if now.duration_since(last_change).as_secs_f64() > self.stuck_threshold_seconds {
                    self.previous_captures.insert(key, combined);
                    return (
                        AgentStatus::Stuck,
                        CONFIDENCE_RUNNING,
                        "no_output_change".to_string(),
                    );
                }
            } else {
                self.last_change_time.insert(key.clone(), now);
            }
        } else {
            self.last_change_time.insert(key.clone(), now);
        }
        self.previous_captures.insert(key, combined.clone());

        let last_line = lines.last().map(String::as_str).unwrap_or("");
        if let Some(pattern) = first_matching_pattern(IDLE_PATTERNS, last_line, false) {
            return (AgentStatus::Idle, CONFIDENCE_IDLE, pattern);
        }

        if combined.trim().len() > MIN_SUBSTANTIAL_OUTPUT_LEN {
            return (
                AgentStatus::Running,
                CONFIDENCE_DEFAULT_RUNNING,
                "has_output".to_string(),
            );
        }

        (AgentStatus::Unknown, CONFIDENCE_UNKNOWN, String::new())
    }
}
