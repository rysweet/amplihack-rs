//! Recipe parser — YAML to Recipe validation and conversion.
//!
//! Matches Python `amplihack/recipes/parser.py`:
//! - Size limit enforcement
//! - Required field validation
//! - Duplicate step ID detection
//! - Step type inference from fields
//! - Bool/int field coercion

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

        let doc: serde_yaml::Value =
            serde_yaml::from_str(yaml_content).map_err(|e| anyhow::anyhow!("YAML parse error: {e}"))?;

        let mapping = doc
            .as_mapping()
            .ok_or_else(|| anyhow::anyhow!("Recipe YAML must be a mapping"))?;

        let name = extract_string(mapping, "name")
            .ok_or_else(|| anyhow::anyhow!("Recipe missing required field: 'name'"))?;

        let version = extract_string(mapping, "version").unwrap_or_else(|| "1.0.0".to_string());
        let description = extract_string(mapping, "description");
        let on_failure = extract_string(mapping, "on_failure");

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

        let id = extract_string(mapping, "id")
            .unwrap_or_else(|| format!("step-{}", index));

        let name = extract_string(mapping, "name")
            .unwrap_or_else(|| format!("Step {}", index));

        // Collect all string keys for type inference
        let field_map: HashMap<String, serde_yaml::Value> = mapping
            .iter()
            .filter_map(|(k, v)| k.as_str().map(|s| (s.to_string(), v.clone())))
            .collect();

        let step_type = extract_string(mapping, "type")
            .and_then(|t| parse_step_type(&t))
            .unwrap_or_else(|| StepType::infer(&field_map));

        let prompt = extract_string(mapping, "prompt");
        let command = extract_string(mapping, "command")
            .or_else(|| extract_string(mapping, "shell"));
        let agent = extract_string(mapping, "agent");
        let description = extract_string(mapping, "description");
        let condition = extract_string(mapping, "condition");
        let timeout_seconds = extract_u64(mapping, "timeout_seconds")
            .or_else(|| extract_u64(mapping, "timeout"));
        let retry_count = extract_u64(mapping, "retry_count")
            .or_else(|| extract_u64(mapping, "retries"))
            .map(|v| v as u32);
        let allow_failure = extract_bool(mapping, "allow_failure")
            .or_else(|| extract_bool(mapping, "continue_on_error"))
            .unwrap_or(false);

        let context = extract_step_context(mapping);

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
    "id", "name", "type", "description", "prompt", "command", "shell",
    "agent", "condition", "timeout_seconds", "timeout", "retry_count",
    "retries", "allow_failure", "continue_on_error", "context",
    "recipe", "sub_recipe", "parallel", "checkpoint",
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
    mapping
        .get(serde_yaml::Value::String("context".into()))
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn extract_step_context(mapping: &serde_yaml::Mapping) -> HashMap<String, serde_json::Value> {
    extract_context(mapping)
}

