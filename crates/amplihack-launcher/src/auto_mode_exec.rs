//! Autonomous execution engine — runtime execution.
//!
//! Implements the `AutoMode` struct with the multi-turn agentic loop,
//! retry logic, instruction injection, and session lifecycle.

use anyhow::{Context, Result};
use std::process::Command;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::auto_mode::{
    AutoModeConfig, SdkBackend, SessionMetrics, SessionResult, TurnResult,
    format_elapsed, is_retryable_output, sanitize_injected_content,
};

/// Autonomous execution engine.
///
/// Runs a multi-turn agentic loop with structured phases:
/// 1. **Clarify** — Analyze request, extract requirements
/// 2. **Plan** — Create execution plan
/// 3. **Execute** — Execute plan steps, evaluate progress
pub struct AutoMode {
    config: AutoModeConfig,
    turn: u32,
    start_time: Instant,
    total_api_calls: u32,
    session_output_bytes: usize,
    turn_results: Vec<TurnResult>,
}

impl AutoMode {
    pub fn new(config: AutoModeConfig) -> Self {
        Self {
            config,
            turn: 0,
            start_time: Instant::now(),
            total_api_calls: 0,
            session_output_bytes: 0,
            turn_results: Vec::new(),
        }
    }

    /// Run the full autonomous session.
    pub fn run(&mut self) -> Result<SessionResult> {
        info!(
            sdk = %self.config.sdk,
            max_turns = self.config.max_turns,
            "Starting autonomous session"
        );

        let clarify_prompt = self.build_clarify_prompt();
        self.execute_turn("clarify", &clarify_prompt)?;

        if self.should_continue() {
            let plan_prompt = self.build_plan_prompt();
            self.execute_turn("plan", &plan_prompt)?;
        }

        while self.should_continue() {
            let exec_prompt = self.build_execution_prompt();
            self.execute_turn("execute", &exec_prompt)?;

            if let Some(extra) = self.check_for_new_instructions()? {
                let inject_prompt = format!("Additional instructions received:\n\n{extra}");
                self.execute_turn("inject", &inject_prompt)?;
            }
        }

        let result = SessionResult {
            total_turns: self.turn,
            completed: true,
            total_duration_secs: self.start_time.elapsed().as_secs_f64(),
            turns: self.turn_results.clone(),
            total_api_calls: self.total_api_calls,
            total_output_bytes: self.session_output_bytes,
        };

        info!(
            turns = result.total_turns,
            duration = format_elapsed(result.total_duration_secs),
            "Session complete"
        );
        Ok(result)
    }

    fn execute_turn(&mut self, phase: &str, prompt: &str) -> Result<()> {
        self.turn += 1;
        let turn_start = Instant::now();
        info!(turn = self.turn, phase, "Executing turn");

        let (exit_code, output) = self.run_sdk_with_retry(prompt, 3, 2.0)?;
        self.total_api_calls += 1;
        self.session_output_bytes += output.len();

        let duration = turn_start.elapsed().as_secs_f64();
        self.turn_results.push(TurnResult {
            turn: self.turn,
            phase: phase.into(),
            exit_code,
            output,
            duration_secs: duration,
        });

        debug!(turn = self.turn, exit_code, duration = format_elapsed(duration), "Turn completed");
        Ok(())
    }

    fn run_sdk_with_retry(
        &self,
        prompt: &str,
        max_retries: u32,
        base_delay_secs: f64,
    ) -> Result<(i32, String)> {
        let mut attempt = 0;
        loop {
            match self.run_sdk(prompt) {
                Ok((code, output)) if code == 0 || !is_retryable_output(&output) => {
                    return Ok((code, output));
                }
                Ok((code, output)) if attempt < max_retries => {
                    let delay = base_delay_secs * 2_f64.powi(attempt as i32);
                    warn!(attempt = attempt + 1, delay_secs = delay, exit_code = code, "Retrying");
                    std::thread::sleep(Duration::from_secs_f64(delay));
                    attempt += 1;
                }
                Ok(result) => return Ok(result),
                Err(e) if attempt < max_retries => {
                    let delay = base_delay_secs * 2_f64.powi(attempt as i32);
                    warn!(attempt = attempt + 1, delay_secs = delay, err = %e, "Retrying");
                    std::thread::sleep(Duration::from_secs_f64(delay));
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn run_sdk(&self, prompt: &str) -> Result<(i32, String)> {
        let binary = match self.config.sdk {
            SdkBackend::Claude => "claude",
            SdkBackend::Copilot => "copilot",
            SdkBackend::Codex => "codex",
        };
        let mut cmd = Command::new(binary);
        cmd.current_dir(&self.config.working_dir);
        cmd.args(["--print", "--output-format", "text", "-p", prompt]);
        if let Some(timeout) = self.config.query_timeout_secs {
            cmd.env("AMPLIHACK_QUERY_TIMEOUT", timeout.to_string());
        }
        let output = cmd.output().with_context(|| format!("failed to run {binary}"))?;
        let code = output.status.code().unwrap_or(1);
        let text = String::from_utf8_lossy(&output.stdout).to_string();
        Ok((code, text))
    }

    pub(crate) fn should_continue(&self) -> bool {
        if self.turn >= self.config.max_turns {
            info!("Max turns ({}) reached", self.config.max_turns);
            return false;
        }
        if self.total_api_calls >= self.config.max_api_calls {
            info!("Max API calls ({}) reached", self.config.max_api_calls);
            return false;
        }
        if self.session_output_bytes >= self.config.max_output_bytes {
            info!("Max output size reached");
            return false;
        }
        if self.start_time.elapsed() >= Duration::from_secs(self.config.max_session_secs) {
            info!("Max session duration reached");
            return false;
        }
        true
    }

    pub(crate) fn check_for_new_instructions(&self) -> Result<Option<String>> {
        let append_dir = self.config.working_dir.join(".amplihack").join("append");
        if !append_dir.exists() {
            return Ok(None);
        }
        let mut instructions = String::new();
        let entries = std::fs::read_dir(&append_dir)
            .with_context(|| format!("failed to read {}", append_dir.display()))?;
        let appended_dir = self.config.working_dir.join(".amplihack").join("appended");

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let content = std::fs::read_to_string(&path)?;
                let sanitized = sanitize_injected_content(&content);
                if !sanitized.is_empty() {
                    instructions.push_str(&sanitized);
                    instructions.push('\n');
                }
                std::fs::create_dir_all(&appended_dir)?;
                let dest = appended_dir.join(entry.file_name());
                std::fs::rename(&path, &dest)?;
                debug!(file = %path.display(), "Archived appended instruction");
            }
        }

        if instructions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(instructions))
        }
    }

