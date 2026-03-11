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
pub mod run;
pub mod show_validate;

pub use list::run_list;
pub use run::run_recipe;
pub use show_validate::{run_show, run_validate};

pub(crate) const MAX_YAML_SIZE_BYTES: usize = 1_000_000;

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

pub(crate) fn parse_recipe_from_path(path: impl AsRef<Path>) -> Result<RecipeDoc> {
    let validated = validate_path(path.as_ref(), false)?;
    let text = fs::read_to_string(&validated)
        .with_context(|| format!("Recipe file not found: {}", validated.display()))?;

    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "Recipe file too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    parse_recipe_text(&text)
}

pub(crate) fn parse_recipe_text(text: &str) -> Result<RecipeDoc> {
    if text.len() > MAX_YAML_SIZE_BYTES {
        anyhow::bail!(
            "YAML content too large ({} bytes). Maximum allowed: {} bytes",
            text.len(),
            MAX_YAML_SIZE_BYTES
        );
    }

    let raw_value: Value = serde_yaml::from_str(text)?;
    let raw_mapping = raw_value
        .as_mapping()
        .context("Recipe YAML must be a mapping at the top level")?;

    require_field(raw_mapping, "name", "Recipe must have a 'name' field")?;
    require_field(
        raw_mapping,
        "steps",
        "Recipe must have a 'steps' field with at least one step",
    )?;

    let recipe: RecipeDoc = serde_yaml::from_value(raw_value)?;
    if recipe.steps.is_empty() {
        anyhow::bail!("Recipe must have a 'steps' field with at least one step");
    }

    let mut seen_ids = BTreeSet::new();
    for step in &recipe.steps {
        if step.id.trim().is_empty() {
            anyhow::bail!("Every step must have a non-empty 'id' field");
        }
        if !seen_ids.insert(step.id.clone()) {
            anyhow::bail!("Duplicate step id: '{}'", step.id);
        }
        if let Some(step_type) = &step.step_type {
            match step_type.to_ascii_lowercase().as_str() {
                "bash" | "agent" | "recipe" => {}
                other => anyhow::bail!("'{}' is not a valid StepType", other),
            }
        }
    }

    Ok(recipe)
}

pub(crate) fn require_field(mapping: &Mapping, key: &str, message: &str) -> Result<()> {
    if mapping.contains_key(Value::String(key.to_string())) {
        return Ok(());
    }
    anyhow::bail!(message.to_string())
}

pub(crate) fn validate_path(path: impl AsRef<Path>, must_exist: bool) -> Result<PathBuf> {
    let path = path.as_ref();
    let display = path.display().to_string();
    if display.trim().is_empty() {
        anyhow::bail!("Path cannot be empty");
    }

    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to read current directory")?
            .join(path)
    };

    if must_exist && !resolved.exists() {
        anyhow::bail!("Path does not exist: {}", resolved.display());
    }

    Ok(resolved)
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
mod tests {
    use super::*;
    use crate::commands::recipe::list::{discover_recipes, filter_by_tags, format_recipe_list};
    use crate::commands::recipe::show_validate::{format_recipe_details, format_validation_result};
    use tempfile::tempdir;

    #[test]
    fn parse_recipe_validates_required_fields() {
        let err = parse_recipe_text("description: nope\nsteps: []").unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn parse_recipe_rejects_duplicate_step_ids() {
        let err = parse_recipe_text(
            r#"
name: demo
steps:
  - id: same
    command: echo one
  - id: same
    command: echo two
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Duplicate step id"));
    }

    #[test]
    fn list_formats_json() {
        let recipes = vec![RecipeInfo {
            name: "demo".into(),
            description: "desc".into(),
            version: "1.0.0".into(),
            author: "me".into(),
            tags: vec!["tag".into()],
            step_count: 2,
        }];
        let output = format_recipe_list(&recipes, OutputFormat::Json, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed[0]["name"], "demo");
        assert_eq!(parsed[0]["step_count"], 2);
    }

    #[test]
    fn validation_matches_python_shape() {
        let recipe = parse_recipe_text(
            r#"
name: demo
steps:
  - id: step-1
    command: echo hi
"#,
        )
        .unwrap();
        let output =
            format_validation_result(Some(&recipe), true, &[], OutputFormat::Table, true).unwrap();
        assert!(output.contains("✓ Recipe is valid"));
        assert!(output.contains("Name: demo"));
    }

    #[test]
    fn detail_table_shows_steps_and_context() {
        let recipe = parse_recipe_text(
            r#"
name: demo
description: Example
tags: [one, two]
context:
  repo_path: .
steps:
  - id: step-1
    agent: helper
    prompt: Test prompt
"#,
        )
        .unwrap();
        let output = format_recipe_details(&recipe, OutputFormat::Table, true, true).unwrap();
        assert!(output.contains("Steps (1):"));
        assert!(output.contains("Context Variables:"));
        assert!(output.contains("repo_path: ."));
    }

    #[test]
    fn discover_recipes_prefers_later_directories() {
        let temp = tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(
            first.join("demo.yaml"),
            "name: demo\nsteps:\n  - id: one\n    command: echo first\n",
        )
        .unwrap();
        fs::write(
            second.join("demo.yaml"),
            "name: demo\ndescription: override\nsteps:\n  - id: one\n    command: echo second\n",
        )
        .unwrap();

        let recipes = discover_recipes(&[first, second]).unwrap();
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].description, "override");
    }

    #[test]
    fn filter_tags_uses_and_logic() {
        let recipes = vec![
            RecipeInfo {
                name: "one".into(),
                description: String::new(),
                version: String::new(),
                author: String::new(),
                tags: vec!["a".into(), "b".into()],
                step_count: 1,
            },
            RecipeInfo {
                name: "two".into(),
                description: String::new(),
                version: String::new(),
                author: String::new(),
                tags: vec!["a".into()],
                step_count: 1,
            },
        ];

        let filtered = filter_by_tags(recipes, &["a".into(), "b".into()]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "one");
    }
}
