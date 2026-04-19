//! TDD tests: Verify `.github/workflows/atlas.yml` for the code-atlas CI
//! workflow (Issue #258).
//!
//! These tests define the contract for the atlas CI workflow file.
//! They are written FIRST (TDD-style) and will FAIL until the workflow
//! file is created with the correct structure.
//!
//! ## What these tests verify
//!
//! 1. The workflow file exists at `.github/workflows/atlas.yml`
//! 2. Trigger: push to main only
//! 3. Security: explicit permissions block (least privilege)
//! 4. Security: job timeout to prevent runaway execution
//! 5. Toolchain: Rust setup matches ci.yml patterns
//! 6. Toolchain: diagram tools installed (graphviz, mermaid-cli)
//! 7. Recipe execution: `amplihack recipe run` with continue-on-error
//! 8. Artifact upload: `actions/upload-artifact@v4` with correct path
//! 9. Artifact upload runs even if recipe step fails (`if: always()`)
//! 10. amplihack installed via `cargo install --path bins/amplihack --locked`
//!
//! ## Failure modes
//!
//! These tests FAIL (red) if:
//! - `.github/workflows/atlas.yml` does not exist
//! - Any required workflow component is missing or misconfigured
//!
//! They PASS (green) once the workflow file is created per the spec.
//!
//! ## Related
//!
//! - `amplifier-bundle/recipes/code-atlas.yaml` — the recipe being executed
//! - `.github/workflows/ci.yml` — existing CI patterns to match
//! - Issue #258: Code Atlas Recipe + CI Workflow

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate `.github/workflows/atlas.yml` relative to this test's workspace.
fn atlas_yml_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins/
    path.pop(); // bins/ → workspace root
    path.push(".github");
    path.push("workflows");
    path.push("atlas.yml");
    path
}

/// Read atlas.yml content, panicking with a clear message if missing.
fn read_atlas_yml() -> String {
    let path = atlas_yml_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "atlas.yml not found at {path:?}\n\
             Create .github/workflows/atlas.yml for the code-atlas CI workflow.\n\
             See Issue #258 for requirements.\n\
             Error: {e}"
        )
    })
}

// ---------------------------------------------------------------------------
// TEST 1: The workflow file exists
// ---------------------------------------------------------------------------

/// The workflow file must be present. If this fails, all other tests
/// will fail too.
///
/// **FAILS** before implementation: file does not exist.
#[test]
fn atlas_yml_file_is_present() {
    let path = atlas_yml_path();
    assert!(
        path.exists(),
        "FAIL: .github/workflows/atlas.yml not found at {path:?}.\n\
         This file must exist for the code-atlas CI workflow.\n\
         See Issue #258."
    );
}

// ---------------------------------------------------------------------------
// TEST 2: Workflow name is descriptive
// ---------------------------------------------------------------------------

/// The workflow must have a human-readable name.
#[test]
fn atlas_yml_has_workflow_name() {
    let content = read_atlas_yml();
    // Match common name patterns: "Code Atlas", "Atlas", "code-atlas"
    let has_name = content.contains("name:");
    assert!(
        has_name,
        "FAIL: atlas.yml must have a top-level `name:` field.\n\
         Example: name: Code Atlas"
    );
}

// ---------------------------------------------------------------------------
// TEST 3: Triggers on push to main only
// ---------------------------------------------------------------------------

/// The workflow must trigger on push to main.
/// It should NOT trigger on pull_request (the recipe is expensive).
#[test]
fn atlas_yml_triggers_on_push_to_main() {
    let content = read_atlas_yml();

    assert!(
        content.contains("push:"),
        "FAIL: atlas.yml must trigger on `push:` events.\n\
         The code-atlas recipe should run on every push to main."
    );

    assert!(
        content.contains("main"),
        "FAIL: atlas.yml push trigger must target the `main` branch.\n\
         Expected: branches: [main]"
    );
}

/// The workflow should NOT trigger on pull_request to avoid expensive
/// recipe runs on every PR.
#[test]
fn atlas_yml_does_not_trigger_on_pull_request() {
    let content = read_atlas_yml();

    // pull_request trigger would waste CI minutes on every PR
    assert!(
        !content.contains("pull_request:"),
        "FAIL: atlas.yml must NOT trigger on `pull_request:`.\n\
         The code-atlas recipe is expensive and should only run on pushes to main.\n\
         Remove the pull_request trigger."
    );
}

// ---------------------------------------------------------------------------
// TEST 4: Security — explicit permissions block
// ---------------------------------------------------------------------------