    pub(crate) fn build_clarify_prompt(&self) -> String {
        format!(
            "Analyze this request and extract requirements, constraints, and success criteria. \
             Do NOT start implementation yet.\n\nRequest: {}",
            self.config.task.as_deref().unwrap_or(&self.config.prompt)
        )
    }

    fn build_plan_prompt(&self) -> String {
        "Based on the analysis, create a detailed execution plan. \
         Identify parallel opportunities and dependencies. \
         Number each step."
            .into()
    }

    fn build_execution_prompt(&self) -> String {
        format!(
            "Execute the next steps of the plan. \
             After completing work, evaluate progress against success criteria. \
             Report: DONE if all criteria met, CONTINUE if more work needed.\n\
             [Turn {}/{}]",
            self.turn + 1,
            self.config.max_turns
        )
    }

    /// Get current metrics.
    pub fn metrics(&self) -> SessionMetrics {
        SessionMetrics {
            turn: self.turn,
            elapsed_secs: self.start_time.elapsed().as_secs_f64(),
            api_calls: self.total_api_calls,
            output_bytes: self.session_output_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_mode::AutoModeConfig;

    #[test]
    fn should_continue_respects_turns() {
        let mut am = AutoMode::new(AutoModeConfig { max_turns: 2, ..Default::default() });
        assert!(am.should_continue());
        am.turn = 2;
        assert!(!am.should_continue());
    }
    #[test]
    fn should_continue_respects_api_calls() {
        let mut am = AutoMode::new(AutoModeConfig { max_api_calls: 3, ..Default::default() });
        am.total_api_calls = 3;
        assert!(!am.should_continue());
    }
    #[test]
    fn metrics_initial() {
        let am = AutoMode::new(AutoModeConfig::default());
        let m = am.metrics();
        assert_eq!(m.turn, 0); assert_eq!(m.api_calls, 0); assert!(m.elapsed_secs < 1.0);
    }
    #[test]
    fn build_clarify_uses_task() {
        let am = AutoMode::new(AutoModeConfig { prompt: "orig".into(), task: Some("override".into()), ..Default::default() });
        let p = am.build_clarify_prompt();
        assert!(p.contains("override")); assert!(!p.contains("orig"));
    }
    #[test]
    fn build_clarify_falls_back() {
        let am = AutoMode::new(AutoModeConfig { prompt: "fix bug".into(), ..Default::default() });
        assert!(am.build_clarify_prompt().contains("fix bug"));
    }
    #[test]
    fn check_instructions_no_dir() {
        let dir = tempfile::tempdir().unwrap();
        let am = AutoMode::new(AutoModeConfig { working_dir: dir.path().into(), ..Default::default() });
        assert!(am.check_for_new_instructions().unwrap().is_none());
    }
    #[test]
    fn check_instructions_reads_and_archives() {
        let dir = tempfile::tempdir().unwrap();
        let append = dir.path().join(".amplihack/append");
        std::fs::create_dir_all(&append).unwrap();
        std::fs::write(append.join("extra.md"), "Do this also").unwrap();
        let am = AutoMode::new(AutoModeConfig { working_dir: dir.path().into(), ..Default::default() });
        let instr = am.check_for_new_instructions().unwrap().unwrap();
        assert!(instr.contains("Do this also"));
        assert!(!append.join("extra.md").exists());
        assert!(dir.path().join(".amplihack/appended/extra.md").exists());
    }
}
