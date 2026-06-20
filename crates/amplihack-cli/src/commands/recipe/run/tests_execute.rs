use super::*;
use std::collections::BTreeMap;
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
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

    let result = execute::execute_recipe_via_rust(
        recipe_path,
        &context,
        true,
        false,
        Path::new("."),
        &[],
        None,
    );

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

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    )
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
    let expected_graph = temp.path().join(".amplihack").join("graph_db");
    assert_eq!(
        result.context.get("graph"),
        Some(&JsonValue::String(
            expected_graph.to_string_lossy().into_owned()
        )),
        "explicit project_root (temp dir) must win over inherited AMPLIHACK_GRAPH_DB_PATH (issue #250)"
    );
    assert_eq!(
        result.context.get("legacy_graph_alias"),
        Some(&JsonValue::String(String::new()))
    );
}

#[test]
fn test_execute_recipe_via_rust_sets_home_from_working_dir_bundle_root() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let repo = temp.path().join("repo");
    let bundle = repo.join("amplifier-bundle");
    let home = temp.path().join("home");
    std::fs::create_dir_all(&bundle).expect("failed to create bundle root");
    std::fs::create_dir_all(&home).expect("failed to create home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"home-probe\",\"success\":true,\"step_results\":[],\"context\":{\"home\":\"$AMPLIHACK_HOME\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = repo.join("recipe.yaml");
    std::fs::write(&recipe, "name: home-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("HOME");
    let prev_amplihack_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("HOME", &home);
        std::env::remove_var("AMPLIHACK_HOME");
    }

    let result =
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), true, false, &repo, &[], None)
            .expect("recipe run must succeed");

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(value) => unsafe { std::env::set_var("HOME", value) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match prev_amplihack_home {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }

    let expected_home = repo.canonicalize().expect("repo should canonicalize");
    assert_eq!(
        result.context.get("home"),
        Some(&JsonValue::String(
            expected_home.to_string_lossy().into_owned()
        )),
        "recipe-runner subprocesses must resolve AMPLIHACK_HOME from working_dir bundle root before HOME"
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

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    )
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
#[cfg(unix)]
fn test_execute_recipe_via_rust_sets_pager_safe_env() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"pager-probe\",\"success\":true,\"step_results\":[],\"context\":{\"git_pager\":\"$GIT_PAGER\",\"gh_pager\":\"$GH_PAGER\",\"pager\":\"$PAGER\",\"less\":\"$LESS\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: pager-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_git_pager = std::env::var_os("GIT_PAGER");
    let prev_gh_pager = std::env::var_os("GH_PAGER");
    let prev_pager = std::env::var_os("PAGER");
    let prev_less = std::env::var_os("LESS");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::remove_var("GIT_PAGER");
        std::env::remove_var("GH_PAGER");
        std::env::remove_var("PAGER");
        std::env::remove_var("LESS");
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect("recipe run must succeed");

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_git_pager {
        Some(value) => unsafe { std::env::set_var("GIT_PAGER", value) },
        None => unsafe { std::env::remove_var("GIT_PAGER") },
    }
    match prev_gh_pager {
        Some(value) => unsafe { std::env::set_var("GH_PAGER", value) },
        None => unsafe { std::env::remove_var("GH_PAGER") },
    }
    match prev_pager {
        Some(value) => unsafe { std::env::set_var("PAGER", value) },
        None => unsafe { std::env::remove_var("PAGER") },
    }
    match prev_less {
        Some(value) => unsafe { std::env::set_var("LESS", value) },
        None => unsafe { std::env::remove_var("LESS") },
    }

    assert_eq!(
        result.context.get("git_pager"),
        Some(&JsonValue::String("cat".to_string())),
        "recipe-runner subprocesses must receive GIT_PAGER=cat to prevent git pager hangs"
    );
    assert_eq!(
        result.context.get("gh_pager"),
        Some(&JsonValue::String("cat".to_string())),
        "recipe-runner subprocesses must receive GH_PAGER=cat to prevent gh pager hangs"
    );
    assert_eq!(
        result.context.get("pager"),
        Some(&JsonValue::String("cat".to_string())),
        "recipe-runner subprocesses must receive PAGER=cat for noninteractive automation"
    );
    assert_eq!(
        result.context.get("less"),
        Some(&JsonValue::String("FRX".to_string())),
        "recipe-runner subprocesses must receive LESS=FRX so unavoidable less invocations do not block"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_forces_noninteractive_and_strips_claudecode() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"subprocess-env-probe\",\"success\":true,\"step_results\":[],\"context\":{\"noninteractive\":\"$AMPLIHACK_NONINTERACTIVE\",\"claudecode\":\"${CLAUDECODE+present}\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: subprocess-env-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_noninteractive = std::env::var_os("AMPLIHACK_NONINTERACTIVE");
    let prev_claudecode = std::env::var_os("CLAUDECODE");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::set_var("AMPLIHACK_NONINTERACTIVE", "0");
        std::env::set_var("CLAUDECODE", "1");
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect("recipe run must succeed");

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_HOME", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_noninteractive {
        Some(value) => unsafe { std::env::set_var("AMPLIHACK_NONINTERACTIVE", value) },
        None => unsafe { std::env::remove_var("AMPLIHACK_NONINTERACTIVE") },
    }
    match prev_claudecode {
        Some(value) => unsafe { std::env::set_var("CLAUDECODE", value) },
        None => unsafe { std::env::remove_var("CLAUDECODE") },
    }

    assert_eq!(
        result.context.get("noninteractive"),
        Some(&JsonValue::String("1".to_string())),
        "recipe-runner subprocesses must force AMPLIHACK_NONINTERACTIVE=1 even when the parent has another value"
    );
    assert_eq!(
        result.context.get("claudecode"),
        Some(&JsonValue::String(String::new())),
        "recipe-runner subprocesses must not inherit CLAUDECODE because nested Claude sessions treat it as an active host marker"
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

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    );

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    let error = result.expect_err("nonzero runner exit must return an error");
    let chain = format!("{error:?}");
    assert!(
        chain.contains("exited with 2"),
        "nonzero exit must surface exit code clearly. Got: {chain}"
    );
    assert!(
        chain.contains("runner exploded"),
        "nonzero exit must surface stderr tail in error chain. Got: {chain}"
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

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    );

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
// recipe run correlation — TDD tests for issue #753
// -------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_sets_stable_run_id_env_and_result_metadata() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"correlation-probe\",\"success\":true,\"step_results\":[],\"context\":{\"env_run_id\":\"$AMPLIHACK_RECIPE_RUN_ID\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: correlation-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false,
        false,
        temp.path(),
        &[],
        None,
    )
    .expect("recipe run must succeed");

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    let result_json = serde_json::to_value(&result).expect("result must serialize");
    let run_id = result_json
        .get("run_id")
        .and_then(JsonValue::as_str)
        .expect("RecipeRunResult must expose additive run_id metadata");
    uuid::Uuid::parse_str(run_id).expect("run_id must be a UUID");
    assert_eq!(
        result.context.get("env_run_id").and_then(JsonValue::as_str),
        Some(run_id),
        "AMPLIHACK_RECIPE_RUN_ID must match the stable result run_id"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_emits_early_and_final_success_log_pointers() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"correlation-probe\",\"success\":true,\"step_results\":[],\"context\":{}}\nEOF\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: correlation-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let mut context = BTreeMap::new();
    context.insert(
        "task_description".to_string(),
        "Fix issue #753 correlation logs".to_string(),
    );
    context.insert("issue_number".to_string(), "753".to_string());
    context.insert(
        "pr_url".to_string(),
        "https://github.com/rysweet/amplihack-rs/pull/999".to_string(),
    );

    let (result, stderr) = capture_stderr_during(|| {
        execute::execute_recipe_via_rust(&recipe, &context, false, false, temp.path(), &[], None)
    });

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    result.expect("recipe run must succeed");
    let pointers = parse_log_pointers(&stderr);
    assert_eq!(
        pointers.len(),
        2,
        "wrapper must emit exactly one early and one final pointer. stderr:\n{stderr}"
    );

    let early = pointers
        .iter()
        .find(|pointer| pointer["event"] == "early")
        .expect("missing early pointer");
    let final_pointer = pointers
        .iter()
        .find(|pointer| pointer["event"] == "final")
        .expect("missing final pointer");
    let run_id = early["run_id"].as_str().expect("early run_id is required");
    uuid::Uuid::parse_str(run_id).expect("run_id must be a UUID");

    assert_eq!(final_pointer["run_id"].as_str(), Some(run_id));
    assert_eq!(early["schema_version"].as_u64(), Some(1));
    assert_eq!(early["recipe_name"].as_str(), Some("correlation-probe"));
    assert_eq!(early["cwd"].as_str(), Some(temp.path().to_str().unwrap()));
    assert_eq!(
        early["worktree"].as_str(),
        Some(temp.path().to_str().unwrap())
    );
    assert_eq!(
        early["runner_path"].as_str(),
        Some(runner.to_str().unwrap())
    );
    assert_eq!(
        early["task_description"].as_str(),
        Some("Fix issue #753 correlation logs")
    );
    assert_eq!(early["issue_number"].as_str(), Some("753"));
    assert_eq!(
        early["pr_url"].as_str(),
        Some("https://github.com/rysweet/amplihack-rs/pull/999")
    );
    assert!(
        early.get("pr_number").is_none(),
        "missing metadata must be omitted rather than guessed: {early}"
    );
    assert!(
        early.get("child_pid").is_none(),
        "early pointer must not claim a child PID before spawn: {early}"
    );

    assert_eq!(final_pointer["status"].as_str(), Some("success"));
    assert_eq!(final_pointer["exit_code"].as_i64(), Some(0));
    assert!(
        final_pointer["child_pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0),
        "final success pointer must include child_pid: {final_pointer}"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_nonzero_exit_emits_failure_pointer_and_result_summary() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"correlation-probe\",\"success\":false,\"step_results\":[],\"context\":{}}\nEOF\nexit 7\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: correlation-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let (result, stderr) = capture_stderr_during(|| {
        execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        )
    });

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    let result = result.expect("parseable nonzero runner result should be returned");
    let final_pointer = only_final_pointer(&stderr);
    assert_eq!(final_pointer["status"].as_str(), Some("failure"));
    assert_eq!(final_pointer["exit_code"].as_i64(), Some(7));
    assert!(
        final_pointer["child_pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0),
        "failure pointer must include the child PID: {final_pointer}"
    );

    let result_json = serde_json::to_value(&result).expect("result must serialize");
    let log_pointer = result_json
        .get("log_pointer")
        .expect("parseable nonzero result must include final log_pointer summary");
    assert_eq!(log_pointer["status"].as_str(), Some("failure"));
    assert_eq!(log_pointer["exit_code"].as_i64(), Some(7));
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_parse_failure_emits_final_pointer() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(&runner, "#!/bin/sh\necho 'not-json'\nexit 0\n")
        .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: parse-failure-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let (result, stderr) = capture_stderr_during(|| {
        execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        )
    });

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    result.expect_err("invalid runner stdout must still fail parsing");
    let final_pointer = only_final_pointer(&stderr);
    assert_eq!(final_pointer["status"].as_str(), Some("parse_failure"));
    assert_eq!(final_pointer["exit_code"].as_i64(), Some(0));
    assert!(
        final_pointer["child_pid"]
            .as_u64()
            .is_some_and(|pid| pid > 0),
        "parse failure pointer must include child_pid: {final_pointer}"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_spawn_failure_emits_final_pointer_without_child_pid() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(&runner, "#!/bin/sh\nexit 0\n").expect("failed to write runner stub");
    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: spawn-failure-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let (result, stderr) = capture_stderr_during(|| {
        execute::execute_recipe_via_rust(
            &recipe,
            &BTreeMap::new(),
            false,
            false,
            temp.path(),
            &[],
            None,
        )
    });

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    result.expect_err("non-executable runner must fail to spawn");
    let final_pointer = only_final_pointer(&stderr);
    assert_eq!(final_pointer["status"].as_str(), Some("spawn_failure"));
    assert_eq!(
        final_pointer["runner_path"].as_str(),
        Some(runner.to_str().unwrap())
    );
    assert!(
        final_pointer.get("child_pid").is_none(),
        "spawn failure must omit child_pid because no child exists: {final_pointer}"
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

    let result =
        execute::execute_recipe_via_rust(&recipe, &context, true, false, temp.path(), &[], None);

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

// -------------------------------------------------------------------------
// step_timeout env var propagation — TDD tests for issue #439
// -------------------------------------------------------------------------

/// When step_timeout=Some(600), AMPLIHACK_STEP_TIMEOUT must be set to "600"
/// in the child process environment.
#[test]
#[cfg(unix)]
fn test_step_timeout_propagated_as_env_var() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    // Stub captures the AMPLIHACK_STEP_TIMEOUT env var and returns it in context.
    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"timeout-probe\",\"success\":true,\"step_results\":[],\"context\":{\"step_timeout\":\"$AMPLIHACK_STEP_TIMEOUT\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: timeout-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_timeout = std::env::var_os("AMPLIHACK_STEP_TIMEOUT");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        // Ensure no inherited value interferes
        std::env::remove_var("AMPLIHACK_STEP_TIMEOUT");
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,  // dry_run
        false, // verbose
        temp.path(),
        &[],
        Some(600), // step_timeout = 600 seconds
    )
    .expect("recipe run must succeed");

    // Restore env
    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_timeout {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_STEP_TIMEOUT", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_STEP_TIMEOUT") },
    }

    assert_eq!(
        result.context.get("step_timeout"),
        Some(&serde_json::json!("600")),
        "step_timeout=Some(600) must set AMPLIHACK_STEP_TIMEOUT=600 in child env"
    );
}

