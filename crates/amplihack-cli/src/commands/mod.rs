//! Command dispatch for all CLI subcommands.

pub mod install;
pub mod launch;
pub mod memory;
pub mod mode;
pub mod plugin;
pub mod recipe;

use crate::{Commands, MemoryCommands, ModeCommands, PluginCommands, RecipeCommands};
use anyhow::Result;

/// Dispatch a parsed CLI command to the appropriate handler.
pub fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Install => install::run_install(),
        Commands::Uninstall => install::run_uninstall(),
        Commands::Launch {
            resume,
            continue_session,
            claude_args,
        } => launch::run_launch("claude", resume, continue_session, claude_args),
        Commands::Claude { claude_args } => launch::run_launch("claude", false, false, claude_args),
        Commands::Copilot { args } => launch::run_launch("copilot", false, false, args),
        Commands::Codex { args } => launch::run_launch("codex", false, false, args),
        Commands::Amplifier { args } => launch::run_launch("amplifier", false, false, args),
        Commands::Plugin { command } => dispatch_plugin(command),
        Commands::Memory { command } => dispatch_memory(command),
        Commands::Recipe { command } => dispatch_recipe(command),
        Commands::Mode { command } => dispatch_mode(command),
        Commands::Version => {
            println!("amplihack-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

fn dispatch_plugin(command: PluginCommands) -> Result<()> {
    match command {
        PluginCommands::Install { name } => plugin::run_install(&name),
        PluginCommands::Uninstall { name } => plugin::run_uninstall(&name),
        PluginCommands::Link { path } => plugin::run_link(&path),
        PluginCommands::Verify => plugin::run_verify(),
    }
}

fn dispatch_memory(command: MemoryCommands) -> Result<()> {
    match command {
        MemoryCommands::Tree => memory::run_tree(),
        MemoryCommands::Export { output } => memory::run_export(output.as_deref()),
        MemoryCommands::Import { file } => memory::run_import(&file),
        MemoryCommands::Clean => memory::run_clean(),
    }
}

fn dispatch_recipe(command: RecipeCommands) -> Result<()> {
    match command {
        RecipeCommands::Run { name, args } => recipe::run_recipe(&name, &args),
        RecipeCommands::List => recipe::run_list(),
        RecipeCommands::Validate { file } => recipe::run_validate(&file),
        RecipeCommands::Show { name } => recipe::run_show(&name),
    }
}

fn dispatch_mode(command: ModeCommands) -> Result<()> {
    match command {
        ModeCommands::Detect => mode::run_detect(),
        ModeCommands::ToPlugin => mode::run_to_plugin(),
        ModeCommands::ToLocal => mode::run_to_local(),
    }
}
