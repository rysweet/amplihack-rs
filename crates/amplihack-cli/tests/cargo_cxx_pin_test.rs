//! WS4: Compile-time verification that cxx-build is pinned to an exact version
//! in the workspace Cargo.toml and that amplihack-cli declares it as a
//! build-dependency.
//!
//! These tests read the real Cargo.toml files from the repository tree and
//! assert the expected content.  They will FAIL (red) until:
//!   1. `[workspace.dependencies]` in the root Cargo.toml contains
//!      `cxx-build = "=1.0.138"`
//!   2. `crates/amplihack-cli/Cargo.toml` has a `[build-dependencies]`
//!      section referencing `cxx-build`
//!   3. `crates/amplihack-cli/build.rs` exists and is a no-op `fn main() {}`

use std::fs;
use std::path::PathBuf;

/// Resolve the workspace root by walking up from the manifest dir of this
/// test crate until we find a Cargo.toml that declares `[workspace]`.
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is set by Cargo for integration tests and points to
    // the directory containing the *test crate's* Cargo.toml.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // amplihack-cli is two levels below the workspace root:
    // <root>/crates/amplihack-cli  →  parent = crates  →  parent = root
    manifest_dir
        .parent()
        .expect("crates dir")
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

/// WS4-1: The root Cargo.toml must pin cxx-build to exactly 1.0.138 in
/// `[workspace.dependencies]`.
#[test]
fn cxx_build_is_pinned_in_workspace() {
    let root_toml = workspace_root().join("Cargo.toml");
    let content = fs::read_to_string(&root_toml)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", root_toml.display()));

    assert!(
        content.contains("cxx-build = \"=1.0.138\""),
        "Root Cargo.toml must contain `cxx-build = \"=1.0.138\"` in \
         [workspace.dependencies].\n\nActual content of {}:\n{}",
        root_toml.display(),
        content
    );
}

/// WS4-2: `crates/amplihack-cli/Cargo.toml` must declare cxx-build under
/// `[build-dependencies]`.
#[test]
fn amplihack_cli_references_cxx_build_as_build_dependency() {
    let cli_toml = workspace_root()
        .join("crates")
        .join("amplihack-cli")
        .join("Cargo.toml");
    let content = fs::read_to_string(&cli_toml)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", cli_toml.display()));

    assert!(
        content.contains("[build-dependencies]"),
        "crates/amplihack-cli/Cargo.toml must contain a [build-dependencies] section.\n\
         Actual content:\n{content}"
    );
    assert!(
        content.contains("cxx-build"),
        "crates/amplihack-cli/Cargo.toml [build-dependencies] must reference cxx-build.\n\
         Actual content:\n{content}"
    );
}

/// WS4-3: `crates/amplihack-cli/build.rs` must exist and must be a no-op stub
/// containing only `fn main()`.
#[test]
fn amplihack_cli_build_rs_exists_and_is_noop() {
    let build_rs = workspace_root()
        .join("crates")
        .join("amplihack-cli")
        .join("build.rs");

    assert!(
        build_rs.exists(),
        "crates/amplihack-cli/build.rs must exist (no-op stub required by WS4)"
    );

    let content = fs::read_to_string(&build_rs)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", build_rs.display()));

    assert!(
        content.contains("fn main()"),
        "crates/amplihack-cli/build.rs must contain `fn main()`.\n\
         Actual content:\n{content}"
    );
}
