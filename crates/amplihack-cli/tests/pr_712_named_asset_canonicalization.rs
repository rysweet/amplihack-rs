//! TDD contracts for PR #712 named asset mapping centralization.
//!
//! These tests protect the canonical helper API and ensure runtime consumers
//! delegate to the ordered mapping helpers instead of duplicating asset names.

use amplihack_cli::{resolve_bundle_asset, runtime_assets};

const EXPECTED_NAMES: &[&str] = &[
    "hooks-dir",
    "helper-path",
    "session-tree-path",
    "multitask-orchestrator",
];

const EXPECTED_MAPPINGS: &[(&str, &[&str])] = &[
    ("hooks-dir", &["amplifier-bundle/tools/amplihack/hooks"]),
    (
        "helper-path",
        &["amplifier-bundle/bin/multitask-orchestrator.sh"],
    ),
    (
        "session-tree-path",
        &["amplifier-bundle/tools/amplihack/session"],
    ),
    (
        "multitask-orchestrator",
        &["amplifier-bundle/bin/multitask-orchestrator.sh"],
    ),
];

#[test]
fn canonical_named_asset_table_is_ordered_complete_and_read_only() {
    assert_eq!(
        resolve_bundle_asset::named_asset_relative_paths(),
        EXPECTED_MAPPINGS,
        "canonical named asset table must be the single ordered source of truth"
    );
}

#[test]
fn canonical_named_asset_names_are_derived_in_table_order() {
    assert_eq!(
        resolve_bundle_asset::named_asset_names(),
        EXPECTED_NAMES,
        "name listing must be derived from canonical mapping order"
    );
}

#[test]
fn runtime_assets_named_mapping_matches_canonical_table_exactly() {
    let runtime_map = runtime_assets::asset_relative_paths();
    let canonical = resolve_bundle_asset::named_asset_relative_paths();

    assert_eq!(
        runtime_map.len(),
        canonical.len(),
        "runtime_assets must not add or omit named assets"
    );

    for (name, relative_paths) in canonical {
        assert_eq!(
            runtime_map.get(name).map(Vec::as_slice),
            Some(relative_paths),
            "runtime_assets mapping for {name} must delegate to the canonical table"
        );
    }
}

#[test]
fn session_tree_path_is_covered_by_every_named_asset_surface() {
    assert!(
        resolve_bundle_asset::named_asset_names().contains(&"session-tree-path"),
        "session-tree-path must remain in generated supported-name lists"
    );
    assert_eq!(
        resolve_bundle_asset::named_asset_relative_paths()
            .iter()
            .find(|(name, _)| *name == "session-tree-path")
            .map(|(_, paths)| *paths),
        Some(["amplifier-bundle/tools/amplihack/session"].as_slice()),
        "session-tree-path must resolve to the legacy session anchor"
    );
    assert_eq!(
        runtime_assets::asset_relative_paths()
            .get("session-tree-path")
            .map(Vec::as_slice),
        Some(["amplifier-bundle/tools/amplihack/session"].as_slice()),
        "runtime compatibility lookup must include session-tree-path"
    );
}

#[test]
fn unknown_named_asset_error_uses_canonical_name_order() {
    let message = resolve_bundle_asset::resolve_named_asset("legacy-python-helper")
        .expect_err("unknown named asset must fail")
        .to_string();

    assert_eq!(
        message,
        format!(
            "Unknown asset name \"legacy-python-helper\". Expected one of: {}",
            EXPECTED_NAMES.join(", ")
        ),
        "unknown-name diagnostics must be generated from canonical names"
    );
    assert_eq!(
        resolve_bundle_asset::run_cli("legacy-python-helper"),
        1,
        "single-token unknown asset names must remain a not-found condition"
    );
}

#[test]
fn docs_named_asset_table_matches_canonical_names_and_paths() {
    let docs = include_str!("../../../docs/reference/resolve-bundle-asset-command.md");

    for (name, relative_paths) in resolve_bundle_asset::named_asset_relative_paths() {
        for relative_path in relative_paths {
            assert!(
                docs.contains(&format!("| `{name}` | `{relative_path}` |")),
                "docs must list canonical mapping {name} -> {relative_path}"
            );
        }
    }
}

#[test]
fn named_asset_lookup_remains_exact_static_name_only() {
    for invalid_name in [
        " helper-path",
        "helper-path ",
        "HELPER-PATH",
        "helper-path/..",
        "../helper-path",
        "$helper-path",
        "/helper-path",
    ] {
        let message = resolve_bundle_asset::resolve_named_asset(invalid_name)
            .expect_err("invalid named asset form must not resolve")
            .to_string();
        assert!(
            message.starts_with("Unknown asset name"),
            "{invalid_name:?} must be rejected as an unknown exact name, got: {message}"
        );
    }
}
