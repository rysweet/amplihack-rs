//! TDD tests: Cargo.lock cxx / cxx-build version consistency.
//!
//! ## Why this test exists (issue #35)
//!
//! The kuzu C++ FFI bridge uses the `cxx` crate for Rust ↔ C++ interop.
//! The `cxx-build` crate generates the C++ glue code at build time.  Both
//! crates must have the **identical** minor version, because `cxx-build`
//! embeds the minor version in every generated bridge symbol, e.g.:
//!
//!   `cxxbridge1$string$new$1_0_138`
//!                            ^^^^^  ← minor version token
//!
//! When cxx = 1.0.138 but cxx-build = 1.0.194, the token in generated
//! symbols is `$1_0_194`, which the linker cannot resolve against the
//! runtime library built with token `$1_0_138`.  The result is:
//!
//!   error: linking with `cc` failed: exit status: 1
//!   undefined reference to `cxxbridge1$string$new$1_0_138'
//!
//! ## How this test catches the regression
//!
//! These tests read Cargo.lock directly and assert version parity.  They
//! **FAIL** when Cargo.lock contains:
//!   - `cxx = "1.0.138"` AND `cxx-build = "1.0.194"`  (the broken state)
//!
//! They **PASS** after the fix:
//!   `cargo update -p cxx-build --precise 1.0.138`
//!
//! ## References
//!
//! - Issue:   https://github.com/rysweet/amplihack-rs/issues/35
//! - PR:      https://github.com/rysweet/amplihack-rs/pull/43
//! - How-to:  docs/howto/resolve-kuzu-linker-errors.md
//! - Concept: docs/concepts/cxx-version-contract.md

use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the workspace Cargo.lock path relative to this crate's manifest.
fn cargo_lock_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk: bins/amplihack → workspace root
    path.pop(); // amplihack/
    path.pop(); // bins/
    path.push("Cargo.lock");
    path
}

/// Parse `Cargo.lock` and return a map of `package-name → version` for every
/// `[[package]]` entry.  When a package appears multiple times (multiple
/// versions in the lock file), the last occurrence is stored.
fn parse_cargo_lock_versions(path: &PathBuf) -> HashMap<String, String> {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read Cargo.lock at {path:?}: {e}"));

    let mut versions: HashMap<String, String> = HashMap::new();
    let mut current_name: Option<String> = None;
    let mut current_version: Option<String> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if line == "[[package]]" {
            // Commit the previous stanza if complete.
            if let (Some(name), Some(version)) = (current_name.take(), current_version.take()) {
                versions.insert(name, version);
            }
            continue;
        }

        if let Some(v) = extract_quoted_value(line, "name") {
            current_name = Some(v);
        }
        if let Some(v) = extract_quoted_value(line, "version") {
            current_version = Some(v);
        }
    }

    // Commit the final stanza.
    if let (Some(name), Some(version)) = (current_name, current_version) {
        versions.insert(name, version);
    }

    versions
}

/// Extract the string value from a TOML key-value line `key = "value"`.
/// Returns `None` if the line does not match that form.
fn extract_quoted_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key} = \"");
    line.strip_prefix(prefix.as_str())
        .and_then(|rest| rest.strip_suffix('"'))
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Unit tests for the Cargo.lock parser itself
// ---------------------------------------------------------------------------

#[test]
fn cargo_lock_file_is_present() {
    let path = cargo_lock_path();
    assert!(
        path.exists(),
        "Cargo.lock not found at {path:?}. Ensure the workspace has been initialized with `cargo build`."
    );
}