/// When step_timeout=Some(0), AMPLIHACK_STEP_TIMEOUT must be set to "0"
/// (meaning: disable all step timeouts).
#[test]
#[cfg(unix)]
fn test_step_timeout_zero_disables_timeouts() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"timeout-probe\",\"success\":true,\"step_results\":[],\"context\":{\"step_timeout\":\"$AMPLIHACK_STEP_TIMEOUT\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: timeout-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_timeout = std::env::var_os("AMPLIHACK_STEP_TIMEOUT");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::remove_var("AMPLIHACK_STEP_TIMEOUT");
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        Some(0), // step_timeout = 0 means disable timeouts
    )
    .expect("recipe run must succeed");

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_timeout {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_STEP_TIMEOUT", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_STEP_TIMEOUT") },
    }

    assert_eq!(
        result.context.get("step_timeout"),
        Some(&serde_json::json!("0")),
        "step_timeout=Some(0) must set AMPLIHACK_STEP_TIMEOUT=0 in child env (disable timeouts)"
    );
}

/// When step_timeout=None, AMPLIHACK_STEP_TIMEOUT must NOT be injected
/// by execute_recipe_via_rust (so parent-inherited or unset values flow
/// through naturally).
#[test]
#[cfg(unix)]
fn test_step_timeout_none_does_not_set_env_var() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    // Stub captures env var; if unset, shell expands $VAR to empty string.
    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"timeout-probe\",\"success\":true,\"step_results\":[],\"context\":{\"step_timeout\":\"$AMPLIHACK_STEP_TIMEOUT\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
            .expect("failed to chmod runner");
    }

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: timeout-probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_timeout = std::env::var_os("AMPLIHACK_STEP_TIMEOUT");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        // Ensure no inherited value
        std::env::remove_var("AMPLIHACK_STEP_TIMEOUT");
    }

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None, // step_timeout = None means no override
    )
    .expect("recipe run must succeed");

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_HOME", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_HOME") },
    }
    match prev_timeout {
        Some(v) => unsafe { std::env::set_var("AMPLIHACK_STEP_TIMEOUT", v) },
        None => unsafe { std::env::remove_var("AMPLIHACK_STEP_TIMEOUT") },
    }

    // When no parent env var is set AND step_timeout=None, the child sees empty string.
    assert_eq!(
        result.context.get("step_timeout"),
        Some(&serde_json::json!("")),
        "step_timeout=None must NOT inject AMPLIHACK_STEP_TIMEOUT into child env"
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

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");
}

