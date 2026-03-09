//! CLI binary for amplihack.
//!
//! Placeholder for Phase 2 — CLI + launcher implementation.
//! Currently just provides version info and help.

use clap::Parser;

/// amplihack CLI — Rust core runtime for deterministic infrastructure.
#[derive(Parser, Debug)]
#[command(name = "amplihack", version, about)]
enum Cli {
    /// Show version information.
    Version,

    /// Run hooks comparison (development tool).
    #[command(subcommand)]
    Hooks(HooksCmd),
}

#[derive(Parser, Debug)]
enum HooksCmd {
    /// Compare Python and Rust hook output for a given input.
    Compare {
        /// Path to the input JSON file.
        #[arg(short, long)]
        input: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli {
        Cli::Version => {
            println!("amplihack-rs {}", env!("CARGO_PKG_VERSION"));
        }
        Cli::Hooks(cmd) => match cmd {
            HooksCmd::Compare { input } => {
                println!(
                    "Hook comparison tool (input: {})",
                    input.as_deref().unwrap_or("stdin")
                );
                println!("TODO: Implement in Phase 2");
            }
        },
    }
}
