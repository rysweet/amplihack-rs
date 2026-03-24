//! Command dispatch for all CLI subcommands.

pub mod completions;
pub mod doctor;
pub mod fleet;
pub mod install;
pub mod launch;
pub mod memory;
pub mod mode;
pub mod new_agent;
pub mod plugin;
pub mod query_code;
pub mod recipe;
pub mod rustyclawd;
pub mod uvx_help;

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
            skip_permissions: _skip_permissions, // always true for Python launcher parity
            skip_update_check,
            claude_args,
        } => launch::run_launch(
            "claude",
            resume,
            continue_session,
            true, // always inject --dangerously-skip-permissions (matches Python launcher)
            skip_update_check,
            claude_args,
        ),
        Commands::Claude {
            resume,
            continue_session,
            claude_args,
        } => {
            // Always inject --dangerously-skip-permissions to match Python launcher parity.
            launch::run_launch("claude", resume, continue_session, true, false, claude_args)
        }
        Commands::Copilot { args } => {
            launch::run_launch("copilot", false, false, true, false, args)
        }
        Commands::Codex { args } => launch::run_launch("codex", false, false, true, false, args),
        Commands::Amplifier { args } => {
            launch::run_launch("amplifier", false, false, true, false, args)
        }
        Commands::Plugin { command } => dispatch_plugin(command),
        Commands::Memory { command } => dispatch_memory(command),
        Commands::IndexCode { input, kuzu_path } => {
            memory::run_index_code(&input, kuzu_path.as_deref())
        }
        Commands::IndexScip {
            project_path,
            languages,
        } => memory::run_index_scip(project_path.as_deref(), &languages),
        Commands::QueryCode {
            kuzu_path,
            json,
            limit,
            command,
        } => query_code::run_query_code(command, kuzu_path.as_deref(), json, limit),
        Commands::Recipe { command } => dispatch_recipe(command),
        Commands::Mode { command } => dispatch_mode(command),
        Commands::Version => {
            println!("amplihack-rs {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Commands::Update => crate::update::run_update(),
        Commands::Fleet { args } => fleet::run_fleet(args),
        Commands::New {
            file,
            output,
            name,
            skills_dir,
            verbose,
            enable_memory,
            sdk,
            multi_agent,
            enable_spawning,
        } => new_agent::run_new(
            &file,
            output.as_deref(),
            name.as_deref(),
            skills_dir.as_deref(),
            verbose,
            enable_memory,
            &sdk,
            multi_agent,
            enable_spawning,
        ),
        #[allow(non_snake_case)]
        Commands::RustyClawd { args } => rustyclawd::run_rustyclawd(args),
        Commands::UvxHelp { find_path, info } => uvx_help::run_uvx_help(find_path, info),
        Commands::Completions { shell } => completions::run_completions(shell),
        Commands::Doctor => doctor::run_doctor(),
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
