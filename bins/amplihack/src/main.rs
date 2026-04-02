//! CLI binary for amplihack.
//!
//! Entry point that parses CLI arguments via amplihack-cli
//! and dispatches to the appropriate command handler.

use amplihack_cli::Cli;
use amplihack_cli::command_error;
use amplihack_cli::commands;
use amplihack_cli::update;
use clap::Parser;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .init();

    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if matches!(
        update::maybe_print_update_notice_from_args(&args),
        update::StartupUpdateOutcome::ExitSuccess
    ) {
        return;
    }
    let cli = Cli::parse_from(args);

    if let Err(e) = commands::dispatch(cli.command) {
        if let Some(code) = command_error::exit_code(&e) {
            std::process::exit(code);
        }
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