/// The workflow must declare explicit permissions (least privilege).
/// Required: contents:read (checkout), actions:write (upload-artifact).
/// Must NOT have contents:write (no commit-back).
#[test]
fn atlas_yml_has_permissions_block() {
    let content = read_atlas_yml();

    assert!(
        content.contains("permissions:"),
        "FAIL: atlas.yml must have an explicit `permissions:` block.\n\
         Required permissions:\n\
           contents: read   # checkout\n\
           actions: write   # upload-artifact\n\
         \n\
         Omitting permissions defaults to overly broad access."
    );
}

/// The workflow must have contents:read permission for checkout.
#[test]
fn atlas_yml_has_contents_read_permission() {
    let content = read_atlas_yml();

    assert!(
        content.contains("contents: read") || content.contains("contents:read"),
        "FAIL: atlas.yml must declare `contents: read` permission.\n\
         This is required for actions/checkout@v4."
    );
}

/// The workflow must NOT have contents:write — no commit-back pattern.
#[test]
fn atlas_yml_does_not_have_contents_write() {
    let content = read_atlas_yml();

    assert!(
        !content.contains("contents: write") && !content.contains("contents:write"),
        "FAIL: atlas.yml must NOT have `contents: write` permission.\n\
         The atlas workflow uploads artifacts — it does NOT commit back to the repo.\n\
         Commit-back would create push loops on the main branch trigger."
    );
}

// ---------------------------------------------------------------------------
// TEST 5: Security — job timeout
// ---------------------------------------------------------------------------

/// The job must have a timeout to prevent runaway recipe execution.
/// The code-atlas recipe has 16 steps with agent calls that could hang.
#[test]
fn atlas_yml_has_job_timeout() {
    let content = read_atlas_yml();

    assert!(
        content.contains("timeout-minutes:"),
        "FAIL: atlas.yml must have `timeout-minutes:` on the atlas job.\n\
         The code-atlas recipe has 16 steps including agent calls that could hang.\n\
         Recommended: timeout-minutes: 30"
    );
}

// ---------------------------------------------------------------------------
// TEST 6: Uses ubuntu-latest runner (matches ci.yml)
// ---------------------------------------------------------------------------

/// The job must run on ubuntu-latest, matching existing CI patterns.
#[test]
fn atlas_yml_uses_ubuntu_latest() {
    let content = read_atlas_yml();

    assert!(
        content.contains("ubuntu-latest"),
        "FAIL: atlas.yml must use `runs-on: ubuntu-latest`.\n\
         This matches the runner used in ci.yml for consistency."
    );
}

// ---------------------------------------------------------------------------
// TEST 7: Rust toolchain setup matches ci.yml
// ---------------------------------------------------------------------------

/// The workflow must set up Rust using dtolnay/rust-toolchain@stable,
/// matching the pattern established in ci.yml.
#[test]
fn atlas_yml_has_rust_toolchain_setup() {
    let content = read_atlas_yml();

    assert!(
        content.contains("dtolnay/rust-toolchain@stable"),
        "FAIL: atlas.yml must use `dtolnay/rust-toolchain@stable` for Rust setup.\n\
         This matches the toolchain action used in ci.yml."
    );
}

/// The workflow must use Swatinem/rust-cache@v2 for build caching.
#[test]
fn atlas_yml_has_rust_cache() {
    let content = read_atlas_yml();

    assert!(
        content.contains("Swatinem/rust-cache"),
        "FAIL: atlas.yml must use `Swatinem/rust-cache@v2` for build caching.\n\
         Without caching, `cargo install` will rebuild from scratch every time."
    );
}

// ---------------------------------------------------------------------------
// TEST 8: amplihack installed from local checkout
// ---------------------------------------------------------------------------

/// amplihack must be installed via `cargo install --path bins/amplihack --locked`.
/// This is the proven pattern from the install-smoke job in ci.yml.
#[test]
fn atlas_yml_installs_amplihack_from_source() {
    let content = read_atlas_yml();

    assert!(
        content.contains("cargo install --path bins/amplihack --locked"),
        "FAIL: atlas.yml must install amplihack via:\n\
           cargo install --path bins/amplihack --locked\n\
         \n\
         This matches the install-smoke job pattern in ci.yml.\n\
         --locked ensures reproducible builds from the lockfile."
    );
}

// ---------------------------------------------------------------------------
// TEST 9: Diagram tool installation
// ---------------------------------------------------------------------------

/// The workflow must install graphviz (provides `dot` command)
/// for rendering .dot diagrams to SVG.
#[test]
fn atlas_yml_installs_graphviz() {
    let content = read_atlas_yml();

    assert!(
        content.contains("graphviz"),
        "FAIL: atlas.yml must install graphviz for .dot diagram rendering.\n\
         The code-atlas recipe produces .dot files that need `dot -Tsvg` to render.\n\
         Expected: apt-get install ... graphviz"
    );
}

