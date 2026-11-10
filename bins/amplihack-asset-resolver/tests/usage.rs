//! TDD contracts for standalone resolver usage output.
//!
//! The binary should not maintain its own named-asset list. Its no-argument
//! usage text must reflect the canonical name helper in `amplihack-utils`.
//!
//! Issue #2025: the binary links `amplihack-utils` directly, not
//! `amplihack-cli`, so lbug, `amplihack-memory` and `amplihack-hive` stay out
//! of its link. This file can only import `amplihack_utils` once the manifest
//! names it, which is how the dependency change is pinned here.

use std::process::Command;

use amplihack_utils::resolve_bundle_asset;

#[test]
fn no_argument_usage_lists_canonical_named_assets() {
    let output = Command::new(env!("CARGO_BIN_EXE_amplihack-asset-resolver"))
        .output()
        .expect("run amplihack-asset-resolver without args");

    assert_eq!(output.status.code(), Some(2));

    let stderr = String::from_utf8(output.stderr).expect("usage stderr is utf-8");
    let canonical_names = resolve_bundle_asset::named_asset_names().join(", ");

    assert!(
        stderr.contains(&format!("    - a named asset: {canonical_names}")),
        "standalone resolver usage must derive supported names from amplihack-utils; stderr was:\n{stderr}"
    );
}
