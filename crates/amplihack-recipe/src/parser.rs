//! Recipe parser — YAML to Recipe validation and conversion.
//!
//! Matches Python `amplihack/recipes/parser.py`:
//! - Size limit enforcement
//! - Required field validation
//! - Duplicate step ID detection
//! - Step type inference from fields
//! - Bool/int field coercion

use crate::condition_eval::validate_condition;
use crate::models::{Recipe, Step, StepType};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tracing::warn;

/// Maximum YAML file size (1 MiB).
pub const MAX_YAML_SIZE_BYTES: usize = 1_048_576;

/// Recipe parser with configurable size limits and validation.
pub struct RecipeParser {
    max_size: usize,
}

impl RecipeParser {
    pub fn new() -> Self {
        Self {
            max_size: MAX_YAML_SIZE_BYTES,
        }
    }

    pub fn with_max_size(max_size: usize) -> Self {
        Self { max_size }
    }

    /// Parse a YAML string into a validated Recipe.
    pub fn parse(&self, yaml_content: &str) -> Result<Recipe> {
        if yaml_content.len() > self.max_size {
            bail!(
                "Recipe YAML exceeds size limit: {} bytes > {} bytes",
                yaml_content.len(),
                self.max_size
            );
        }

        let doc: serde_yaml::Value = serde_yaml::from_str(yaml_content)
            .map_err(|e| anyhow::anyhow!("YAML parse error: {e}"))?;

        let mapping = doc
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Recipe YAML must be a mapping"))?;

        let name = extract_string(mapping, "name")
            .ok_or_else(|| anyhow::anyhow!("Recipe missing required field: 'name'"))?;

        let version = extract_string(mapping, "version").unwrap_or_else(|| "1.0.0".to_string());
        let description = extract_string(mapping, "description");
        let on_failure = extract_string(mapping, "on_failure");
        let default_step_timeout = extract_u64(mapping, "default_step_timeout");
        let author = extract_string(mapping, "author");
        let tags = extract_string_list(mapping, "tags");
        let context_validation = extract_string_map(mapping, "context_validation");
        let recursion = extract_recursion_config(mapping);

        let steps_value = mapping
            .get(serde_yaml::Value::String("steps".into()))
            .ok_or_else(|| anyhow::anyhow!("Recipe missing required field: 'steps'"))?;

        let steps_seq = steps_value
            .as_sequence()
            .ok_or_else(|| anyhow::anyhow!("Recipe 'steps' must be a sequence"))?;

        let mut steps = Vec::with_capacity(steps_seq.len());
        let mut seen_ids = HashSet::new();

        for (idx, step_value) in steps_seq.iter().enumerate() {
            let step = self.parse_step(step_value, idx)?;
            if !seen_ids.insert(step.id.clone()) {
                bail!("Duplicate step ID: '{}'", step.id);
            }
            steps.push(step);
        }

        let context = extract_context(mapping);

        Ok(Recipe {
            name,
            version,
            description,
            steps,
            context,
            on_failure,
            default_step_timeout,
            context_validation,
            recursion,
            author,
            tags,
        })
    }

