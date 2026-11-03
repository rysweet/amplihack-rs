//! Read-only repository contracts. No registry cache or user profile is read.
use crate::support::digest;
use serde_json::Value;
use std::{fs, path::PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

#[test]
fn recorded_unmodified_source_files_match_the_upstream_inventory() {
    let root = root();
    let inventory: Value = serde_json::from_slice(
        &fs::read(root.join("build-support/lbug-cache/provenance/upstream-files.json"))
            .expect("RED: upstream file inventory is missing"),
    )
    .unwrap();
    let files = inventory["files"].as_array().expect("files inventory");
    assert!(files.len() > 100, "inventory must cover the complete crate");
    for file in files {
        let relative = file["path"].as_str().unwrap();
        assert!(!relative.starts_with('/') && !relative.split('/').any(|c| c == ".."));
        if ["Cargo.toml", "build.rs"].contains(&relative) {
            continue; // Exact reviewed delta is enforced below and by vendor guard.
        }
        let bytes = fs::read(root.join("vendor/lbug").join(relative)).unwrap();
        assert_eq!(
            digest(&bytes),
            file["sha256"].as_str().unwrap(),
            "{relative}"
        );
    }
}

#[test]
fn pinned_registry_digest_and_reviewed_patch_have_narrow_provenance() {
    let root = root();
    let provenance: Value = serde_json::from_slice(
        &fs::read(root.join("build-support/lbug-cache/provenance/lbug-0.15.4.json"))
            .expect("RED: pinned source provenance is missing"),
    )
    .unwrap();
    assert_eq!(
        provenance["registry_archive_sha256"],
        "7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f"
    );
    let patch = fs::read_to_string(root.join("build-support/lbug-cache/patches/lbug-0.15.4.patch"))
        .unwrap();
    let changed: Vec<_> = patch
        .lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .collect();
    assert!(!changed.is_empty());
    for path in changed {
        assert!(
            [
                "Cargo.toml",
                "build.rs",
                "vendor/lbug/Cargo.toml",
                "vendor/lbug/build.rs"
            ]
            .contains(&path),
            "unapproved upstream delta: {path}"
        );
    }
    let build_script = fs::read_to_string(root.join("crates/amplihack-cli/build.rs")).unwrap();
    let code: String = build_script
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    assert_eq!(
        code.trim(),
        "fn main() {}",
        "reserved application build script must stay a no-op"
    );
}
