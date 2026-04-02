//! Native `fleet` subcommands.
//!
//! The Rust CLI now owns the live `amplihack fleet` runtime surface. This
//! module preserves the Python behavior where practical while keeping the
//! implementation explicit, testable, and fully native.
//!
//! INVARIANT: All external session name inputs MUST pass `validate_session_name()`
//! before use in any subprocess or tmux command invocation.

use crate::binary_finder::BinaryFinder;
use crate::command_error::exit_error;
use crate::env_builder::EnvBuilder;
use crate::util::run_output_with_timeout;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local};
use clap::{CommandFactory, Parser};
use lru::LruCache;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::iter;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

mod constants;
use constants::*;

mod cli_args;
use cli_args::*;

mod models;
use models::*;

mod fleet_state;
use fleet_state::*;

mod admiral;
use admiral::*;

mod admiral_ops;

mod auth;
use auth::*;

mod adopter;
use adopter::*;

mod observer;
use observer::*;

mod fleet_task;
use fleet_task::*;

mod task_queue;
use task_queue::*;

mod reports;
use reports::*;

mod reasoning;
use reasoning::*;

mod reasoning_engine;
use reasoning_engine::*;

mod reasoning_records;
use reasoning_records::*;

mod reasoning_helpers;
use reasoning_helpers::*;

mod reasoning_validation;
use reasoning_validation::*;

mod projects;
use projects::*;

mod project_dashboard;
use project_dashboard::*;

mod project_registry;
use project_registry::*;

mod helpers;
use helpers::*;

mod tui_state;
use tui_state::*;

mod tui_input;
use tui_input::*;

mod tui_render;
use tui_render::*;

mod tui_fleet_view;
use tui_fleet_view::*;

mod tui_views;
use tui_views::*;

mod tui_ui_methods;

mod tui_ui_methods_ext;

mod tui_actions;
use tui_actions::*;

mod commands;
pub use commands::run_fleet;
use commands::*;

mod commands_scout;
use commands_scout::*;

mod commands_advance;
use commands_advance::*;

mod commands_manage;
use commands_manage::*;

mod commands_ext;
use commands_ext::*;

mod tui_main;
use tui_main::*;

mod tui_collect;
use tui_collect::*;

#[cfg(all(test, unix))]
mod tests;
