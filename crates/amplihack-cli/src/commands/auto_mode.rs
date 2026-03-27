//! Native auto-mode loop for launcher commands.

use crate::auto_mode_append::process_appended_instructions;
use crate::auto_mode_completion_signals::CompletionSignalDetector;
use crate::auto_mode_completion_verifier::CompletionVerifier;
use crate::auto_mode_state::AutoModeState;
use crate::auto_mode_ui::AutoModeUiHandle;
use crate::auto_mode_work_summary_generator::WorkSummaryGenerator;
use crate::auto_stager::AutoStager;
use crate::env_builder::EnvBuilder;
use crate::memory_config::prepare_memory_config;
use crate::nesting::NestingDetector;
use crate::session_tracker::SessionTracker;
use crate::util::run_output_with_timeout;
use anyhow::{Context, Result, bail};
use chrono::Local;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoModeTool {
    Claude,
    Copilot,
    Codex,
    Amplifier,
    RustyClawd,
}

impl AutoModeTool {
    fn slug(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::RustyClawd => "claude",
        }
    }

    fn subcommand(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::RustyClawd => "RustyClawd",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedPromptArgs {
    prompt: String,
    passthrough_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecutionResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

trait PromptExecutor {
    fn run_prompt(
        &self,
        tool: AutoModeTool,
        execution_dir: &Path,
        project_dir: &Path,
        passthrough_args: &[String],
        prompt: &str,
    ) -> Result<ExecutionResult>;
}

#[derive(Clone, Debug, Default)]
struct SystemPromptExecutor {
    ui_active: Option<Arc<AtomicBool>>,
    node_options: Option<String>,
}

impl PromptExecutor for SystemPromptExecutor {
    fn run_prompt(
        &self,
        tool: AutoModeTool,
        execution_dir: &Path,
        project_dir: &Path,
        passthrough_args: &[String],
        prompt: &str,
    ) -> Result<ExecutionResult> {
        let command = build_auto_command(
            tool,
            execution_dir,
            project_dir,
            self.node_options.as_deref(),
            passthrough_args,
            prompt,
        )
        .with_context(|| {
            format!(
                "failed to build auto-mode command for {}",
                tool.subcommand()
            )
        })?;
        let output = run_output_with_timeout(command, QUERY_TIMEOUT)?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let ui_is_active = self
            .ui_active
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire));
        if !ui_is_active && !stdout.is_empty() {
            io::stdout().write_all(stdout.as_bytes())?;
            io::stdout().flush()?;
        }
        if !ui_is_active && !stderr.is_empty() {
            io::stderr().write_all(stderr.as_bytes())?;
            io::stderr().flush()?;
        }

        Ok(ExecutionResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout,
            stderr,
        })
    }
}

