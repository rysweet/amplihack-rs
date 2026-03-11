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

const MAX_YAML_SIZE_BYTES: usize = 1_000_000;
const MAX_OUTPUT_LENGTH: usize = 200;

#[derive(Debug, Clone, Copy)]
enum OutputFormat {
    Table,
    Json,
    Yaml,
}

impl OutputFormat {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "yaml" => Ok(Self::Yaml),
            other => anyhow::bail!("Invalid format: {other}. Must be table, json, or yaml"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecipeDoc {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_version")]
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    context: BTreeMap<String, Value>,
    #[serde(default)]
    steps: Vec<RawStep>,
    #[serde(default)]
    recursion: Option<Value>,
    #[serde(default)]
    output: Option<Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawStep {
    id: String,
    #[serde(rename = "type")]
    step_type: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    condition: Option<String>,
    #[serde(default)]
    parse_json: bool,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout: Option<i64>,
    #[serde(default)]
    auto_stage: Option<bool>,
    #[serde(default)]
    recipe: Option<String>,
    #[serde(default, rename = "context")]
    sub_context: Option<BTreeMap<String, Value>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
struct RecipeInfo {
    name: String,
    description: String,
    version: String,
    author: String,
    tags: Vec<String>,
    step_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecipeRunResult {
    recipe_name: String,
    success: bool,
    #[serde(default)]
    step_results: Vec<RecipeRunStepResult>,
    #[serde(default)]
    context: JsonMap<String, JsonValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecipeRunStepResult {
    step_id: String,
    status: String,
    #[serde(default)]
    output: String,
    #[serde(default)]
    error: String,
}

pub fn run_recipe(
    recipe_path: &str,
    context_args: &[String],
    dry_run: bool,
    verbose: bool,
    format: &str,
    working_dir: Option<&str>,
) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let (context, errors) = parse_context_args(context_args);
    if !errors.is_empty() {
        for error in errors {
            writeln!(io::stderr(), "Error: {error}")?;
        }
        return Err(exit_error(1));
    }

    let validated_path = validate_path(recipe_path, false)?;
    let recipe = parse_recipe_from_path(&validated_path)?;
    let (merged_context, inferred) = infer_missing_context(&recipe.context, &context);
    let working_dir = working_dir.unwrap_or(".");
    if verbose {
        writeln!(io::stderr(), "Executing recipe: {}", recipe.name)?;
        if dry_run {
            writeln!(io::stderr(), "DRY RUN MODE - No actual execution")?;
        }
        if !inferred.is_empty() {
            writeln!(
                io::stderr(),
                "[context] Inferred {} variable(s): {}",
                inferred.len(),
                inferred.join(", ")
            )?;
        }
    }
    let result = execute_recipe_via_rust(&validated_path, &merged_context, dry_run, working_dir)?;

    println!("{}", format_recipe_run_result(&result, format, false)?);

    if result.success {
        Ok(())
    } else {
        Err(exit_error(1))
    }
}

pub fn run_list(
    recipe_dir: Option<&str>,
    format: &str,
    tags: &[String],
    verbose: bool,
) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let search_dirs = build_search_dirs(recipe_dir)?;
    let recipes = discover_recipes(&search_dirs)?;
    let filtered = filter_by_tags(recipes, tags);
    println!("{}", format_recipe_list(&filtered, format, verbose)?);
    Ok(())
}

pub fn run_validate(file: &str, verbose: bool, format: &str) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let mut stdout = io::stdout();

    match parse_recipe_from_path(file) {
        Ok(recipe) => {
            writeln!(
                stdout,
                "{}",
                format_validation_result(Some(&recipe), true, &[], format, verbose)?
            )?;
            Ok(())
        }
        Err(error) => {
            writeln!(
                stdout,
                "{}",
                format_validation_result(None, false, &[error.to_string()], format, verbose)?
            )?;
            Err(exit_error(1))
        }
    }
}

pub fn run_show(name: &str, format: &str, show_steps: bool, show_context: bool) -> Result<()> {
    let format = OutputFormat::parse(format)?;
    let mut stdout = io::stdout();

    match parse_recipe_from_path(name) {
        Ok(recipe) => {
            writeln!(
                stdout,
                "{}",
                format_recipe_details(&recipe, format, show_steps, show_context)?
            )?;
            Ok(())
        }
        Err(error) => {
            writeln!(io::stderr(), "Error: {error}")?;
            Err(exit_error(1))
        }
    }
}

fn build_search_dirs(recipe_dir: Option<&str>) -> Result<Vec<PathBuf>> {
    if let Some(dir) = recipe_dir {
        return Ok(vec![validate_path(dir, false)?]);
    }

    let mut dirs = vec![
        home_dir()?
            .join(".amplihack")
            .join(".claude")
            .join("recipes"),
        PathBuf::from("amplifier-bundle").join("recipes"),
        PathBuf::from("src")
            .join("amplihack")
            .join("amplifier-bundle")
            .join("recipes"),
        PathBuf::from(".claude").join("recipes"),
    ];

    if let Some(repo_root) = repo_root_from_cwd()? {
        dirs.insert(0, repo_root.join("amplifier-bundle").join("recipes"));
    }

    Ok(dirs)
}

fn parse_context_args(context_args: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut context = BTreeMap::new();
    let mut errors = Vec::new();

    for arg in context_args {
        if let Some((key, value)) = arg.split_once('=') {
            context.insert(key.to_string(), value.to_string());
        } else {
            errors.push(format!(
                "Invalid context format '{arg}'. Use key=value format (e.g., -c 'question=What is X?' -c 'var=value')"
            ));
        }
    }

    (context, errors)
}

fn infer_missing_context(
    recipe_defaults: &BTreeMap<String, Value>,
    user_context: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut merged = recipe_defaults
        .iter()
        .map(|(key, value)| (key.clone(), scalar_to_context_value(value)))
        .collect::<BTreeMap<_, _>>();

    for (key, value) in user_context {
        merged.insert(key.clone(), value.clone());
    }

    let mut inferred = Vec::new();
    let keys = merged.keys().cloned().collect::<Vec<_>>();
    for key in keys {
        if merged.get(&key).is_some_and(|value| !value.is_empty()) {
            continue;
        }

        let env_key = format!("AMPLIHACK_CONTEXT_{}", key.to_uppercase());
        if let Ok(value) = std::env::var(&env_key)
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from ${env_key})"));
            continue;
        }

        if key == "task_description"
            && let Ok(value) = std::env::var("AMPLIHACK_TASK_DESCRIPTION")
            && !value.is_empty()
        {
            merged.insert(key.clone(), value);
            inferred.push(format!("{key} (from $AMPLIHACK_TASK_DESCRIPTION)"));
        } else if key == "repo_path" {
            let value = std::env::var("AMPLIHACK_REPO_PATH").unwrap_or_else(|_| ".".to_string());
            if value != "." {
                inferred.push(format!("{key} (from $AMPLIHACK_REPO_PATH)"));
            }
            merged.insert(key.clone(), value);
        }
    }

    (merged, inferred)
}

fn scalar_to_context_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(v) => {
            if *v {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn execute_recipe_via_rust(
    recipe_path: &Path,
    context: &BTreeMap<String, String>,
    dry_run: bool,
    working_dir: &str,
) -> Result<RecipeRunResult> {
    let binary = find_recipe_runner_binary()?;
    let abs_working_dir = validate_path(working_dir, false)?;
    let mut command = Command::new(binary);
    command
        .arg(recipe_path)
        .arg("--output-format")
        .arg("json")
        .arg("-C")
        .arg(&abs_working_dir);

    if dry_run {
        command.arg("--dry-run");
    }

    for (key, value) in context {
        command.arg("--set").arg(format!("{key}={value}"));
    }

    let output = command
        .output()
        .context("failed to spawn recipe-runner-rs")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: RecipeRunResult = serde_json::from_str(&stdout).map_err(|_| {
        anyhow::anyhow!(
            "Rust recipe runner returned unparseable output (exit {}): {}",
            output.status,
            if output.status.success() {
                stdout.chars().take(500).collect::<String>()
            } else if stderr.is_empty() {
                "no stderr".to_string()
            } else {
                stderr.chars().take(1000).collect::<String>()
            }
        )
    })?;

    Ok(parsed)
}

fn find_recipe_runner_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("RECIPE_RUNNER_RS_PATH")
        && let Some(resolved) = resolve_binary_path(&path)
    {
        return Ok(resolved);
    }

    for candidate in [
        "recipe-runner-rs",
        "~/.cargo/bin/recipe-runner-rs",
        "~/.local/bin/recipe-runner-rs",
    ] {
        if let Some(resolved) = resolve_binary_path(candidate) {
            return Ok(resolved);
        }
    }

    anyhow::bail!(
        "recipe-runner-rs binary not found. Install it: cargo install --git https://github.com/rysweet/amplihack-recipe-runner or set RECIPE_RUNNER_RS_PATH."
    )
}

fn resolve_binary_path(candidate: &str) -> Option<PathBuf> {
    let expanded = if let Some(rest) = candidate.strip_prefix("~/") {
        home_dir().ok()?.join(rest)
    } else {
        PathBuf::from(candidate)
    };

    if expanded.components().count() > 1 {
        return expanded.is_file().then_some(expanded);
    }

    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&expanded))
        .find(|entry| entry.is_file())
}

