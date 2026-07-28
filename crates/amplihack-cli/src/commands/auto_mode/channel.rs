//! `AutoModeChannel` — the [`Channel`] half of the auto-mode split.
//!
//! It owns the full phase state machine that the old `AutoModeSession::run()`
//! expressed as a hand-rolled loop:
//!
//! ```text
//!   Clarify (turn 1, required)
//!     -> Plan (turn 2, required)
//!       -> for turn in 3..=max_turns:
//!            Execute -> Evaluate -> [Adjust (required)]
//!         -> stopped
//! ```
//!
//! [`Channel::next_prompt`] emits the prompt for the *current* phase (or
//! `Closed` once terminal); [`Channel::publish_output`] is where the state
//! machine actually advances — it inspects the [`TurnOutput`]'s `exit_code()`
//! and `text()`, runs completion detection/verification, ingests appended
//! instructions, logs, and sets the terminal status + exit code. Because the
//! executor is synchronous and serial, this channel NEVER returns
//! [`NextPrompt::Idle`].

use super::helpers::philosophy_context;
use super::*;
use amplihack_turn::{Channel, ChannelError, ChannelId, ChannelResult, NextPrompt, TurnOutput};
use async_trait::async_trait;

/// Which phase prompt [`AutoModeChannel::next_prompt`] will emit next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Clarify,
    Plan,
    Execute,
    Evaluate,
    Adjust,
    Closed,
}

pub(super) struct AutoModeChannel {
    id: String,
    tool: AutoModeTool,
    prompt: String,
    max_turns: u32,
    log_dir: PathBuf,
    append_dir: PathBuf,
    appended_dir: PathBuf,
    log_path: PathBuf,
    state: Arc<AutoModeState>,
    ui_active: Option<Arc<AtomicBool>>,
    summary_generator: WorkSummaryGenerator,
    completion_detector: CompletionSignalDetector,
    completion_verifier: CompletionVerifier,
    // --- state machine ---
    phase: Phase,
    turn: u32,
    objective: String,
    plan: String,
    last_evaluation: String,
    exit_code: i32,
    abort: Option<String>,
}

