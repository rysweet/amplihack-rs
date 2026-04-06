use super::helpers::{extract_prompt_args, prepare_auto_mode_execution};
use super::session::AutoModeSession;
use super::*;

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
                "--auto requires a prompt: {} --auto -- \"prompt\" (or -- -p \"prompt\")",
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

pub(super) fn render_auto_session_argv(
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
