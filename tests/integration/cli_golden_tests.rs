//! CLI golden output tests.
//!
//! Verify that CLI commands produce expected output patterns.
//! These catch regressions in help text, error messages, and formatting.

use std::path::PathBuf;
use std::process::Command;

fn amplihack_bin() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("target/debug/amplihack");
    path
}

fn run_cmd(args: &[&str]) -> (String, String, bool) {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    let output = Command::new(&bin)
        .args(args)
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

// ── Version output ──

#[test]
fn version_format_is_semver() {
    let (stdout, _, ok) = run_cmd(&["--version"]);
    assert!(ok);
    // Should contain "amplihack X.Y.Z"
    let version_line = stdout.trim();
    assert!(
        version_line.starts_with("amplihack "),
        "Version should start with 'amplihack ', got: {version_line}"
    );
    let version = version_line.strip_prefix("amplihack ").unwrap();
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "Version should be semver (X.Y.Z), got: {version}"
    );
    for part in &parts {
        assert!(
            part.parse::<u32>().is_ok(),
            "Version component should be numeric: {part}"
        );
    }
}

// ── Help text structure ──

#[test]
fn help_contains_all_subcommands() {
    let (stdout, _, ok) = run_cmd(&["--help"]);
    assert!(ok);

    let expected_commands = ["install", "launch", "recipe", "memory", "plugin", "version"];

    for cmd in &expected_commands {
        assert!(
            stdout.contains(cmd),
            "Help output should mention '{cmd}' subcommand.\nGot:\n{stdout}"
        );
    }
}

#[test]
fn recipe_help_contains_subcommands() {
    let (stdout, _, ok) = run_cmd(&["recipe", "--help"]);
    assert!(ok);

    for sub in &["list", "validate"] {
        assert!(stdout.contains(sub), "recipe --help should mention '{sub}'");
    }
}

#[test]
fn memory_help_contains_subcommands() {
    let (stdout, _, ok) = run_cmd(&["memory", "--help"]);
    assert!(ok);

    for sub in &["tree", "export", "import", "clean"] {
        assert!(
            stdout.to_lowercase().contains(sub),
            "memory --help should mention '{sub}'"
        );
    }
}

// ── Error messages ──

#[test]
fn unknown_command_shows_suggestion() {
    let (_, stderr, ok) = run_cmd(&["recipee"]);
    assert!(!ok);
    // clap should suggest the correct command
    let combined = stderr.to_lowercase();
    assert!(
        combined.contains("recipe")
            || combined.contains("unrecognized")
            || combined.contains("not recognized"),
        "Unknown command error should hint at correct spelling.\nGot:\n{stderr}"
    );
}

// ── Recipe list output format ──

#[test]
fn recipe_list_outputs_count() {
    let (stdout, stderr, _) = run_cmd(&["recipe", "list"]);
    let combined = format!("{stdout}{stderr}");
    // Should mention recipe count or "No recipes found"
    assert!(
        combined.contains("recipe") || combined.contains("Recipe"),
        "recipe list should mention recipes.\nGot:\n{combined}"
    );
}

// ── Recipe validate ──

#[test]
fn recipe_validate_nonexistent_file_fails() {
    let (_, _, ok) = run_cmd(&["recipe", "validate", "/tmp/nonexistent-recipe-xyz.yaml"]);
    assert!(!ok, "validate should fail for a nonexistent file");
}

#[test]
fn recipe_validate_real_recipe_succeeds() {
    // Find the recipe relative to workspace root
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push("amplifier-bundle/recipes/default-workflow.yaml");

    if path.exists() {
        let (stdout, _, ok) = run_cmd(&["recipe", "validate", path.to_str().unwrap()]);
        assert!(ok, "validate should succeed for default-workflow.yaml");
        assert!(
            stdout.contains("valid") || stdout.contains("Valid") || stdout.contains("✓"),
            "validate output should indicate valid recipe.\nGot:\n{stdout}"
        );
    }
}
