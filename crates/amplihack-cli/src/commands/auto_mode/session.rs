use super::*;
use super::helpers::philosophy_context;

pub(super) struct AutoModeSession<E: PromptExecutor> {
    pub(super) tool: AutoModeTool,
    pub(super) prompt: String,
    passthrough_args: Vec<String>,
    pub(super) max_turns: u32,
    execution_dir: PathBuf,
    project_dir: PathBuf,
    log_dir: PathBuf,
    append_dir: PathBuf,
    appended_dir: PathBuf,
    log_path: PathBuf,
    pub(super) state: Arc<AutoModeState>,
    ui_active: Option<Arc<AtomicBool>>,
    summary_generator: WorkSummaryGenerator,
    completion_detector: CompletionSignalDetector,
    completion_verifier: CompletionVerifier,
    executor: E,
}

impl<E: PromptExecutor> AutoModeSession<E> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        tool: AutoModeTool,
        prompt: String,
        passthrough_args: Vec<String>,
        max_turns: u32,
        execution_dir: PathBuf,
        project_dir: PathBuf,
        executor: E,
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
        let state = Arc::new(AutoModeState::new(session_id, max_turns, prompt.clone()));
        let mut session = Self {
            tool,
            prompt,
            passthrough_args,
            max_turns,
            execution_dir,
            project_dir: project_dir.clone(),
            log_dir,
            append_dir,
            appended_dir,
            log_path,
            state,
            ui_active,
            summary_generator: WorkSummaryGenerator::new(project_dir),
            completion_detector: CompletionSignalDetector::default(),
            completion_verifier: CompletionVerifier::default(),
            executor,
        };
        session.write_prompt_file()?;
        Ok(session)
    }

    pub(super) fn run(&mut self) -> Result<i32> {
        self.log("Starting native auto mode")?;
        self.log(&format!("Prompt: {}", self.prompt))?;

        let objective =
            self.run_required_turn(1, "Clarify Objective", &self.build_clarify_prompt())?;
        let mut plan =
            self.run_required_turn(2, "Create Plan", &self.build_plan_prompt(&objective))?;

        for turn in 3..=self.max_turns {
            self.state.update_turn(turn);
            self.log(&format!(
                "--- Turn {turn}/{max} Execute ---",
                max = self.max_turns
            ))?;

            let new_instructions =
                process_appended_instructions(&self.append_dir, &self.appended_dir)
                    .context("failed processing appended instructions")?;
            let execute_prompt =
                self.build_execute_prompt(&objective, &plan, turn, &new_instructions);
            let execution_result = self.executor.run_prompt(
                self.tool,
                &self.execution_dir,
                &self.project_dir,
                &self.passthrough_args,
                &execute_prompt,
            )?;
            self.log_command_result("execute", &execution_result)?;
            if execution_result.exit_code != 0 {
                self.log(&format!(
                    "Warning: execute step returned exit code {}",
                    execution_result.exit_code
                ))?;
            }

            self.log(&format!(
                "--- Turn {turn}/{max} Evaluate ---",
                max = self.max_turns
            ))?;
            let evaluation_prompt = self.build_evaluation_prompt(&objective, turn)?;
            let evaluation_result = self.executor.run_prompt(
                self.tool,
                &self.execution_dir,
                &self.project_dir,
                &self.passthrough_args,
                &evaluation_prompt,
            )?;
            self.log_command_result("evaluate", &evaluation_result)?;
            if evaluation_result.exit_code != 0 {
                self.state.update_status("error");
                return Ok(evaluation_result.exit_code);
            }

            if !self.should_continue_loop(&evaluation_result.stdout)? {
                self.state.update_status("completed");
                self.log("Objective achieved")?;
                return Ok(0);
            }

            if evaluation_result
                .stdout
                .to_ascii_lowercase()
                .contains("needs adjustment")
            {
                plan = self.run_required_turn(
                    turn,
                    "Adjust Plan",
                    &self.build_plan_adjustment_prompt(
                        &objective,
                        &plan,
                        &evaluation_result.stdout,
                    ),
                )?;
            }
        }

        self.state.update_status("stopped");
        self.log("Reached max turns without verified completion")?;
        Ok(0)
    }

    fn run_required_turn(&mut self, turn: u32, label: &str, prompt: &str) -> Result<String> {
        self.state.update_turn(turn);
        self.log(&format!(
            "--- Turn {turn}/{max} {label} ---",
            max = self.max_turns
        ))?;
        let result = self.executor.run_prompt(
            self.tool,
            &self.execution_dir,
            &self.project_dir,
            &self.passthrough_args,
            prompt,
        )?;
        self.log_command_result(label, &result)?;
        if result.exit_code != 0 {
            self.state.update_status("error");
            bail!("{label} failed with exit code {}", result.exit_code);
        }
        Ok(result.stdout)
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

    fn log_command_result(&mut self, label: &str, result: &ExecutionResult) -> Result<()> {
        self.log(&format!(
            "{} exit code: {} (stdout {} chars, stderr {} chars)",
            label,
            result.exit_code,
            result.stdout.len(),
            result.stderr.len()
        ))
    }
}
