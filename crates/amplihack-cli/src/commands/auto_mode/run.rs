use super::helpers::{extract_prompt_args, prepare_auto_mode_execution};
use super::*;
use amplihack_turn::run_session_loop;

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
        let execution_dir = execution.execution_dir;
        let project_dir = execution.project_dir;
        let mut channel = AutoModeChannel::new(
            tool,
            prompt,
            max_turns,
            execution_dir.clone(),
            project_dir.clone(),
            ui_active.clone(),
        )?;
        let runner_session_id = channel.state().snapshot().session_id;
        let mut runner = AutoModeRunner::new(
            SystemPromptExecutor {
                ui_active: ui_active.clone(),
                node_options: Some(node_options.clone()),
            },
            tool,
            execution_dir,
            project_dir,
            parsed.passthrough_args,
            runner_session_id,
        );
        let ui_handle = if let Some(active) = ui_active {
            Some(AutoModeUiHandle::start(
                Arc::clone(channel.state()),
                channel.prompt().to_string(),
                active,
            )?)
        } else {
            None
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .context("failed to build auto-mode runtime")?;
        let run_result = runtime.block_on(run_session_loop(&mut runner, &mut channel));
        if let Some(handle) = ui_handle {
            handle.finish();
        }
        // A driver-loop error (executor failure to RUN, or a channel receive /
        // publish error) propagates unchanged — the outer guard crashes the
        // session tracker, matching the old `?`-propagation behaviour.
        run_result.map_err(anyhow::Error::from)?;
        // A failed *required* turn (Clarify / Plan / Adjust) is a ran-but-
        // non-zero subprocess, so it is a clean loop close carrying an abort
        // reason. Reproduce the old `bail!`: propagate the error so the outer
        // guard crashes the session tracker.
        if let Some(abort) = channel.abort() {
            bail!("{abort}");
        }
        let exit_code = channel.exit_code();
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