fn restore_env_var(key: &str, previous: Option<std::ffi::OsString>) {
    match previous {
        Some(value) => unsafe { std::env::set_var(key, value) },
        None => unsafe { std::env::remove_var(key) },
    }
}

#[cfg(unix)]
fn capture_stderr_during<F, T>(f: F) -> (T, String)
where
    F: FnOnce() -> T,
{
    use std::os::unix::io::AsRawFd;

    let file = tempfile::NamedTempFile::new().expect("failed to create stderr capture file");
    let stderr_fd = libc::STDERR_FILENO;
    let saved_stderr = unsafe { libc::dup(stderr_fd) };
    assert!(saved_stderr >= 0, "failed to duplicate stderr fd");

    let redirect_result = unsafe { libc::dup2(file.as_file().as_raw_fd(), stderr_fd) };
    assert!(redirect_result >= 0, "failed to redirect stderr");

    let result = f();
    let _ = std::io::stderr().lock().flush();

    let restore_result = unsafe { libc::dup2(saved_stderr, stderr_fd) };
    assert!(restore_result >= 0, "failed to restore stderr");
    unsafe {
        libc::close(saved_stderr);
    }

    let stderr = std::fs::read_to_string(file.path()).expect("failed to read captured stderr");
    (result, stderr)
}

fn parse_log_pointers(stderr: &str) -> Vec<JsonValue> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("amplihack.recipe.log_pointer "))
        .map(|payload| serde_json::from_str(payload).expect("pointer payload must be valid JSON"))
        .collect()
}

fn only_final_pointer(stderr: &str) -> JsonValue {
    let pointers = parse_log_pointers(stderr);
    pointers
        .iter()
        .find(|pointer| pointer["event"] == "final")
        .cloned()
        .unwrap_or_else(|| panic!("missing final pointer in stderr:\n{stderr}"))
}

// -------------------------------------------------------------------------
// parse_recipe_output — pure parser unit tests (issue #332)
// -------------------------------------------------------------------------

/// Empty stdout + exit success is a hollow workflow success and must fail
/// closed with explicit terminal/finalization state instead of becoming a
/// success-shaped no-op.
#[test]
fn parse_empty_stdout_success_returns_hollow_success_terminal_failure() {
    let result =
        execute::parse_recipe_output("", "", true).expect("empty stdout on success must not error");
    assert_eq!(
        result.success, false,
        "empty successful runner output must not be reported as workflow success"
    );
    assert_eq!(
        result
            .extra
            .get("workflow_result")
            .and_then(|value| value.get("terminal_state"))
            .and_then(JsonValue::as_str),
        Some("HOLLOW_SUCCESS")
    );
    assert_eq!(
        result
            .extra
            .get("workflow_result")
            .and_then(|value| value.get("terminal_success"))
            .and_then(JsonValue::as_bool),
        Some(false)
    );
}

/// Empty stdout + exit failure: must error with stderr tail surfaced
/// in the message so callers can diagnose upstream failures.
#[test]
fn parse_empty_stdout_failure_errors_with_stderr_tail() {
    let stderr = "warm-up line\nERROR: recipe-runner crashed\nbacktrace omitted";
    let err = execute::parse_recipe_output("", stderr, false)
        .expect_err("empty stdout on failure must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("ERROR: recipe-runner crashed"),
        "stderr tail must appear in error chain, got: {msg}"
    );
}

/// Plain (non-JSON) text on stdout must error with context that includes
/// a stdout preview so users can see what was actually returned.
#[test]
fn parse_plain_text_stdout_errors_with_context() {
    let stdout = "this is not JSON, just a plain text line";
    let err =
        execute::parse_recipe_output(stdout, "", true).expect_err("plain text stdout must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("plain text"),
        "stdout preview must appear in error context, got: {msg}"
    );
}

/// Malformed JSON on stdout must error with context (truncated preview
/// + stderr tail) so the user sees the bad payload.
#[test]
fn parse_malformed_json_errors_with_context() {
    let stdout = r#"{"recipe_name": "x", "success": tru"#; // truncated
    let stderr = "stderr-marker-xyz";
    let err =
        execute::parse_recipe_output(stdout, stderr, true).expect_err("malformed JSON must error");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("recipe_name") || msg.contains("success"),
        "stdout preview must appear in error, got: {msg}"
    );
    assert!(
        msg.contains("stderr-marker-xyz"),
        "stderr tail must appear in error, got: {msg}"
    );
}

/// Valid JSON must parse successfully into RecipeRunResult.
#[test]
fn parse_valid_json_succeeds() {
    let stdout = r#"{
        "recipe_name": "demo",
        "success": true,
        "step_results": [
            {"step_id": "s1", "status": "ok", "output": "hello", "error": ""}
        ],
        "context": {"k": "v"}
    }"#;
    let result = execute::parse_recipe_output(stdout, "", true).expect("valid JSON must parse");
    assert_eq!(result.recipe_name, "demo");
    assert!(result.success);
    assert_eq!(result.step_results.len(), 1);
    assert_eq!(result.step_results[0].step_id, "s1");
    assert_eq!(
        result.context.get("k").and_then(serde_json::Value::as_str),
        Some("v")
    );
}

