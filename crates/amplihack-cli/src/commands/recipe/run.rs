use super::*;

const MAX_OUTPUT_LENGTH: usize = 200;

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
                tracing::warn!(
                    step_id = %step.step_id,
                    original_len = step.output.chars().count(),
                    max_len = MAX_OUTPUT_LENGTH,
                    "Step output truncated"
                );
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
