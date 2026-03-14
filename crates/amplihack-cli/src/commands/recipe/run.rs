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

// =============================================================================
// TDD Step 7: WS3 — Recipe Runner Unit Tests
//
// These tests specify the contract for the recipe run delegation chain.
// They cover:
//   - parse_context_args: key=value parsing and error cases
//   - resolve_binary_path: path resolution with ~ expansion
//   - infer_missing_context: env var inference and default merging
//   - format_recipe_run_result: JSON/table output formatting
//   - find_recipe_runner_binary: error path when binary not present
//   - execute_recipe_via_rust: integration-level test (FAILS if
//     recipe-runner-rs binary is not installed or working)
//
// FAILING TESTS (fail until recipe-runner-rs E2E is working):
//   test_execute_recipe_via_rust_dry_run_succeeds_with_known_recipe
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write as IoWrite;
    use tempfile::NamedTempFile;

    // -------------------------------------------------------------------------
    // parse_context_args — key=value parsing
    // -------------------------------------------------------------------------

    /// Valid single key=value pair must be parsed without errors.
    #[test]
    fn test_parse_context_args_valid_single_pair() {
        let args = vec!["task_description=hello world".to_string()];
        let (ctx, errs) = parse_context_args(&args);
        assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
        assert_eq!(
            ctx.get("task_description").map(String::as_str),
            Some("hello world")
        );
    }

    /// Multiple valid key=value pairs must all be parsed correctly.
    #[test]
    fn test_parse_context_args_multiple_pairs() {
        let args = vec![
            "foo=bar".to_string(),
            "baz=qux".to_string(),
            "repo_path=/tmp".to_string(),
        ];
        let (ctx, errs) = parse_context_args(&args);
        assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
        assert_eq!(ctx.len(), 3);
        assert_eq!(ctx.get("foo").map(String::as_str), Some("bar"));
        assert_eq!(ctx.get("baz").map(String::as_str), Some("qux"));
        assert_eq!(ctx.get("repo_path").map(String::as_str), Some("/tmp"));
    }

    /// Empty context args must produce an empty map with no errors.
    #[test]
    fn test_parse_context_args_empty_input() {
        let (ctx, errs) = parse_context_args(&[]);
        assert!(errs.is_empty(), "Empty input must produce no errors");
        assert!(ctx.is_empty(), "Empty input must produce empty context map");
    }

    /// An arg without '=' must produce an error with a helpful message.
    #[test]
    fn test_parse_context_args_invalid_no_equals_sign() {
        let args = vec!["no-equals-sign".to_string()];
        let (ctx, errs) = parse_context_args(&args);
        assert_eq!(errs.len(), 1, "Exactly one error expected. Got: {:?}", errs);
        assert!(
            errs[0].contains("key=value"),
            "Error message must mention 'key=value' format. Got: {}",
            errs[0]
        );
        assert!(ctx.is_empty(), "No context should be parsed on error");
    }

    /// A value that itself contains '=' must be preserved correctly.
    /// The split must only happen on the FIRST '='.
    #[test]
    fn test_parse_context_args_value_contains_equals() {
        let args = vec!["url=https://example.com?a=1&b=2".to_string()];
        let (ctx, errs) = parse_context_args(&args);
        assert!(errs.is_empty(), "No errors expected. Got: {:?}", errs);
        assert_eq!(
            ctx.get("url").map(String::as_str),
            Some("https://example.com?a=1&b=2"),
            "Value with embedded '=' must not be truncated"
        );
    }

    // -------------------------------------------------------------------------
    // resolve_binary_path — path resolution and ~ expansion
    // -------------------------------------------------------------------------

    /// A path that does not exist must return None.
    #[test]
    fn test_resolve_binary_path_returns_none_for_nonexistent_path() {
        let result = resolve_binary_path("/definitely/does/not/exist/binary");
        assert!(
            result.is_none(),
            "Non-existent path must resolve to None. Got: {:?}",
            result
        );
    }

    /// A bare name not in PATH must return None.
    #[test]
    fn test_resolve_binary_path_returns_none_for_unknown_binary_name() {
        let result = resolve_binary_path("this-binary-cannot-possibly-exist-amplihack-test");
        assert!(
            result.is_none(),
            "Unknown binary name must resolve to None. Got: {:?}",
            result
        );
    }

    /// A well-known binary that IS in PATH must resolve to Some(path).
    #[test]
    #[cfg(unix)]
    fn test_resolve_binary_path_finds_known_binary_in_path() {
        // `true` is guaranteed to exist on any Unix system
        let result = resolve_binary_path("true");
        assert!(
            result.is_some(),
            "'true' binary must be found in PATH via resolve_binary_path"
        );
        let resolved = result.unwrap();
        assert!(
            resolved.is_file(),
            "Resolved path must point to an existing file. Got: {:?}",
            resolved
        );
    }

    /// A ~/... path must be expanded using the home directory.
    #[test]
    #[cfg(unix)]
    fn test_resolve_binary_path_expands_tilde_to_home_dir() {
        // Create a temp file inside the home directory to test expansion
        let home = std::env::var("HOME").expect("HOME env var must be set");
        let temp =
            tempfile::NamedTempFile::new_in(&home).expect("failed to create temp file in HOME");

        // Make it executable so resolve_binary_path treats it as a file candidate
        let tilde_path = format!("~/{}", temp.path().file_name().unwrap().to_str().unwrap());

        // resolve_binary_path checks is_file(), which is true for NamedTempFile
        let result = resolve_binary_path(&tilde_path);
        assert!(
            result.is_some(),
            "Tilde path '{}' must expand to HOME and resolve. Got: None",
            tilde_path
        );
        let resolved = result.unwrap();
        assert!(
            resolved.starts_with(&home),
            "Resolved path must start with HOME ({home}). Got: {:?}",
            resolved
        );
    }

    // -------------------------------------------------------------------------
    // infer_missing_context — env var inference and default merging
    // -------------------------------------------------------------------------

    /// User-provided context must override recipe defaults.
    #[test]
    fn test_infer_missing_context_user_values_override_recipe_defaults() {
        let mut recipe_defaults = BTreeMap::new();
        recipe_defaults.insert(
            "task_description".to_string(),
            serde_yaml::Value::String("recipe default".to_string()),
        );

        let mut user_context = BTreeMap::new();
        user_context.insert("task_description".to_string(), "user override".to_string());

        let (merged, inferred) = infer_missing_context(&recipe_defaults, &user_context);

        assert_eq!(
            merged.get("task_description").map(String::as_str),
            Some("user override"),
            "User context must override recipe defaults"
        );
        assert!(
            inferred.is_empty(),
            "No inference should occur when user provides the value. Got: {:?}",
            inferred
        );
    }

    /// When task_description is missing from user context and AMPLIHACK_TASK_DESCRIPTION
    /// env var is set, it must be inferred automatically.
    #[test]
    fn test_infer_missing_context_infers_task_description_from_env() {
        // SAFETY: test-only env manipulation
        unsafe { std::env::set_var("AMPLIHACK_TASK_DESCRIPTION", "from env var") };

        let mut recipe_defaults = BTreeMap::new();
        recipe_defaults.insert(
            "task_description".to_string(),
            serde_yaml::Value::String(String::new()), // empty default
        );

        let (merged, inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

        unsafe { std::env::remove_var("AMPLIHACK_TASK_DESCRIPTION") };

        assert_eq!(
            merged.get("task_description").map(String::as_str),
            Some("from env var"),
            "task_description must be inferred from AMPLIHACK_TASK_DESCRIPTION"
        );
        assert!(
            inferred.iter().any(|s| s.contains("task_description")),
            "Inferred list must mention task_description. Got: {:?}",
            inferred
        );
    }

    /// When repo_path is a required context key, it defaults to "." if no env var is set.
    #[test]
    fn test_infer_missing_context_repo_path_defaults_to_dot() {
        // Ensure no override is present
        unsafe { std::env::remove_var("AMPLIHACK_REPO_PATH") };

        let mut recipe_defaults = BTreeMap::new();
        recipe_defaults.insert(
            "repo_path".to_string(),
            serde_yaml::Value::String(String::new()),
        );

        let (merged, _inferred) = infer_missing_context(&recipe_defaults, &BTreeMap::new());

        assert_eq!(
            merged.get("repo_path").map(String::as_str),
            Some("."),
            "repo_path must default to '.' when AMPLIHACK_REPO_PATH is not set"
        );
    }

    // -------------------------------------------------------------------------
    // format_recipe_run_result — output formatting
    // -------------------------------------------------------------------------

    fn make_result(success: bool) -> RecipeRunResult {
        RecipeRunResult {
            recipe_name: "test-recipe".to_string(),
            success,
            step_results: vec![RecipeRunStepResult {
                step_id: "step-1".to_string(),
                status: if success {
                    "completed".to_string()
                } else {
                    "failed".to_string()
                },
                output: "step output here".to_string(),
                error: if success {
                    String::new()
                } else {
                    "something broke".to_string()
                },
            }],
            context: Default::default(),
        }
    }

    /// JSON format must include recipe_name, success, and step_results.
    #[test]
    fn test_format_recipe_run_result_json_contains_required_fields() {
        let result = make_result(true);
        let json_str =
            format_recipe_run_result(&result, OutputFormat::Json, false).expect("format failed");

        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("output must be valid JSON");

        assert_eq!(
            parsed["recipe_name"].as_str(),
            Some("test-recipe"),
            "JSON output must contain 'recipe_name'"
        );
        assert_eq!(
            parsed["success"].as_bool(),
            Some(true),
            "JSON output must contain 'success: true'"
        );
        assert!(
            parsed["step_results"].is_array(),
            "JSON output must contain 'step_results' array"
        );
        assert_eq!(
            parsed["step_results"][0]["step_id"].as_str(),
            Some("step-1"),
            "step_results must contain step_id"
        );
    }

    /// Table format success output must contain '✓' symbol and recipe name.
    #[test]
    fn test_format_recipe_run_result_table_shows_success_symbol() {
        let result = make_result(true);
        let table =
            format_recipe_run_result(&result, OutputFormat::Table, false).expect("format failed");

        assert!(
            table.contains("✓ Success") || table.contains("✓"),
            "Table output must show success symbol '✓'. Got:\n{table}"
        );
        assert!(
            table.contains("test-recipe"),
            "Table output must show the recipe name. Got:\n{table}"
        );
    }

    /// Table format failure output must contain '✗' symbol.
    #[test]
    fn test_format_recipe_run_result_table_shows_failure_symbol() {
        let result = make_result(false);
        let table =
            format_recipe_run_result(&result, OutputFormat::Table, false).expect("format failed");

        assert!(
            table.contains("✗ Failed") || table.contains("✗"),
            "Table output must show failure symbol '✗'. Got:\n{table}"
        );
        assert!(
            table.contains("something broke"),
            "Table output must show the step error message. Got:\n{table}"
        );
    }

    /// Table format must show step status symbol per status string.
    #[test]
    fn test_format_recipe_run_result_table_step_symbols() {
        let result = RecipeRunResult {
            recipe_name: "r".to_string(),
            success: true,
            step_results: vec![
                RecipeRunStepResult {
                    step_id: "s1".to_string(),
                    status: "completed".to_string(),
                    output: String::new(),
                    error: String::new(),
                },
                RecipeRunStepResult {
                    step_id: "s2".to_string(),
                    status: "skipped".to_string(),
                    output: String::new(),
                    error: String::new(),
                },
                RecipeRunStepResult {
                    step_id: "s3".to_string(),
                    status: "failed".to_string(),
                    output: String::new(),
                    error: String::new(),
                },
            ],
            context: Default::default(),
        };
        let table =
            format_recipe_run_result(&result, OutputFormat::Table, false).expect("format failed");

        assert!(table.contains("✓"), "completed step must show '✓'");
        assert!(table.contains("⊘"), "skipped step must show '⊘'");
        assert!(table.contains("✗"), "failed step must show '✗'");
    }

    // -------------------------------------------------------------------------
    // find_recipe_runner_binary — error message quality
    // -------------------------------------------------------------------------

    /// When recipe-runner-rs is not installed and RECIPE_RUNNER_RS_PATH is not
    /// set, find_recipe_runner_binary must return an Err with an actionable
    /// error message directing the user to install the binary.
    ///
    /// This test PASSES unconditionally (it verifies error message quality, not
    /// binary presence).  It does require that recipe-runner-rs is absent from
    /// all standard locations; if the binary IS installed the test is skipped.
    #[test]
    fn test_find_recipe_runner_binary_error_message_is_actionable() {
        // Skip if the binary happens to be installed (CI may have it)
        if which_recipe_runner_available() {
            return;
        }
        // Unset the env override so we get the real discovery path
        unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") };

        let result = find_recipe_runner_binary();
        assert!(
            result.is_err(),
            "find_recipe_runner_binary must fail when binary is not installed. Got: {:?}",
            result.ok()
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("recipe-runner-rs"),
            "Error message must name the binary. Got: {msg}"
        );
        assert!(
            msg.contains("cargo install") || msg.contains("RECIPE_RUNNER_RS_PATH"),
            "Error message must suggest how to fix the problem. Got: {msg}"
        );
    }

    /// When RECIPE_RUNNER_RS_PATH points to a non-existent file, the env override
    /// must be ignored and discovery falls through to the standard locations.
    #[test]
    fn test_find_recipe_runner_binary_ignores_nonexistent_env_path() {
        unsafe {
            std::env::set_var(
                "RECIPE_RUNNER_RS_PATH",
                "/nonexistent/path/recipe-runner-rs",
            )
        };

        let result = find_recipe_runner_binary();

        unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") };

        // The env path doesn't exist, so we fall through.
        // If recipe-runner-rs is not installed either, result is Err.
        // If it IS installed in a standard location, result is Ok.
        // Either way the env path must not prevent legitimate discovery.
        match result {
            Ok(path) => {
                assert!(
                    path.is_file(),
                    "Resolved binary must be a real file. Got: {:?}",
                    path
                );
                assert_ne!(
                    path,
                    std::path::PathBuf::from("/nonexistent/path/recipe-runner-rs"),
                    "Must not return the invalid env path"
                );
            }
            Err(e) => {
                // Acceptable: binary genuinely not installed anywhere
                assert!(
                    e.to_string().contains("recipe-runner-rs"),
                    "Error must mention the binary name. Got: {e}"
                );
            }
        }
    }

    // -------------------------------------------------------------------------
    // execute_recipe_via_rust — E2E integration (dry-run)
    //
    // *** THIS TEST FAILS until recipe-runner-rs is installed and working. ***
    //
    // It is the primary E2E gate for Workstream 3.  Once recipe-runner-rs is
    // installed (or built locally), this test must pass.
    // -------------------------------------------------------------------------

    /// When a valid recipe file is provided with --dry-run, execute_recipe_via_rust
    /// must succeed and return a RecipeRunResult with the correct recipe_name.
    ///
    /// PRECONDITIONS:
    ///   - recipe-runner-rs binary must be installed in PATH or ~/.cargo/bin/
    ///     OR RECIPE_RUNNER_RS_PATH must point to a valid binary
    ///   - A valid YAML recipe file must exist at the path used below
    ///
    /// *** FAILS currently because recipe-runner-rs binary is not installed. ***
    /// *** PASSES once WS3 fix installs / registers the binary correctly.    ***
    #[test]
    fn test_execute_recipe_via_rust_dry_run_succeeds_with_known_recipe() {
        // Skip if recipe-runner-rs is definitely not available and no override set
        if !which_recipe_runner_available() && std::env::var("RECIPE_RUNNER_RS_PATH").is_err() {
            // Still run the test so CI catches the missing binary — it will fail
            // with a clear message rather than silently pass.
        }

        // Write a minimal valid recipe YAML to a temp file.
        // NOTE: recipe-runner-rs supports step types: bash, agent, recipe.
        // The type "shell" is NOT supported — use "bash" instead.
        let mut tmp = NamedTempFile::new().expect("failed to create temp file");
        writeln!(
            tmp,
            r#"name: dry-run-test
description: Minimal recipe for E2E dry-run validation
version: "1.0"
steps:
  - id: hello
    type: bash
    command: echo hello
"#
        )
        .expect("failed to write recipe");

        let recipe_path = tmp.path();
        let context = BTreeMap::new();

        let result = execute_recipe_via_rust(recipe_path, &context, true, ".");

        assert!(
            result.is_ok(),
            "execute_recipe_via_rust with --dry-run must succeed. \
             FIX: install recipe-runner-rs (cargo install --git … or set \
             RECIPE_RUNNER_RS_PATH). Error: {:?}",
            result.err()
        );

        let run_result = result.unwrap();
        assert_eq!(
            run_result.recipe_name, "dry-run-test",
            "RecipeRunResult.recipe_name must match the recipe's 'name' field. \
             Got: {:?}",
            run_result.recipe_name
        );
        assert!(
            run_result.success,
            "Dry-run of a valid recipe must report success. \
             step_results: {:?}",
            run_result.step_results
        );
    }

    // -------------------------------------------------------------------------
    // Helper
    // -------------------------------------------------------------------------

    /// Returns true if recipe-runner-rs appears to be available on this system.
    fn which_recipe_runner_available() -> bool {
        if let Ok(p) = std::env::var("RECIPE_RUNNER_RS_PATH") {
            if std::path::Path::new(&p).is_file() {
                return true;
            }
        }
        for candidate in [
            "recipe-runner-rs",
            "~/.cargo/bin/recipe-runner-rs",
            "~/.local/bin/recipe-runner-rs",
        ] {
            if resolve_binary_path(candidate).is_some() {
                return true;
            }
        }
        false
    }
}