impl AutoModeChannel {
    pub(super) fn new(
        tool: AutoModeTool,
        prompt: String,
        max_turns: u32,
        execution_dir: PathBuf,
        project_dir: PathBuf,
        ui_active: Option<Arc<AtomicBool>>,
    ) -> Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let log_dir = execution_dir
            .join(".claude")
            .join("runtime")
            .join("logs")
            .join(format!("auto_{}_{}", tool.slug(), timestamp));
        let append_dir = log_dir.join("append");
        let appended_dir = log_dir.join("appended");
        fs::create_dir_all(&append_dir)?;
        fs::create_dir_all(&appended_dir)?;
        let log_path = log_dir.join("auto_mode.log");
        let session_id = log_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("auto_{}_session", tool.slug()));
        let state = Arc::new(AutoModeState::new(
            session_id.clone(),
            max_turns,
            prompt.clone(),
        ));
        let mut channel = Self {
            id: session_id,
            tool,
            prompt,
            max_turns,
            log_dir,
            append_dir,
            appended_dir,
            log_path,
            state,
            ui_active,
            summary_generator: WorkSummaryGenerator::new(project_dir),
            completion_detector: CompletionSignalDetector::default(),
            completion_verifier: CompletionVerifier::default(),
            phase: Phase::Clarify,
            turn: 0,
            objective: String::new(),
            plan: String::new(),
            last_evaluation: String::new(),
            exit_code: 0,
            abort: None,
        };
        channel.write_prompt_file()?;
        channel.log("Starting native auto mode")?;
        channel.log(&format!("Prompt: {}", channel.prompt.clone()))?;
        Ok(channel)
    }

    /// The shared, observable session state (status / turn / logs).
    pub(super) fn state(&self) -> &Arc<AutoModeState> {
        &self.state
    }

    /// The original user prompt (used to start the optional UI handle).
    pub(super) fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The terminal process exit code once the loop has closed. Mirrors the old
    /// `run()` return value: 0 for completed/stopped, the evaluation code on a
    /// non-zero Evaluate turn.
    pub(super) fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// The abort reason if a *required* turn (Clarify / Plan / Adjust) failed —
    /// the exact message the old code passed to `bail!`. `None` for every clean
    /// path (completed, stopped, or a non-zero Evaluate that exits cleanly).
    pub(super) fn abort(&self) -> Option<&str> {
        self.abort.as_deref()
    }

    /// The append-queue directory (appended instructions are read from here).
    #[cfg(test)]
    pub(super) fn append_dir(&self) -> &Path {
        &self.append_dir
    }

    /// The archive directory (ingested instructions are moved here).
    #[cfg(test)]
    pub(super) fn appended_dir(&self) -> &Path {
        &self.appended_dir
    }

    /// Emit the prompt for the current phase, logging the turn header exactly as
    /// the old hand-rolled loop did just before running each step.
    fn emit_next(&mut self) -> Result<NextPrompt> {
        match self.phase {
            Phase::Clarify => {
                self.state.update_turn(1);
                self.log(&format!(
                    "--- Turn 1/{max} Clarify Objective ---",
                    max = self.max_turns
                ))?;
                Ok(NextPrompt::Ready(self.build_clarify_prompt()))
            }
            Phase::Plan => {
                self.state.update_turn(2);
                self.log(&format!(
                    "--- Turn 2/{max} Create Plan ---",
                    max = self.max_turns
                ))?;
                let objective = self.objective.clone();
                Ok(NextPrompt::Ready(self.build_plan_prompt(&objective)))
            }
            Phase::Execute => {
                let turn = self.turn;
                self.state.update_turn(turn);
                self.log(&format!(
                    "--- Turn {turn}/{max} Execute ---",
                    max = self.max_turns
                ))?;
                let new_instructions =
                    process_appended_instructions(&self.append_dir, &self.appended_dir)
                        .context("failed processing appended instructions")?;
                let objective = self.objective.clone();
                let plan = self.plan.clone();
                Ok(NextPrompt::Ready(self.build_execute_prompt(
                    &objective,
                    &plan,
                    turn,
                    &new_instructions,
                )))
            }
            Phase::Evaluate => {
                let turn = self.turn;
                self.log(&format!(
                    "--- Turn {turn}/{max} Evaluate ---",
                    max = self.max_turns
                ))?;
                let objective = self.objective.clone();
                Ok(NextPrompt::Ready(
                    self.build_evaluation_prompt(&objective, turn)?,
                ))
            }
            Phase::Adjust => {
                let turn = self.turn;
                self.state.update_turn(turn);
                self.log(&format!(
                    "--- Turn {turn}/{max} Adjust Plan ---",
                    max = self.max_turns
                ))?;
                let objective = self.objective.clone();
                let plan = self.plan.clone();
                let evaluation = self.last_evaluation.clone();
                Ok(NextPrompt::Ready(self.build_plan_adjustment_prompt(
                    &objective,
                    &plan,
                    &evaluation,
                )))
            }
            Phase::Closed => Ok(NextPrompt::Closed),
        }
    }

    /// Advance the state machine from the just-completed turn's output.
    fn ingest(&mut self, out: &TurnOutput) -> Result<()> {
        let exit_code = out.exit_code().unwrap_or(0);
        let text = out.text();
        match self.phase {
            Phase::Clarify => {
                self.log_command_result("Clarify Objective", text, exit_code)?;
                if exit_code != 0 {
                    self.fail_required("Clarify Objective", exit_code);
                } else {
                    self.objective = text.to_string();
                    self.phase = Phase::Plan;
                }
            }
            Phase::Plan => {
                self.log_command_result("Create Plan", text, exit_code)?;
                if exit_code != 0 {
                    self.fail_required("Create Plan", exit_code);
                } else {
                    self.plan = text.to_string();
                    self.turn = 3;
                    self.enter_execute_or_stop()?;
                }
            }
            Phase::Execute => {
                self.log_command_result("execute", text, exit_code)?;
                if exit_code != 0 {
                    self.log(&format!(
                        "Warning: execute step returned exit code {exit_code}"
                    ))?;
                }
                self.phase = Phase::Evaluate;
            }
            Phase::Evaluate => {
                self.log_command_result("evaluate", text, exit_code)?;
                if exit_code != 0 {
                    self.state.update_status("error");
                    self.exit_code = exit_code;
                    self.phase = Phase::Closed;
                } else if !self.should_continue_loop(text)? {
                    self.state.update_status("completed");
                    self.log("Objective achieved")?;
                    self.exit_code = 0;
                    self.phase = Phase::Closed;
                } else if text.to_ascii_lowercase().contains("needs adjustment") {
                    self.last_evaluation = text.to_string();
                    self.phase = Phase::Adjust;
                } else {
                    self.turn += 1;
                    self.enter_execute_or_stop()?;
                }
            }
            Phase::Adjust => {
                self.log_command_result("Adjust Plan", text, exit_code)?;
                if exit_code != 0 {
                    self.fail_required("Adjust Plan", exit_code);
                } else {
                    self.plan = text.to_string();
                    self.turn += 1;
                    self.enter_execute_or_stop()?;
                }
            }
            Phase::Closed => {}
        }
        Ok(())
    }

    /// Reproduce the old `bail!` for a failed required turn: mark the run as
    /// errored, record the exact bail message as the abort reason, and close so
    /// no further turns run.
    fn fail_required(&mut self, label: &str, exit_code: i32) {
        self.state.update_status("error");
        self.abort = Some(format!("{label} failed with exit code {exit_code}"));
        self.phase = Phase::Closed;
    }

    /// Enter the next Execute phase, or terminate as "stopped" if the current
    /// turn has passed `max_turns` (parity with the old `for` loop exhausting).
    fn enter_execute_or_stop(&mut self) -> Result<()> {
        if self.turn > self.max_turns {
            self.state.update_status("stopped");
            self.log("Reached max turns without verified completion")?;
            self.exit_code = 0;
            self.phase = Phase::Closed;
        } else {
            self.phase = Phase::Execute;
        }
        Ok(())
    }

    fn build_clarify_prompt(&self) -> String {
        format!(
            "{ctx}\n\nTask: Analyze this user request and clarify the objective with evaluation criteria.\n\n1. IDENTIFY EXPLICIT REQUIREMENTS\n2. IDENTIFY IMPLICIT PREFERENCES\n3. APPLY PHILOSOPHY\n4. DEFINE SUCCESS CRITERIA\n\nUser Request:\n{prompt}",
            ctx = philosophy_context(),
            prompt = self.prompt,
        )
    }

    fn build_plan_prompt(&self, objective: &str) -> String {
        format!(
            "{ctx}\n\nTask: Create an execution plan that preserves the explicit requirements, applies ruthless simplicity, identifies parallel work, and defines clear success criteria.\n\nObjective:\n{objective}",
            ctx = philosophy_context(),
        )
    }

    fn build_plan_adjustment_prompt(
        &self,
        objective: &str,
        current_plan: &str,
        evaluation_result: &str,
    ) -> String {
        format!(
            "{ctx}\n\nTask: Adjust the plan based on the latest evaluation while preserving all explicit requirements.\n\nObjective:\n{objective}\n\nCurrent Plan:\n{current_plan}\n\nLatest Evaluation:\n{evaluation_result}",
            ctx = philosophy_context(),
        )
    }

    fn build_execute_prompt(
        &self,
        objective: &str,
        plan: &str,
        turn: u32,
        new_instructions: &str,
    ) -> String {
        format!(
            "{ctx}\n\nTask: Execute the next part of the plan using specialized agents where possible.\n\nExecution Guidelines:\n- Use parallel execution by default.\n- Implement complete features with no stubs or placeholders.\n- Make implementation decisions autonomously.\n\nCurrent Plan:\n{plan}\n\nOriginal Objective:\n{objective}\n{new_instructions}\n\nCurrent Turn: {turn}/{max_turns}",
            ctx = philosophy_context(),
            max_turns = self.max_turns,
        )
    }

    fn build_evaluation_prompt(&self, objective: &str, turn: u32) -> Result<String> {
        let summary = self.summary_generator.generate(self.state.as_ref());
        let signals = self.completion_detector.detect(&summary);
        let work_summary_text = summary.format_for_prompt();
        let signal_explanation = self.completion_detector.explain(&signals);
        Ok(format!(
            "{ctx}\n\nTask: Evaluate if the objective is achieved based on explicit requirements, applied philosophy, verified implementation, and workflow completion.\n\n{work_summary_text}\n\n{signal_explanation}\n\nRespond with one of:\n- \"auto-mode EVALUATION: COMPLETE\"\n- \"auto-mode EVALUATION: IN PROGRESS\"\n- \"auto-mode EVALUATION: NEEDS ADJUSTMENT\"\n\nObjective:\n{objective}\n\nCurrent Turn: {turn}/{max_turns}",
            ctx = philosophy_context(),
            max_turns = self.max_turns,
        ))
    }

    fn should_continue_loop(&mut self, evaluation_result: &str) -> Result<bool> {
        let summary = self.summary_generator.generate(self.state.as_ref());
        let signals = self.completion_detector.detect(&summary);
        let verification = self.completion_verifier.verify(evaluation_result, &signals);
        self.log(&format!(
            "Completion score: {:.1}% | verification: {:?}",
            signals.completion_score * 100.0,
            verification.status
        ))?;
        if !verification.discrepancies.is_empty() {
            self.log(&format!(
                "Verification discrepancies: {}",
                verification.discrepancies.join("; ")
            ))?;
        }

        let eval_lower = evaluation_result.to_ascii_lowercase();
        if verification.verified
            && (eval_lower.contains("auto-mode evaluation: complete")
                || eval_lower.contains("objective achieved")
                || eval_lower.contains("all criteria met"))
        {
            return Ok(false);
        }
        Ok(true)
    }

    fn write_prompt_file(&mut self) -> Result<()> {
        let started = Local::now().format("%Y-%m-%d %H:%M:%S");
        fs::write(
            self.log_dir.join("prompt.md"),
            format!(
                "# Original Auto Mode Prompt\n\n{}\n\n---\n\n**Session Started**: {}\n**SDK**: {}\n**Max Turns**: {}\n",
                self.prompt,
                started,
                self.tool.slug(),
                self.max_turns
            ),
        )?;
        Ok(())
    }

    fn log(&mut self, message: &str) -> Result<()> {
        let line = format!("[{}] {}\n", Local::now().format("%H:%M:%S"), message);
        let ui_is_active = self
            .ui_active
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire));
        if !ui_is_active {
            print!("{line}");
            io::stdout().flush()?;
        }
        self.state.add_log(message.to_string(), true);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    fn log_command_result(&mut self, label: &str, stdout: &str, exit_code: i32) -> Result<()> {
        self.log(&format!(
            "{} exit code: {} (stdout {} chars)",
            label,
            exit_code,
            stdout.len()
        ))
    }
}

#[async_trait]
impl Channel for AutoModeChannel {
    fn id(&self) -> ChannelId {
        ChannelId::from(self.id.clone())
    }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt> {
        self.emit_next()
            .map_err(|error| ChannelError::Recv(format!("{error:#}")))
    }

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        self.ingest(out)
            .map_err(|error| ChannelError::Publish(format!("{error:#}")))
    }
}