fn format_recipe_run_result(
    result: &RecipeRunResult,
    format: OutputFormat,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = JsonMap::new();
            data.insert(
                "recipe_name".to_string(),
                JsonValue::String(result.recipe_name.clone()),
            );
            data.insert("success".to_string(), JsonValue::Bool(result.success));
            data.insert(
                "step_results".to_string(),
                JsonValue::Array(
                    result
                        .step_results
                        .iter()
                        .map(|step| {
                            json!({
                                "step_id": step.step_id,
                                "status": step.status,
                                "output": step.output,
                                "error": step.error,
                            })
                        })
                        .collect(),
                ),
            );
            if show_context && !result.context.is_empty() {
                data.insert(
                    "context".to_string(),
                    JsonValue::Object(result.context.clone()),
                );
            }
            Ok(serde_json::to_string_pretty(&JsonValue::Object(data))?)
        }
        OutputFormat::Yaml => {
            let mut data = JsonMap::new();
            data.insert(
                "recipe_name".to_string(),
                JsonValue::String(result.recipe_name.clone()),
            );
            data.insert("success".to_string(), JsonValue::Bool(result.success));
            data.insert(
                "step_results".to_string(),
                JsonValue::Array(
                    result
                        .step_results
                        .iter()
                        .map(|step| {
                            JsonValue::Object(JsonMap::from_iter([
                                (
                                    "step_id".to_string(),
                                    JsonValue::String(step.step_id.clone()),
                                ),
                                ("status".to_string(), JsonValue::String(step.status.clone())),
                                ("output".to_string(), JsonValue::String(step.output.clone())),
                                ("error".to_string(), JsonValue::String(step.error.clone())),
                            ]))
                        })
                        .collect(),
                ),
            );
            if show_context && !result.context.is_empty() {
                data.insert(
                    "context".to_string(),
                    JsonValue::Object(result.context.clone()),
                );
            }
            Ok(serde_yaml::to_string(&JsonValue::Object(data))?)
        }
        OutputFormat::Table => Ok(format_recipe_run_table(result, show_context)),
    }
}