#[test]
fn extract_quoted_value_parses_correctly() {
    assert_eq!(
        extract_quoted_value(r#"name = "cxx""#, "name"),
        Some("cxx".to_string())
    );
    assert_eq!(
        extract_quoted_value(r#"version = "1.0.138""#, "version"),
        Some("1.0.138".to_string())
    );
    assert_eq!(extract_quoted_value(r#"checksum = "abc""#, "name"), None);
    assert_eq!(extract_quoted_value("", "name"), None);
}

#[test]
fn cargo_lock_contains_cxx_and_cxx_build() {
    let path = cargo_lock_path();
    let versions = parse_cargo_lock_versions(&path);

    assert!(
        versions.contains_key("cxx"),
        "cxx not found in Cargo.lock — is kuzu still a dependency?"
    );
    assert!(
        versions.contains_key("cxx-build"),
        "cxx-build not found in Cargo.lock — is kuzu still a dependency?"
    );
}

// ---------------------------------------------------------------------------
// Core contract tests (directly verify the fix for issue #35)
// ---------------------------------------------------------------------------

/// `cxx` and `cxx-build` must have the **same version** in Cargo.lock.
///
/// This is the primary test for the fix in PR #43.  It fails when Cargo.lock
/// contains `cxx = "1.0.138"` but `cxx-build = "1.0.194"`.
///
/// ```
/// FAIL MESSAGE (before fix):
///   thread 'cxx_and_cxx_build_versions_match' panicked at:
///   cxx (1.0.138) and cxx-build (1.0.194) must have the same version.
///   Version mismatch causes undefined-symbol linker errors with kuzu.
///   Fix: cargo update -p cxx-build --precise 1.0.138
/// ```
#[test]
fn cxx_and_cxx_build_versions_match() {
    let path = cargo_lock_path();
    let versions = parse_cargo_lock_versions(&path);

    let cxx = versions
        .get("cxx")
        .unwrap_or_else(|| panic!("cxx not found in Cargo.lock at {path:?}"));
    let cxx_build = versions
        .get("cxx-build")
        .unwrap_or_else(|| panic!("cxx-build not found in Cargo.lock at {path:?}"));

    assert_eq!(
        cxx,
        cxx_build,
        "\n\
        ┌─────────────────────────────────────────────────────────────────────┐\n\
        │  cxx version mismatch detected — kuzu C++ FFI will fail to link!    │\n\
        ├─────────────────────────────────────────────────────────────────────┤\n\
        │  cxx         = {cxx:<60}│\n\
        │  cxx-build   = {cxx_build:<60}│\n\
        ├─────────────────────────────────────────────────────────────────────┤\n\
        │  Fix:  cargo update -p cxx-build --precise {cxx}                    \n\
        │  Docs: docs/howto/resolve-kuzu-linker-errors.md                     │\n\
        │  Issue: https://github.com/rysweet/amplihack-rs/issues/35           │\n\
        └─────────────────────────────────────────────────────────────────────┘"
    );
}

/// All cxxbridge components (cmd, flags, macro) must match the `cxx` version.
///
/// The entire cxxbridge suite is released in lockstep.  Drift in any
/// component can cause symbol incompatibilities between crates in the
/// dependency graph.
#[test]
fn cxxbridge_suite_versions_match_cxx() {
    let path = cargo_lock_path();
    let versions = parse_cargo_lock_versions(&path);

    let cxx = versions
        .get("cxx")
        .unwrap_or_else(|| panic!("cxx not found in Cargo.lock at {path:?}"));

    for component in &["cxxbridge-cmd", "cxxbridge-flags", "cxxbridge-macro"] {
        if let Some(component_version) = versions.get(*component) {
            assert_eq!(
                cxx,
                component_version,
                "{component} version ({component_version}) must match cxx ({cxx}).\n\
                Bridge component drift can cause subtle FFI incompatibilities.\n\
                See docs/concepts/cxx-version-contract.md"
            );
        }
    }
}

/// `cxx-build` must be pinned to patch version 138, which is the version
/// required by kuzu 0.11.3.
///
/// The cxx versioning scheme uses the **patch** number as the distinguishing
/// identifier.  `cxx = "1.0.138"` means major=1, minor=0, patch=138.
/// The bridge symbol token `$1_0_138` encodes all three parts.
///
/// kuzu 0.11.3 specifies `cxx = "=1.0.138"` (exact pin), so cxx-build
/// must also have patch = 138: `cxx-build = "1.0.138"`.
///
/// **FAILS** when patch ≠ 138 (e.g., cxx-build = 1.0.194 → patch = 194).
/// **PASSES** after: `cargo update -p cxx-build --precise 1.0.138`
#[test]
fn cxx_build_patch_version_compatible_with_kuzu_0_11_3() {
    let path = cargo_lock_path();
    let versions = parse_cargo_lock_versions(&path);

    let cxx_build_version = versions
        .get("cxx-build")
        .unwrap_or_else(|| panic!("cxx-build not found in Cargo.lock at {path:?}"));

    let parts: Vec<&str> = cxx_build_version.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "cxx-build version must be semver x.y.z, got: {cxx_build_version}"
    );

    let major: u64 = parts[0]
        .parse()
        .expect("cxx-build major version must be numeric");
    // cxx versioning: major=1, minor=0, patch=<N>.  The patch is the key.
    let patch: u64 = parts[2]
        .parse()
        .expect("cxx-build patch version must be numeric");

    assert_eq!(major, 1, "cxx-build major must be 1, got: {cxx_build_version}");
    assert_eq!(
        patch,
        138,
        "\n\
        cxx-build patch version must be 138 to be compatible with kuzu 0.11.3.\n\
        Found:    cxx-build = {cxx_build_version} (patch = {patch})\n\
        Required: cxx-build = 1.0.138            (patch = 138)\n\
        \n\
        Fix:  cargo update -p cxx-build --precise 1.0.138\n\
        Docs: docs/howto/resolve-kuzu-linker-errors.md\n\
        Issue: https://github.com/rysweet/amplihack-rs/issues/35"
    );
}

/// `cxx` itself must be pinned at 1.0.138 (the exact version kuzu requires).
///
/// kuzu 0.11.3 specifies `cxx = "=1.0.138"`.  The patch version (138) must
/// match to ensure symbol name compatibility in the generated bridge code.
///
/// This test ensures the Cargo.lock has not been updated to a different cxx
/// version that would require re-validating kuzu compatibility.
#[test]
fn cxx_version_is_kuzu_compatible() {
    let path = cargo_lock_path();
    let versions = parse_cargo_lock_versions(&path);

    let cxx_version = versions
        .get("cxx")
        .unwrap_or_else(|| panic!("cxx not found in Cargo.lock at {path:?}"));

    let parts: Vec<&str> = cxx_version.split('.').collect();
    assert_eq!(parts.len(), 3, "cxx version must be semver x.y.z");

    let major: u64 = parts[0].parse().expect("major must be numeric");
    // cxx versioning: patch carries the meaningful version identity.
    let patch: u64 = parts[2].parse().expect("patch must be numeric");

    assert_eq!(major, 1, "cxx major must be 1, got: {cxx_version}");
    assert_eq!(
        patch,
        138,
        "cxx patch version must be 138 (kuzu 0.11.3 requires cxx = '=1.0.138').\n\
        If cxx has been upgraded, re-validate kuzu compatibility and update this test.\n\
        See docs/concepts/cxx-version-contract.md for the version compatibility table."
    );
}