/// Valid JSON with extra/unknown top-level fields must still parse —
/// serde ignores unknown fields by default and we rely on that contract
/// for forward compatibility with future recipe-runner-rs versions.
#[test]
fn parse_valid_json_with_unknown_fields_succeeds() {
    let stdout = r#"{
        "recipe_name": "demo",
        "success": true,
        "step_results": [],
        "context": {},
        "future_field_xyz": 42,
        "another_unknown": {"nested": "value"}
    }"#;
    let result = execute::parse_recipe_output(stdout, "", true)
        .expect("unknown fields must be ignored, not rejected");
    assert_eq!(result.recipe_name, "demo");
    assert!(result.success);
}

#[test]
fn parse_and_format_preserves_additive_transparency_fields() {
    let stdout = r#"{
        "recipe_name": "transparent-demo",
        "success": false,
        "status": "failed",
        "phase": "agent",
        "elapsed_ms": 91234,
        "heartbeat_at": "2026-06-03T18:25:41Z",
        "progress_summary": {
            "phase": "agent",
            "status": "running",
            "heartbeat_count": 3
        },
        "step_results": [
            {
                "step_id": "step-07-tdd",
                "step_name": "TDD - Write Tests First",
                "status": "failed",
                "phase": "agent",
                "elapsed_ms": 45678,
                "child": { "kind": "agent", "name": "builder" },
                "recent_stdout": ["stdout tail line"],
                "recent_stderr": ["stderr tail line"],
                "error": "agent failed"
            }
        ],
        "context": {}
    }"#;

    let result =
        execute::parse_recipe_output(stdout, "", false).expect("additive JSON fields must parse");
    let formatted = format::format_recipe_run_result(&result, OutputFormat::Json, false)
        .expect("formatting parsed result as JSON must succeed");
    let parsed: JsonValue =
        serde_json::from_str(&formatted).expect("formatted JSON must remain valid JSON");

    assert_eq!(
        parsed["status"].as_str(),
        Some("failed"),
        "top-level additive status must be preserved in formatted JSON: {formatted}",
    );
    assert_eq!(
        parsed["phase"].as_str(),
        Some("agent"),
        "top-level additive phase must be preserved in formatted JSON: {formatted}",
    );
    assert_eq!(
        parsed["step_results"][0]["step_name"].as_str(),
        Some("TDD - Write Tests First"),
        "step name must be preserved in formatted JSON: {formatted}",
    );
    assert_eq!(
        parsed["step_results"][0]["child"]["name"].as_str(),
        Some("builder"),
        "child identity must be preserved in formatted JSON: {formatted}",
    );
    assert_eq!(
        parsed["step_results"][0]["recent_stderr"][0].as_str(),
        Some("stderr tail line"),
        "recent stderr snippets must be preserved in formatted JSON: {formatted}",
    );
}

#[test]
fn failure_table_surfaces_step_timing_child_and_recent_output() {
    let stdout = r#"{
        "recipe_name": "transparent-demo",
        "success": false,
        "step_results": [
            {
                "step_id": "step-08-implementation",
                "step_name": "Implementation",
                "status": "failed",
                "phase": "agent",
                "elapsed_ms": 65000,
                "child": { "kind": "agent", "name": "builder" },
                "recent_stdout": ["compiled 4 crates"],
                "recent_stderr": ["error[E0425]: cannot find value `x`"],
                "error": "step failed"
            }
        ],
        "context": {}
    }"#;

    let result =
        execute::parse_recipe_output(stdout, "", false).expect("additive JSON fields must parse");
    let table = format::format_recipe_run_result(&result, OutputFormat::Table, false)
        .expect("formatting parsed result as table must succeed");

    assert!(
        table.contains("Implementation"),
        "failure table must include human step name for actionable context. Got:\n{table}",
    );
    assert!(
        table.contains("65s") || table.contains("65000ms"),
        "failure table must include elapsed time. Got:\n{table}",
    );
    assert!(
        table.contains("builder"),
        "failure table must include child agent/subprocess identity. Got:\n{table}",
    );
    assert!(
        table.contains("cannot find value"),
        "failure table must include bounded recent stderr snippet. Got:\n{table}",
    );
    assert!(
        table.contains("compiled 4 crates"),
        "failure table must include bounded recent stdout snippet. Got:\n{table}",
    );
}

#[test]
fn failure_table_bounds_recent_output_snippets() {
    let stderr_lines = (0..40)
        .map(|i| format!("stderr tail line {i:02}"))
        .collect::<Vec<_>>();
    let stdout_lines = (0..40)
        .map(|i| format!("stdout tail line {i:02}"))
        .collect::<Vec<_>>();
    let stdout = serde_json::json!({
        "recipe_name": "bounded-demo",
        "success": false,
        "step_results": [
            {
                "step_id": "long-agent-step",
                "step_name": "Long Agent Step",
                "status": "failed",
                "phase": "agent",
                "elapsed_ms": 120000,
                "child": { "kind": "agent", "name": "builder" },
                "recent_stdout": stdout_lines,
                "recent_stderr": stderr_lines,
                "error": "step failed"
            }
        ],
        "context": {}
    })
    .to_string();

    let result = execute::parse_recipe_output(&stdout, "", false)
        .expect("additive JSON with long snippets must parse");
    let table = format::format_recipe_run_result(&result, OutputFormat::Table, false)
        .expect("formatting parsed result as table must succeed");

    assert!(
        table.contains("stderr tail line 39"),
        "bounded snippet display should keep the newest stderr lines. Got:\n{table}",
    );
    assert!(
        table.contains("stdout tail line 39"),
        "bounded snippet display should keep the newest stdout lines. Got:\n{table}",
    );
    assert!(
        !table.contains("stderr tail line 00"),
        "bounded snippet display must omit oldest stderr lines instead of dumping all child output. Got:\n{table}",
    );
    assert!(
        !table.contains("stdout tail line 00"),
        "bounded snippet display must omit oldest stdout lines instead of dumping all child output. Got:\n{table}",
    );
    assert!(
        table.len() < 4_000,
        "bounded snippet display must stay compact; got {} bytes:\n{table}",
        table.len(),
    );
}

