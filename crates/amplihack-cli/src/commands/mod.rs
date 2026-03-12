//! Command dispatch for all CLI subcommands.

pub mod install;
pub mod launch;
pub mod memory;
pub mod mode;
pub mod plugin;
pub mod python_delegate;
pub mod recipe;

use crate::{Commands, MemoryCommands, ModeCommands, PluginCommands, RecipeCommands};
use anyhow::Result;

/// Dispatch a parsed CLI command to the appropriate handler.
pub fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Install { local } => install::run_install(local),
        Commands::Uninstall => install::run_uninstall(),
        Commands::Launch {
            resume,
            continue_session,
            skip_permissions,
            claude_args,
        } => launch::run_launch(
            "claude",
            resume,
            continue_session,
            skip_permissions,
            claude_args,
        ),
        Commands::Claude { claude_args } => {
            launch::run_launch("claude", false, false, false, claude_args)
        }
        Commands::Copilot { args } => launch::run_launch("copilot", false, false, false, args),
        Commands::Codex { args } => launch::run_launch("codex", false, false, false, args),
        Commands::Amplifier { args } => launch::run_launch("amplifier", false, false, false, args),
        Commands::Plugin { command } => dispatch_plugin(command),
        Commands::Memory { command } => dispatch_memory(command),
        Commands::Recipe { command } => dispatch_recipe(command),
        Commands::Mode { command } => dispatch_mode(command),
        Commands::Version => {
            println!("amplihack-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::Update => crate::update::run_update(),
        Commands::Fleet { args } => python_delegate::delegate_to_python("fleet", &args),
        Commands::New { args } => python_delegate::delegate_to_python("new", &args),
        #[allow(non_snake_case)]
        Commands::RustyClawd { args } => python_delegate::delegate_to_python("RustyClawd", &args),
        Commands::UvxHelp { args } => python_delegate::delegate_to_python("uvx-help", &args),
    }
}

fn dispatch_plugin(command: PluginCommands) -> Result<()> {
    match command {
        PluginCommands::Install { source, force } => plugin::run_install(&source, force),
        PluginCommands::Uninstall { plugin_name } => plugin::run_uninstall(&plugin_name),
        PluginCommands::Link { plugin_name } => plugin::run_link(&plugin_name),
        PluginCommands::Verify { plugin_name } => plugin::run_verify(&plugin_name),
    }
}

fn dispatch_memory(command: MemoryCommands) -> Result<()> {
    match command {
        MemoryCommands::Tree {
            session,
            memory_type,
            depth,
            backend,
        } => memory::run_tree(session.as_deref(), memory_type.as_deref(), depth, &backend),
        MemoryCommands::Export {
            agent,
            output,
            format,
            storage_path,
        } => memory::run_export(&agent, &output, &format, storage_path.as_deref()),
        MemoryCommands::Import {
            agent,
            input,
            format,
            merge,
            storage_path,
        } => memory::run_import(&agent, &input, &format, merge, storage_path.as_deref()),
        MemoryCommands::Clean {
            pattern,
            backend,
            no_dry_run,
            confirm,
        } => memory::run_clean(&pattern, &backend, !no_dry_run, confirm),
    }
}

fn dispatch_recipe(command: RecipeCommands) -> Result<()> {
    match command {
        RecipeCommands::Run {
            recipe_path,
            context,
            dry_run,
            verbose,
            format,
            working_dir,
        } => recipe::run_recipe(
            &recipe_path,
            &context,
            dry_run,
            verbose,
            &format,
            working_dir.as_deref(),
        ),
        RecipeCommands::List {
            recipe_dir,
            format,
            tags,
            verbose,
        } => recipe::run_list(recipe_dir.as_deref(), &format, &tags, verbose),
        RecipeCommands::Validate {
            file,
            verbose,
            format,
        } => recipe::run_validate(&file, verbose, &format),
        RecipeCommands::Show {
            name,
            format,
            no_steps,
            no_context,
        } => recipe::run_show(&name, &format, !no_steps, !no_context),
    }
}

fn dispatch_mode(command: ModeCommands) -> Result<()> {
    match command {
        ModeCommands::Detect => mode::run_detect(),
        ModeCommands::ToPlugin => mode::run_to_plugin(),
        ModeCommands::ToLocal => mode::run_to_local(),
    }
}
