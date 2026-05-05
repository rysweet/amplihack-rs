use super::*;

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
    let json_str = format::format_recipe_run_result(&result, OutputFormat::Json, false)
        .expect("format failed");

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
    let table = format::format_recipe_run_result(&result, OutputFormat::Table, false)
        .expect("format failed");

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
    let table = format::format_recipe_run_result(&result, OutputFormat::Table, false)
        .expect("format failed");

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
    let table = format::format_recipe_run_result(&result, OutputFormat::Table, false)
        .expect("format failed");

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
#[test]
fn test_find_recipe_runner_binary_error_message_is_actionable() {
    if which_recipe_runner_available() {
        return;
    }
    unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") };

    let result = binary::find_recipe_runner_binary();
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

    let result = binary::find_recipe_runner_binary();

    unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") };

    match result {
        Ok(path) => {
            assert!(
                path.is_file(),
                "Resolved binary must be a real file. Got: {path:?}"
            );
            assert_ne!(
                path,
                std::path::PathBuf::from("/nonexistent/path/recipe-runner-rs"),
                "Must not return the invalid env path"
            );
        }
        Err(e) => {
            assert!(
                e.to_string().contains("recipe-runner-rs"),
                "Error must mention the binary name. Got: {e}"
            );
        }
    }
}

#[test]
fn test_meaningful_stderr_tail_skips_progress_noise() {
    let tail = execute::meaningful_stderr_tail(
        "▶ step-02b-analyze-codebase\n  [agent] ... working\nreal error one\n✓ step-02b-analyze-codebase\nreal error two\n",
    );

    assert_eq!(tail, "real error one\nreal error two");
}

/// Returns true if recipe-runner-rs appears to be available on this system.
fn which_recipe_runner_available() -> bool {
    if let Ok(p) = std::env::var("RECIPE_RUNNER_RS_PATH")
        && std::path::Path::new(&p).is_file()
    {
        return true;
    }
    for candidate in [
        "recipe-runner-rs",
        "~/.cargo/bin/recipe-runner-rs",
        "~/.local/bin/recipe-runner-rs",
    ] {
        if binary::resolve_binary_path(candidate).is_some() {
            return true;
        }
    }
    false
}
