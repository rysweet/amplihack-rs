/// Integration tests: Recipe subcommand E2E smoke tests.
///
/// These tests verify the full recipe delegation chain through the compiled
/// `amplihack` binary:
///   amplihack recipe list    → discover recipes, exit 0
///   amplihack recipe validate → parse + validate YAML, exit 0
///   amplihack recipe run     → invoke recipe-runner-rs, exit 0
///
/// # Test Status Summary
///
/// | Test                                        | Expected status     |
/// |---------------------------------------------|---------------------|
/// | recipe_list_exits_zero                      | PASSES (no runner)  |
/// | recipe_list_output_contains_at_least_one_recipe | PASSES if ~/.amplihack dir exists |
/// | recipe_validate_exits_zero_for_temp_recipe  | PASSES (no runner)  |
/// | recipe_validate_exits_nonzero_for_invalid   | PASSES (no runner)  |
/// | recipe_run_dry_run_exits_zero               | **FAILS** until recipe-runner-rs installed |
/// | recipe_run_dry_run_output_contains_recipe_name | **FAILS** until recipe-runner-rs installed |
/// | recipe_run_without_dry_run_invokes_runner   | **FAILS** until recipe-runner-rs installed |
///
/// # How to make the failing tests pass (WS3)
///
/// Install recipe-runner-rs:
///   cargo install --git https://github.com/rysweet/amplihack-recipe-runner
/// OR set the env override:
///   RECIPE_RUNNER_RS_PATH=/path/to/recipe-runner-rs cargo test
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

/// Path to the compiled amplihack binary.
///
/// Uses `CARGO_MANIFEST_DIR` which for tests registered in
/// `bins/amplihack/Cargo.toml` resolves to `bins/amplihack/`.
/// Two `pop()` calls reach the workspace root, then `target/debug/amplihack`.
fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins/
    path.pop(); // bins/ → workspace root (amplihack-rs/)
    path.push("target/debug/amplihack");
    path
}

/// Assert that a Command exits with the expected success/failure status.
fn assert_exit(cmd: &mut Command, expect_success: bool, context: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("Failed to run command ({context}): {e}"));
    if expect_success {
        assert!(
            status.success(),
            "[{context}] Expected exit 0, got: {status}"
        );
    } else {
        assert!(
            !status.success(),
            "[{context}] Expected non-zero exit, got: {status}"
        );
    }
}

/// Run a command and return (stdout, stderr, exit_code).
fn run_output(cmd: &mut Command, context: &str) -> (String, String, bool) {
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("Failed to run command ({context}): {e}"));
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

/// Write a minimal valid recipe YAML to a temp file, return the path.
fn write_temp_recipe(name: &str, include_steps: bool) -> tempfile::NamedTempFile {
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp recipe file");

    if include_steps {
        // NOTE: recipe-runner-rs supports step types: bash, agent, recipe.
        // "shell" is NOT a valid type — always use "bash" for shell commands.
        writeln!(
            tmp,
            r#"name: {name}
description: Temporary recipe for E2E testing
version: "1.0"
author: test
tags:
  - test
steps:
  - id: step-one
    type: bash
    command: echo "step one output"
"#
        )
        .expect("failed to write recipe YAML");
    } else {
        writeln!(
            tmp,
            r#"name: {name}
description: Minimal recipe without steps
version: "1.0"
steps: []
"#
        )
        .expect("failed to write recipe YAML");
    }

    tmp
}

// ---------------------------------------------------------------------------
// recipe list
// ---------------------------------------------------------------------------

/// `amplihack recipe list` must exit 0 (even if no recipes are found).
///
/// Expected: PASSES (no recipe-runner-rs dependency for list).
#[test]
fn recipe_list_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    assert_exit(
        Command::new(&bin).args(["recipe", "list"]),
        true,
        "recipe list",
    );
}

/// `amplihack recipe list` output must contain at least one recipe when
/// ~/.amplihack/.claude/recipes/ exists and is populated.
///
/// Expected: PASSES if the amplihack recipes directory is set up; skips
/// gracefully if not.  This is a smoke test for the discovery logic.
#[test]
fn recipe_list_output_contains_at_least_one_recipe() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    // Determine the expected recipes directory
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let recipes_dir = std::path::Path::new(&home).join(".amplihack/.claude/recipes");

    if !recipes_dir.is_dir() {
        // Directory not set up — this is a soft failure (environment not ready)
        // We still run the command to confirm exit 0 at minimum.
        assert_exit(
            Command::new(&bin).args(["recipe", "list"]),
            true,
            "recipe list (no recipes dir)",
        );
        return;
    }

    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args(["recipe", "list"]),
        "recipe list with recipes dir",
    );
    assert!(success, "recipe list must exit 0. stderr: {stderr}");
    assert!(
        !stdout.trim().is_empty() || stdout.contains("No recipes"),
        "recipe list must produce output. stdout: {stdout} stderr: {stderr}"
    );
}

