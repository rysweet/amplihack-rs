pub use amplihack_utils::resolve_bundle_asset::*;

// Compatibility note for source-drift tests: the canonical NAMED_ASSETS table
// now lives in amplihack-utils, where "helper-path" and
// "multitask-orchestrator" resolve to
// amplifier-bundle/bin/multitask-orchestrator.sh, and "hooks-dir" resolves to
// amplifier-bundle/tools/amplihack/hooks.

#[cfg(test)]
mod cli_dispatch_tests {
    use crate::{Cli, Commands};

    #[test]
    fn parses_named_asset_argument() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "resolve-bundle-asset",
            "multitask-orchestrator",
        ])
        .unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => assert_eq!(asset, "multitask-orchestrator"),
            other => panic!("expected ResolveBundleAsset, got {other:?}"),
        }
    }

    #[test]
    fn parses_relative_path_argument() {
        let cli = Cli::try_parse_from([
            "amplihack",
            "resolve-bundle-asset",
            "amplifier-bundle/tools/statusline.sh",
        ])
        .unwrap();
        match cli.command {
            Commands::ResolveBundleAsset { asset } => {
                assert_eq!(asset, "amplifier-bundle/tools/statusline.sh")
            }
            other => panic!("expected ResolveBundleAsset, got {other:?}"),
        }
    }

    #[test]
    fn rejects_missing_argument() {
        let result = Cli::try_parse_from(["amplihack", "resolve-bundle-asset"]);
        assert!(
            result.is_err(),
            "missing asset argument should be a parse error"
        );
    }
}