/// Whitespace-only stdout (e.g. trailing newline) must be treated as empty.
#[test]
fn parse_whitespace_only_stdout_success_returns_hollow_success_terminal_failure() {
    let result = execute::parse_recipe_output("   \n\t  \n", "", true)
        .expect("whitespace-only stdout on success must not error");
    assert!(!result.success);
    assert_eq!(
        result
            .extra
            .get("workflow_result")
            .and_then(|value| value.get("terminal_state"))
            .and_then(JsonValue::as_str),
        Some("HOLLOW_SUCCESS")
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_sets_isolated_runtime_directories() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let repo = temp.path().join("repo");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&repo).expect("failed to create repo dir");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\ncat <<EOF\n{\"recipe_name\":\"runtime-isolation-probe\",\"success\":true,\"step_results\":[],\"context\":{\"runtime_dir\":\"$AMPLIHACK_WORKFLOW_RUNTIME_DIR\",\"artifact_dir\":\"$AMPLIHACK_WORKFLOW_ARTIFACT_DIR\",\"tmpdir\":\"$TMPDIR\"}}\nEOF\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = repo.join("recipe.yaml");
    std::fs::write(&recipe, "name: runtime-isolation-probe\nsteps: []\n")
        .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
    }

    let result =
        execute::execute_recipe_via_rust(&recipe, &BTreeMap::new(), false, false, &repo, &[], None)
            .expect("recipe run must succeed");

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);

    let runtime_dir = result
        .context
        .get("runtime_dir")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let artifact_dir = result
        .context
        .get("artifact_dir")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let tmpdir = result
        .context
        .get("tmpdir")
        .and_then(JsonValue::as_str)
        .unwrap_or_default();
    let repo_prefix = repo.to_string_lossy();

    assert!(
        !runtime_dir.is_empty(),
        "runtime must expose AMPLIHACK_WORKFLOW_RUNTIME_DIR"
    );
    assert!(
        !runtime_dir.starts_with(repo_prefix.as_ref()),
        "workflow runtime dir must be outside the commit worktree"
    );
    assert!(
        artifact_dir.starts_with(runtime_dir),
        "workflow artifact dir should be scoped under the isolated runtime dir"
    );
    assert!(
        tmpdir.starts_with(runtime_dir),
        "TMPDIR should be scoped under the isolated runtime dir for child steps"
    );
}

// Issue #691: recipe-runner-rs owns progress emission. The CLI must not pass
// the old unsupported --progress flag; it should forward child stderr by
// default instead.
#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_verbose_does_not_pass_progress_flag_to_child() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    // Stub records its argv and exits 0 with empty stdout (parser treats as success).
    std::fs::write(
        &runner,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> {log}; done\nexit 0\n",
            log = arg_log.display()
        ),
    )
    .expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false, // dry_run
        true,  // verbose
        temp.path(),
        &[],
        None, // step_timeout
    );

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("verbose mode with empty stdout success must not error");
    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    assert!(
        !logged.lines().any(|l| l == "--progress"),
        "verbose=true must not pass unsupported --progress; progress is emitted by recipe-runner-rs by default.\nargv was:\n{logged}",
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_non_verbose_does_not_pass_progress() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    std::fs::write(
        &runner,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> {log}; done\nexit 0\n",
            log = arg_log.display()
        ),
    )
    .expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let _ = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false, // dry_run
        false, // verbose
        temp.path(),
        &[],
        None, // step_timeout
    );

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    assert!(
        !logged.lines().any(|l| l == "--progress"),
        "verbose=false must NOT pass --progress.\nargv was:\n{logged}",
    );
}

// Issue #366 (COE feedback): when child writes non-UTF-8 bytes to stderr,
// the pump must NOT terminate (would risk a stalled-pipe hang on the child).
// We use a stub that emits raw 0xFF bytes followed by valid UTF-8 lines and
// then exits 0 with empty stdout. If the pump dies on the bad bytes, the
// trailing UTF-8 lines won't be captured AND the child can't progress past
// a full pipe.
#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_verbose_survives_non_utf8_stderr() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    // The stub writes 0xFF (invalid UTF-8 start byte), then a valid line,
    // then enough valid lines to fill a typical pipe buffer (~64KB on Linux).
    std::fs::write(
        &runner,
        "#!/bin/sh\n\
         printf '\\xff\\n' >&2\n\
         echo 'after-bad-bytes' >&2\n\
         i=0; while [ $i -lt 200 ]; do echo \"flood-line-$i\" >&2; i=$((i+1)); done\n\
         exit 0\n",
    )
    .expect("failed to write runner stub");

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    // The real assertion is that this does not hang. If the pump dies, the
    // child's stderr pipe fills (~64KB) and SIGPIPE/blocking-write hangs the
    // child. With a 30s test timeout, that hang would manifest as the test
    // being killed by the runner — but locally just observed via wall-clock.
    let start = std::time::Instant::now();
    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        false, // dry_run
        true,  // verbose
        temp.path(),
        &[],
        None, // step_timeout
    );
    let elapsed = start.elapsed();

    match prev_runner {
        Some(value) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", value) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("non-UTF-8 stderr must NOT abort the pump or hang the child");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "non-UTF-8 stderr caused suspiciously slow run ({elapsed:?}) — \
         pump likely died and child blocked on full stderr pipe"
    );
}

// -------------------------------------------------------------------------
// Issue #494 — sub-recipe discovery: pass -R search dirs to recipe-runner-rs
// -------------------------------------------------------------------------

/// Helper: build a recipe-runner-rs stub that logs every argv entry
/// (one per line) to `arg_log` and exits 0 with empty stdout.
#[cfg(unix)]
fn write_argv_logging_stub(runner: &Path, arg_log: &Path) {
    std::fs::write(
        runner,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> {log}; done\nexit 0\n",
            log = arg_log.display()
        ),
    )
    .expect("failed to write runner stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_emits_dash_r_per_search_dir() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    write_argv_logging_stub(&runner, &arg_log);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let dir_a = temp.path().join("recipes-a");
    let dir_b = temp.path().join("recipes-b");
    let dir_c = temp.path().join("recipes-c");
    let search_dirs: Vec<PathBuf> = vec![dir_a.clone(), dir_b.clone(), dir_c.clone()];

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &search_dirs,
        None,
    );

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("execute_recipe_via_rust must succeed with search_dirs");

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    let argv: Vec<&str> = logged.lines().collect();
    let r_pairs: Vec<(usize, &str)> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            if *a == "-R" && i + 1 < argv.len() {
                Some((i, argv[i + 1]))
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        r_pairs.len(),
        3,
        "expected one -R per search dir, got {} in argv:\n{logged}",
        r_pairs.len()
    );
    assert_eq!(r_pairs[0].1, dir_a.to_string_lossy());
    assert_eq!(r_pairs[1].1, dir_b.to_string_lossy());
    assert_eq!(r_pairs[2].1, dir_c.to_string_lossy());
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_no_search_dirs_emits_no_dash_r() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    write_argv_logging_stub(&runner, &arg_log);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &[],
        None,
    );

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("empty search_dirs must still succeed");

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    assert!(
        !logged.lines().any(|l| l == "-R"),
        "empty search_dirs slice must NOT emit any -R flag.\nargv was:\n{logged}"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_skips_empty_path_strings() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    write_argv_logging_stub(&runner, &arg_log);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let valid = temp.path().join("recipes");
    let search_dirs: Vec<PathBuf> = vec![PathBuf::new(), valid.clone(), PathBuf::new()];

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &BTreeMap::new(),
        true,
        false,
        temp.path(),
        &search_dirs,
        None,
    );

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("partially-empty search_dirs must still succeed");

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    let argv: Vec<&str> = logged.lines().collect();
    let r_values: Vec<&str> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            if *a == "-R" {
                argv.get(i + 1).copied()
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        r_values.len(),
        1,
        "exactly one -R expected (empty PathBufs skipped), got {}:\n{logged}",
        r_values.len()
    );
    assert_eq!(r_values[0], valid.to_string_lossy());
    assert!(
        !argv.windows(2).any(|w| w[0] == "-R" && w[1].is_empty()),
        "empty PathBuf must never expand to `-R \"\"`.\nargv was:\n{logged}"
    );
}

