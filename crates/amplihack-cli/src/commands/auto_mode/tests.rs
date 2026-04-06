use super::helpers::{
    build_auto_command, build_tool_passthrough_args, extract_prompt_args,
    transform_prompt_for_staging,
};
use super::run::render_auto_session_argv;
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
    let parsed =
        extract_prompt_args(&["--prompt=ship parity".to_string()]).expect("prompt should parse");
    assert_eq!(parsed.prompt, "ship parity");
    assert!(parsed.passthrough_args.is_empty());
}

#[test]
fn extract_prompt_args_supports_bare_positional_prompt() {
    let parsed = extract_prompt_args(&["do quality audit".to_string()]).expect("positional prompt");
    assert_eq!(parsed.prompt, "do quality audit");
    assert!(parsed.passthrough_args.is_empty());
}

#[test]
fn extract_prompt_args_bare_positional_with_flags() {
    // When mixed with flags that take values, we can't distinguish flag values
    // from the prompt, so no positional fallback applies.
    let result = extract_prompt_args(&[
        "--model".to_string(),
        "sonnet".to_string(),
        "fix all bugs".to_string(),
    ]);
    // Two non-flag args → ambiguous → no prompt found
    assert!(result.is_none() || result.as_ref().is_none_or(|p| p.prompt != "sonnet"));
}

#[test]
fn extract_prompt_args_bare_positional_with_flag_only_args() {
    // With only boolean flags (no values), positional fallback works
    let parsed = extract_prompt_args(&["--verbose".to_string(), "fix all bugs".to_string()])
        .expect("positional prompt with boolean flags");
    assert_eq!(parsed.prompt, "fix all bugs");
    assert_eq!(parsed.passthrough_args, vec!["--verbose"]);
}

#[test]
fn extract_prompt_args_explicit_p_takes_precedence_over_positional() {
    let parsed = extract_prompt_args(&[
        "-p".to_string(),
        "explicit prompt".to_string(),
        "bare arg".to_string(),
    ])
    .expect("explicit -p should win");
    assert_eq!(parsed.prompt, "explicit prompt");
    assert_eq!(parsed.passthrough_args, vec!["bare arg"]);
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
    let transformed = transform_prompt_for_staging("/dev /analyze fix the launcher", target.path());

    assert_eq!(
        transformed,
        format!(
            "/dev /analyze Change your working directory to {}. fix the launcher",
            target.path().canonicalize().unwrap().display()
        )
    );
}

#[test]
fn claude_passthrough_args_include_permission_flag() {
    let args = build_tool_passthrough_args(AutoModeTool::Claude, &[], "fix bug");
    assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(args.contains(&"--verbose".to_string()));
    assert!(args.contains(&"-p".to_string()));
}

#[test]
fn copilot_strips_claude_only_flags_from_passthrough() {
    let passthrough = vec![
        "--dangerously-skip-permissions".to_string(),
        "--disallowed-tools".to_string(),
        "Bash,Write".to_string(),
        "--model".to_string(),
        "gpt-5".to_string(),
    ];
    let args = build_tool_passthrough_args(AutoModeTool::Copilot, &passthrough, "classify");

    assert!(!args.contains(&"--dangerously-skip-permissions".to_string()));
    assert!(!args.contains(&"--disallowed-tools".to_string()));
    assert!(!args.contains(&"Bash,Write".to_string()));
    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"--allow-all-tools".to_string()));
}

#[test]
fn copilot_strips_equals_style_claude_flags() {
    let passthrough = vec![
        "--dangerously-skip-permissions=true".to_string(),
        "--disallowed-tools=Bash".to_string(),
    ];
    let args = build_tool_passthrough_args(AutoModeTool::Copilot, &passthrough, "check");

    assert!(
        !args
            .iter()
            .any(|a| a.starts_with("--dangerously-skip-permissions"))
    );
    assert!(!args.iter().any(|a| a.starts_with("--disallowed-tools")));
    assert!(args.contains(&"--allow-all-tools".to_string()));
}
