//! TDD tests (issue #744): CI speed-up + robustness contract.
//!
//! These structural tests lock the low-risk, high-leverage CI optimizations
//! identified while investigating why the required `Test` job takes ~30 min and
//! why runs periodically break on toolchain drift or disk exhaustion:
//!
//!   1. Pin the Rust toolchain (a `rust-toolchain.toml` at the workspace root
//!      plus `dtolnay/rust-toolchain@1.97.0` in every CI job) so a surprise
//!      `@stable` bump can never break clippy/fmt again (cf. emergency PR #878).
//!   2. Run the heavy workspace test suite with `cargo-nextest` (better
//!      parallelism, lower peak disk) while preserving doctests via an explicit
//!      `cargo test --workspace --doc` step (nextest does not run doctests).
//!   3. Shrink the test build with `CARGO_PROFILE_TEST_DEBUG=0`.
//!   4. Free runner disk with the robust `jlumbroso/free-disk-space` action
//!      instead of the fragile hand-rolled `sudo rm -rf` step (issue #744).
//!   5. Keep the cache from bloating: `save-if` only on `main`.
//!   6. SHA-pin every third-party action introduced here (supply-chain).
//!   7. Preserve the disk-safety invariant on the `Test` job
//!      (`cache-targets: false`) that issue #744 established.
//!
//! ## Failure modes (these are RED until the implementation lands)
//!
//! Each test FAILS (red) against the current `ci.yml` / missing
//! `rust-toolchain.toml`, and PASSES (green) once the corresponding change is
//! applied. They read files only — they never build or run the binary — so
//! they are fast and deterministic.
//!
//! ## Related
//! - `tests/integration/ci_cxx_build_pin_test.rs` — sibling ci.yml contract
//! - `tests/integration/test_harness_binary_path_contract_test.rs`
//! - `docs/reference/ci-pipeline.md`

use std::path::PathBuf;

use regex::Regex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Workspace root, reached from this test binary's manifest dir
/// (`bins/amplihack` → `bins` → workspace root).
fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // bins/amplihack → bins
    path.pop(); // bins → workspace root
    path
}

fn ci_yml_path() -> PathBuf {
    let mut p = workspace_root();
    p.push(".github");
    p.push("workflows");
    p.push("ci.yml");
    p
}

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

fn rust_toolchain_path() -> PathBuf {
    let mut p = workspace_root();
    p.push("rust-toolchain.toml");
    p
}

/// Slice of `ci.yml` covering the `test:` job only (from `  test:` up to the
/// next top-level job `  install-smoke:`). Used to assert job-scoped invariants.
fn test_job_slice(content: &str) -> &str {
    let start = content
        .find("\n  test:")
        .unwrap_or_else(|| panic!("FAIL: no `test:` job found in ci.yml"));
    let rest = &content[start + 1..];
    let end = rest
        .find("\n  install-smoke:")
        .or_else(|| rest.find("\n  cross-compile:"))
        .or_else(|| rest.find("\n  release:"))
        .map(|e| start + 1 + e)
        .unwrap_or(content.len());
    &content[start + 1..end]
}

// ---------------------------------------------------------------------------
// 1. Toolchain pin — rust-toolchain.toml
// ---------------------------------------------------------------------------

#[test]
fn rust_toolchain_toml_exists() {
    let path = rust_toolchain_path();
    assert!(
        path.exists(),
        "FAIL: rust-toolchain.toml not found at {path:?}.\n\
         Create it at the workspace root to pin the Rust toolchain and stop\n\
         `@stable` drift from silently breaking clippy/fmt (issue #744)."
    );
}

#[test]
fn rust_toolchain_pins_1_97_0() {
    let content = std::fs::read_to_string(rust_toolchain_path()).unwrap_or_default();
    let re = Regex::new(r#"channel\s*=\s*"1\.97\.0""#).unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: rust-toolchain.toml must pin the toolchain to 1.97.0.\n\
         Expected a line like: channel = \"1.97.0\"\n\
         (1.97.0 is the known-good version the latest green run resolved).\n\
         Found:\n{content}"
    );
}

