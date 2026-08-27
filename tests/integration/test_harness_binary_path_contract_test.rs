//! TDD tests (issue #744): integration-test binary-path robustness contract.
//!
//! Several integration tests locate the compiled binary by hand-walking
//! `CARGO_MANIFEST_DIR` up to the workspace root and appending
//! `target/debug/amplihack`. That is fragile:
//!
//!   * it panics with "amplihack binary not found at target/debug/amplihack"
//!     when a prior `cargo build` was disrupted (an observed transient), and
//!   * it ignores `CARGO_TARGET_DIR` and the release/debug distinction.
//!
//! Cargo already exposes the correct, build-ordered path via the
//! `CARGO_BIN_EXE_<name>` environment variable, but *only* for test targets
//! that belong to the package producing that binary. So the fix is twofold:
//!
//!   1. Every affected test must resolve the binary via
//!      `env!("CARGO_BIN_EXE_amplihack")` / `env!("CARGO_BIN_EXE_amplihack-hooks")`
//!      and must NOT hand-roll `target/debug/<bin>`.
//!   2. `skip_update_check_flag_test.rs` — currently an orphan file not wired
//!      as a `[[test]]` — must be wired into `bins/amplihack/Cargo.toml` so it
//!      actually compiles/runs AND so `CARGO_BIN_EXE_amplihack` resolves for it.
//!
//! Using `env!` (compile-time) rather than `option_env!`/`var_os` (soft, with a
//! hardcoded fallback) also forces the binary to be built as a test
//! prerequisite, eliminating the "binary not found" race entirely.
//!
//! ## Failure modes (RED until the harness fix lands)
//! Each assertion FAILS against the current fragile helpers and PASSES once the
//! path helpers are converted and the orphan test is wired.
//!
//! ## Issue #1378
//! Two more tests were carrying the same defect from
//! `crates/amplihack-cli/tests/`, where `CARGO_BIN_EXE_amplihack` does not
//! exist because that package does not produce the binary. They have been
//! moved into `bins/amplihack/tests/` so the Cargo-provided path resolves and
//! the binary is built as a test prerequisite; they are covered by the lists
//! below, which now hold workspace-relative paths rather than bare file names.
//!
//! ## Related
//! - `tests/integration/ci_speedup_optimization_test.rs`
//! - `docs/reference/ci-pipeline.md`

use std::path::PathBuf;

/// (workspace-relative test file path, binary name whose `CARGO_BIN_EXE_<name>`
/// it must use).
const AMPLIHACK_BIN: &str = "amplihack";
const HOOKS_BIN: &str = "amplihack-hooks";

const FRAGILE_TESTS: &[(&str, &str)] = &[
    ("tests/integration/cli_golden_tests.rs", AMPLIHACK_BIN),
    ("tests/integration/kuzu_path_notice_test.rs", AMPLIHACK_BIN),
    ("tests/integration/cli_launch_test.rs", AMPLIHACK_BIN),
    ("tests/integration/recipe_e2e_test.rs", AMPLIHACK_BIN),
    ("tests/integration/security_hygiene_test.rs", AMPLIHACK_BIN),
    ("tests/integration/fleet_probe.rs", AMPLIHACK_BIN),
    (
        "tests/integration/skip_update_check_flag_test.rs",
        AMPLIHACK_BIN,
    ),
    ("tests/integration/no_python_probe_test.rs", AMPLIHACK_BIN),
    (
        "tests/integration/doctor_node_remediation_test.rs",
        AMPLIHACK_BIN,
    ),
    ("tests/integration/hook_dispatch_test.rs", HOOKS_BIN),
    // Issue #1378: these two used to live in `crates/amplihack-cli/tests/`,
    // where `CARGO_BIN_EXE_amplihack` does not exist, and hand-rolled
    // `<workspace>/target/debug/amplihack` — which is wrong under any
    // redirected `CARGO_TARGET_DIR`. Moving them into the binary's own package
    // makes the Cargo-provided path available and builds the binary as a test
    // prerequisite.
    (
        "bins/amplihack/tests/issue_538_install_completeness.rs",
        AMPLIHACK_BIN,
    ),
    (
        "bins/amplihack/tests/issue_625_update_prompt_subprocess_safe.rs",
        AMPLIHACK_BIN,
    ),
];

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins
    path.pop(); // bins → workspace root
    path
}

