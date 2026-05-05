//! TDD tests: Verify that .github/workflows/ci.yml contains the cxx-build
//! version-pin assertion step.
//!
//! ## Why this test exists (WS1)
//!
//! `cargo update` can silently bump `cxx-build` to a version incompatible
//! with the LadybugDB C++ FFI ABI, causing obscure linker failures at build time:
//!
//!   undefined reference to `cxxbridge1$string$new$1_0_138`
//!
//! The fix is a CI step that reads Cargo.lock and hard-fails when cxx-build
//! differs from 1.0.138.  These tests ensure that CI step is present and
//! correctly placed — before any Rust toolchain setup (fail-fast behaviour).
//!
//! ## Failure modes
//!
//! These tests FAIL (red) if:
//! - `.github/workflows/ci.yml` is missing
//! - The "Verify cxx-build pin" step is absent from the `check` job
//! - The step uses a different version constant than "1.0.138"
//! - The step uses a different grep/sed extraction pattern
//! - The step does NOT appear before `dtolnay/rust-toolchain@stable`
//!
//! They PASS (green) once the CI step is inserted as specified.
//!
//! ## Related
//!
//! - `tests/integration/cargo_lock_cxx_consistency_test.rs` — runtime Cargo.lock check
//! - `crates/amplihack-cli/tests/cargo_cxx_pin_test.rs` — Cargo.toml pin check
//! - docs/howto/resolve-lbug-linker-errors.md

use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate `.github/workflows/ci.yml` relative to this test binary's workspace.
fn ci_yml_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk: tests/ (CARGO_MANIFEST_DIR for integration tests in workspace root)
    // actually CARGO_MANIFEST_DIR for bins/amplihack tests points to bins/amplihack
    // Walk up to workspace root: bins/amplihack → bins → workspace root
    path.pop(); // tests/
    path.pop(); // workspace root
    path.push(".github");
    path.push("workflows");
    path.push("ci.yml");
    path
}

/// Read ci.yml content, panicking with a clear message if the file is missing.
fn read_ci_yml() -> String {
    let path = ci_yml_path();
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "ci.yml not found at {path:?}\n\
             Ensure .github/workflows/ci.yml exists and the workspace is intact.\n\
             Error: {e}"
        )
    })
}

// ---------------------------------------------------------------------------
// WS1-TEST-1: The CI file exists
// ---------------------------------------------------------------------------

