//! CLI binary for amplihack.
//!
//! Entry point that parses CLI arguments via amplihack-cli
//! and dispatches to the appropriate command handler.

use amplihack_cli::Cli;
use amplihack_cli::command_error;
use amplihack_cli::commands;
use amplihack_cli::update;

mod startup;

fn main() {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "internal") {
        let cli = Cli::parse_from(&args);
        if let Err(error) = commands::dispatch(cli.command) {
            eprintln!("error: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    // Logs go to stderr; stdout is data. `tracing_subscriber::fmt()` writes to
    // stdout unless told otherwise, so under `RUST_LOG` every log line used to
    // land in front of a command's real output. For
    // `$(amplihack agent-binary --shell)`, which is spliced into a command line,
    // that turned the hand-off into `2026-...Z: not found` (#1525). The same
    // applies to any caller that captures amplihack's stdout.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();

    let startup_intent = amplihack_cli::commands::litellm::startup_guard::classify_startup(&args);
    if startup_intent == amplihack_cli::commands::litellm::startup_guard::StartupIntent::VerifyLive
        && amplihack_cli::commands::litellm::startup_guard::ci_environment_present()
    {
        amplihack_cli::commands::litellm::startup_guard::print_ci_refusal();
        std::process::exit(78);
    }

    if startup_intent != amplihack_cli::commands::litellm::startup_guard::StartupIntent::Ordinary {
        let cli = Cli::parse_from(&args);
        if let Err(error) = commands::dispatch(cli.command) {
            if let Some(code) = command_error::exit_code(&error) {
                std::process::exit(code);
            }
            eprintln!("error: {error:#}");
            std::process::exit(1);
        }
        return;
    }

    // Parse once before any updater can install/reconcile assets. Reset is a
    // locked metadata operation; failure stops both updates and self-healing.
    let mut cli = Cli::parse_from(&args);
    match startup::prepare(&mut cli.command, &mut args) {
        Ok(update::StartupUpdateOutcome::ExitSuccess) => return,
        Ok(update::StartupUpdateOutcome::Continue) => {}
        Err(error) => {
            eprintln!("amplihack: startup recovery failed: {error:#}");
            std::process::exit(1);
        }
    }

    if let Err(e) = amplihack_cli::self_heal::resolve_startup_client(&cli.command)
        .and_then(|()| amplihack_cli::self_heal::ensure_assets_for_command(&args, &cli.command))
    {
        eprintln!("amplihack: self-heal failed: {e:#}");
        std::process::exit(1);
    }

    if let Err(e) = commands::dispatch(cli.command) {
        if let Some(code) = command_error::exit_code(&e) {
            std::process::exit(code);
        }
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