fn format_recipe_run_table(result: &RecipeRunResult, show_context: bool) -> String {
    let mut lines = vec![
        format!("Recipe: {}", result.recipe_name),
        format!(
            "Status: {}",
            if result.success {
                "✓ Success"
            } else {
                "✗ Failed"
            }
        ),
        String::new(),
    ];

    if result.step_results.is_empty() {
        lines.push("No steps executed (0 steps)".to_string());
        return lines.join("\n");
    }

    lines.push("Steps:".to_string());
    for step in &result.step_results {
        let status_symbol = match step.status.as_str() {
            "completed" => "✓",
            "failed" => "✗",
            "skipped" => "⊘",
            _ => "?",
        };
        lines.push(format!(
            "  {status_symbol} {}: {}",
            step.step_id, step.status
        ));

        if !step.output.is_empty() {
            let output = if step.output.chars().count() > MAX_OUTPUT_LENGTH {
                format!(
                    "{}... (truncated)",
                    step.output
                        .chars()
                        .take(MAX_OUTPUT_LENGTH)
                        .collect::<String>()
                )
            } else {
                step.output.clone()
            };
            lines.push(format!("    Output: {output}"));
        }

        if !step.error.is_empty() {
            lines.push(format!("    Error: {}", step.error));
        }
    }

    if show_context && !result.context.is_empty() {
        lines.push(String::new());
        lines.push("Context:".to_string());
        for (key, value) in &result.context {
            lines.push(format!("  {key}: {}", json_scalar_to_string(value)));
        }
    }

    lines.join("\n")
}

