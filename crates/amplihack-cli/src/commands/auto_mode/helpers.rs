use super::*;
use amplihack_launcher::flag_matrix::{AgentBinary, prompt_delivery_caps_for};
use amplihack_launcher::prompt_delivery::{
    DeliveredCommand, build_command_with_prompt_delivery, validate_prompt_delivery_request,
};
use amplihack_utils::prompt_delivery::PromptDelivery;
use std::ffi::OsString;
#[cfg(test)]
use std::process::Command;

pub(super) fn philosophy_context() -> &'static str {
    "AUTONOMOUS MODE: You are in auto mode. Do NOT ask questions. Make decisions using:\n1. Explicit user requirements (highest priority)\n2. @.claude/context/USER_PREFERENCES.md guidance\n3. @.claude/context/PHILOSOPHY.md principles\n4. The `default-workflow` skill/recipe routed through `dev-orchestrator` and `amplihack recipe run smart-orchestrator`\n5. @.claude/context/USER_REQUIREMENT_PRIORITY.md for conflicts\n\nDecision Authority:\n- YOU DECIDE implementation details and architecture\n- YOU PRESERVE explicit user requirements and hard constraints\n- WHEN AMBIGUOUS choose the simplest modular option"
}

/// Splits the passthrough args into the auto-mode prompt and the args for the
/// agent.
///
/// With `prompt_file`, the file's contents are the prompt, unchanged, and any
/// `-p`/`--prompt` in `args` is consumed and ignored with a warning (#1570).
/// A missing, unreadable or blank prompt file is an error naming the path.
/// Without it, `Ok(None)` means no prompt was given.
pub(super) fn extract_prompt_args(
    args: &[String],
    prompt_file: Option<&Path>,
) -> Result<Option<ParsedPromptArgs>> {
    let file_prompt = prompt_file.map(read_prompt_file).transpose()?;
    let mut passthrough_args = Vec::new();
    let mut prompt = None;
    let mut ignored_flags = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        let (flag, value, consumed) = if arg == "-p" || arg == "--prompt" {
            if index + 1 >= args.len() {
                if file_prompt.is_none() {
                    return Ok(None);
                }
                (arg.as_str(), None, 1)
            } else {
                (arg.as_str(), Some(args[index + 1].clone()), 2)
            }
        } else if let Some(value) = arg.strip_prefix("-p=") {
            ("-p", Some(value.to_string()), 1)
        } else if let Some(value) = arg.strip_prefix("--prompt=") {
            ("--prompt", Some(value.to_string()), 1)
        } else {
            passthrough_args.push(arg.clone());
            index += 1;
            continue;
        };
        if file_prompt.is_some() {
            ignored_flags.push(flag);
        } else {
            prompt = value;
        }
        index += consumed;
    }

    if let (Some(prompt), Some(path)) = (file_prompt, prompt_file) {
        if !ignored_flags.is_empty() {
            eprintln!(
                "warning: ignoring {} after -- because --prompt-file {} takes precedence",
                ignored_flags.join(", "),
                path.display()
            );
        }
        if lone_positional_arg(&passthrough_args).is_some() {
            eprintln!(
                "warning: --prompt-file {} takes precedence over a positional prompt argument",
                path.display()
            );
        }
        return Ok(Some(ParsedPromptArgs {
            prompt,
            passthrough_args,
        }));
    }

    // If no explicit -p/--prompt was given, treat the last non-flag passthrough
    // arg as the prompt when it's the only such arg. This lets users write
    // `amplihack copilot --auto -- "do X"` instead of `-- -p "do X"`.
    // We only do this when there's exactly one non-flag arg to avoid ambiguity
    // (e.g. `--model sonnet "prompt"` has two non-flag args: "sonnet" and "prompt").
    if prompt.is_none()
        && let Some(index) = lone_positional_arg(&passthrough_args)
    {
        prompt = Some(passthrough_args.remove(index));
    }

    Ok(prompt.map(|prompt| ParsedPromptArgs {
        prompt,
        passthrough_args,
    }))
}

