use std::collections::HashMap;

use amplihack_delegation::platform_cli::{
    parsers, validate_extra_args, validate_working_dir, AmplifierCli, ClaudeCodeCli,
    CopilotCli, PlatformCli, available_platforms, get_platform,
};

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

#[test]
fn validate_extra_args_accepts_whitelist() {
    let ok = vec!["--debug".into(), "-v".into(), "--json".into()];
    assert!(validate_extra_args(&ok).is_ok());
}

#[test]
fn validate_extra_args_rejects_bad_flag() {
    let bad = vec!["--rm-rf".into()];
    let err = validate_extra_args(&bad).unwrap_err();
    assert!(err.to_string().contains("not allowed"));
}

#[test]
fn validate_working_dir_rejects_empty() {
    assert!(validate_working_dir("").is_err());
}

#[test]
fn validate_working_dir_rejects_path_traversal() {
    assert!(validate_working_dir("/tmp/../etc").is_err());
}

#[test]
fn validate_working_dir_accepts_existing_dir() {
    // The repo root must exist.
    assert!(validate_working_dir("/home/azureuser/src/amplihack-rs").is_ok());
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

#[test]
fn default_platform_is_claude_code() {
    let name = get_platform(None).expect("default");
    assert_eq!(name, "claude-code");
}

#[test]
fn get_platform_copilot() {
    let name = get_platform(Some("copilot")).expect("copilot exists");
    assert_eq!(name, "copilot");
}

#[test]
fn get_platform_amplifier() {
    let name = get_platform(Some("amplifier")).expect("amplifier exists");
    assert_eq!(name, "amplifier");
}

#[test]
fn unknown_platform_errors() {
    assert!(get_platform(Some("quantum-ai")).is_err());
}

#[test]
fn available_platforms_has_three() {
    let platforms = available_platforms();
    assert!(platforms.len() >= 3, "expected ≥3 platforms: {platforms:?}");
    assert!(platforms.contains(&"claude-code".to_string()));
    assert!(platforms.contains(&"copilot".to_string()));
    assert!(platforms.contains(&"amplifier".to_string()));
}

// ---------------------------------------------------------------------------
// Prompt formatting (via parsers module)
// ---------------------------------------------------------------------------

#[test]
fn claude_guide_prompt() {
    let p = parsers::format_claude_prompt("build API", "guide", "REST");
    assert!(p.contains("guide persona"));
    assert!(p.contains("build API"));
}

#[test]
fn claude_unknown_persona_fallback() {
    let p = parsers::format_claude_prompt("goal", "alien", "ctx");
    assert!(p.starts_with("**Goal:**"));
}

#[test]
fn copilot_prompt_prefix() {
    let p = parsers::format_copilot_prompt("fix bug", "architect", "prod");
    assert!(p.starts_with("As a software architect, "));
}

#[test]
fn amplifier_prompt_structure() {
    let p = parsers::format_amplifier_prompt("do thing", "guide", "context");
    assert!(p.contains("Goal: do thing"));
    assert!(p.contains("Persona: guide"));
    assert!(p.contains("Context: context"));
}

// ---------------------------------------------------------------------------
// SpawnConfig building
// ---------------------------------------------------------------------------

#[test]
fn claude_spawn_config() {
    let cli = ClaudeCodeCli;
    let env = HashMap::new();
    let cfg = cli
        .build_spawn_config(
            "test goal",
            "guide",
            "/home/azureuser/src/amplihack-rs",
            &env,
            &[],
            "ctx",
        )
        .expect("config");
    assert_eq!(cfg.command[0], "claude");
    assert!(cfg.command.contains(&"-p".to_string()));
    assert_eq!(cfg.environment.get("CI").map(|s| s.as_str()), Some("true"));
}

#[test]
fn copilot_spawn_config() {
    let cli = CopilotCli;
    let cfg = cli
        .build_spawn_config(
            "goal",
            "qa_engineer",
            "/home/azureuser/src/amplihack-rs",
            &HashMap::new(),
            &[],
            "",
        )
        .expect("config");
    assert_eq!(cfg.command[0], "gh");
    assert_eq!(cfg.command[1], "copilot");
    assert_eq!(cfg.command[2], "suggest");
}

#[test]
fn amplifier_spawn_config() {
    let cli = AmplifierCli;
    let cfg = cli
        .build_spawn_config(
            "goal",
            "junior_dev",
            "/home/azureuser/src/amplihack-rs",
            &HashMap::new(),
            &[],
            "",
        )
        .expect("config");
    assert_eq!(cfg.command[0], "amplifier");
    assert_eq!(cfg.command[1], "run");
}

#[test]
fn spawn_config_rejects_bad_working_dir() {
    let cli = ClaudeCodeCli;
    let err = cli
        .build_spawn_config("g", "p", "/nonexistent/dir", &HashMap::new(), &[], "")
        .unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn spawn_config_rejects_bad_extra_args() {
    let cli = ClaudeCodeCli;
    let err = cli
        .build_spawn_config(
            "g",
            "p",
            "/home/azureuser/src/amplihack-rs",
            &HashMap::new(),
            &["--evil".to_string()],
            "",
        )
        .unwrap_err();
    assert!(err.to_string().contains("not allowed"));
}

// ---------------------------------------------------------------------------
// parse_output
// ---------------------------------------------------------------------------

#[test]
fn parse_output_returns_stdout() {
    let cli = ClaudeCodeCli;
    let out = cli.parse_output("hello world");
    assert_eq!(out.get("stdout").map(|s| s.as_str()), Some("hello world"));
}

#[test]
fn platform_names() {
    assert_eq!(ClaudeCodeCli.platform_name(), "claude-code");
    assert_eq!(CopilotCli.platform_name(), "copilot");
    assert_eq!(AmplifierCli.platform_name(), "amplifier");
}
