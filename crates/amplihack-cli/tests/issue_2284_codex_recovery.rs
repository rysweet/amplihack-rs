//! Issue #2284: frontdoor parsing and automatic startup recovery contracts.
#![cfg(unix)]
#[path = "issue_2284_codex_recovery/fixture.rs"]
pub(crate) mod fixture;
use fixture::*;
use std::fs;

#[test]
fn cli_help_describes_metadata_only_recovery_reset() {
    let f = LaunchFixture::new();
    let out = f.command().args(["codex", "--help"]).output().unwrap();
    assert!(out.status.success(), "{}", diagnostic(&out));
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(help.contains("--reset-recovery"), "{help}");
    assert!(
        help.to_lowercase().contains("journal") && help.to_lowercase().contains("preserv"),
        "{help}"
    );
}

#[test]
fn automatic_orphan_archive_reports_once_and_commits_a_new_install() {
    let f = LaunchFixture::new();
    let bytes = f.orphan();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = f
        .command()
        .args(["install", "--local", source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", diagnostic(&out));
    f.assert_report(&out, &bytes);
    let ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.join("ownership.json")).unwrap()).unwrap();
    assert_ne!(ledger["transaction"], "abandoned");
    assert_eq!(ledger["codex_home"], serde_json::json!(f.codex));
    assert!(f.root.join("market/plugin/bin/hook").is_file());
    assert!(f.codex.join("hooks.json").is_file());
    assert!(f.home.join("registered").is_file());
    assert!(!f.home.join(".codex").exists());
}