fn json_scalar_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(v) => v.to_string(),
        JsonValue::Number(v) => v.to_string(),
        JsonValue::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn repo_root_from_cwd() -> Result<Option<PathBuf>> {
    let mut current = std::env::current_dir().context("failed to read current directory")?;
    loop {
        if current.join(".git").exists() {
            return Ok(Some(current));
        }
        if !current.pop() {
            return Ok(None);
        }
    }
}

fn discover_recipes(search_dirs: &[PathBuf]) -> Result<Vec<RecipeInfo>> {
    let mut recipes = BTreeMap::<String, RecipeInfo>::new();

    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue;
        }

        let mut yaml_paths = Vec::new();
        for entry in fs::read_dir(search_dir)
            .with_context(|| format!("failed to read {}", search_dir.display()))?
        {
            let path = entry?.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
                yaml_paths.push(path);
            }
        }
        yaml_paths.sort();

        for path in yaml_paths {
            if let Ok(recipe) = parse_recipe_from_path(&path) {
                recipes.insert(
                    recipe.name.clone(),
                    RecipeInfo {
                        name: recipe.name,
                        description: recipe.description,
                        version: recipe.version,
                        author: recipe.author,
                        tags: recipe.tags,
                        step_count: recipe.steps.len(),
                    },
                );
            }
        }
    }

    Ok(recipes.into_values().collect())
}

fn filter_by_tags(recipes: Vec<RecipeInfo>, tags: &[String]) -> Vec<RecipeInfo> {
    if tags.is_empty() {
        return recipes;
    }

    recipes
        .into_iter()
        .filter(|recipe| {
            let recipe_tags: BTreeSet<&str> = recipe.tags.iter().map(String::as_str).collect();
            tags.iter().all(|tag| recipe_tags.contains(tag.as_str()))
        })
        .collect()
}