#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_dash_r_position_in_argv() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    write_argv_logging_stub(&runner, &arg_log);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: probe\nsteps: []\n").expect("failed to write recipe");

    let mut context = BTreeMap::new();
    context.insert("k".to_string(), "v".to_string());

    let dir_a = temp.path().join("recipes-a");
    let search_dirs: Vec<PathBuf> = vec![dir_a];

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = execute::execute_recipe_via_rust(
        &recipe,
        &context,
        true,
        false,
        temp.path(),
        &search_dirs,
        None,
    );

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("execution must succeed");

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    let argv: Vec<&str> = logged.lines().collect();
    let pos = |needle: &str| argv.iter().position(|a| *a == needle);

    let pos_c = pos("-C").expect("-C must appear in argv");
    let pos_r = pos("-R").expect("-R must appear in argv");
    let pos_dry = pos("--dry-run").expect("--dry-run must appear in argv");
    let pos_set = pos("--set").expect("--set must appear in argv");

    assert!(pos_c < pos_r, "-R must appear AFTER -C\nargv:\n{logged}");
    assert!(
        pos_r < pos_dry,
        "-R must appear BEFORE --dry-run\nargv:\n{logged}"
    );
    assert!(
        pos_r < pos_set,
        "-R must appear BEFORE --set\nargv:\n{logged}"
    );
}

// -------------------------------------------------------------------------
// build_search_dirs — pure helper unit tests
// -------------------------------------------------------------------------

#[test]
fn test_build_search_dirs_recipe_parent_first() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let recipe = temp.path().join("co-located").join("recipe.yaml");
    std::fs::create_dir_all(recipe.parent().unwrap()).unwrap();
    std::fs::write(&recipe, "name: x\nsteps: []\n").unwrap();

    let dirs = super::build_search_dirs(&recipe, temp.path())
        .expect("build_search_dirs must not error on valid inputs");

    assert!(!dirs.is_empty(), "must return at least the recipe parent");
    assert_eq!(
        dirs[0],
        recipe.parent().unwrap().to_path_buf(),
        "recipe parent must be FIRST entry; got dirs={dirs:?}"
    );
}

#[test]
fn test_build_search_dirs_dedups_paths() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let recipes_dir = temp.path().join("amplifier-bundle").join("recipes");
    std::fs::create_dir_all(&recipes_dir).unwrap();
    let recipe = recipes_dir.join("dup.yaml");
    std::fs::write(&recipe, "name: dup\nsteps: []\n").unwrap();

    let dirs =
        super::build_search_dirs(&recipe, temp.path()).expect("build_search_dirs must not error");

    let occurrences = dirs.iter().filter(|p| *p == &recipes_dir).count();
    assert_eq!(
        occurrences, 1,
        "recipe-parent must appear EXACTLY once after dedup; got dirs={dirs:?}"
    );
}

#[test]
fn test_build_search_dirs_handles_no_parent_gracefully() {
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let recipe = PathBuf::from("/");

    let result = super::build_search_dirs(&recipe, temp.path());
    assert!(
        result.is_ok(),
        "build_search_dirs must handle root-as-recipe gracefully: {result:?}"
    );
}

#[test]
#[cfg(unix)]
fn test_run_recipe_forwards_recipe_parent_as_dash_r() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let arg_log = temp.path().join("args.log");
    std::fs::write(
        &runner,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\" >> {log}; done\n\
             echo '{{\"recipe_name\":\"probe\",\"success\":true,\"step_results\":[],\"context\":{{}}}}'\n\
             exit 0\n",
            log = arg_log.display()
        ),
    )
    .expect("failed to write runner stub");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o755))
        .expect("failed to chmod runner");

    let recipe_dir = temp.path().join("co-located");
    std::fs::create_dir_all(&recipe_dir).unwrap();
    let recipe = recipe_dir.join("probe.yaml");
    std::fs::write(
        &recipe,
        "name: probe\nsteps:\n  - id: noop\n    name: noop\n    type: bash\n    command: \"true\"\n",
    )
    .expect("failed to write recipe");

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner) };

    let result = super::run_recipe(
        recipe.to_str().unwrap(),
        &[],
        true,
        false,
        "json",
        Some(temp.path().to_str().unwrap()),
        None,
    );

    match prev_runner {
        Some(v) => unsafe { std::env::set_var("RECIPE_RUNNER_RS_PATH", v) },
        None => unsafe { std::env::remove_var("RECIPE_RUNNER_RS_PATH") },
    }

    result.expect("run_recipe must succeed");

    let logged = std::fs::read_to_string(&arg_log).expect("argv log must exist");
    let argv: Vec<&str> = logged.lines().collect();
    let r_values: Vec<&str> = argv
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            if *a == "-R" {
                argv.get(i + 1).copied()
            } else {
                None
            }
        })
        .collect();

    assert!(
        r_values.iter().any(|v| *v == recipe_dir.to_string_lossy()),
        "run_recipe must forward recipe-parent dir as -R; got r_values={r_values:?}\nargv:\n{logged}"
    );
}

// =========================================================================
// Issue #784 / #4583 — recipe context variables must be exported as
// environment variables for bash steps (TASK_DESCRIPTION, REPO_PATH, ...).
//
// TDD (RED first): these tests specify the contract for the not-yet-existing
// `execute::context_env_pairs` pure transform and the env-export wired into
// `execute_recipe_via_rust`. They fail until the fix is implemented:
//   * the unit/security tests reference `execute::context_env_pairs`, which
//     does not exist yet (compile-time RED);
//   * the integration tests reproduce the runtime symptom — a bash step under
//     `set -u` that reads $TASK_DESCRIPTION / $REPO_PATH aborts with
//     "unbound variable" because the context is never exported.
// =========================================================================

// -------------------------------------------------------------------------
// Pure transform: context_env_pairs — uppercasing, validation, denylist
// -------------------------------------------------------------------------

/// Collect `context_env_pairs` output into a map for order-independent
/// assertions. Last-writer-wins on collision (deterministic BTreeMap order).
fn context_env_map(context: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    execute::context_env_pairs(context).into_iter().collect()
}

