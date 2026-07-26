//! TDD tests (PR #1027, `spike/ci-wallclock-parallelize`): CI wall-clock
//! parallelization + conditional release-profile contract.
//!
//! These structural tests lock the workflow-only optimization merged in
//! commit `2bed5482 (#1027)`: the CI job graph is parallelized (the required
//! jobs no longer serialize behind `check`) and the expensive full-LTO release
//! profile is disabled on the hot non-tag path while remaining full-fat for
//! shipped artifacts. The invariants are:
//!
//!   1. The 7 required status-check names are unchanged, so branch protection
//!      keeps matching them after the refactor:
//!        - `Lint & Format`
//!        - `Test`
//!        - `Install Smoke Test`
//!        - 4× `Build ${{ matrix.target }}` (the cross-compile matrix)
//!   2. The `test`, `install-smoke`, and `cross-compile` jobs run in parallel
//!      with `Lint & Format` — none of them carries a `needs: check`.
//!   3. The tag-gated `release` job still fans in behind the required build
//!      jobs via `needs: [test, cross-compile]` (so a broken build can't ship).
//!   4. `install-smoke` disables full LTO unconditionally
//!      (`CARGO_PROFILE_RELEASE_LTO: "false"`, `CODEGEN_UNITS: "16"`) — it only
//!      proves the binary installs and launches.
//!   5. `cross-compile` keys the release profile on the git ref: full LTO +
//!      codegen-units=1 on `refs/tags/v*`, off (16 CGU) otherwise — non-tag
//!      runs are fast, tag runs ship identical optimized artifacts.
//!   6. `release.yml` sets NO `CARGO_PROFILE_RELEASE_*` overrides, so shipped
//!      release binaries inherit the workspace `[profile.release]`.
//!   7. The workspace `[profile.release]` keeps `lto = true` and
//!      `codegen-units = 1` — the single source of truth the tag path inherits.
//!
//! ## Failure modes (RED before the change, GREEN after)
//!
//! Each test FAILS against a serialized / unconditional-LTO `ci.yml` and PASSES
//! once the PR #1027 change is applied. They read files only — never build or
//! run the binary — so they are fast and deterministic.
//!
//! ## Related
//! - `tests/integration/ci_speedup_optimization_test.rs` — sibling ci.yml contract
//! - `tests/integration/ci_cxx_build_pin_test.rs` — sibling ci.yml contract
//! - `docs/reference/ci-wallclock-parallelization.md`

use std::path::PathBuf;
use std::sync::LazyLock;

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

fn read_workflow(name: &str) -> String {
    let mut p = workspace_root();
    p.push(".github");
    p.push("workflows");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| {
        panic!(
            "{name} not found at {p:?}\n\
             Ensure .github/workflows/{name} exists and the workspace is intact.\n\
             Error: {e}"
        )
    })
}

fn read_ci_yml() -> String {
    read_workflow("ci.yml")
}

fn read_root_cargo_toml() -> String {
    let mut p = workspace_root();
    p.push("Cargo.toml");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("Cargo.toml not found at {p:?}: {e}"))
}

/// Slice of a workflow covering a single job, from `\n  <job>:` up to the next
/// top-level job key (2-space indent) or end of file. Used for job-scoped
/// assertions so an env var in one job can't satisfy an assertion about another.
fn job_slice<'a>(content: &'a str, job: &str) -> &'a str {
    let needle = format!("\n  {job}:");
    let start = content
        .find(&needle)
        .unwrap_or_else(|| panic!("FAIL: no `{job}:` job found in workflow"));
    let after = start + 1;
    let rest = &content[after..];
    // Next top-level job = a newline followed by exactly two spaces and an
    // identifier char. Skip the current job's own header line first.
    static NEXT_JOB_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\n  [A-Za-z][A-Za-z0-9_-]*:").unwrap());
    let end = NEXT_JOB_RE
        .find(&rest[needle.len() - 1..])
        .map(|m| after + (needle.len() - 1) + m.start())
        .unwrap_or(content.len());
    &content[after..end]
}