pub fn run_auto_mode(
    tool: AutoModeTool,
    max_turns: u32,
    ui: bool,
    raw_args: Vec<String>,
    checkout_repo: Option<String>,
    working_dir: Option<PathBuf>,
) -> Result<()> {
    let project_dir =
        working_dir.unwrap_or(env::current_dir().context("failed to resolve current directory")?);
    let existing_node_options = std::env::var("NODE_OPTIONS").ok();
    let node_options = prepare_memory_config(existing_node_options.as_deref())?.node_options;
    crate::commands::launch::maybe_prompt_re_enable_power_steering(&project_dir)?;
    let nesting = NestingDetector::detect();
    let execution = prepare_auto_mode_execution(&project_dir)?;
    let tracker = SessionTracker::new(&execution.execution_dir)?;
    let session_id = tracker.start_session(
        std::process::id(),
        &execution.execution_dir,
        &render_auto_session_argv(tool, max_turns, ui, checkout_repo.as_deref(), &raw_args),
        true,
        &nesting,
    )?;
    let result = (|| -> Result<()> {
        let parsed = extract_prompt_args(&raw_args).with_context(|| {
            format!(
                "--auto requires a prompt via {} -- -p \"prompt\"",
                tool.subcommand()
            )
        })?;
        if ui {
            bail!("--ui is not yet supported in native Rust auto mode");
        }

        if tool == AutoModeTool::Amplifier {
            let result = SystemPromptExecutor {
                ui_active: None,
                node_options: Some(node_options.clone()),
            }
            .run_prompt(
                AutoModeTool::Amplifier,
                &execution.execution_dir,
                &execution.project_dir,
                &parsed.passthrough_args,
                &execution.transform_prompt(&parsed.prompt),
            )?;
            if result.exit_code != 0 {
                tracker.complete_session(&session_id)?;
                std::process::exit(result.exit_code);
            }
            tracker.complete_session(&session_id)?;
            return Ok(());
        }

        let ui_active = ui.then(|| Arc::new(AtomicBool::new(true)));
        let prompt = execution.transform_prompt(&parsed.prompt);
        let mut session = AutoModeSession::new(
            tool,
            prompt,
            parsed.passthrough_args,
            max_turns,
            execution.execution_dir,
            execution.project_dir,
            SystemPromptExecutor {
                ui_active: ui_active.clone(),
                node_options: Some(node_options.clone()),
            },
            ui_active.clone(),
        )?;
        let ui_handle = if let Some(active) = ui_active {
            Some(AutoModeUiHandle::start(
                Arc::clone(&session.state),
                session.prompt.clone(),
                active,
            )?)
        } else {
            None
        };
        let run_result = session.run();
        if let Some(handle) = ui_handle {
            handle.finish();
        }
        let exit_code = run_result?;
        tracker.complete_session(&session_id)?;
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = tracker.crash_session(&session_id);
    }
    result
}

fn render_auto_session_argv(
    tool: AutoModeTool,
    max_turns: u32,
    ui: bool,
    checkout_repo: Option<&str>,
    raw_args: &[String],
) -> Vec<String> {
    let mut argv = vec![
        "amplihack".to_string(),
        tool.subcommand().to_string(),
        "--auto".to_string(),
    ];
    if max_turns != 10 {
        argv.push("--max-turns".to_string());
        argv.push(max_turns.to_string());
    }
    if ui {
        argv.push("--ui".to_string());
    }
    if let Some(repo) = checkout_repo {
        argv.push("--checkout-repo".to_string());
        argv.push(repo.to_string());
    }
    argv.extend(raw_args.iter().cloned());
    argv
}

struct AutoModeSession<E: PromptExecutor> {
    tool: AutoModeTool,
    prompt: String,
    passthrough_args: Vec<String>,
    max_turns: u32,
    execution_dir: PathBuf,
    project_dir: PathBuf,
    log_dir: PathBuf,
    append_dir: PathBuf,
    appended_dir: PathBuf,
    log_path: PathBuf,
    state: Arc<AutoModeState>,
    ui_active: Option<Arc<AtomicBool>>,
    summary_generator: WorkSummaryGenerator,
    completion_detector: CompletionSignalDetector,
    completion_verifier: CompletionVerifier,
    executor: E,
}

impl<E: PromptExecutor> AutoModeSession<E> {
    #[allow(clippy::too_many_arguments)]
    fn new(
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

    fn run(&mut self) -> Result<i32> {
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

fn philosophy_context() -> &'static str {
    "AUTONOMOUS MODE: You are in auto mode. Do NOT ask questions. Make decisions using:\n1. Explicit user requirements (highest priority)\n2. @.claude/context/USER_PREFERENCES.md guidance\n3. @.claude/context/PHILOSOPHY.md principles\n4. @.claude/workflow/DEFAULT_WORKFLOW.md patterns\n5. @.claude/context/USER_REQUIREMENT_PRIORITY.md for conflicts\n\nDecision Authority:\n- YOU DECIDE implementation details and architecture\n- YOU PRESERVE explicit user requirements and hard constraints\n- WHEN AMBIGUOUS choose the simplest modular option"
}

fn extract_prompt_args(args: &[String]) -> Option<ParsedPromptArgs> {
    let mut passthrough_args = Vec::new();
    let mut prompt = None;
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "-p" || arg == "--prompt" {
            if index + 1 >= args.len() {
                return None;
            }
            prompt = Some(args[index + 1].clone());
            index += 2;
            continue;
        }
        if let Some(value) = arg.strip_prefix("-p=") {
            prompt = Some(value.to_string());
            index += 1;
            continue;
        }
        if let Some(value) = arg.strip_prefix("--prompt=") {
            prompt = Some(value.to_string());
            index += 1;
            continue;
        }
        passthrough_args.push(arg.clone());
        index += 1;
    }

    Some(ParsedPromptArgs {
        prompt: prompt?,
        passthrough_args,
    })
}

fn build_auto_command(
    tool: AutoModeTool,
    execution_dir: &Path,
    project_dir: &Path,
    node_options: Option<&str>,
    passthrough_args: &[String],
    prompt: &str,
) -> Result<Command> {
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let mut command = Command::new(current_exe);
    command.current_dir(execution_dir);
    let env_builder = EnvBuilder::new()
        .with_amplihack_session_id()
        .with_incremented_session_tree_context()
        .with_amplihack_vars_with_node_options(node_options)
        .with_agent_binary(tool.slug())
        .with_amplihack_home()
        .with_asset_resolver()
        .with_project_graph_db(project_dir)?;
    let env_builder = if execution_dir != project_dir {
        env_builder.set("AMPLIHACK_IS_STAGED", "1").set(
            "AMPLIHACK_ORIGINAL_CWD",
            project_dir.to_string_lossy().into_owned(),
        )
    } else {
        env_builder
    };
    env_builder.apply_to_command(&mut command);
    command.arg(tool.subcommand());
    command.arg("--no-reflection");
    command.arg("--subprocess-safe");
    command.arg("--");
    command.args(build_tool_passthrough_args(tool, passthrough_args, prompt));
    Ok(command)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedAutoModeExecution {
    execution_dir: PathBuf,
    project_dir: PathBuf,
}

impl PreparedAutoModeExecution {
    fn transform_prompt(&self, original_prompt: &str) -> String {
        if self.execution_dir == self.project_dir {
            return original_prompt.to_string();
        }
        transform_prompt_for_staging(original_prompt, &self.project_dir)
    }
}

fn prepare_auto_mode_execution(project_dir: &Path) -> Result<PreparedAutoModeExecution> {
    println!("\n🚨 SELF-MODIFICATION PROTECTION ACTIVATED");
    println!("   Auto-staging .claude/ to temp directory for safety");
    let staging = AutoStager::stage_for_nested_execution(
        project_dir,
        &format!("nested-{}", std::process::id()),
    )?;
    println!("   📁 Staged to: {}", staging.temp_root.display());
    println!("   Your original .claude/ files are protected");
    println!(
        "   📂 Auto mode execution dir: {}\n",
        staging.temp_root.display()
    );
    Ok(PreparedAutoModeExecution {
        execution_dir: staging.temp_root,
        project_dir: staging.original_cwd,
    })
}

fn transform_prompt_for_staging(original_prompt: &str, target_directory: &Path) -> String {
    let target_directory = target_directory
        .canonicalize()
        .unwrap_or_else(|_| target_directory.to_path_buf());
    let (slash_commands, remaining_prompt) = extract_leading_slash_commands(original_prompt);
    let dir_instruction = format!(
        "Change your working directory to {}. ",
        target_directory.display()
    );
    if slash_commands.is_empty() {
        return format!("{dir_instruction}{remaining_prompt}");
    }
    format!("{slash_commands} {dir_instruction}{remaining_prompt}")
}

fn extract_leading_slash_commands(prompt: &str) -> (String, String) {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }

    let mut commands = Vec::new();
    let mut index = 0usize;
    while index < trimmed.len() {
        let remaining = &trimmed[index..];
        let mut chars = remaining.chars();
        if chars.next() != Some('/') {
            break;
        }

        let command_len = remaining
            .char_indices()
            .skip(1)
            .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':' | '_'))
            .map(|(offset, ch)| offset + ch.len_utf8())
            .last()
            .unwrap_or(1);
        let command = &remaining[..command_len];
        if command == "/" {
            break;
        }
        commands.push(command.to_string());
        index += command_len;

        let whitespace_len = trimmed[index..]
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
        if whitespace_len == 0 {
            break;
        }
        index += whitespace_len;
    }

    if commands.is_empty() {
        return (String::new(), trimmed.to_string());
    }
    (commands.join(" "), trimmed[index..].trim().to_string())
}