fn ctx(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

#[test]
fn test_context_env_pairs_uppercases_known_keys() {
    let env = context_env_map(&ctx(&[
        ("task_description", "Fix the bug"),
        ("repo_path", "/work/repo"),
    ]));
    assert_eq!(
        env.get("TASK_DESCRIPTION"),
        Some(&"Fix the bug".to_string()),
        "context key `task_description` must be exported as TASK_DESCRIPTION"
    );
    assert_eq!(
        env.get("REPO_PATH"),
        Some(&"/work/repo".to_string()),
        "context key `repo_path` must be exported as REPO_PATH"
    );
    // Lowercased originals must NOT appear — only the uppercased name is exported.
    assert!(env.get("task_description").is_none());
    assert!(env.get("repo_path").is_none());
}

#[test]
fn test_context_env_pairs_preserves_values_verbatim() {
    // Keys are transformed; values must pass through unchanged (spaces,
    // punctuation, unicode, leading/trailing whitespace are all preserved).
    let value = "  Multi word: résumé, 50% done (a=b)  ";
    let env = context_env_map(&ctx(&[("task_description", value)]));
    assert_eq!(env.get("TASK_DESCRIPTION"), Some(&value.to_string()));
}

#[test]
fn test_context_env_pairs_accepts_mixed_case_and_leading_underscore() {
    let env = context_env_map(&ctx(&[
        ("MyVar", "1"),
        ("ALREADY_UPPER", "2"),
        ("_private", "3"),
    ]));
    assert_eq!(env.get("MYVAR"), Some(&"1".to_string()));
    assert_eq!(env.get("ALREADY_UPPER"), Some(&"2".to_string()));
    assert_eq!(
        env.get("_PRIVATE"),
        Some(&"3".to_string()),
        "a leading underscore is a valid env identifier start"
    );
}

#[test]
fn test_context_env_pairs_drops_invalid_identifier_keys() {
    // Keys that do not match ^[A-Z_][A-Z0-9_]*$ after uppercasing are dropped,
    // not sanitized — exporting a bogus name is worse than skipping it.
    let env = context_env_map(&ctx(&[
        ("my-var", "hyphen"),      // hyphen is illegal -> dropped
        ("my.var", "dot"),         // dot is illegal -> dropped
        ("1bad", "leading digit"), // leading digit -> dropped
        ("with space", "space"),   // space is illegal -> dropped
        ("", "empty key"),         // empty -> dropped
        ("ok_key", "kept"),        // control: valid -> kept
    ]));
    assert_eq!(env.get("OK_KEY"), Some(&"kept".to_string()));
    assert!(env.get("MY-VAR").is_none());
    assert!(env.get("MY.VAR").is_none());
    assert!(env.get("1BAD").is_none());
    assert!(env.get("WITH SPACE").is_none());
    assert!(env.get("").is_none());
    assert_eq!(
        env.len(),
        1,
        "only the single valid key should survive; got {env:?}"
    );
}

#[test]
fn test_context_env_pairs_drops_values_containing_nul() {
    // A NUL byte in a value is rejected by the OS for env vars; drop the pair
    // rather than letting Command::env panic at spawn time.
    let env = context_env_map(&ctx(&[
        ("task_description", "bad\0value"),
        ("repo_path", "/clean"),
    ]));
    assert!(
        env.get("TASK_DESCRIPTION").is_none(),
        "values containing NUL must be dropped"
    );
    assert_eq!(env.get("REPO_PATH"), Some(&"/clean".to_string()));
}

#[test]
fn test_context_env_pairs_drops_oversized_values() {
    // A single env string longer than the kernel's MAX_ARG_STRLEN causes
    // execve to fail with E2BIG. Oversized values must be skipped from the
    // env mirror (they remain available via the context file), while
    // normal-sized values in the same context are still exported. This guards
    // the regression on `test_large_context_does_not_hit_e2big`.
    let huge = "x".repeat(256 * 1024);
    let env = context_env_map(&ctx(&[
        ("task_description", huge.as_str()),
        ("repo_path", "/work/repo"),
    ]));
    assert!(
        env.get("TASK_DESCRIPTION").is_none(),
        "an oversized value must not be exported as an environment variable"
    );
    assert_eq!(
        env.get("REPO_PATH"),
        Some(&"/work/repo".to_string()),
        "normal-sized values are still exported alongside a skipped oversized one"
    );
}

// -------------------------------------------------------------------------
// Security: reserved / dangerous env names must never be settable from
// untrusted recipe context (issue bodies, task descriptions, 3rd-party
// recipes flow into context). The denylist is the PRIMARY control.
// -------------------------------------------------------------------------

#[test]
fn test_context_env_pairs_drops_reserved_and_dangerous_names() {
    // Each of these context keys uppercases to a name that must be dropped.
    // Coverage spans: path/identity, dynamic-linker, shell-startup RCE
    // vectors, word-splitting, and interpreter option injection.
    let dangerous = [
        // path / identity
        "path",
        "home",
        "shell",
        "pwd",
        "user",
        "logname",
        // dynamic linker
        "ld_preload",
        "ld_library_path",
        "dyld_insert_libraries",
        "dyld_library_path",
        "glibc_tunables",
        // shell-startup remote-code-execution vectors
        "bash_env",
        "env",
        "ps4",
        "prompt_command",
        "shellopts",
        "bashopts",
        // word splitting
        "ifs",
        // interpreter option injection
        "pythonpath",
        "node_options",
        "perl5opt",
        "rubyopt",
    ];
    for key in dangerous {
        let env = context_env_map(&ctx(&[(key, "attacker-controlled")]));
        assert!(
            env.is_empty(),
            "reserved/dangerous context key `{key}` must be dropped, got {env:?}"
        );
    }
}

#[test]
fn test_context_env_pairs_drops_amplihack_prefixed_keys() {
    // The AMPLIHACK_ namespace is owned by EnvBuilder; context must never be
    // able to collide with or override builder-managed correlation/config vars.
    let env = context_env_map(&ctx(&[
        ("amplihack_home", "/evil/home"),
        ("amplihack_recipe_run_id", "spoofed"),
        ("AMPLIHACK_ASSET_RESOLVER", "/evil/resolver"),
        ("task_description", "kept"),
    ]));
    assert_eq!(
        env.get("TASK_DESCRIPTION"),
        Some(&"kept".to_string()),
        "ordinary keys still pass through"
    );
    assert!(env.get("AMPLIHACK_HOME").is_none());
    assert!(env.get("AMPLIHACK_RECIPE_RUN_ID").is_none());
    assert!(env.get("AMPLIHACK_ASSET_RESOLVER").is_none());
}

// -------------------------------------------------------------------------
// Integration (T1): top-level recipe — a bash step under `set -u` reads
// $TASK_DESCRIPTION and $REPO_PATH from the environment.
// -------------------------------------------------------------------------

/// RED reproduction of the reported bug: the stub runner runs `set -u` and
/// dereferences $TASK_DESCRIPTION / $REPO_PATH (exactly what a real bash step
/// does). Without the env-export fix, `set -u` aborts with
/// "TASK_DESCRIPTION: unbound variable", the stub prints nothing, and
/// `execute_recipe_via_rust` returns Err. With the fix, the stub echoes the
/// values back through `context` and the run succeeds.
#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_exports_context_as_env_under_set_u() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\nset -u\nprintf '{\"recipe_name\":\"ctx-env-probe\",\"success\":true,\"step_results\":[],\"context\":{\"task\":\"%s\",\"repo\":\"%s\"}}' \"$TASK_DESCRIPTION\" \"$REPO_PATH\"\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: ctx-env-probe\nsteps: []\n").expect("failed to write recipe");

    let context = ctx(&[
        ("task_description", "Fix the bug"),
        ("repo_path", "/work/repo"),
    ]);

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_task = std::env::var_os("TASK_DESCRIPTION");
    let prev_repo = std::env::var_os("REPO_PATH");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        // Ensure no inherited values mask the bug.
        std::env::remove_var("TASK_DESCRIPTION");
        std::env::remove_var("REPO_PATH");
    }

    let result =
        execute::execute_recipe_via_rust(&recipe, &context, true, false, temp.path(), &[], None);

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);
    restore_env_var("TASK_DESCRIPTION", prev_task);
    restore_env_var("REPO_PATH", prev_repo);

    let result = result.expect(
        "recipe run must succeed: recipe context must be exported as env so a \
         bash step under `set -u` can read $TASK_DESCRIPTION / $REPO_PATH \
         (issue #784 / #4583)",
    );
    assert_eq!(
        result.context.get("task"),
        Some(&JsonValue::String("Fix the bug".to_string())),
        "TASK_DESCRIPTION must be exported to the child env from context key `task_description`"
    );
    assert_eq!(
        result.context.get("repo"),
        Some(&JsonValue::String("/work/repo".to_string())),
        "REPO_PATH must be exported to the child env from context key `repo_path`"
    );
}

