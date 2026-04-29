//! Command dispatch for all CLI subcommands.

pub mod append;
pub mod auto_mode;
pub mod completions;
pub mod doctor;
pub mod fleet;
pub mod hive_haymaker;
pub mod install;
pub mod launch;
pub mod memory;
pub mod mode;
pub mod multitask;
pub mod new_agent;
pub mod orch;
pub mod plugin;
pub mod pr;
pub mod query_code;
pub mod recipe;
pub mod rustyclawd;
pub mod session_tree;
pub mod uvx_help;

use crate::{
    Commands, MemoryCommands, ModeCommands, MultitaskCommands, PluginCommands, RecipeCommands,
};
use anyhow::Result;

/// Dispatch a parsed CLI command to the appropriate handler.
pub fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Install { local, interactive } => install::run_install(local, interactive),
        Commands::Uninstall => install::run_uninstall(),
        Commands::Launch {
            resume,
            continue_session,
            skip_permissions: _skip_permissions, // always true for Python launcher parity
            skip_update_check,
            no_reflection,
            subprocess_safe,
            checkout_repo,
            docker,
            append,
            auto,
            max_turns,
            ui,
            claude_args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                let working_dir = launch::resolve_checkout_repo(checkout_repo.as_deref())?;
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::Claude,
                    max_turns,
                    ui,
                    claude_args,
                    checkout_repo,
                    working_dir,
                );
            }
            launch::run_launch(
                "claude",
                "launch",
                docker,
                resume,
                continue_session,
                true, // always inject --dangerously-skip-permissions (matches Python launcher)
                skip_update_check,
                no_reflection,
                subprocess_safe,
                checkout_repo,
                claude_args,
            )
        }
        Commands::Claude {
            no_reflection,
            subprocess_safe,
            checkout_repo,
            docker,
            append,
            auto,
            max_turns,
            ui,
            claude_args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                let working_dir = launch::resolve_checkout_repo(checkout_repo.as_deref())?;
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::Claude,
                    max_turns,
                    ui,
                    claude_args,
                    checkout_repo,
                    working_dir,
                );
            }
            // Always inject --dangerously-skip-permissions to match Python launcher parity.
            launch::run_launch(
                "claude",
                "claude",
                docker,
                false,
                false,
                true,
                false,
                no_reflection,
                subprocess_safe,
                checkout_repo,
                claude_args,
            )
        }
        Commands::Copilot {
            no_reflection,
            subprocess_safe,
            docker,
            append,
            auto,
            max_turns,
            ui,
            args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::Copilot,
                    max_turns,
                    ui,
                    args,
                    None,
                    None,
                );
            }
            launch::run_launch(
                "copilot",
                "copilot",
                docker,
                false,
                false,
                true,
                false,
                no_reflection,
                subprocess_safe,
                None,
                args,
            )
        }
        Commands::Codex {
            no_reflection,
            subprocess_safe,
            docker,
            append,
            auto,
            max_turns,
            ui,
            args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::Codex,
                    max_turns,
                    ui,
                    args,
                    None,
                    None,
                );
            }
            launch::run_launch(
                "codex",
                "codex",
                docker,
                false,
                false,
                true,
                false,
                no_reflection,
                subprocess_safe,
                None,
                args,
            )
        }
        Commands::Amplifier {
            no_reflection,
            subprocess_safe,
            docker,
            append,
            auto,
            max_turns,
            ui,
            args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::Amplifier,
                    max_turns,
                    ui,
                    args,
                    None,
                    None,
                );
            }
            launch::run_launch(
                "amplifier",
                "amplifier",
                docker,
                false,
                false,
                true,
                false,
                no_reflection,
                subprocess_safe,
                None,
                args,
            )
        }
        Commands::Plugin { command } => dispatch_plugin(command),
        Commands::Memory { command } => dispatch_memory(command),
        Commands::IndexCode {
            input,
            db_path,
            legacy_kuzu_path,
        } => memory::run_index_code(
            &input,
            db_path.as_deref().or(legacy_kuzu_path.as_deref()),
            legacy_kuzu_path.is_some(),
        ),

        Commands::IndexScip {
            project_path,
            languages,
        } => memory::run_index_scip(project_path.as_deref(), &languages),
        Commands::QueryCode {
            db_path,
            legacy_kuzu_path,
            json,
            limit,
            command,
        } => query_code::run_query_code(
            command,
            db_path.as_deref().or(legacy_kuzu_path.as_deref()),
            legacy_kuzu_path.is_some(),
            json,
            limit,
        ),
        Commands::Recipe { command } => dispatch_recipe(command),
        Commands::Mode { command } => dispatch_mode(command),
        Commands::Version => {
            println!("amplihack-rs {}", crate::VERSION);
            Ok(())
        }
        Commands::Update { skip_install } => crate::update::run_update(skip_install),
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
        Commands::RustyClawd {
            append,
            no_reflection,
            subprocess_safe,
            auto,
            max_turns,
            ui,
            args,
        } => {
            if let Some(instruction) = append {
                return append::run_append(&instruction);
            }
            if auto {
                return auto_mode::run_auto_mode(
                    auto_mode::AutoModeTool::RustyClawd,
                    max_turns,
                    ui,
                    args,
                    None,
                    None,
                );
            }
            rustyclawd::run_rustyclawd(args, no_reflection, subprocess_safe)
        }
        Commands::UvxHelp { find_path, info } => uvx_help::run_uvx_help(find_path, info),
        Commands::Completions { shell } => completions::run_completions(shell),
        Commands::Doctor => doctor::run_doctor(),
        Commands::ResolveBundleAsset { asset } => {
            let code = crate::resolve_bundle_asset::run_cli(&asset);
            std::process::exit(code);
        }
        Commands::Multitask { command } => dispatch_multitask(command),
        Commands::Orch { command } => orch::dispatch(command),
        Commands::Pr { command } => pr::run(command),
        Commands::SessionTree { command } => session_tree::run(command),
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
            step_timeout,
        } => recipe::run_recipe(
            &recipe_path,
            &context,
            dry_run,
            verbose,
            &format,
            working_dir.as_deref(),
            step_timeout,
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

fn dispatch_multitask(command: MultitaskCommands) -> Result<()> {
    match command {
        MultitaskCommands::Run {
            config,
            mode,
            recipe,
            max_runtime,
            timeout_policy,
            dry_run,
        } => multitask::run_multitask(
            &config,
            &mode,
            &recipe,
            max_runtime,
            timeout_policy.as_deref(),
            dry_run,
        ),
        MultitaskCommands::Cleanup { config, dry_run } => multitask::run_cleanup(&config, dry_run),
        MultitaskCommands::Status { base_dir } => multitask::run_status(base_dir.as_deref()),
    }
}
