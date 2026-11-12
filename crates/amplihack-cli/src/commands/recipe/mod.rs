//! Native recipe read commands (`list`, `show`, `validate`).

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Issue #1606: `recipe run --detach`.
pub mod detach;
pub mod list;
mod parse;
/// Issue #1606: the processes `recipe stop` signals.
#[cfg(target_os = "linux")]
mod process_tree;
mod resolve;
pub mod run;
pub mod show_validate;
/// Issue #1606: `recipe stop`.
pub mod stop;

pub use list::run_list;
pub(crate) use run::reject_removed_context_keys;
pub use run::{resolve_context_args, run_recipe, run_recipe_fresh};
pub use show_validate::{run_show, run_validate};

#[cfg(test)]
pub(crate) use parse::parse_recipe_text;
pub(crate) use parse::{parse_recipe_from_input, parse_recipe_from_path, validate_path};
pub(crate) use resolve::{push_unique_path, recipe_search_dirs, resolve_recipe_path};

pub(crate) const MAX_YAML_SIZE_BYTES: usize = 1_000_000;
const RECIPE_FILE_EXTENSIONS: &[&str] = &["yaml", "yml"];

#[derive(Debug, Clone, Copy)]
pub(crate) enum OutputFormat {
    Table,
    Json,
    Yaml,
}

impl OutputFormat {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => anyhow::bail!("Invalid format: {other}. Must be table, json, or yaml"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeDoc {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default = "default_version")]
    pub(crate) version: String,
    #[serde(default)]
    pub(crate) author: String,
    #[serde(default)]
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) context: BTreeMap<String, Value>,
    #[serde(default)]
    pub(crate) steps: Vec<RawStep>,
    #[serde(default)]
    pub(crate) recursion: Option<Value>,
    #[serde(default)]
    pub(crate) output: Option<Value>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RawStep {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) step_type: Option<String>,
    #[serde(default)]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) agent: Option<String>,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
    #[serde(default)]
    pub(crate) output: Option<String>,
    #[serde(default)]
    pub(crate) condition: Option<String>,
    #[serde(default)]
    pub(crate) parse_json: bool,
    #[serde(default)]
    pub(crate) mode: Option<String>,
    #[serde(default)]
    pub(crate) working_dir: Option<String>,
    #[serde(default)]
    pub(crate) timeout: Option<i64>,
    #[serde(default)]
    pub(crate) auto_stage: Option<bool>,
    #[serde(default)]
    pub(crate) recipe: Option<String>,
    #[serde(default, rename = "context")]
    pub(crate) sub_context: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RecipeInfo {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) author: String,
    pub(crate) tags: Vec<String>,
    pub(crate) step_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RecipeRunResult {
    pub(crate) recipe_name: String,
    pub(crate) success: bool,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) log_pointer: Option<RecipeLogPointerSummary>,
    #[serde(default)]
    pub(crate) step_results: Vec<RecipeRunStepResult>,
    #[serde(default)]
    pub(crate) context: JsonMap<String, JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) heartbeat_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) progress_summary: Option<JsonValue>,
    #[serde(default, flatten)]
    pub(crate) extra: JsonMap<String, JsonValue>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RecipeLogPointerSummary {
    #[serde(default)]
    pub(crate) run_id: String,
    #[serde(default)]
    pub(crate) recipe_name: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) child_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runner_path: Option<String>,
    #[serde(default, skip_serializing_if = "is_json_map_empty")]
    pub(crate) log_paths: JsonMap<String, JsonValue>,
}

fn deserialize_optional_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match Option::<JsonValue>::deserialize(deserializer)? {
        Some(JsonValue::String(value)) if !value.is_empty() => Some(value),
        _ => None,
    })
}

fn is_json_map_empty(value: &JsonMap<String, JsonValue>) -> bool {
    value.is_empty()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct RecipeRunStepResult {
    pub(crate) step_id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) step_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) heartbeat_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) child: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recent_stdout: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recent_stderr: Vec<String>,
    #[serde(default, flatten)]
    pub(crate) extra: JsonMap<String, JsonValue>,
}

impl RecipeRunStepResult {
    /// Decode the terminating process status from the pinned runner's error envelope.
    ///
    /// recipe-runner-rs `90286bc` (the pin in `claude-plugin/recipe-runner.rev`
    /// and `Cargo.toml`) emits `Command failed (exit N): <stderr>` for bash
    /// steps and `amplihack <cli> failed (exit N)\n<diagnostics>` for agents
    /// (`adapters/cli_subprocess.rs`). The runner wraps those headers in
    /// `Step '<id>' failed: <kind> step failed: ` (`models.rs`,
    /// `StepExecutionError`). At the pin it writes `<id>` through
    /// `diagnostic_identifier`, which is `str::escape_debug`; `4d5187e`, the
    /// pin before it, wrote the raw id. For an ordinary id such as
    /// `step-04-run` the two forms are the same; for an id holding a quote,
    /// backslash or control character only the escaped form appears, so both
    /// are accepted. Anything else decodes to `None`, which is never a refusal.
    /// Only this envelope is authoritative; captured output can quote any status.
    /// Keep this compatibility decoder at the subprocess result boundary until
    /// the runner exposes a separate numeric exit-code field.
    pub(crate) fn termination_exit_code(&self) -> Option<i32> {
        let escaped: String = self.step_id.escape_debug().collect();
        let enveloped = self
            .error
            .strip_prefix(&format!("Step '{escaped}' failed: "))
            .or_else(|| {
                self.error
                    .strip_prefix(&format!("Step '{}' failed: ", self.step_id))
            });
        let error = if let Some(message) = enveloped {
            message
                .strip_prefix("bash step failed: ")
                .or_else(|| message.strip_prefix("agent step failed: "))?
        } else {
            self.error.as_str()
        };
        if let Some(rest) = error.strip_prefix("Command failed (exit ") {
            let (code, _) = rest.split_once("): ")?;
            return code.parse().ok();
        }
        let header = error.lines().next()?;
        let rest = header.strip_prefix("amplihack ")?;
        let (cli, rest) = rest.split_once(" failed (exit ")?;
        if cli.is_empty() || cli.chars().any(char::is_whitespace) {
            return None;
        }
        rest.strip_suffix(')')?.parse().ok()
    }
}

pub(crate) fn default_version() -> String {
    "1.0.0".to_string()
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty())
        .context("HOME is not set")
}

#[cfg(test)]
mod recipe_tests;
