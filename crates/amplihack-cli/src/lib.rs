//! CLI command parsing, launcher, and process management for amplihack.
//!
//! This crate provides the full CLI experience: argument parsing via clap derive,
//! binary discovery, environment construction, process management with signal
//! handling, and nesting detection.

pub mod auto_mode_append;
pub mod auto_mode_completion_signals;
pub mod auto_mode_completion_verifier;
pub mod auto_mode_state;
pub mod auto_mode_ui;
pub mod auto_mode_work_summary;
pub mod auto_mode_work_summary_generator;
pub mod auto_stager;
pub mod auto_update;
pub mod binary_finder;
pub mod bootstrap;
pub mod claude_plugin;
mod cli_commands;
pub mod cli_extensions;
mod cli_subcommands;
#[cfg(test)]
mod cli_tests;
pub mod command_error;
pub mod commands;
pub mod copilot_setup;
pub mod docker;
pub mod env_builder;
/// Local session management dashboard (fleet_local).
///
/// Python-to-Rust port of the amploxy local session TUI.
/// Reads `~/.claude/runtime/locks/*` to discover and manage active Claude
/// sessions on the local machine.  Separate from Azure-VM fleet orchestration
/// in `commands/fleet.rs`.
pub mod fleet_local;
pub mod freshness;
pub mod health_check;
pub mod launcher;
pub mod launcher_context;
pub mod memory_config;
pub mod nesting;
pub mod resolve_bundle_asset;
pub mod runtime_assets;
pub mod rust_trial;
pub mod self_heal;
pub mod session_tracker;
pub mod settings_manager;
pub mod signals;
#[cfg(test)]
pub mod test_support;
pub mod tool_update_check;
pub mod uninstall;
pub mod update;
pub mod util;

use clap::{
    Parser,
    builder::{PossibleValue, PossibleValuesParser},
};

/// The version amplihack reports to users via `--version`, update checks,
/// plugin manifests, and the `AMPLIHACK_VERSION` env var.
///
/// Prefers the `AMPLIHACK_RELEASE_VERSION` env var *set at build time* (used by
/// the release workflow to pin every binary to its tagged version) and falls
/// back to `CARGO_PKG_VERSION` for local/dev builds.
///
/// The root cause of the self-update prompt loop was that release binaries
/// were shipped with `CARGO_PKG_VERSION` baked in from a stale `Cargo.toml`,
/// so a binary tagged `v0.7.46` still self-reported `0.7.32` and the update
/// check kept offering the "newer" version forever. Feeding the release tag
/// through `AMPLIHACK_RELEASE_VERSION` at build time closes that gap without
/// needing to sed both `Cargo.toml` and `Cargo.lock` on every release.
pub const VERSION: &str = match option_env!("AMPLIHACK_RELEASE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub use cli_commands::Commands;
pub use cli_subcommands::{
    BuilderCommands, MemoryCommands, ModeCommands, MultitaskCommands, PluginCommands,
    QueryCodeCommands, RecipeCommands, ReflectCommands,
};

fn graph_db_backend_value_parser() -> PossibleValuesParser {
    PossibleValuesParser::new([
        PossibleValue::new("graph-db"),
        PossibleValue::new("kuzu").hide(true),
        PossibleValue::new("sqlite"),
    ])
}

fn raw_db_format_value_parser() -> PossibleValuesParser {
    PossibleValuesParser::new([
        PossibleValue::new("json"),
        PossibleValue::new("raw-db"),
        PossibleValue::new("kuzu").hide(true),
    ])
}

/// amplihack CLI — Rust core runtime.
#[derive(Parser, Debug)]
#[command(
    name = "amplihack",
    version = VERSION,
    about = "amplihack CLI — Rust core runtime"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

pub mod memory {
    pub use crate::commands::memory::{
        CodeGraphSummary, IndexStatus, PromptContextMemory, SessionSummary,
        background_index_job_active, background_index_job_path, check_index_status,
        default_code_graph_db_path_for_project, record_background_index_pid,
        resolve_code_graph_db_path_for_project, retrieve_prompt_context_memories,
        store_session_learning, summarize_code_graph,
    };

    /// Hidden integration-test-only Kuzu FFI exports.
    #[doc(hidden)]
    pub mod ffi_test_support {
        pub use crate::commands::memory::backend::graph_db::{
            graph_rows, init_graph_backend_schema, list_graph_sessions_from_conn,
        };
    }
}
