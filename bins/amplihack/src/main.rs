//! CLI binary for amplihack.
//!
//! Entry point that parses CLI arguments via amplihack-cli
//! and dispatches to the appropriate command handler.

use amplihack_cli::Cli;
use amplihack_cli::command_error;
use amplihack_cli::commands;
use amplihack_cli::update;

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "internal")
        && args.get(2).is_some_and(|arg| arg == "workflow-checkpoint")
    {
        // The private checkpoint surface (#1554) bypasses clap: clap would
        // read `--help`, `-V` or a first `--` as its own before the engine's
        // gate saw them (17.138 #5). Only this route passes a coded exit
        // status through; the checkpoint has already said why on stderr.
        amplihack_cli::workflow_checkpoint::install_panic_hook();
        let status = match amplihack_cli::workflow_checkpoint::run_argv(&args[3..]) {
            Ok(()) => 0,
            Err(error) => command_error::exit_code(&error).unwrap_or_else(|| {
                // Every checkpoint failure carries a status; one without is
                // a contract break, reported on one line as status 1.
                eprintln!(
                    "{}{error:#}",
                    amplihack_cli::workflow_checkpoint::CRASH_PREFIX
                );
                1
            }),
        };
        // std::process::exit skips destructors, so flush both streams here
        // or a buffered record or diagnostic could be lost. A flush error is
        // discarded: the stream is already broken, so there is nowhere left
        // to report it, and the exit status must stay the checkpoint's own.
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(status);
    }
    if args.get(1).is_some_and(|arg| arg == "internal") {
        let cli = Cli::parse_from(&args);
        // Every other internal command keeps main's behaviour: print the
        // error and exit 1. validate-recipe-runner prints its JSON and exits
        // itself, so it never reaches this printer on failure.
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

    if matches!(
        update::maybe_print_update_notice_from_args(&args),
        update::StartupUpdateOutcome::ExitSuccess
    ) {
        return;
    }

    // clap parses (and short-circuits on --help/--version/unknown commands)
    // before self_heal runs, so typos and help requests never pay for an
    // install. Subcommands like `install`/`update`/`uninstall` are filtered
    // by self_heal's own skip-list to prevent recursion.
    let cli = Cli::parse_from(&args);

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