#[test]
fn rust_toolchain_declares_rustfmt_and_clippy() {
    let content = std::fs::read_to_string(rust_toolchain_path()).unwrap_or_default();
    assert!(
        content.contains("rustfmt") && content.contains("clippy"),
        "FAIL: rust-toolchain.toml must declare the `rustfmt` and `clippy`\n\
         components so local dev / pre-commit have the same tools CI uses.\n\
         Expected: components = [\"rustfmt\", \"clippy\"]\n\
         Found:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// 2. Toolchain pin — ci.yml no longer floats on @stable
// ---------------------------------------------------------------------------

#[test]
fn ci_has_no_unpinned_stable_toolchain() {
    let content = read_ci_yml();
    assert!(
        !content.contains("dtolnay/rust-toolchain@stable"),
        "FAIL: ci.yml still references `dtolnay/rust-toolchain@stable`.\n\
         All uses must be pinned (e.g. `dtolnay/rust-toolchain@1.97.0`) so a\n\
         surprise stable bump cannot break clippy/fmt (cf. PR #878)."
    );
}

#[test]
fn ci_pins_dtolnay_toolchain_to_1_97_0() {
    let content = read_ci_yml();
    assert!(
        content.contains("dtolnay/rust-toolchain@1.97.0"),
        "FAIL: ci.yml must pin the toolchain action to `dtolnay/rust-toolchain@1.97.0`.\n\
         This matches rust-toolchain.toml and the last known-good run."
    );
}

#[test]
fn ci_all_dtolnay_refs_are_pinned() {
    let content = read_ci_yml();
    let total = content.matches("dtolnay/rust-toolchain@").count();
    let stable = content.matches("dtolnay/rust-toolchain@stable").count();
    assert!(
        total >= 4,
        "FAIL: expected all 4 toolchain-setup jobs (check, test, install-smoke,\n\
         cross-compile) to use dtolnay/rust-toolchain@, found {total}."
    );
    assert_eq!(
        stable, 0,
        "FAIL: {stable} of {total} dtolnay/rust-toolchain refs still float on @stable.\n\
         Every ref must be pinned to a concrete version."
    );
}

// ---------------------------------------------------------------------------
// 3. Test job — cargo-nextest with doctests preserved
// ---------------------------------------------------------------------------

#[test]
fn ci_test_job_uses_nextest() {
    let content = read_ci_yml();
    assert!(
        content.contains("cargo nextest run"),
        "FAIL: the Test job must run the suite with `cargo nextest run`\n\
         (faster parallelism + lower peak disk than `cargo test`)."
    );
}

#[test]
fn ci_nextest_invocation_is_workspace_locked() {
    let content = read_ci_yml();
    let re = Regex::new(r"cargo nextest run[^\n]*--workspace[^\n]*--locked").unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: the nextest invocation must include `--workspace --locked` so it\n\
         covers every crate and honours Cargo.lock (no dependency substitution).\n\
         Expected: cargo nextest run --workspace --locked"
    );
}

#[test]
fn ci_preserves_doctests_after_nextest() {
    let content = read_ci_yml();
    // nextest does NOT run doctests; an explicit doc step prevents coverage loss.
    let re = Regex::new(r"cargo test[^\n]*--workspace[^\n]*--doc").unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: nextest does not run doctests. The Test job must keep an explicit\n\
         `cargo test --workspace --doc --locked` step so doctest coverage is not lost."
    );
}

#[test]
fn ci_installs_nextest_via_taiki_e_action() {
    let content = read_ci_yml();
    assert!(
        content.contains("taiki-e/install-action"),
        "FAIL: nextest must be installed via `taiki-e/install-action`\n\
         (prebuilt binary; avoids a slow `cargo install nextest`)."
    );
}

#[test]
fn ci_taiki_e_install_action_is_sha_pinned() {
    let content = read_ci_yml();
    let re = Regex::new(r"taiki-e/install-action@[0-9a-fA-F]{40}").unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: `taiki-e/install-action` must be pinned to a full 40-char commit\n\
         SHA (supply-chain hardening) — a mutable tag like @v2 is not acceptable."
    );
}

#[test]
fn ci_sets_cargo_profile_test_debug_zero() {
    let content = read_ci_yml();
    let re = Regex::new(r#"CARGO_PROFILE_TEST_DEBUG\s*[:=]\s*"?0"?"#).unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: the Test job must set CARGO_PROFILE_TEST_DEBUG=0 to shrink the\n\
         test build (the workspace's many test binaries) and reduce peak disk/link time."
    );
}

// ---------------------------------------------------------------------------
// 4. Robust disk freeing
// ---------------------------------------------------------------------------

#[test]
fn ci_uses_free_disk_space_action() {
    let content = read_ci_yml();
    assert!(
        content.contains("jlumbroso/free-disk-space"),
        "FAIL: the Test job must free runner disk with the robust\n\
         `jlumbroso/free-disk-space` action to stop `No space left on device`\n\
         transients (issue #744)."
    );
}

#[test]
fn ci_free_disk_space_action_is_sha_pinned() {
    let content = read_ci_yml();
    let re = Regex::new(r"jlumbroso/free-disk-space@[0-9a-fA-F]{40}").unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: `jlumbroso/free-disk-space` must be pinned to a full 40-char\n\
         commit SHA (supply-chain hardening) — not a mutable tag."
    );
}

#[test]
fn ci_removes_fragile_manual_rm_disk_step() {
    let content = read_ci_yml();
    assert!(
        !content.contains("sudo rm -rf /usr/share/dotnet"),
        "FAIL: the fragile hand-rolled `sudo rm -rf /usr/share/dotnet ...` disk\n\
         step must be replaced by `jlumbroso/free-disk-space`, not kept alongside it."
    );
}

// ---------------------------------------------------------------------------
// 5. Cache hygiene — save only on main
// ---------------------------------------------------------------------------

#[test]
fn ci_cache_save_is_main_only() {
    let content = read_ci_yml();
    assert!(
        content.contains("save-if:") && content.contains("refs/heads/main"),
        "FAIL: rust-cache must set `save-if: ${{{{ github.ref == 'refs/heads/main' }}}}`\n\
         so PR branches restore-but-don't-save — preventing cache bloat and\n\
         fork-PR cache poisoning."
    );
}

// ---------------------------------------------------------------------------
// 6. Preserve the issue #744 disk-safety invariant
// ---------------------------------------------------------------------------

#[test]
fn ci_test_job_keeps_cache_targets_false() {
    let content = read_ci_yml();
    let job = test_job_slice(&content);
    assert!(
        job.contains("cache-targets: false"),
        "FAIL: the Test job must KEEP `cache-targets: false`.\n\
         Restoring a large `target/` can exhaust runner disk before tests start\n\
         (issue #744). nextest lowers peak disk; re-enabling target caching here\n\
         would fight that. Test-job slice:\n{job}"
    );
}