/// Index of the only argument that is not a flag, if there is exactly one.
fn lone_positional_arg(args: &[String]) -> Option<usize> {
    let mut positional = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| !arg.starts_with('-'));
    let (index, _) = positional.next()?;
    positional.next().is_none().then_some(index)
}

/// Reads a `--prompt-file`; the contents are returned unchanged, but a file
/// that is empty after trimming is rejected.
fn read_prompt_file(path: &Path) -> Result<String> {
    // Bounded like the recipe context files (16 MiB), with the same errors.
    let prompt = crate::commands::recipe::run::context_sources::read_source("--prompt-file", path)?;
    if prompt.trim().is_empty() {
        bail!(
            "--prompt-file {} is empty after trimming whitespace",
            path.display()
        );
    }
    Ok(prompt)
}

#[cfg(test)]
pub(super) fn build_auto_command(
    tool: AutoModeTool,
    execution_dir: &Path,
    project_dir: &Path,
    node_options: Option<&str>,
    passthrough_args: &[String],
    prompt: &str,
) -> Result<Command> {
    Ok(
        build_auto_command_with_prompt_delivery(AutoModePromptDeliveryOptions {
            tool,
            execution_dir: execution_dir.to_path_buf(),
            project_dir: project_dir.to_path_buf(),
            node_options: node_options.map(str::to_string),
            passthrough_args: passthrough_args.to_vec(),
            prompt: prompt.to_string(),
            requested_delivery: amplihack_utils::prompt_delivery::from_env(),
        })?
        .command,
    )
}

#[derive(Debug, Clone)]
pub struct AutoModePromptDeliveryOptions {
    pub tool: AutoModeTool,
    pub execution_dir: PathBuf,
    pub project_dir: PathBuf,
    pub node_options: Option<String>,
    pub passthrough_args: Vec<String>,
    pub prompt: String,
    pub requested_delivery: PromptDelivery,
}