/// True if the given job slice declares `needs: check` (either bare or inside a
/// `[...]` list). Anchored to line-start so an explanatory `# No needs: check`
/// comment never false-matches.
fn job_needs_check(job: &str) -> bool {
    static NEEDS_CHECK_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^\s*needs:\s*(\[[^\]]*check[^\]]*\]|check\b)").unwrap());
    NEEDS_CHECK_RE.is_match(job)
}

// ---------------------------------------------------------------------------
// 1. Required status-check names are unchanged (branch protection contract)
// ---------------------------------------------------------------------------

#[test]
fn ci_preserves_required_check_names() {
    let content = read_ci_yml();
    for name in [
        "name: Lint & Format",
        "name: Test",
        "name: Install Smoke Test",
    ] {
        assert!(
            content.contains(name),
            "FAIL: ci.yml must keep the required check `{name}` verbatim.\n\
             Branch protection matches on the job's `name:`; renaming it would\n\
             orphan the required check and permanently block merges."
        );
    }
}

#[test]
fn ci_cross_compile_check_name_is_matrix_build() {
    let content = read_ci_yml();
    // The 4 required `Build <target>` checks are produced by one matrix job
    // whose name interpolates the target. The literal name template must not
    // change, or all four required checks are renamed at once.
    let re = Regex::new(r"name:\s*Build \$\{\{\s*matrix\.target\s*\}\}").unwrap();
    assert!(
        re.is_match(&content),
        "FAIL: the cross-compile job must keep `name: Build ${{{{ matrix.target }}}}`\n\
         so the four required `Build <target>` checks keep their protected names."
    );
}

// ---------------------------------------------------------------------------
// 2. Parallelization — required jobs no longer serialize behind `check`
// ---------------------------------------------------------------------------

#[test]
fn ci_test_job_has_no_needs_check() {
    let content = read_ci_yml();
    let job = job_slice(&content, "test");
    assert!(
        !job_needs_check(job),
        "FAIL: the `test` job must NOT declare `needs: check`.\n\
         PR #1027 parallelizes it with `Lint & Format` to cut wall-clock.\n\
         Test-job slice:\n{job}"
    );
}

#[test]
fn ci_install_smoke_job_has_no_needs_check() {
    let content = read_ci_yml();
    let job = job_slice(&content, "install-smoke");
    assert!(
        !job_needs_check(job),
        "FAIL: the `install-smoke` job must NOT declare `needs: check`.\n\
         It must run in parallel with `Lint & Format`.\n\
         install-smoke slice:\n{job}"
    );
}

#[test]
fn ci_cross_compile_job_has_no_needs_check() {
    let content = read_ci_yml();
    let job = job_slice(&content, "cross-compile");
    assert!(
        !job_needs_check(job),
        "FAIL: the `cross-compile` job must NOT declare `needs: check`.\n\
         It must run in parallel with `Lint & Format`.\n\
         cross-compile slice:\n{job}"
    );
}

// ---------------------------------------------------------------------------
// 3. Release fan-in is preserved (a broken build must not ship)
// ---------------------------------------------------------------------------

#[test]
fn ci_release_job_fans_in_behind_builds() {
    let content = read_ci_yml();
    let job = job_slice(&content, "release");
    let re = Regex::new(r"needs:\s*\[[^\]]*\btest\b[^\]]*\bcross-compile\b[^\]]*\]").unwrap();
    assert!(
        re.is_match(job),
        "FAIL: the `release` job must keep `needs: [test, cross-compile]` so a\n\
         failing test or cross-compile can never publish artifacts.\n\
         release slice:\n{job}"
    );
}

#[test]
fn ci_release_job_is_tag_gated() {
    let content = read_ci_yml();
    let job = job_slice(&content, "release");
    assert!(
        job.contains("startsWith(github.ref, 'refs/tags/v')"),
        "FAIL: the `release` job must stay gated on `refs/tags/v*` so it only\n\
         runs on version tags — non-tag runs must not attempt a release.\n\
         release slice:\n{job}"
    );
}

// ---------------------------------------------------------------------------
// 4. install-smoke disables full LTO unconditionally
// ---------------------------------------------------------------------------

