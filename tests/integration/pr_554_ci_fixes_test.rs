//! Regression tests for PR #554 CI fixes.
//!
//! Two structural failures blocked PR #554 CI:
//!
//!   1. `cargo run --package amplihack` was ambiguous because `bins/amplihack`
//!      declared two `[[bin]]` targets (`amplihack` and `scan-invisible-chars`)
//!      without specifying `default-run`.  The `issue_538_install_completeness`
//!      integration test exercises this path.
//!
//!   2. `tests/outside-in/scenario2-code-graph-no-python.yaml` lacked a
//!      top-level `agents:` section, causing `gadugi-test validate` to fail
//!      with a missing-agents error.
//!
//! These tests lock both contracts so neither regression can be reintroduced
//! silently.  They are pure file-content assertions — no binary execution
//! required.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = bins/amplihack  →  pop twice → workspace root
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins
    path.pop(); // workspace root
    path
}

fn read_file(relative: &str) -> String {
    let path = workspace_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

// ═══════════════════════════════════════════════════════════════════════════
// Fix 1: bins/amplihack/Cargo.toml must declare default-run = "amplihack"
// ═══════════════════════════════════════════════════════════════════════════

/// `cargo run --package amplihack` must resolve to the `amplihack` binary, not
/// fail with "multiple bins found".  The `default-run` directive is the correct
/// fix; it must be present and must name the right target.
#[test]
fn amplihack_cargo_toml_has_default_run_amplihack() {
    let content = read_file("bins/amplihack/Cargo.toml");
    assert!(
        content.contains("default-run = \"amplihack\""),
        "bins/amplihack/Cargo.toml must contain `default-run = \"amplihack\"` so that \
         `cargo run --package amplihack` resolves unambiguously when multiple [[bin]] \
         targets are present (PR #554 CI fix 1)"
    );
}

/// The manifest must still declare two distinct `[[bin]]` sections (the second
/// binary is `scan-invisible-chars`).  This ensures the test above isn't
/// satisfied by removing the second binary rather than adding `default-run`.
#[test]
fn amplihack_cargo_toml_still_has_two_bin_targets() {
    let content = read_file("bins/amplihack/Cargo.toml");
    let bin_count = content.matches("[[bin]]").count();
    assert_eq!(
        bin_count, 2,
        "bins/amplihack/Cargo.toml must declare exactly two [[bin]] targets \
         (amplihack and scan-invisible-chars); found {bin_count} (PR #554 CI fix 1)"
    );
}

/// Confirm the `amplihack` binary target is still present by name.
#[test]
fn amplihack_bin_target_is_named_amplihack() {
    let content = read_file("bins/amplihack/Cargo.toml");
    assert!(
        content.contains("name = \"amplihack\""),
        "bins/amplihack/Cargo.toml must contain a [[bin]] named \"amplihack\" \
         (PR #554 CI fix 1)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Fix 2: scenario2 outside-in fixture must have a top-level agents: section
// ═══════════════════════════════════════════════════════════════════════════

/// `gadugi-test validate` rejects scenario YAML files that lack a top-level
/// `agents:` key.  scenario2 must declare at least one agent entry.
#[test]
fn scenario2_yaml_has_top_level_agents_key() {
    let content = read_file("tests/outside-in/scenario2-code-graph-no-python.yaml");
    assert!(
        content.starts_with("agents:"),
        "tests/outside-in/scenario2-code-graph-no-python.yaml must start with a top-level \
         `agents:` key so that `gadugi-test validate` passes (PR #554 CI fix 2); \
         actual start: {:?}",
        &content[..content.len().min(40)]
    );
}

/// The agents section must declare an agent of type `cli` to match the other
/// scenarios in the corpus (scenario3, scenario4) that gadugi-test validates.
#[test]
fn scenario2_yaml_agents_section_contains_cli_type() {
    let content = read_file("tests/outside-in/scenario2-code-graph-no-python.yaml");
    assert!(
        content.contains("type: cli"),
        "tests/outside-in/scenario2-code-graph-no-python.yaml agents: section must \
         declare `type: cli` to satisfy the gadugi-test validate schema \
         (PR #554 CI fix 2)"
    );
}

/// The cli agent must be named `amplihack-cli` and specify `command: amplihack`,
/// matching the pattern established in scenario3 and scenario4.
#[test]
fn scenario2_yaml_agents_section_has_amplihack_cli_agent() {
    let content = read_file("tests/outside-in/scenario2-code-graph-no-python.yaml");
    assert!(
        content.contains("name: amplihack-cli"),
        "agents: section must declare an agent named `amplihack-cli` \
         (PR #554 CI fix 2)"
    );
    assert!(
        content.contains("command: amplihack"),
        "agents: section must declare `command: amplihack` \
         (PR #554 CI fix 2)"
    );
}

/// The `scenario:` wrapper block (Format A) must still be present after adding
/// the `agents:` section — we must not have broken the existing scenario
/// content while inserting the agents header.
#[test]
fn scenario2_yaml_still_has_scenario_wrapper() {
    let content = read_file("tests/outside-in/scenario2-code-graph-no-python.yaml");
    assert!(
        content.contains("\nscenario:"),
        "tests/outside-in/scenario2-code-graph-no-python.yaml must still contain the \
         `scenario:` wrapper block after inserting the agents: section \
         (PR #554 CI fix 2 must not remove existing scenario content)"
    );
}
