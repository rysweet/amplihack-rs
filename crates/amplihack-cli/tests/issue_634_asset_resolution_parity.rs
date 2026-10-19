//! TDD regression contracts for issue #634.
//!
//! These tests define the helper-path and runtime asset-resolution parity
//! contract that the Rust port must preserve after PR #641 / v0.9.80.

use std::path::Path;

use amplihack_cli::{Cli, Commands, resolve_bundle_asset, runtime_assets};

use crate::support::quiet_process::{child_set_var, in_quiet_process};

#[derive(Clone, Copy)]
enum ExpectedKind {
    File,
    Dir,
}

#[derive(Clone, Copy)]
struct AssetCase {
    name: &'static str,
    rel_path: &'static str,
    kind: ExpectedKind,
}

const ISSUE_634_ASSETS: &[AssetCase] = &[
    AssetCase {
        name: "helper-path",
        rel_path: "amplifier-bundle/bin/multitask-orchestrator.sh",
        kind: ExpectedKind::File,
    },
    AssetCase {
        name: "multitask-orchestrator",
        rel_path: "amplifier-bundle/bin/multitask-orchestrator.sh",
        kind: ExpectedKind::File,
    },
    AssetCase {
        name: "hooks-dir",
        rel_path: "amplifier-bundle/tools/amplihack/hooks",
        kind: ExpectedKind::Dir,
    },
    AssetCase {
        name: "session-tree-path",
        rel_path: "amplifier-bundle/tools/amplihack/session",
        kind: ExpectedKind::Dir,
    },
];

/// A temp runtime root that HOME and AMPLIHACK_HOME point at. The variables
/// are set in-process because the code under test reads them there, so every
/// test that builds one runs in its own process (`in_quiet_process`) and the
/// shared `cli_integration` process never changes its environment.
struct IsolatedRuntimeRoot {
    temp: tempfile::TempDir,
}

impl IsolatedRuntimeRoot {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("create isolated runtime root");
        for case in ISSUE_634_ASSETS {
            let path = temp.path().join(case.rel_path);
            match case.kind {
                ExpectedKind::File => {
                    std::fs::create_dir_all(path.parent().expect("asset file has parent"))
                        .expect("create asset parent");
                    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write asset file");
                }
                ExpectedKind::Dir => {
                    std::fs::create_dir_all(&path).expect("create asset directory");
                }
            }
        }

        child_set_var("HOME", temp.path());
        child_set_var("AMPLIHACK_HOME", temp.path());
        Self { temp }
    }

    fn expected_path(&self, rel_path: &str) -> std::path::PathBuf {
        self.temp
            .path()
            .join(rel_path)
            .canonicalize()
            .expect("expected asset path canonicalizes")
    }
}

#[test]
fn issue_634_named_assets_resolve_to_expected_runtime_paths() {
    in_quiet_process(
        module_path!(),
        "issue_634_named_assets_resolve_to_expected_runtime_paths",
        || {
            let runtime = IsolatedRuntimeRoot::new();

            for case in ISSUE_634_ASSETS {
                let resolved = resolve_bundle_asset::resolve_named_asset(case.name)
                    .unwrap_or_else(|err| panic!("resolve {}: {err:#}", case.name));
                let expected = runtime.expected_path(case.rel_path);

                assert_eq!(
                    resolved, expected,
                    "{} must resolve to {}",
                    case.name, case.rel_path
                );
                match case.kind {
                    ExpectedKind::File => assert!(
                        resolved.is_file(),
                        "{} must resolve to a file: {}",
                        case.name,
                        resolved.display()
                    ),
                    ExpectedKind::Dir => assert!(
                        resolved.is_dir(),
                        "{} must resolve to a directory: {}",
                        case.name,
                        resolved.display()
                    ),
                }
            }
        },
    );
}

#[test]
fn issue_634_legacy_runtime_assets_module_matches_named_asset_contract() {
    in_quiet_process(
        module_path!(),
        "issue_634_legacy_runtime_assets_module_matches_named_asset_contract",
        || {
            let runtime = IsolatedRuntimeRoot::new();
            let roots = vec![runtime.temp.path().to_path_buf()];
            let asset_map = runtime_assets::asset_relative_paths();

            for case in ISSUE_634_ASSETS {
                assert_eq!(
                    asset_map.get(case.name).map(Vec::as_slice),
                    Some([case.rel_path].as_slice()),
                    "runtime_assets mapping for {} must match resolve-bundle-asset",
                    case.name
                );

                let resolved = runtime_assets::resolve_asset_path(case.name, &roots)
                    .unwrap_or_else(|err| panic!("runtime_assets resolve {}: {err:#}", case.name));
                assert_eq!(
                    resolved,
                    runtime.expected_path(case.rel_path),
                    "runtime_assets {} must resolve to the same compatibility path",
                    case.name
                );
            }
        },
    );
}

#[test]
fn issue_634_resolve_bundle_asset_cli_accepts_all_parity_asset_names() {
    in_quiet_process(
        module_path!(),
        "issue_634_resolve_bundle_asset_cli_accepts_all_parity_asset_names",
        || {
            let _runtime = IsolatedRuntimeRoot::new();

            for case in ISSUE_634_ASSETS {
                assert_eq!(
                    resolve_bundle_asset::run_cli(case.name),
                    0,
                    "resolve-bundle-asset {} must succeed from AMPLIHACK_HOME",
                    case.name
                );
            }
        },
    );
}

#[test]
fn issue_634_clap_dispatch_preserves_asset_argument_verbatim() {
    for case in ISSUE_634_ASSETS {
        let cli = Cli::try_parse_from(["amplihack", "resolve-bundle-asset", case.name])
            .unwrap_or_else(|err| panic!("parse {}: {err}", case.name));

        match cli.command {
            Commands::ResolveBundleAsset { asset } => assert_eq!(asset, case.name),
            other => panic!(
                "expected ResolveBundleAsset for {}, got {other:?}",
                case.name
            ),
        }
    }
}

#[test]
fn issue_634_unknown_asset_error_lists_supported_parity_asset_names() {
    let err = resolve_bundle_asset::resolve_named_asset("legacy-python-helper")
        .expect_err("unknown named asset must fail");
    let message = err.to_string();

    for case in ISSUE_634_ASSETS {
        assert!(
            message.contains(case.name),
            "unknown-asset error must mention supported asset `{}`; got: {message}",
            case.name
        );
    }
}

#[test]
fn issue_634_helper_path_does_not_resolve_to_removed_python_helper() {
    in_quiet_process(
        module_path!(),
        "issue_634_helper_path_does_not_resolve_to_removed_python_helper",
        || {
            let runtime = IsolatedRuntimeRoot::new();
            let resolved = resolve_bundle_asset::resolve_named_asset("helper-path")
                .expect("helper-path must resolve from isolated runtime root");

            assert_eq!(
                resolved,
                runtime.expected_path("amplifier-bundle/bin/multitask-orchestrator.sh")
            );
            assert!(
                !path_contains_component(&resolved, "orch_helper.py"),
                "helper-path must not regress to the removed Python orch_helper.py path"
            );
        },
    );
}

fn path_contains_component(path: &Path, component: &str) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy() == component)
}
