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

pub mod list;
mod parse;
mod resolve;
pub mod run;
pub mod show_validate;

pub use list::run_list;
pub use run::run_recipe;
pub use show_validate::{run_show, run_validate};

#[cfg(test)]
pub(crate) use parse::parse_recipe_text;
pub(crate) use parse::{parse_recipe_from_input, parse_recipe_from_path, validate_path};
pub(crate) use resolve::{recipe_search_dirs, resolve_recipe_path};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeRunResult {
    pub(crate) recipe_name: String,
    pub(crate) success: bool,
    #[serde(default)]
    pub(crate) step_results: Vec<RecipeRunStepResult>,
    #[serde(default)]
    pub(crate) context: JsonMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecipeRunStepResult {
    pub(crate) step_id: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) output: String,
    #[serde(default)]
    pub(crate) error: String,
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