fn parse_step_type(s: &str) -> Option<StepType> {
    match s.to_lowercase().as_str() {
        "agent" => Some(StepType::Agent),
        "shell" => Some(StepType::Shell),
        "prompt" => Some(StepType::Prompt),
        "sub_recipe" | "subrecipe" => Some(StepType::SubRecipe),
        "checkpoint" => Some(StepType::Checkpoint),
        "parallel" => Some(StepType::Parallel),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StepType;

    fn parser() -> RecipeParser {
        RecipeParser::new()
    }

    #[test]
    fn parse_minimal_recipe() {
        let yaml = r#"
name: test-recipe
steps:
  - id: s1
    name: First step
    shell: echo hello
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.name, "test-recipe");
        assert_eq!(recipe.step_count(), 1);
        assert_eq!(recipe.steps[0].step_type, StepType::Shell);
        assert_eq!(recipe.steps[0].command.as_deref(), Some("echo hello"));
    }

    #[test]
    fn parse_full_recipe() {
        let yaml = r#"
name: full-recipe
version: "2.0"
description: A fully specified recipe
on_failure: cleanup-step
steps:
  - id: init
    name: Initialize
    type: shell
    command: "cargo check"
    timeout_seconds: 60
    allow_failure: false
  - id: analyze
    name: Analyze code
    type: agent
    prompt: "Analyze the codebase"
    agent: amplihack:analyzer
    retry_count: 2
  - id: verify
    name: Verify result
    shell: "cargo test"
    continue_on_error: true
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.name, "full-recipe");
        assert_eq!(recipe.version, "2.0");
        assert_eq!(recipe.description.as_deref(), Some("A fully specified recipe"));
        assert_eq!(recipe.on_failure.as_deref(), Some("cleanup-step"));
        assert_eq!(recipe.step_count(), 3);

        let init = recipe.get_step("init").unwrap();
        assert_eq!(init.step_type, StepType::Shell);
        assert_eq!(init.timeout_seconds, Some(60));

        let analyze = recipe.get_step("analyze").unwrap();
        assert_eq!(analyze.step_type, StepType::Agent);
        assert_eq!(analyze.agent.as_deref(), Some("amplihack:analyzer"));
        assert_eq!(analyze.retry_count, Some(2));

        let verify = recipe.get_step("verify").unwrap();
        assert_eq!(verify.step_type, StepType::Shell);
        assert!(verify.allow_failure);
    }

    #[test]
    fn parse_rejects_missing_name() {
        let yaml = "steps:\n  - id: s1\n    shell: echo hi\n";
        let result = parser().parse(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("name"));
    }

    #[test]
    fn parse_rejects_missing_steps() {
        let yaml = "name: no-steps\n";
        let result = parser().parse(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("steps"));
    }

    #[test]
    fn parse_rejects_duplicate_step_ids() {
        let yaml = r#"
name: dupes
steps:
  - id: s1
    shell: echo a
  - id: s1
    shell: echo b
"#;
        let result = parser().parse(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Duplicate"));
    }

    #[test]
    fn parse_enforces_size_limit() {
        let small_parser = RecipeParser::with_max_size(50);
        let yaml = "name: big\nsteps:\n  - id: s1\n    shell: echo this is too long for the limit\n";
        let result = small_parser.parse(yaml);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("size limit"));
    }

    #[test]
    fn parse_infers_step_type_from_fields() {
        let yaml = r#"
name: inferred
steps:
  - id: cmd
    name: Shell step
    command: "ls"
  - id: ask
    name: Prompt step
    prompt: "Explain this"
  - id: sub
    name: Sub recipe
    recipe: "other-recipe"
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.steps[0].step_type, StepType::Shell);
        assert_eq!(recipe.steps[1].step_type, StepType::Agent);
        assert_eq!(recipe.steps[2].step_type, StepType::SubRecipe);
    }

    #[test]
    fn parse_coerces_bool_strings() {
        let yaml = r#"
name: coerce
steps:
  - id: s1
    shell: echo hi
    allow_failure: "yes"
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert!(recipe.steps[0].allow_failure);
    }

    #[test]
    fn parse_coerces_timeout_from_string() {
        let yaml = r#"
name: coerce-timeout
steps:
  - id: s1
    shell: echo hi
    timeout_seconds: "120"
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.steps[0].timeout_seconds, Some(120));
    }

    #[test]
    fn parse_auto_generates_step_ids() {
        let yaml = r#"
name: auto-ids
steps:
  - name: First
    shell: echo 1
  - name: Second
    shell: echo 2
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.steps[0].id, "step-0");
        assert_eq!(recipe.steps[1].id, "step-1");
    }

    #[test]
    fn parse_step_context() {
        let yaml = r#"
name: with-context
context:
  repo_path: "."
steps:
  - id: s1
    shell: echo hi
    context:
      verbose: true
"#;
        let recipe = parser().parse(yaml).unwrap();
        assert!(recipe.context.contains_key("repo_path"));
        assert!(recipe.steps[0].context.contains_key("verbose"));
    }

    #[test]
    fn parse_file_nonexistent() {
        let result = parser().parse_file(Path::new("/nonexistent/recipe.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_file_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.yaml");
        std::fs::write(
            &path,
            "name: file-recipe\nsteps:\n  - id: s1\n    shell: echo ok\n",
        )
        .unwrap();
        let recipe = parser().parse_file(&path).unwrap();
        assert_eq!(recipe.name, "file-recipe");
    }

    #[test]
    fn default_version_applied() {
        let yaml = "name: no-version\nsteps:\n  - id: s1\n    shell: echo hi\n";
        let recipe = parser().parse(yaml).unwrap();
        assert_eq!(recipe.version, "1.0.0");
    }

    #[test]
    fn parse_rejects_non_mapping() {
        let result = parser().parse("- just a list\n- not a recipe\n");
        assert!(result.is_err());
    }

    #[test]
    fn step_type_parsing() {
        assert_eq!(parse_step_type("agent"), Some(StepType::Agent));
        assert_eq!(parse_step_type("Shell"), Some(StepType::Shell));
        assert_eq!(parse_step_type("sub_recipe"), Some(StepType::SubRecipe));
        assert_eq!(parse_step_type("subrecipe"), Some(StepType::SubRecipe));
        assert_eq!(parse_step_type("unknown"), None);
    }
}