pub fn build_auto_command_with_prompt_delivery(
    options: AutoModePromptDeliveryOptions,
) -> Result<DeliveredCommand> {
    let current_exe = env::current_exe().context("failed to resolve current executable")?;
    let agent_binary = agent_binary_for_tool(options.tool);
    validate_prompt_delivery_request(agent_binary, options.requested_delivery).with_context(
        || {
            format!(
                "invalid prompt delivery configuration for {}",
                agent_binary.env_value()
            )
        },
    )?;
    let mut args = vec![
        OsString::from(options.tool.subcommand()),
        OsString::from("--no-reflection"),
        OsString::from("--subprocess-safe"),
        OsString::from("--"),
    ];
    args.extend(build_tool_passthrough_prefix_args(
        options.tool,
        &options.passthrough_args,
        options.tool == AutoModeTool::Copilot && amplihack_utils::litellm_proxy::proxy_requested(),
    ));
    let mut delivered = build_command_with_prompt_delivery(
        current_exe.as_os_str(),
        args.iter(),
        &options.prompt,
        options.requested_delivery,
        prompt_delivery_caps_for(agent_binary),
    )?;
    delivered.command.current_dir(&options.execution_dir);
    let env_builder = EnvBuilder::new()
        .with_amplihack_session_id()
        .with_incremented_session_tree_context()
        .with_amplihack_vars_with_node_options(options.node_options.as_deref())
        .with_agent_binary(options.tool.slug())
        .with_amplihack_home()
        .with_asset_resolver()
        .with_project_graph_db(&options.project_dir)?;
    let env_builder = if options.execution_dir != options.project_dir {
        env_builder.set("AMPLIHACK_IS_STAGED", "1").set(
            "AMPLIHACK_ORIGINAL_CWD",
            options.project_dir.to_string_lossy().into_owned(),
        )
    } else {
        env_builder
    };
    // Whenever the prompt is a payload (stdin or a file reference) the
    // session is non-interactive; the flag describes the session, not the
    // transport (#1570).
    let payload_present = delivered.stdin_payload.is_some()
        || delivered.selected_mode == amplihack_utils::prompt_delivery::DeliveryMode::FileReference;
    let env_builder = env_builder.set("AMPLIHACK_AUTO_MODE", "1").set_if(
        payload_present,
        "AMPLIHACK_NONINTERACTIVE",
        "1",
    );
    env_builder.apply_to_command(&mut delivered.command);
    Ok(delivered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedAutoModeExecution {
    pub(super) execution_dir: PathBuf,
    pub(super) project_dir: PathBuf,
}

impl PreparedAutoModeExecution {
    pub(super) fn transform_prompt(&self, original_prompt: &str) -> String {
        if self.execution_dir == self.project_dir {
            return original_prompt.to_string();
        }
        transform_prompt_for_staging(original_prompt, &self.project_dir)
    }
}

pub(super) fn prepare_auto_mode_execution(project_dir: &Path) -> Result<PreparedAutoModeExecution> {
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

pub(super) fn transform_prompt_for_staging(
    original_prompt: &str,
    target_directory: &Path,
) -> String {
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

#[cfg(test)]
pub(super) fn build_tool_passthrough_args(
    tool: AutoModeTool,
    passthrough_args: &[String],
    prompt: &str,
) -> Vec<String> {
    let mut args = build_tool_passthrough_prefix_args(tool, passthrough_args, false)
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    args.push(prompt.to_string());
    args
}

fn build_tool_passthrough_prefix_args(
    tool: AutoModeTool,
    passthrough_args: &[String],
    routed_copilot: bool,
) -> Vec<OsString> {
    if tool == AutoModeTool::Amplifier {
        let mut args = Vec::with_capacity(passthrough_args.len() + 1);
        args.push(OsString::from("run"));
        args.extend(passthrough_args.iter().map(OsString::from));
        return args;
    }

    let mut args = passthrough_args.to_vec();
    match tool {
        AutoModeTool::Claude | AutoModeTool::RustyClawd => {
            // These args go to a nested `amplihack claude`, whose launcher sets
            // IS_SANDBOX=1 for a root sandbox or fails naming it (issue #1482).
            if !args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
            {
                args.push("--dangerously-skip-permissions".to_string());
            }
            if !args.iter().any(|arg| arg == "--verbose") {
                args.push("--verbose".to_string());
            }
            args.push("-p".to_string());
        }
        AutoModeTool::Copilot => {
            args = strip_claude_only_flags(args);
            let has_allow_all = args.iter().any(|a| a == "--allow-all");
            let has_allow_all_tools = args.iter().any(|a| a == "--allow-all-tools");
            if !has_allow_all && !has_allow_all_tools {
                args.push("--allow-all".to_string());
            }
            if !routed_copilot && !args.iter().any(|arg| arg == "--add-dir") {
                args.push("--add-dir".to_string());
                args.push("/".to_string());
            }
            args.push("-p".to_string());
        }
        AutoModeTool::Codex => {
            if !args
                .iter()
                .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox")
            {
                args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
            }
            args.push("exec".to_string());
        }
        AutoModeTool::Amplifier => {}
    }
    args.into_iter().map(OsString::from).collect()
}

fn agent_binary_for_tool(tool: AutoModeTool) -> AgentBinary {
    match tool {
        AutoModeTool::Claude | AutoModeTool::RustyClawd => AgentBinary::Claude,
        AutoModeTool::Copilot => AgentBinary::Copilot,
        AutoModeTool::Codex => AgentBinary::Codex,
        AutoModeTool::Amplifier => AgentBinary::Amplifier,
    }
}

/// Strip Claude-only flags that Copilot CLI does not accept.
///
/// Removes `--dangerously-skip-permissions` and `--disallowed-tools` (with their
/// values) from args.  Mirrors the normalizer in Python `rust_runner_copilot.py`
/// (PR #4142).
fn strip_claude_only_flags(args: Vec<String>) -> Vec<String> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--dangerously-skip-permissions"
            || arg.starts_with("--dangerously-skip-permissions=")
        {
            i += 1;
            continue;
        }
        if arg == "--disallowed-tools" {
            i += if i + 1 < args.len() { 2 } else { 1 };
            continue;
        }
        if arg.starts_with("--disallowed-tools=") {
            i += 1;
            continue;
        }
        filtered.push(arg.clone());
        i += 1;
    }
    filtered
}