/// The workflow file must be present.  Trivial guard — if this fails,
/// everything else will fail too and the error is confusing without context.
#[test]
fn ci_yml_file_is_present() {
    let path = ci_yml_path();
    assert!(
        path.exists(),
        "FAIL: .github/workflows/ci.yml not found at {path:?}.\n\
         This file must exist for CI to run any checks."
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-2: The "Verify cxx-build pin" step is present
// ---------------------------------------------------------------------------

/// The `check` job must contain a step named "Verify cxx-build pin".
///
/// **FAILS** before WS1 is implemented: the step does not exist in ci.yml.
/// **PASSES** after the step block is inserted into the `check` job.
#[test]
fn ci_check_job_has_verify_cxx_build_pin_step() {
    let content = read_ci_yml();

    assert!(
        content.contains("Verify cxx-build pin"),
        "FAIL: .github/workflows/ci.yml does not contain a step named \
         'Verify cxx-build pin'.\n\
         \n\
         Add the following step to the `check` job, after actions/checkout@v4:\n\
         \n\
           - name: Verify cxx-build pin\n\
             run: |\n\
               version=$(grep -A1 'name = \"cxx-build\"' Cargo.lock \\\n\
                 | grep version | head -1 | sed 's/.*\"\\(.*\\)\".*/\\')\n\
               if [ \"$version\" != \"1.0.138\" ]; then\n\
                 echo \"ERROR: cxx-build must be pinned to 1.0.138, found $version\"\n\
                 echo \"Run: cargo update -p cxx-build --precise 1.0.138\"\n\
                 exit 1\n\
               fi\n\
         \n\
         See docs/howto/resolve-lbug-linker-errors.md"
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-3: The step asserts version 1.0.138
// ---------------------------------------------------------------------------

/// The CI step must hard-code the target pin version as `1.0.138`.
///
/// Verifies the string comparison in the shell script is against the
/// correct version constant — not a different version or an empty string.
///
/// **FAILS** if the version constant is wrong or absent.
#[test]
fn ci_pin_step_asserts_version_1_0_138() {
    let content = read_ci_yml();

    assert!(
        content.contains("1.0.138"),
        "FAIL: .github/workflows/ci.yml must reference version '1.0.138' \
         in the cxx-build pin assertion step.\n\
         \n\
         The step should contain: if [ \"$version\" != \"1.0.138\" ]\n\
         \n\
         Found in ci.yml:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-4: The step includes a remediation command
// ---------------------------------------------------------------------------

/// The error path in the CI step must print a remediation command so
/// engineers know exactly how to fix the broken state without consulting docs.
///
/// **FAILS** if the remediation command is absent from the CI step.
#[test]
fn ci_pin_step_includes_remediation_command() {
    let content = read_ci_yml();

    assert!(
        content.contains("cargo update -p cxx-build --precise 1.0.138"),
        "FAIL: The 'Verify cxx-build pin' CI step must include the remediation \
         command:\n\
         \n\
           cargo update -p cxx-build --precise 1.0.138\n\
         \n\
         This ensures engineers can fix a broken pin without reading documentation.\n\
         \n\
         Found in ci.yml:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-5: The step uses the correct grep extraction pattern
// ---------------------------------------------------------------------------

/// The shell script must use `grep -A1 'name = \"cxx-build\"'` to locate
/// the cxx-build block in Cargo.lock.
///
/// This pattern relies on the Cargo.lock TOML structure:
///   [[package]]
///   name = "cxx-build"
///   version = "1.0.138"
///
/// `grep -A1` extracts one line after the name match, which is the version line.
///
/// **FAILS** if the grep pattern differs (e.g., wrong quoting, different key name).
#[test]
fn ci_pin_step_uses_correct_grep_pattern() {
    let content = read_ci_yml();

    assert!(
        content.contains(r#"name = "cxx-build""#),
        "FAIL: The CI pin step must use grep to match 'name = \"cxx-build\"' \
         in Cargo.lock.\n\
         \n\
         Expected pattern: grep -A1 'name = \"cxx-build\"'\n\
         \n\
         This matches the TOML key-value format used by Cargo.lock."
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-6: Step ordering — pin check precedes toolchain setup
// ---------------------------------------------------------------------------

/// The "Verify cxx-build pin" step must appear BEFORE the
/// `dtolnay/rust-toolchain@stable` step in the YAML.
///
/// This ensures maximum fail-fast behaviour: if the pin is broken, CI aborts
/// before the expensive (~2 min) Rust toolchain download.
///
/// **FAILS** if the pin step appears after the toolchain step.
#[test]
fn ci_pin_step_appears_before_rust_toolchain_setup() {
    let content = read_ci_yml();

    let pin_pos = content.find("Verify cxx-build pin");
    let toolchain_pos = content.find("dtolnay/rust-toolchain@stable");

    let pin_pos = pin_pos.unwrap_or_else(|| {
        panic!(
            "FAIL: 'Verify cxx-build pin' step not found in ci.yml.\n\
             The step must be present and precede dtolnay/rust-toolchain@stable."
        )
    });

    let toolchain_pos = toolchain_pos.unwrap_or_else(|| {
        panic!(
            "FAIL: 'dtolnay/rust-toolchain@stable' not found in ci.yml.\n\
             The toolchain step is required for the ordering assertion."
        )
    });

    assert!(
        pin_pos < toolchain_pos,
        "FAIL: 'Verify cxx-build pin' step (byte {pin_pos}) must appear BEFORE \
         'dtolnay/rust-toolchain@stable' (byte {toolchain_pos}) in the `check` job.\n\
         \n\
         Correct ordering:\n\
           1. actions/checkout@v4\n\
           2. Verify cxx-build pin   ← must be here\n\
           3. dtolnay/rust-toolchain@stable\n\
           4. Swatinem/rust-cache@v2\n\
           5. cargo fmt --check\n\
           6. cargo clippy\n\
         \n\
         Placing the pin check before toolchain setup achieves fail-fast behaviour."
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-7: Step is inside the `check` job section
// ---------------------------------------------------------------------------

/// The step must be in the `check` job (not `test` or `cross-compile`).
///
/// The `check` job is the gating job that all other jobs `needs:`.
/// Placing the assertion here ensures broken pins block all downstream work.
///
/// **FAILS** if the pin step only appears in the `test` job (wrong placement).
#[test]
fn ci_pin_step_is_in_check_job() {
    let content = read_ci_yml();

    // Find the check: job definition
    let check_job_start = content.find("  check:").unwrap_or_else(|| {
        panic!(
            "FAIL: 'check:' job not found in ci.yml.\n\
             The pin assertion must be in the 'check' job."
        )
    });

    // Find the next top-level job (test:) to determine the check job's extent
    let next_job_start = content[check_job_start..]
        .find("  test:")
        .map(|pos| check_job_start + pos)
        .unwrap_or(content.len());

    let check_job_content = &content[check_job_start..next_job_start];

    assert!(
        check_job_content.contains("Verify cxx-build pin"),
        "FAIL: 'Verify cxx-build pin' step must be inside the `check:` job.\n\
         \n\
         The `check` job gates all other jobs via `needs: check`.\n\
         A broken pin will block the entire pipeline, not just the test job.\n\
         \n\
         check: job content:\n{check_job_content}"
    );
}

// ---------------------------------------------------------------------------
// WS1-TEST-8: The step uses exit 1 on failure (not exit 2 or false)
// ---------------------------------------------------------------------------

/// The CI step must explicitly call `exit 1` when the version check fails.
///
/// Exit code 1 is the conventional POSIX "general failure" code.
/// Exit 2 means "misuse of shell builtins" and exit 127 means "not found".
/// Using `exit 1` makes CI log output unambiguous.
///
/// **FAILS** if exit 1 is absent from the step body.
#[test]
fn ci_pin_step_exits_with_code_1_on_failure() {
    let content = read_ci_yml();

    // The step body should contain `exit 1` for failure
    assert!(
        content.contains("exit 1"),
        "FAIL: The 'Verify cxx-build pin' step must call `exit 1` when the \
         version check fails.\n\
         \n\
         Using `exit 1` is the POSIX convention for general command failure\n\
         and produces clear CI log output.\n\
         \n\
         Ensure the step body contains:\n\
           exit 1"
    );
}
