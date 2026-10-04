//! CLI golden output tests.
//!
//! Verify that CLI commands produce expected output patterns.
//! These catch regressions in help text, error messages, and formatting.

use std::path::PathBuf;
use std::process::Command;

fn amplihack_bin() -> PathBuf {
    // Cargo sets CARGO_BIN_EXE_amplihack for this [[test]] target and builds the
    // binary as a prerequisite, so the path is always correct (honouring the
    // active profile / CARGO_TARGET_DIR) and present — no build race.
    PathBuf::from(env!("CARGO_BIN_EXE_amplihack"))
}

fn run_cmd(args: &[&str]) -> (String, String, bool) {
    let bin = amplihack_bin();
    if !bin.exists() {
        panic!("amplihack binary not found at {bin:?}. Run `cargo build` first.");
    }
    let output = Command::new(&bin)
        .args(args)
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

// ── Version output ──

/// Issue #1526: a release build reports exactly its tag; an unstamped source
/// build reports `<CARGO_PKG_VERSION>-dev`, which semver orders below the
/// release it came from, so it can never look newer than an installed release.
/// `hook_dispatch::hooks_version_matches_release_or_dev_formula` uses the same
/// formula, which pins the two binaries to the same string.
#[test]
fn version_format_is_semver() {
    let (stdout, _, ok) = run_cmd(&["--version"]);
    assert!(ok);
    let expected = match option_env!("AMPLIHACK_RELEASE_VERSION") {
        Some(v) => v.to_string(),
        None => format!("{}-dev", env!("CARGO_PKG_VERSION")),
    };
    let version_line = stdout.trim();
    assert_eq!(
        version_line,
        format!("amplihack {expected}"),
        "--version must report the release tag, or <CARGO_PKG_VERSION>-dev when unstamped"
    );
    let version = version_line.strip_prefix("amplihack ").unwrap();
    let semver =
        regex::Regex::new(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$").unwrap();
    assert!(
        semver.is_match(version),
        "Version should be semver, got: {version}"
    );
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
