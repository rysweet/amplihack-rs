//! TDD contracts for standalone resolver usage output.
//!
//! The binary should not maintain its own named-asset list. Its no-argument
//! usage text must reflect the CLI library's canonical name helper.

use std::process::Command;

use amplihack_cli::resolve_bundle_asset;

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
        "standalone resolver usage must derive supported names from the CLI library; stderr was:\n{stderr}"
    );
}
