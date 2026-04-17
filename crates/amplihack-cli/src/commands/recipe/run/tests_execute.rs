use super::*;
use std::collections::BTreeMap;
use std::io::Write as IoWrite;
use std::path::Path;
use tempfile::NamedTempFile;

// -------------------------------------------------------------------------
// execute_recipe_via_rust — E2E integration (dry-run, requires binary in PATH)
// -------------------------------------------------------------------------

/// When a valid recipe file is provided with --dry-run, execute_recipe_via_rust
/// must succeed and return a RecipeRunResult with the correct recipe_name.
///
/// Ignored unless recipe-runner-rs binary is installed in PATH or
/// RECIPE_RUNNER_RS_PATH is set.
#[test]
#[ignore = "requires recipe-runner-rs binary in PATH"]
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

    let result = execute::execute_recipe_via_rust(recipe_path, &context, true, Path::new("."));

    assert!(
        result.is_ok(),
        "execute_recipe_via_rust with --dry-run must succeed \
         (requires recipe-runner-rs binary in PATH or RECIPE_RUNNER_RS_PATH). \
         Error: {:?}",
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

#[test]
fn test_execute_recipe_via_rust_propagates_asset_resolver_env() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let resolver = temp.path().join("amplihack-asset-resolver");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"env-probe\",\"success\":true,\"step_results\":[],\"context\":{\"resolver\":\"$AMPLIHACK_ASSET_RESOLVER\",\"home\":\"$AMPLIHACK_HOME\",\"graph\":\"$AMPLIHACK_GRAPH_DB_PATH\",\"legacy_graph_alias\":\"$AMPLIHACK_KUZU_DB_PATH\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");
    std::fs::write(&resolver, "#!/bin/sh\nexit 0\n").expect("failed to write resolver stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
        std::fs::set_permissions(&resolver, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod resolver");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: env-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_path = std::env::var_os("PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_resolver = std::env::var_os("AMPLIHACK_ASSET_RESOLVER");
    let prev_graph = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
    let prev_kuzu = std::env::var_os("AMPLIHACK_KUZU_DB_PATH");
    let new_path = match &prev_path {
        Some(value) if !value.is_empty() => {
            format!("{}:{}", temp.path().display(), value.to_string_lossy())
        }
        _ => temp.path().display().to_string(),
    };
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("PATH", &new_path);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", "/custom/graph");
        std::env::set_var("AMPLIHACK_KUZU_DB_PATH", "/custom/legacy");
        std::env::remove_var("AMPLIHACK_ASSET_RESOLVER");
    }

    let result = execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), true, temp.path())
        .expect("recipe run must succeed");

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_path {
        Some(value) => unsafe { std::env::set_var("PATH", value) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    match prev_home {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_resolver {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_ASSET_RESOLVER", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_ASSET_RESOLVER") },
    }
    match prev_graph {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") },
    }
    match prev_kuzu {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_KUZU_DB_PATH", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_KUZU_DB_PATH") },
    }

    assert_eq!(
        result.context.get("resolver"),
        Some(&JsonValue::String(resolver.to_string_lossy().into_owned()))
    );
    assert_eq!(
        result.context.get("home"),
        Some(&JsonValue::String(
            amplihack_home.to_string_lossy().into_owned()
        ))
    );
    assert_eq!(
        result.context.get("graph"),
        Some(&JsonValue::String("/custom/graph".to_string()))
    );
    assert_eq!(
        result.context.get("legacy_graph_alias"),
        Some(&JsonValue::String(String::new()))
    );
}

#[test]
fn test_execute_recipe_via_rust_propagates_agent_binary_env() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"env-probe\",\"success\":true,\"step_results\":[],\"context\":{\"agent_binary\":\"$AMPLIHACK_AGENT_BINARY\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: env-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_agent = std::env::var_os("AMPLIHACK_AGENT_BINARY");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::set_var("AMPLIHACK_AGENT_BINARY", "copilot");
    }

    let result = execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), true, temp.path())
        .expect("recipe run must succeed");

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_agent {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_AGENT_BINARY", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_AGENT_BINARY") },
    }

    assert_eq!(
        result.context.get("agent_binary"),
        Some(&JsonValue::String("copilot".to_string()))
    );
}

