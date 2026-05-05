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
    precommit_prefs, protocol::run_hook, session_start::SessionStartHook,
    session_stop::SessionStopHook, stop::StopHook, user_prompt::UserPromptSubmitHook,
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
        // `session-end` and `session-stop` are aliases for `stop`. Dispatching
        // to the same StopHook instance keeps behavior identical for hosts that
        // still use either event name.
        "stop" | "session-end" | "session-stop" => run_hook(StopHook),
        "session-start" => run_hook(SessionStartHook),
        // SessionStop event handler (distinct from the alias above) — kept
        // for hosts that wire SessionStop separately from Stop.
        "session-stop-event" => run_hook(SessionStopHook),
        "workflow-classification-reminder" => run_hook(WorkflowClassificationReminderHook),
        "user-prompt" | "user-prompt-submit" => run_hook(UserPromptSubmitHook),
        "pre-compact" => run_hook(PreCompactHook),
        // No-op pre-commit hook. Drains stdin and exits 0 — see
        // precommit_prefs::run docs for the security contract (no logging, no
        // echoing payload).
        "precommit-prefs" => {
            let mut stdin = std::io::stdin().lock();
            if let Err(e) = precommit_prefs::run(&mut stdin) {
                // Hooks must never block the session on infrastructure faults.
                // Surface the diagnostic on stderr (observable in hook logs)
                // and exit 0 — same fail-open contract used by run_hook().
                eprintln!("precommit-prefs: stdin drain failed: {e}");
            }
        }
        other => {
            eprintln!(
                "amplihack-hooks: unknown subcommand '{other}'\n\n\
                Usage: amplihack-hooks <hook-name>\n\n\
                Available hooks:\n  \
                pre-tool-use\n  \
                post-tool-use\n  \
                stop\n  \
                session-end (alias for stop)\n  \
                session-stop (alias for stop)\n  \
                session-start\n  \
                session-stop-event\n  \
                workflow-classification-reminder\n  \
                user-prompt\n  \
                pre-compact\n  \
                precommit-prefs"
            );
            std::process::exit(1);
        }
    }
}