fn test_source_path(rel: &str) -> PathBuf {
    workspace_root().join(rel)
}

fn read_test_source(rel: &str) -> String {
    let p = test_source_path(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("FAIL: cannot read {p:?}: {e}"))
}

// ---------------------------------------------------------------------------
// Guard: the files we are asserting about exist
// ---------------------------------------------------------------------------

#[test]
fn all_target_test_files_exist() {
    for (rel, _) in FRAGILE_TESTS {
        let p = test_source_path(rel);
        assert!(p.exists(), "FAIL: expected integration test {p:?} to exist");
    }
}

// ---------------------------------------------------------------------------
// 1. Each affected test must resolve the binary via CARGO_BIN_EXE_<name>
// ---------------------------------------------------------------------------

#[test]
fn fragile_tests_resolve_binary_via_cargo_bin_exe() {
    let mut offenders = Vec::new();
    for (file, bin) in FRAGILE_TESTS {
        let src = read_test_source(file);
        let needle = format!("env!(\"CARGO_BIN_EXE_{bin}\")");
        if !src.contains(&needle) {
            offenders.push(format!("  {file}: missing `{needle}`"));
        }
    }
    assert!(
        offenders.is_empty(),
        "FAIL: these integration tests must locate the binary via the Cargo-provided\n\
         env var instead of a hand-walked path:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 2. No hand-rolled target/debug/<bin> path push remains
// ---------------------------------------------------------------------------

#[test]
fn fragile_tests_drop_hardcoded_target_debug_path() {
    let mut offenders = Vec::new();
    for (file, bin) in FRAGILE_TESTS {
        let src = read_test_source(file);
        let bad = format!("push(\"target/debug/{bin}\")");
        if src.contains(&bad) {
            offenders.push(format!("  {file}: still contains `{bad}`"));
        }
    }
    assert!(
        offenders.is_empty(),
        "FAIL: these tests still hand-roll a `target/debug/<bin>` path, which\n\
         panics with 'binary not found' when a build is disrupted and ignores\n\
         CARGO_TARGET_DIR:\n{}",
        offenders.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 3. The two "soft" helpers must become hard `env!` (no silent fallback)
// ---------------------------------------------------------------------------

#[test]
fn soft_lookup_tests_are_tightened_to_hard_env() {
    let no_python = read_test_source("tests/integration/no_python_probe_test.rs");
    assert!(
        !no_python.contains("option_env!(\"CARGO_BIN_EXE_amplihack\")"),
        "FAIL: no_python_probe_test.rs must use the hard `env!(\"CARGO_BIN_EXE_amplihack\")`,\n\
         not `option_env!` with a hardcoded fallback (which re-introduces the race)."
    );

    let doctor = read_test_source("tests/integration/doctor_node_remediation_test.rs");
    assert!(
        !doctor.contains("var_os(\"CARGO_BIN_EXE_amplihack\")"),
        "FAIL: doctor_node_remediation_test.rs must use the hard\n\
         `env!(\"CARGO_BIN_EXE_amplihack\")`, not `std::env::var_os(...)` with a\n\
         hardcoded `target/debug` fallback."
    );
}

// ---------------------------------------------------------------------------
// 4. skip_update_check_flag_test.rs must be wired as a [[test]] target
// ---------------------------------------------------------------------------

#[test]
fn skip_update_check_flag_test_is_wired_into_bin_package() {
    let mut cargo = workspace_root();
    cargo.push("bins");
    cargo.push("amplihack");
    cargo.push("Cargo.toml");
    let manifest = std::fs::read_to_string(&cargo)
        .unwrap_or_else(|e| panic!("FAIL: cannot read {cargo:?}: {e}"));
    assert!(
        manifest.contains("../../tests/integration/skip_update_check_flag_test.rs"),
        "FAIL: skip_update_check_flag_test.rs is an orphan file — it is not\n\
         declared as a `[[test]]` in bins/amplihack/Cargo.toml, so it never\n\
         compiles/runs in CI and `CARGO_BIN_EXE_amplihack` is unavailable to it.\n\
         Add:\n\
         \n\
           [[test]]\n\
           name = \"skip_update_check_flag\"\n\
           path = \"../../tests/integration/skip_update_check_flag_test.rs\"\n"
    );
}
