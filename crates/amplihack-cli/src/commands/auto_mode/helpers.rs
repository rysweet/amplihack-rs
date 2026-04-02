use super::*;

pub(super) fn philosophy_context() -> &'static str {
    "AUTONOMOUS MODE: You are in auto mode. Do NOT ask questions. Make decisions using:\n1. Explicit user requirements (highest priority)\n2. @.claude/context/USER_PREFERENCES.md guidance\n3. @.claude/context/PHILOSOPHY.md principles\n4. @.claude/workflow/DEFAULT_WORKFLOW.md patterns\n5. @.claude/context/USER_REQUIREMENT_PRIORITY.md for conflicts\n\nDecision Authority:\n- YOU DECIDE implementation details and architecture\n- YOU PRESERVE explicit user requirements and hard constraints\n- WHEN AMBIGUOUS choose the simplest modular option"
}

pub(super) fn extract_prompt_args(args: &[String]) -> Option<ParsedPromptArgs> {
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

pub(super) fn build_auto_command(
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

pub(super) fn build_tool_passthrough_args(
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