/// The workflow must install mermaid-cli (provides `mmdc` command)
/// for rendering .mmd diagrams to SVG.
#[test]
fn atlas_yml_installs_mermaid_cli() {
    let content = read_atlas_yml();

    assert!(
        content.contains("mermaid") || content.contains("mmdc"),
        "FAIL: atlas.yml must install mermaid-cli for .mmd diagram rendering.\n\
         The code-atlas recipe produces .mmd files that need `mmdc` to render.\n\
         Expected: npm install -g @mermaid-js/mermaid-cli"
    );
}

// ---------------------------------------------------------------------------
// TEST 10: Recipe execution step
// ---------------------------------------------------------------------------

/// The workflow must run the code-atlas recipe.
#[test]
fn atlas_yml_runs_code_atlas_recipe() {
    let content = read_atlas_yml();

    assert!(
        content.contains("code-atlas"),
        "FAIL: atlas.yml must run the code-atlas recipe.\n\
         Expected: amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml\n\
         or equivalent invocation."
    );
}

/// The recipe run step must have continue-on-error: true.
/// The recipe may fail (no LLM backend, partial output) but the
/// artifact upload must still proceed.
#[test]
fn atlas_yml_recipe_step_has_continue_on_error() {
    let content = read_atlas_yml();

    assert!(
        content.contains("continue-on-error: true"),
        "FAIL: atlas.yml recipe run step must have `continue-on-error: true`.\n\
         The code-atlas recipe may fail partially (e.g., no LLM backend in CI).\n\
         The artifact upload must still proceed with whatever output was produced."
    );
}

// ---------------------------------------------------------------------------
// TEST 11: Artifact upload
// ---------------------------------------------------------------------------

/// The workflow must upload the atlas output as an artifact.
#[test]
fn atlas_yml_uploads_artifact() {
    let content = read_atlas_yml();

    assert!(
        content.contains("actions/upload-artifact"),
        "FAIL: atlas.yml must use `actions/upload-artifact@v4` to upload results.\n\
         The docs/atlas/ output should be uploaded as a CI artifact."
    );
}

/// The artifact must be named "code-atlas" for easy identification.
#[test]
fn atlas_yml_artifact_named_code_atlas() {
    let content = read_atlas_yml();

    assert!(
        content.contains("code-atlas"),
        "FAIL: atlas.yml artifact must be named 'code-atlas'.\n\
         This makes it easy to find and download the atlas output from CI."
    );
}

/// The artifact upload must target docs/atlas/ directory.
#[test]
fn atlas_yml_artifact_path_is_docs_atlas() {
    let content = read_atlas_yml();

    assert!(
        content.contains("docs/atlas"),
        "FAIL: atlas.yml artifact path must include 'docs/atlas'.\n\
         The code-atlas recipe writes output to docs/atlas/ by default."
    );
}

/// The artifact upload step must run even if the recipe step fails.
/// This requires `if: always()` on the upload step.
#[test]
fn atlas_yml_upload_runs_on_failure() {
    let content = read_atlas_yml();

    assert!(
        content.contains("if: always()"),
        "FAIL: atlas.yml upload step must have `if: always()`.\n\
         This ensures the artifact is uploaded even if the recipe step fails.\n\
         Without this, a partial recipe failure would lose all output."
    );
}

// ---------------------------------------------------------------------------
// TEST 12: Uses actions/checkout@v4 (required for all workflows)
// ---------------------------------------------------------------------------

/// The workflow must checkout the repository.
#[test]
fn atlas_yml_has_checkout_step() {
    let content = read_atlas_yml();

    assert!(
        content.contains("actions/checkout@v4"),
        "FAIL: atlas.yml must use `actions/checkout@v4`.\n\
         The recipe needs repository source code to analyze."
    );
}

// ---------------------------------------------------------------------------
// TEST 13: NODE_OPTIONS environment variable
// ---------------------------------------------------------------------------

/// The workflow should set NODE_OPTIONS=--max-old-space-size=32768
/// as a saved preference for mermaid-cli and other Node.js tools.
#[test]
fn atlas_yml_sets_node_options() {
    let content = read_atlas_yml();

    assert!(
        content.contains("NODE_OPTIONS") || content.contains("max-old-space-size"),
        "FAIL: atlas.yml should set NODE_OPTIONS=--max-old-space-size=32768.\n\
         This is a saved preference that prevents mermaid-cli OOM on large diagrams."
    );
}