fn parse_recipe_from_path(path: impl AsRef<Path>) -> Result<RecipeDoc> {
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

fn parse_recipe_text(text: &str) -> Result<RecipeDoc> {
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

fn require_field(mapping: &Mapping, key: &str, message: &str) -> Result<()> {
    if mapping.contains_key(Value::String(key.to_string())) {
        return Ok(());
    }
    anyhow::bail!(message.to_string())
}

fn validate_path(path: impl AsRef<Path>, must_exist: bool) -> Result<PathBuf> {
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

fn default_version() -> String {
    "1.0.0".to_string()
}

fn format_recipe_list(
    recipes: &[RecipeInfo],
    format: OutputFormat,
    verbose: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(
            &recipes
                .iter()
                .map(|recipe| {
                    if verbose {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                            "version": recipe.version,
                            "author": recipe.author,
                            "tags": recipe.tags,
                            "step_count": recipe.step_count,
                        })
                    } else {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                        })
                    }
                })
                .collect::<Vec<_>>(),
        )?),
        OutputFormat::Yaml => Ok(serde_yaml::to_string(
            &recipes
                .iter()
                .map(|recipe| {
                    if verbose {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                            "version": recipe.version,
                            "author": recipe.author,
                            "tags": recipe.tags,
                            "step_count": recipe.step_count,
                        })
                    } else {
                        json!({
                            "name": recipe.name,
                            "description": recipe.description,
                        })
                    }
                })
                .collect::<Vec<_>>(),
        )?),
        OutputFormat::Table => {
            if recipes.is_empty() {
                return Ok("No recipes found (0 recipes)".to_string());
            }

            let mut lines = vec![
                format!("Available Recipes ({}):", recipes.len()),
                String::new(),
            ];
            for recipe in recipes {
                lines.push(format!("• {}", recipe.name));
                if !recipe.description.is_empty() {
                    lines.push(format!("  {}", recipe.description));
                }
                if verbose {
                    if !recipe.version.is_empty() {
                        lines.push(format!("  Version: {}", recipe.version));
                    }
                    if !recipe.author.is_empty() {
                        lines.push(format!("  Author: {}", recipe.author));
                    }
                    lines.push(format!("  Steps: {}", recipe.step_count));
                }
                if !recipe.tags.is_empty() {
                    lines.push(format!("  Tags: {}", recipe.tags.join(", ")));
                }
                lines.push(String::new());
            }
            Ok(lines.join("\n"))
        }
    }
}

