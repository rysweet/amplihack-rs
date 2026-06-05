//! Release/version contract tests: the root Cargo workspace version is the
//! single source of truth for crates, npm metadata, and release automation.

use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

fn read_workspace_file(path: &str) -> String {
    let full = workspace_root().join(path);
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("read {}: {e}", full.display()))
}

fn workspace_version() -> String {
    let cargo = read_workspace_file("Cargo.toml");
    let re = Regex::new(r#"(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*"([^"]+)""#)
        .expect("valid regex");
    re.captures(&cargo)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
        .expect("root Cargo.toml must define [workspace.package] version")
}

fn workspace_members() -> Vec<String> {
    let cargo = read_workspace_file("Cargo.toml");
    let re =
        Regex::new(r#"(?ms)^\[workspace\]\s*.*?members\s*=\s*\[(.*?)\]"#).expect("valid regex");
    let members = re
        .captures(&cargo)
        .and_then(|captures| captures.get(1))
        .map(|m| m.as_str())
        .expect("root Cargo.toml must define workspace members");
    Regex::new(r#""([^"]+)""#)
        .expect("valid regex")
        .captures_iter(members)
        .map(|captures| captures[1].to_string())
        .collect()
}

#[test]
fn workspace_crates_inherit_or_match_root_version() {
    let version = workspace_version();
    for member in workspace_members() {
        let manifest_path = workspace_root().join(&member).join("Cargo.toml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));
        assert!(
            manifest.contains("version.workspace = true")
                || manifest.contains(&format!(r#"version = "{version}""#)),
            "{} must inherit workspace version or match root version {version}",
            manifest_path.display()
        );
    }
}

#[test]
fn package_json_version_matches_root_workspace_version() {
    let version = workspace_version();
    let package: Value =
        serde_json::from_str(&read_workspace_file("package.json")).expect("package.json parses");
    let package_version = package
        .get("version")
        .and_then(Value::as_str)
        .expect("package.json must declare version");
    assert_eq!(
        package_version, version,
        "package.json version must match [workspace.package].version"
    );
}

#[test]
fn release_workflow_reads_workspace_package_version_without_drift_prone_grep() {
    let release = read_workspace_file(".github/workflows/release.yml");
    assert!(
        release.contains("[workspace.package]")
            || release.contains("cargo metadata")
            || release.contains("workspace.package.version"),
        "release workflow must explicitly derive the release version from Cargo workspace package metadata"
    );
    assert!(
        !release.contains("grep '^version' Cargo.toml"),
        "release workflow must not use drift-prone unscoped grep of Cargo.toml version"
    );
}