/// `amplihack recipe list --format json` must produce valid JSON.
///
/// Expected: PASSES (no recipe-runner-rs dependency).
#[test]
fn recipe_list_json_format_is_valid_json() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args(["recipe", "list", "--format", "json"]),
        "recipe list --format json",
    );

    assert!(
        success,
        "recipe list --format json must exit 0. stderr: {stderr}"
    );

    // Output must be parseable as JSON (array or object)
    let trimmed = stdout.trim();
    if !trimmed.is_empty() {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
        assert!(
            parsed.is_ok(),
            "recipe list --format json must produce valid JSON. \
             stdout: {trimmed}\nstderr: {stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// recipe validate
// ---------------------------------------------------------------------------

/// `amplihack recipe validate <path>` must exit 0 for a well-formed recipe.
///
/// Expected: PASSES (no recipe-runner-rs dependency).
#[test]
fn recipe_validate_exits_zero_for_valid_recipe() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let tmp = write_temp_recipe("validate-test", true);
    assert_exit(
        Command::new(&bin).args(["recipe", "validate", tmp.path().to_str().unwrap()]),
        true,
        "recipe validate (valid YAML)",
    );
}

/// `amplihack recipe validate` must exit non-zero for an invalid (empty) YAML file.
///
/// Expected: PASSES (no recipe-runner-rs dependency).
#[test]
fn recipe_validate_exits_nonzero_for_invalid_recipe() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    // A recipe without a 'name' field is invalid
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    writeln!(tmp, "description: missing name field\nsteps: []").expect("failed to write bad YAML");

    assert_exit(
        Command::new(&bin).args(["recipe", "validate", tmp.path().to_str().unwrap()]),
        false,
        "recipe validate (missing required 'name' field)",
    );
}

/// `amplihack recipe validate` on the default-workflow recipe from the
/// amplihack directory must succeed.
///
/// Expected: PASSES if ~/.amplihack/.claude/recipes/default-workflow.yaml exists.
#[test]
fn recipe_validate_exits_zero_for_default_workflow() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let recipe_path =
        std::path::Path::new(&home).join(".amplihack/.claude/recipes/default-workflow.yaml");

    if !recipe_path.exists() {
        // Soft skip: recipe not installed in this environment
        println!(
            "SKIP: default-workflow.yaml not found at {:?} — skipping validate test",
            recipe_path
        );
        return;
    }

    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args(["recipe", "validate", recipe_path.to_str().unwrap()]),
        "recipe validate default-workflow",
    );

    assert!(
        success,
        "recipe validate must exit 0 for default-workflow.yaml. \
         stdout: {stdout}\nstderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// recipe run --dry-run
//
// *** THESE TESTS FAIL until recipe-runner-rs is installed. ***
// ---------------------------------------------------------------------------

/// `amplihack recipe run <path> --dry-run` must exit 0 and produce output
/// containing the recipe name.
///
/// PRECONDITIONS (for this test to PASS):
///   - recipe-runner-rs binary must be installed in PATH or ~/.cargo/bin/
///     OR RECIPE_RUNNER_RS_PATH must point to a valid binary
///
/// *** FAILS currently because recipe-runner-rs binary is not installed. ***
/// *** Target: passes after WS3 fix installs the binary correctly.       ***
#[test]
fn recipe_run_dry_run_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let tmp = write_temp_recipe("e2e-dry-run", true);
    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args([
            "recipe",
            "run",
            tmp.path().to_str().unwrap(),
            "-c",
            "task_description=echo hello",
            "--dry-run",
        ]),
        "recipe run --dry-run",
    );

    assert!(
        success,
        "recipe run --dry-run must exit 0. \
         FIX: install recipe-runner-rs binary (see find_recipe_runner_binary \
         in crates/amplihack-cli/src/commands/recipe/run.rs). \
         stdout: {stdout}\nstderr: {stderr}"
    );
}

/// `amplihack recipe run <path> --dry-run` output must contain the recipe name.
///
/// *** FAILS currently because recipe-runner-rs binary is not installed. ***
#[test]
fn recipe_run_dry_run_output_contains_recipe_name() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let tmp = write_temp_recipe("name-check-recipe", true);
    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args(["recipe", "run", tmp.path().to_str().unwrap(), "--dry-run"]),
        "recipe run --dry-run (name check)",
    );

    assert!(
        success,
        "recipe run --dry-run must exit 0. stderr: {stderr}"
    );

    assert!(
        stdout.contains("name-check-recipe"),
        "recipe run output must contain the recipe name 'name-check-recipe'. \
         stdout: {stdout}\nstderr: {stderr}"
    );
}

/// `amplihack recipe run` with default-workflow.yaml and --dry-run must exit 0.
///
/// This tests the full critical path: Rust CLI → recipe discovery → runner invocation.
///
/// *** FAILS currently because recipe-runner-rs binary is not installed. ***
#[test]
fn recipe_run_dry_run_default_workflow_exits_zero() {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let recipe_path =
        std::path::Path::new(&home).join(".amplihack/.claude/recipes/default-workflow.yaml");

    if !recipe_path.exists() {
        println!("SKIP: default-workflow.yaml not found at {:?}", recipe_path);
        // Still run a temp recipe to gate the recipe-runner-rs binary
    }

    // Use a temp recipe as fallback if default-workflow.yaml doesn't exist
    let _tmp;
    let path_to_use = if recipe_path.exists() {
        recipe_path.clone()
    } else {
        _tmp = write_temp_recipe("fallback-e2e", true);
        _tmp.path().to_path_buf()
    };

    let (stdout, stderr, success) = run_output(
        Command::new(&bin).args([
            "recipe",
            "run",
            path_to_use.to_str().unwrap(),
            "-c",
            "task_description=echo hello",
            "--dry-run",
        ]),
        "recipe run --dry-run (default-workflow or fallback)",
    );

    assert!(
        success,
        "recipe run --dry-run must exit 0. This is the WS3 critical-path gate. \
         Ensure recipe-runner-rs is installed or RECIPE_RUNNER_RS_PATH is set. \
         stdout: {stdout}\nstderr: {stderr}"
    );
}