fn format_validation_result(
    recipe: Option<&RecipeDoc>,
    is_valid: bool,
    errors: &[String],
    format: OutputFormat,
    verbose: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => {
            let mut data = serde_json::Map::new();
            data.insert("valid".into(), json!(is_valid));
            data.insert("errors".into(), json!(errors));
            if let Some(recipe) = recipe {
                data.insert("recipe_name".into(), json!(recipe.name));
            }
            Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
                data,
            ))?)
        }
        OutputFormat::Yaml => {
            let mut data = Mapping::new();
            data.insert(
                Value::String("valid".into()),
                serde_yaml::to_value(is_valid)?,
            );
            data.insert(
                Value::String("errors".into()),
                serde_yaml::to_value(errors)?,
            );
            if let Some(recipe) = recipe {
                data.insert(
                    Value::String("recipe_name".into()),
                    Value::String(recipe.name.clone()),
                );
            }
            Ok(serde_yaml::to_string(&Value::Mapping(data))?)
        }
        OutputFormat::Table => {
            let mut lines = Vec::new();
            if is_valid {
                lines.push("✓ Recipe is valid".to_string());
                if let Some(recipe) = recipe {
                    lines.push(format!("  Name: {}", recipe.name));
                    if verbose {
                        lines.push(format!(
                            "  Description: {}",
                            if recipe.description.is_empty() {
                                "(none)"
                            } else {
                                &recipe.description
                            }
                        ));
                        lines.push(format!("  Steps: {}", recipe.steps.len()));
                    }
                }
            } else {
                lines.push("✗ Recipe is invalid".to_string());
                if !errors.is_empty() {
                    lines.push(String::new());
                    lines.push("Errors:".to_string());
                    for error in errors {
                        lines.push(format!("  • {}", error));
                    }
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

fn format_recipe_details(
    recipe: &RecipeDoc,
    format: OutputFormat,
    show_steps: bool,
    show_context: bool,
) -> Result<String> {
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(&json!({
            "name": recipe.name,
            "description": recipe.description,
            "version": recipe.version,
            "author": recipe.author,
            "tags": recipe.tags,
            "steps": recipe.steps.iter().map(step_json).collect::<Vec<_>>(),
            "context": recipe.context,
        }))?),
        OutputFormat::Yaml => {
            let mut root = Mapping::new();
            root.insert(
                Value::String("name".into()),
                Value::String(recipe.name.clone()),
            );
            root.insert(
                Value::String("description".into()),
                Value::String(recipe.description.clone()),
            );
            root.insert(
                Value::String("version".into()),
                Value::String(recipe.version.clone()),
            );
            root.insert(
                Value::String("author".into()),
                Value::String(recipe.author.clone()),
            );
            root.insert(
                Value::String("tags".into()),
                serde_yaml::to_value(&recipe.tags)?,
            );
            root.insert(
                Value::String("steps".into()),
                serde_yaml::to_value(&recipe.steps)?,
            );
            root.insert(
                Value::String("context".into()),
                serde_yaml::to_value(&recipe.context)?,
            );
            Ok(serde_yaml::to_string(&Value::Mapping(root))?)
        }
        OutputFormat::Table => {
            let mut lines = vec![
                format!("Recipe: {}", recipe.name),
                format!(
                    "Description: {}",
                    if recipe.description.is_empty() {
                        "(none)"
                    } else {
                        &recipe.description
                    }
                ),
                format!(
                    "Version: {}",
                    if recipe.version.is_empty() {
                        "(not specified)"
                    } else {
                        &recipe.version
                    }
                ),
                format!(
                    "Author: {}",
                    if recipe.author.is_empty() {
                        "(not specified)"
                    } else {
                        &recipe.author
                    }
                ),
            ];

            if !recipe.tags.is_empty() {
                lines.push(format!("Tags: {}", recipe.tags.join(", ")));
            }

            if show_steps && !recipe.steps.is_empty() {
                lines.push(String::new());
                lines.push(format!("Steps ({}):", recipe.steps.len()));
                for (index, step) in recipe.steps.iter().enumerate() {
                    lines.push(format!(
                        "  {}. {} ({})",
                        index + 1,
                        step.id,
                        infer_step_type(step)
                    ));
                    if let Some(command) = &step.command {
                        lines.push(format!("     Command: {}", command));
                    }
                    if let Some(agent) = &step.agent {
                        lines.push(format!("     Agent: {}", agent));
                    }
                    if let Some(prompt) = &step.prompt {
                        let prompt = if prompt.len() > 100 {
                            format!("{}...", &prompt[..100])
                        } else {
                            prompt.clone()
                        };
                        lines.push(format!("     Prompt: {}", prompt));
                    }
                }
            }

            if show_context && !recipe.context.is_empty() {
                lines.push(String::new());
                lines.push("Context Variables:".to_string());
                for (key, value) in &recipe.context {
                    lines.push(format!("  {}: {}", key, yaml_scalar(value)));
                }
            }

            Ok(lines.join("\n"))
        }
    }
}

fn step_json(step: &RawStep) -> serde_json::Value {
    json!({
        "id": step.id,
        "type": infer_step_type(step),
        "command": step.command,
        "agent": step.agent,
        "prompt": step.prompt,
    })
}

fn infer_step_type(step: &RawStep) -> &'static str {
    match step.step_type.as_deref() {
        Some("bash") | Some("BASH") => "bash",
        Some("agent") | Some("AGENT") => "agent",
        Some("recipe") | Some("RECIPE") => "recipe",
        Some(_) => "bash",
        None if step.recipe.is_some() => "recipe",
        None if step.agent.is_some() => "agent",
        None if step.prompt.is_some() && step.command.is_none() => "agent",
        _ => "bash",
    }
}

fn yaml_scalar(value: &Value) -> String {
    match value {
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty())
        .context("HOME is not set")
}

#[cfg(test)]
mod tests {
    use super::*;
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