#[test]
fn ci_install_smoke_disables_full_lto() {
    let content = read_ci_yml();
    let job = job_slice(&content, "install-smoke");
    let lto = Regex::new(r#"CARGO_PROFILE_RELEASE_LTO:\s*"false""#).unwrap();
    let cgu = Regex::new(r#"CARGO_PROFILE_RELEASE_CODEGEN_UNITS:\s*"16""#).unwrap();
    assert!(
        lto.is_match(job) && cgu.is_match(job),
        "FAIL: the `install-smoke` job must disable the expensive release\n\
         profile unconditionally (LTO=false, codegen-units=16) — it only proves\n\
         the binary installs and launches, it never ships an artifact.\n\
         install-smoke slice:\n{job}"
    );
}

// ---------------------------------------------------------------------------
// 5. cross-compile keys the release profile on the git ref
// ---------------------------------------------------------------------------

#[test]
fn ci_cross_compile_lto_is_tag_conditional() {
    let content = read_ci_yml();
    let job = job_slice(&content, "cross-compile");
    // Full LTO only on refs/tags/v*, off otherwise.
    let re = Regex::new(
        r"CARGO_PROFILE_RELEASE_LTO:\s*\$\{\{\s*startsWith\(github\.ref,\s*'refs/tags/v'\)\s*&&\s*'true'\s*\|\|\s*'false'\s*\}\}",
    )
    .unwrap();
    assert!(
        re.is_match(job),
        "FAIL: the `cross-compile` job must key LTO on the git ref:\n\
         `CARGO_PROFILE_RELEASE_LTO: ${{{{ startsWith(github.ref, 'refs/tags/v') && 'true' || 'false' }}}}`\n\
         so non-tag runs are fast and tag runs ship full-LTO artifacts.\n\
         cross-compile slice:\n{job}"
    );
}

#[test]
fn ci_cross_compile_codegen_units_is_tag_conditional() {
    let content = read_ci_yml();
    let job = job_slice(&content, "cross-compile");
    let re = Regex::new(
        r"CARGO_PROFILE_RELEASE_CODEGEN_UNITS:\s*\$\{\{\s*startsWith\(github\.ref,\s*'refs/tags/v'\)\s*&&\s*'1'\s*\|\|\s*'16'\s*\}\}",
    )
    .unwrap();
    assert!(
        re.is_match(job),
        "FAIL: the `cross-compile` job must key codegen-units on the git ref:\n\
         1 on `refs/tags/v*` (matches [profile.release]), 16 otherwise.\n\
         cross-compile slice:\n{job}"
    );
}

// ---------------------------------------------------------------------------
// 6. Shipped artifacts are unchanged — release.yml sets no profile overrides
// ---------------------------------------------------------------------------

#[test]
fn release_workflow_has_no_profile_overrides() {
    let content = read_workflow("release.yml");
    assert!(
        !content.contains("CARGO_PROFILE_RELEASE_LTO")
            && !content.contains("CARGO_PROFILE_RELEASE_CODEGEN_UNITS"),
        "FAIL: release.yml must NOT set any CARGO_PROFILE_RELEASE_* override.\n\
         Shipped release binaries must inherit the workspace [profile.release]\n\
         (lto = true, codegen-units = 1). An override here would silently\n\
         de-optimize published artifacts."
    );
}

// ---------------------------------------------------------------------------
// 7. The inherited release profile is still fully optimized
// ---------------------------------------------------------------------------

#[test]
fn workspace_release_profile_keeps_full_optimization() {
    let content = read_root_cargo_toml();
    let start = content
        .find("[profile.release]")
        .unwrap_or_else(|| panic!("FAIL: no [profile.release] in root Cargo.toml"));
    let section = &content[start..];
    let lto = Regex::new(r"lto\s*=\s*true").unwrap();
    let cgu = Regex::new(r"codegen-units\s*=\s*1\b").unwrap();
    assert!(
        lto.is_match(section) && cgu.is_match(section),
        "FAIL: [profile.release] must keep `lto = true` and `codegen-units = 1`.\n\
         This is the single source of truth the tag/release path inherits; the\n\
         CI wall-clock optimization only overrides it on non-tag runs.\n\
         [profile.release] section:\n{section}"
    );
}
