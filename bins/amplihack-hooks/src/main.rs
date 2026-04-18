//! Multicall hook binary.
//!
//! Single binary that dispatches to the correct hook based on the
//! first CLI argument (subcommand):
//!
//! ```text
//! amplihack-hooks pre-tool-use    → PreToolUseHook
//! amplihack-hooks post-tool-use   → PostToolUseHook
//! amplihack-hooks stop            → StopHook
//! amplihack-hooks session-start   → SessionStartHook
//! amplihack-hooks session-stop    → SessionStopHook
//! amplihack-hooks user-prompt     → UserPromptSubmitHook
//! amplihack-hooks pre-compact     → PreCompactHook
//! ```

use amplihack_hooks::{
    post_tool_use::PostToolUseHook, pre_compact::PreCompactHook, pre_tool_use::PreToolUseHook,
    protocol::run_hook, session_start::SessionStartHook, session_stop::SessionStopHook,
    stop::StopHook, user_prompt::UserPromptSubmitHook,
    workflow_classification::WorkflowClassificationReminderHook,
};

fn main() {
    // Initialize minimal tracing for telemetry.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .json()
        .try_init();

    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str).unwrap_or("");

    // Keeps the hooks binary on the same version-override contract as the
    // main `amplihack` binary (see `amplihack_cli::VERSION`). Without the
    // `AMPLIHACK_RELEASE_VERSION` env override here, the hooks binary would
    // self-report the stale Cargo.toml version while amplihack itself
    // reports the tagged release version, tripping the post-update
    // `verify_installed_version` check.
    const VERSION: &str = match option_env!("AMPLIHACK_RELEASE_VERSION") {
        Some(v) => v,
        None => env!("CARGO_PKG_VERSION"),
    };

    match subcommand {
        "--version" | "-V" => {
            // Mirrors clap's default `--version` output. Used by the self-update
            // post-install check to verify the hooks binary was replaced in
            // lockstep with the amplihack binary.
            println!("amplihack-hooks {VERSION}");
        }
        "pre-tool-use" => run_hook(PreToolUseHook),
        "post-tool-use" => run_hook(PostToolUseHook),
        "stop" => run_hook(StopHook),
        "session-start" => run_hook(SessionStartHook),
        "session-stop" => run_hook(SessionStopHook),
        "workflow-classification-reminder" => run_hook(WorkflowClassificationReminderHook),
        "user-prompt" | "user-prompt-submit" => run_hook(UserPromptSubmitHook),
        "pre-compact" => run_hook(PreCompactHook),
        other => {
            eprintln!(
                "amplihack-hooks: unknown subcommand '{}'\n\n\
                Usage: amplihack-hooks <hook-name>\n\n\
                Available hooks:\n  \
                pre-tool-use\n  \
                post-tool-use\n  \
                stop\n  \
                session-start\n  \
                session-stop\n  \
                workflow-classification-reminder\n  \
                user-prompt\n  \
                pre-compact",
                other
            );
            std::process::exit(1);
        }
    }
}