    /// Parse a YAML file into a validated Recipe.
    pub fn parse_file(&self, path: &Path) -> Result<Recipe> {
        let metadata = std::fs::metadata(path)
            .map_err(|e| anyhow::anyhow!("Cannot read recipe file {}: {e}", path.display()))?;

        if metadata.len() as usize > self.max_size {
            bail!(
                "Recipe file {} exceeds size limit: {} bytes > {} bytes",
                path.display(),
                metadata.len(),
                self.max_size
            );
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read recipe file {}: {e}", path.display()))?;

        self.parse(&content)
    }

    fn parse_step(&self, value: &serde_yaml::Value, index: usize) -> Result<Step> {
        let mapping = value
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Step {} must be a mapping", index))?;

        let id = extract_string(mapping, "id").unwrap_or_else(|| format!("step-{}", index));

        let name = extract_string(mapping, "name").unwrap_or_else(|| format!("Step {}", index));

        // Collect field names for type inference (no value cloning needed)
        let field_keys: std::collections::HashSet<String> = mapping
            .iter()
            .filter_map(|(k, _)| k.as_str().map(|s| s.to_string()))
            .collect();

        let step_type = extract_string(mapping, "type")
            .and_then(|t| parse_step_type(&t))
            .unwrap_or_else(|| StepType::infer(&field_keys));

        let prompt = extract_string(mapping, "prompt");
        let command =
            extract_string(mapping, "command").or_else(|| extract_string(mapping, "shell"));
        let agent = extract_string(mapping, "agent");
        let description = extract_string(mapping, "description");
        let condition = extract_string(mapping, "condition");

        // Validate condition expression syntax early (issue #212).
        if let Some(ref cond_str) = condition
            && let Err(e) = validate_condition(cond_str)
        {
            warn!(step_id = %id, error = %e, "Invalid condition expression");
        }

        let timeout_seconds =
            extract_u64(mapping, "timeout_seconds").or_else(|| extract_u64(mapping, "timeout"));
        let retry_count = extract_u64(mapping, "retry_count")
            .or_else(|| extract_u64(mapping, "retries"))
            .map(|v| match u32::try_from(v) {
                Ok(n) => n,
                Err(_) => {
                    warn!("retry_count value {v} exceeds u32::MAX, clamping");
                    u32::MAX
                }
            });
        let allow_failure = extract_bool(mapping, "allow_failure")
            .or_else(|| extract_bool(mapping, "continue_on_error"))
            .unwrap_or(false);

        let context = extract_step_context(mapping);

        let max_env_value_bytes = extract_u64(mapping, "max_env_value_bytes").map(|v| v as usize);

        let output_key = extract_string(mapping, "output");

        warn_unrecognized_fields(mapping, &id);

        Ok(Step {
            id,
            name,
            step_type,
            description,
            prompt,
            command,
            agent,
            condition,
            timeout_seconds,
            retry_count,
            allow_failure,
            context,
            output_key,
            max_env_value_bytes,
        })
    }
}

impl Default for RecipeParser {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const KNOWN_STEP_FIELDS: &[&str] = &[
    "id",
    "name",
    "type",
    "description",
    "prompt",
    "command",
    "shell",
    "agent",
    "condition",
    "timeout_seconds",
    "timeout",
    "retry_count",
    "retries",
    "allow_failure",
    "continue_on_error",
    "context",
    "recipe",
    "sub_recipe",
    "parallel",
    "checkpoint",
    "output",
    "max_env_value_bytes",
];

fn warn_unrecognized_fields(mapping: &serde_yaml::Mapping, step_id: &str) {
    for key in mapping.keys() {
        if let Some(name) = key.as_str()
            && !KNOWN_STEP_FIELDS.contains(&name)
        {
            warn!(step_id, field = name, "Unrecognized step field");
        }
    }
}

fn extract_string(mapping: &serde_yaml::Mapping, key: &str) -> Option<String> {
    mapping
        .get(serde_yaml::Value::String(key.into()))
        .and_then(|v| match v {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Number(n) => Some(n.to_string()),
            serde_yaml::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        })
}

fn extract_u64(mapping: &serde_yaml::Mapping, key: &str) -> Option<u64> {
    mapping
        .get(serde_yaml::Value::String(key.into()))
        .and_then(|v| match v {
            serde_yaml::Value::Number(n) => n.as_u64(),
            serde_yaml::Value::String(s) => s.parse().ok(),
            _ => None,
        })
}

fn extract_bool(mapping: &serde_yaml::Mapping, key: &str) -> Option<bool> {
    mapping
        .get(serde_yaml::Value::String(key.into()))
        .and_then(|v| match v {
            serde_yaml::Value::Bool(b) => Some(*b),
            serde_yaml::Value::String(s) => match s.to_lowercase().as_str() {
                "true" | "yes" | "1" => Some(true),
                "false" | "no" | "0" => Some(false),
                _ => None,
            },
            serde_yaml::Value::Number(n) => n.as_u64().map(|u| u != 0),
            _ => None,
        })
}

fn extract_context(mapping: &serde_yaml::Mapping) -> HashMap<String, serde_json::Value> {
    match mapping.get(serde_yaml::Value::String("context".into())) {
        Some(v) => match serde_json::to_value(v) {
            Ok(json_val) => match serde_json::from_value(json_val) {
                Ok(map) => map,
                Err(e) => {
                    warn!(error = %e, "Failed to deserialize recipe context as map, using default");
                    HashMap::new()
                }
            },
            Err(e) => {
                warn!(error = %e, "Failed to convert recipe context to JSON, using default");
                HashMap::new()
            }
        },
        None => HashMap::new(),
    }
}

fn extract_step_context(mapping: &serde_yaml::Mapping) -> HashMap<String, serde_json::Value> {
    extract_context(mapping)
}

fn parse_step_type(s: &str) -> Option<StepType> {
    match s.to_lowercase().as_str() {
        "agent" => Some(StepType::Agent),
        "shell" | "bash" | "command" => Some(StepType::Shell),
        "prompt" => Some(StepType::Prompt),
        "sub_recipe" | "subrecipe" => Some(StepType::SubRecipe),
        "checkpoint" => Some(StepType::Checkpoint),
        "parallel" => Some(StepType::Parallel),
        _ => None,
    }
}

fn extract_string_list(mapping: &serde_yaml::Mapping, key: &str) -> Vec<String> {
    mapping
        .get(serde_yaml::Value::String(key.into()))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn extract_string_map(mapping: &serde_yaml::Mapping, key: &str) -> HashMap<String, String> {
    mapping
        .get(serde_yaml::Value::String(key.into()))
        .and_then(|v| v.as_mapping())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    let ks = k.as_str()?;
                    let vs = match v {
                        serde_yaml::Value::String(s) => s.clone(),
                        other => format!("{other:?}"),
                    };
                    Some((ks.to_string(), vs))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn extract_recursion_config(
    mapping: &serde_yaml::Mapping,
) -> Option<crate::models::RecursionConfig> {
    let val = mapping.get(serde_yaml::Value::String("recursion".into()))?;
    let m = val.as_mapping()?;
    let max_depth = extract_u64(m, "max_depth").map(|v| v as u32).unwrap_or(3);
    let max_total_steps = extract_u64(m, "max_total_steps")
        .map(|v| v as u32)
        .unwrap_or(50);
    Some(crate::models::RecursionConfig {
        max_depth,
        max_total_steps,
    })
}

#[cfg(test)]
#[path = "tests/parser_tests.rs"]
mod tests;