fn build_tool_passthrough_args(
    tool: AutoModeTool,
    passthrough_args: &[String],
    prompt: &str,
) -> Vec<String> {
    let mut args = passthrough_args.to_vec();
    match tool {
        AutoModeTool::Claude | AutoModeTool::RustyClawd => {
            if !args.iter().any(|arg| arg == "--verbose") {
                args.push("--verbose".to_string());
            }
            args.push("-p".to_string());
            args.push(prompt.to_string());
        }
        AutoModeTool::Copilot => {
            if !args.iter().any(|arg| arg == "--allow-all-tools") {
                args.push("--allow-all-tools".to_string());
            }
            if !args.iter().any(|arg| arg == "--add-dir") {
                args.push("--add-dir".to_string());
                args.push("/".to_string());
            }
            args.push("-p".to_string());
            args.push(prompt.to_string());
        }
        AutoModeTool::Codex => {
            if !args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
            {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            args.push("exec".to_string());
            args.push(prompt.to_string());
        }
        AutoModeTool::Amplifier => {
            args.push("-p".to_string());
            args.push(prompt.to_string());
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn extract_prompt_args_supports_split_prompt_flag() {
        let parsed = extract_prompt_args(&[
            "--model".to_string(),
            "sonnet".to_string(),
            "-p".to_string(),
            "ship parity".to_string(),
        ])
        .expect("prompt should parse");

        assert_eq!(parsed.prompt, "ship parity");
        assert_eq!(parsed.passthrough_args, vec!["--model", "sonnet"]);
    }

    #[test]
    fn extract_prompt_args_supports_equals_prompt_flag() {
        let parsed = extract_prompt_args(&["--prompt=ship parity".to_string()])
            .expect("prompt should parse");
        assert_eq!(parsed.prompt, "ship parity");
        assert!(parsed.passthrough_args.is_empty());
    }

    #[test]
    fn build_tool_passthrough_args_matches_codex_and_copilot_contracts() {
        let codex = build_tool_passthrough_args(AutoModeTool::Codex, &[], "refactor module");
        assert_eq!(
            codex,
            vec![
                "--dangerously-bypass-approvals-and-sandbox",
                "exec",
                "refactor module"
            ]
        );

        let copilot = build_tool_passthrough_args(AutoModeTool::Copilot, &[], "add logging");
        assert_eq!(
            copilot,
            vec!["--allow-all-tools", "--add-dir", "/", "-p", "add logging"]
        );
    }

    #[test]
    fn render_auto_session_argv_includes_auto_flags() {
        let argv = render_auto_session_argv(
            AutoModeTool::Copilot,
            12,
            true,
            Some("owner/repo"),
            &["--model".to_string(), "gpt-5".to_string()],
        );
        assert_eq!(
            argv,
            vec![
                "amplihack",
                "copilot",
                "--auto",
                "--max-turns",
                "12",
                "--ui",
                "--checkout-repo",
                "owner/repo",
                "--model",
                "gpt-5",
            ]
        );
    }

    #[test]
    fn build_auto_command_propagates_launcher_environment() {
        let dir = tempfile::tempdir().unwrap();

        let command = build_auto_command(
            AutoModeTool::Copilot,
            dir.path(),
            dir.path(),
            Some("--max-old-space-size=16384"),
            &[],
            "add logging",
        )
        .expect("auto-mode command should build");
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|entry| entry.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(
            env.get("AMPLIHACK_AGENT_BINARY")
                .and_then(|value| value.as_deref()),
            Some("copilot")
        );
        assert!(env.contains_key("AMPLIHACK_SESSION_ID"));
        assert!(env.contains_key("AMPLIHACK_DEPTH"));
        assert!(env.contains_key("AMPLIHACK_HOME"));
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH")
                .and_then(|value| value.as_deref()),
            Some(
                dir.path()
                    .join(".amplihack")
                    .join("graph_db")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert_eq!(
            env.get("AMPLIHACK_RUST_RUNTIME")
                .and_then(|value| value.as_deref()),
            Some("1")
        );
        assert_eq!(
            env.get("NODE_OPTIONS").and_then(|value| value.as_deref()),
            Some("--max-old-space-size=16384")
        );
    }

    #[test]
    fn build_auto_command_marks_staged_execution_context() {
        let execution_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();

        let command = build_auto_command(
            AutoModeTool::Claude,
            execution_dir.path(),
            project_dir.path(),
            Some("--max-old-space-size=32768"),
            &[],
            "ship parity",
        )
        .expect("staged auto-mode command should build");
        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|entry| entry.to_string_lossy().into_owned()),
                )
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(command.get_current_dir(), Some(execution_dir.path()));
        assert_eq!(
            env.get("AMPLIHACK_IS_STAGED")
                .and_then(|value| value.as_deref()),
            Some("1")
        );
        assert_eq!(
            env.get("AMPLIHACK_ORIGINAL_CWD")
                .and_then(|value| value.as_deref()),
            Some(project_dir.path().to_string_lossy().as_ref())
        );
        assert_eq!(
            env.get("AMPLIHACK_GRAPH_DB_PATH")
                .and_then(|value| value.as_deref()),
            Some(
                project_dir
                    .path()
                    .join(".amplihack")
                    .join("graph_db")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn transform_prompt_for_staging_preserves_leading_slash_commands() {
        let target = tempfile::tempdir().unwrap();
        let transformed =
            transform_prompt_for_staging("/dev /analyze fix the launcher", target.path());

        assert_eq!(
            transformed,
            format!(
                "/dev /analyze Change your working directory to {}. fix the launcher",
                target.path().canonicalize().unwrap().display()
            )
        );
    }
}