// -------------------------------------------------------------------------
// Integration (T2): nested / sub-recipe — the env must propagate to
// grandchild processes (a sub-recipe's bash step) via OS inheritance.
// This is the canary for "parent context propagated to child sub-recipes".
// -------------------------------------------------------------------------

/// The stub runner spawns a GRANDCHILD `sh -c 'set -u; ...'`, mirroring a
/// sub-recipe whose bash step reads $TASK_DESCRIPTION / $REPO_PATH. The
/// grandchild inherits env from the runner, which inherits from amplihack-cli.
/// Before the fix the grandchild's `set -u` aborts and the captured values are
/// empty (assertion RED); after the fix the values propagate through both hops.
#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_exports_context_to_nested_subprocess() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    std::fs::write(
        &runner,
        "#!/bin/sh\nset -u\nNESTED_TASK=$(sh -c 'set -u; printf %s \"$TASK_DESCRIPTION\"')\nNESTED_REPO=$(sh -c 'set -u; printf %s \"$REPO_PATH\"')\nprintf '{\"recipe_name\":\"nested-ctx-probe\",\"success\":true,\"step_results\":[],\"context\":{\"nested_task\":\"%s\",\"nested_repo\":\"%s\"}}' \"$NESTED_TASK\" \"$NESTED_REPO\"\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: nested-ctx-probe\nsteps: []\n").expect("failed to write recipe");

    let context = ctx(&[
        ("task_description", "Nested task value"),
        ("repo_path", "/work/nested/repo"),
    ]);

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_task = std::env::var_os("TASK_DESCRIPTION");
    let prev_repo = std::env::var_os("REPO_PATH");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::remove_var("TASK_DESCRIPTION");
        std::env::remove_var("REPO_PATH");
    }

    let result =
        execute::execute_recipe_via_rust(&recipe, &context, true, false, temp.path(), &[], None);

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);
    restore_env_var("TASK_DESCRIPTION", prev_task);
    restore_env_var("REPO_PATH", prev_repo);

    let result = result.expect(
        "nested recipe run must succeed: context env must propagate to a \
         grandchild sub-recipe bash step under `set -u`",
    );
    assert_eq!(
        result.context.get("nested_task"),
        Some(&JsonValue::String("Nested task value".to_string())),
        "TASK_DESCRIPTION must propagate to a grandchild (sub-recipe) process"
    );
    assert_eq!(
        result.context.get("nested_repo"),
        Some(&JsonValue::String("/work/nested/repo".to_string())),
        "REPO_PATH must propagate to a grandchild (sub-recipe) process"
    );
}

// -------------------------------------------------------------------------
// Integration (T4): no-regression + security/precedence at the spawn seam.
// Context is applied at LOWEST precedence: EnvBuilder-managed and reserved
// names must win, and untrusted context must never clobber PATH/AMPLIHACK_*.
// -------------------------------------------------------------------------

/// Even when an attacker-controlled recipe supplies `path`, `ld_preload`, and
/// `amplihack_home` keys, the child must NOT see those values: PATH is left as
/// inherited, LD_PRELOAD stays empty, and AMPLIHACK_HOME keeps the
/// builder-managed value. Meanwhile an ordinary key (`task_description`) is
/// still exported — proving the export works without opening a clobber hole.
#[test]
#[cfg(unix)]
fn test_execute_recipe_via_rust_context_cannot_clobber_reserved_or_builder_env() {
    let _guard = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("failed to create temp dir");
    let runner = temp.path().join("recipe-runner-rs");
    let amplihack_home = temp.path().join("amplihack-home");
    std::fs::create_dir_all(&amplihack_home).expect("failed to create amplihack home");

    // No `set -u` here: LD_PRELOAD is legitimately unset, so we read it with a
    // `:-` default to distinguish "unset" (safe) from "attacker value" (bug).
    std::fs::write(
        &runner,
        "#!/bin/sh\nprintf '{\"recipe_name\":\"sec-probe\",\"success\":true,\"step_results\":[],\"context\":{\"task\":\"%s\",\"path\":\"%s\",\"preload\":\"%s\",\"ahome\":\"%s\"}}' \"${TASK_DESCRIPTION:-MISSING}\" \"$PATH\" \"${LD_PRELOAD:-}\" \"${AMPLIHACK_HOME:-}\"\n",
    )
    .expect("failed to write runner stub");
    make_executable(&runner);

    let recipe = temp.path().join("recipe.yaml");
    std::fs::write(&recipe, "name: sec-probe\nsteps: []\n").expect("failed to write recipe");

    let context = ctx(&[
        ("task_description", "ok"),
        ("path", "/nonexistent/evil"),
        ("ld_preload", "/evil.so"),
        ("amplihack_home", "/evil/home"),
    ]);

    let prev_runner = std::env::var_os("RECIPE_RUNNER_RS_PATH");
    let prev_home = std::env::var_os("AMPLIHACK_HOME");
    let prev_task = std::env::var_os("TASK_DESCRIPTION");
    let prev_preload = std::env::var_os("LD_PRELOAD");
    unsafe {
        std::env::set_var("RECIPE_RUNNER_RS_PATH", &runner);
        std::env::set_var("AMPLIHACK_HOME", &amplihack_home);
        std::env::remove_var("TASK_DESCRIPTION");
        std::env::remove_var("LD_PRELOAD");
    }

    let result =
        execute::execute_recipe_via_rust(&recipe, &context, true, false, temp.path(), &[], None);

    restore_env_var("RECIPE_RUNNER_RS_PATH", prev_runner);
    restore_env_var("AMPLIHACK_HOME", prev_home);
    restore_env_var("TASK_DESCRIPTION", prev_task);
    restore_env_var("LD_PRELOAD", prev_preload);

    let result = result.expect("recipe run must succeed");

    assert_eq!(
        result.context.get("task"),
        Some(&JsonValue::String("ok".to_string())),
        "ordinary context keys must still be exported"
    );
    let child_path = result
        .context
        .get("path")
        .and_then(JsonValue::as_str)
        .expect("stub must report PATH");
    assert_ne!(
        child_path, "/nonexistent/evil",
        "context key `path` must NOT clobber the child's PATH"
    );
    assert_eq!(
        result.context.get("preload"),
        Some(&JsonValue::String(String::new())),
        "context key `ld_preload` must be dropped (LD_PRELOAD stays unset)"
    );
    let child_ahome = result
        .context
        .get("ahome")
        .and_then(JsonValue::as_str)
        .expect("stub must report AMPLIHACK_HOME");
    assert_ne!(
        child_ahome, "/evil/home",
        "context key `amplihack_home` must NOT override builder-managed AMPLIHACK_HOME"
    );
}