#[test]
fn test_execute_recipe_via_rust_reports_nonzero_exit_with_stderr() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    std::fs::write(&runner, "#!/bin/sh\necho \"runner exploded\" >&2\nexit 2\n")
        .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: env-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), true, temp.path());

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    let error = result.expect_err("nonzero runner exit must return an error");
    assert!(
        error
            .to_string()
            .contains("Rust recipe runner failed (exit 2): runner exploded"),
        "nonzero exit must surface stderr clearly. Got: {error}"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_reports_signal_kill_clearly() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    std::fs::write(&runner, "#!/bin/sh\nkill -TERM $$\n").expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: env-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), true, temp.path());

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    let error = result.expect_err("signal-killed runner must return an error");
    assert!(
        error.to_string().contains("SIGTERM"),
        "signal kill must surface SIGTERM clearly. Got: {error}"
    );
}

// -------------------------------------------------------------------------
// pass_context — unit tests for E2BIG mitigation (issues #209, #211)
// -------------------------------------------------------------------------

#[test]
fn test_pass_context_uses_args_for_small_payloads() {
    let mut context = BTreeMap::new();
    context.insert("key1".to_string(), "value1".to_string());
    context.insert("key2".to_string(), "value2".to_string());

    let mut command = Command::new("echo");
    let tmp = execute::pass_context(&mut command, &context).unwrap();

    // Small payloads should not produce a temp file.
    assert!(
        tmp.is_none(),
        "small context should use CLI args, not a file"
    );
}

#[test]
fn test_pass_context_uses_file_for_large_payloads() {
    let mut context = BTreeMap::new();
    // Create a payload well over the 128KB threshold.
    let big_value = "x".repeat(200 * 1024);
    context.insert("task_description".to_string(), big_value.clone());

    let mut command = Command::new("echo");
    let tmp = execute::pass_context(&mut command, &context).unwrap();

    assert!(tmp.is_some(), "large context must use a temp file");

    // Verify the temp file contains valid JSON with the context.
    let file = tmp.unwrap();
    let content = std::fs::read_to_string(file.path()).unwrap();
    let parsed: BTreeMap<String, String> = serde_json::from_str(&content).unwrap();
    assert_eq!(
        parsed.get("task_description").map(String::as_str),
        Some(big_value.as_str())
    );
}

#[test]
fn test_pass_context_empty_returns_none() {
    let context = BTreeMap::new();
    let mut command = Command::new("echo");
    let tmp = execute::pass_context(&mut command, &context).unwrap();
    assert!(tmp.is_none());
}

#[test]
#[cfg(unix)]
fn test_large_context_does_not_hit_e2big() {
    // End-to-end: run a stub binary with a context value larger than typical
    // ARG_MAX. The binary reads --context-file and echoes success.
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");

    // Stub: if --context-file is present, cat it and report success.
    // Otherwise report the --set args count.
    std::fs::write(
        &runner,
        r#"#!/bin/sh
CONTEXT_FILE=""
for arg in "$@"; do
    if [ "$prev" = "--context-file" ]; then
        CONTEXT_FILE="$arg"
    fi
    prev="$arg"
done
if [ -n "$CONTEXT_FILE" ]; then
    # Verify the file exists and is valid JSON
    python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps({'recipe_name':'large-ctx','success':True,'step_results':[],'context':{'got_file':'true','keys':str(len(d))}}))" "$CONTEXT_FILE"
else
    echo '{"recipe_name":"large-ctx","success":true,"step_results":[],"context":{"got_file":"false"}}'
fi
"#,
    )
    .expect("failed to write runner stub");

    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: large-ctx\nsteps: []\n").expect("failed to write recipe");

    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).unwrap();

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    // Context of ~256KB — would exceed ARG_MAX on many systems.
    let mut context = BTreeMap::new();
    context.insert("task_description".to_string(), "x".repeat(256 * 1024));

    let result = execute::execute_recipe_via_rust(&recipe, &context, true, temp.path());

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }

    let run_result =
        result.expect("execute_recipe_via_rust must not fail with E2BIG for large context");
    assert_eq!(run_result.recipe_name, "large-ctx");
    assert!(run_result.success);
    assert_eq!(
        run_result.context.get("got_file"),
        Some(&serde_json::json!("true")),
        "large context must be passed via --context-file, not --set args"
    );
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
