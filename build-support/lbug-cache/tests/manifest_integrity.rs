//! Manifest tests cover inventory/hash/provenance parsing only. Tiny archive
//! fixtures do not claim ABI compatibility or substitute for native smoke tests.
use crate::support::{KEY, digest};
use lbug_cache::artifact::{validate_payload_inventory, verify_expected_provenance};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

const ARCHIVES: &[&str] = &[
    "liblbug.a",
    "libutf8proc.a",
    "libantlr4_cypher.a",
    "libantlr4_runtime.a",
    "libre2.a",
    "libfastpfor.a",
    "libparquet.a",
    "libthrift.a",
    "libsnappy.a",
    "libzstd.a",
    "libminiz.a",
    "libmbedtls.a",
    "libbrotlidec.a",
    "libbrotlicommon.a",
    "liblz4.a",
    "libroaring_bitmap.a",
    "libsimsimd.a",
    "libyyjson.a",
];

fn fixture(root: &Path) -> Value {
    let mut files = Vec::new();
    for (name, bytes, role) in ARCHIVES
        .iter()
        .map(|name| {
            (
                format!("lib/{name}"),
                b"!<arch>\n".as_slice(),
                "static_archive",
            )
        })
        .chain([
            (
                "include/lbug.hpp".into(),
                b"// header\n".as_slice(),
                "header",
            ),
            (
                "licenses/LICENSE".into(),
                b"fixture license\n".as_slice(),
                "license",
            ),
            (
                "provenance/source.json".into(),
                b"{}\n".as_slice(),
                "provenance",
            ),
        ])
    {
        let path = root.join(&name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        files.push(
            json!({"path": name, "role": role, "size": bytes.len(), "sha256": digest(bytes)}),
        );
    }
    json!({"schema_version": 1, "key": KEY, "files": files})
}

#[test]
fn inventory_requires_all_eighteen_archives_and_revalidates_extracted_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = fixture(dir.path());
    validate_payload_inventory(&manifest.to_string(), dir.path()).unwrap();
    for name in ARCHIVES {
        let path = dir.path().join("lib").join(name);
        let original = fs::read(&path).unwrap();
        fs::write(&path, b"replaced after extraction").unwrap();
        assert!(
            validate_payload_inventory(&manifest.to_string(), dir.path()).is_err(),
            "{name}"
        );
        fs::write(path, original).unwrap();
    }
    fs::remove_file(dir.path().join("lib/libyyjson.a")).unwrap();
    assert!(validate_payload_inventory(&manifest.to_string(), dir.path()).is_err());
}

#[test]
fn manifest_rejects_duplicate_keys_paths_extra_files_and_unsupported_schema() {
    let dir = tempfile::tempdir().unwrap();
    let valid = fixture(dir.path());
    for invalid in [
        format!(r#"{{"schema_version":1,"schema_version":1,"key":"{KEY}","files":[]}}"#),
        {
            let mut m = valid.clone();
            m["schema_version"] = json!(2);
            m.to_string()
        },
        {
            let mut m = valid.clone();
            let first = m["files"][0].clone();
            m["files"].as_array_mut().unwrap().push(first);
            m.to_string()
        },
    ] {
        assert!(validate_payload_inventory(&invalid, dir.path()).is_err());
    }
    fs::write(
        dir.path().join("lib/liblbug_rs.a"),
        b"FFI shim must not ship",
    )
    .unwrap();
    assert!(validate_payload_inventory(&valid.to_string(), dir.path()).is_err());
}

#[test]
fn a_rehashed_thin_archive_is_still_forbidden() {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest = fixture(dir.path());
    let thin = b"!<thin>\n";
    fs::write(dir.path().join("lib/liblbug.a"), thin).unwrap();
    manifest["files"][0]["sha256"] = json!(digest(thin));
    manifest["files"][0]["size"] = json!(thin.len());
    assert!(validate_payload_inventory(&manifest.to_string(), dir.path()).is_err());
}

#[test]
fn header_closure_and_include_root_conflicts_are_checked_without_producer_paths() {
    use lbug_cache::artifact::{merge_include_roots, validate_header_closure};
    let dir = tempfile::tempdir().unwrap();
    let include = dir.path().join("include");
    fs::create_dir_all(include.join("nested")).unwrap();
    fs::write(include.join("lbug.hpp"), b"#include \"nested/value.h\"\n").unwrap();
    assert!(validate_header_closure(&include, &["lbug.hpp"]).is_err());
    fs::write(include.join("nested/value.h"), b"#pragma once\n").unwrap();
    validate_header_closure(&include, &["lbug.hpp"]).unwrap();
    let other = dir.path().join("other");
    fs::create_dir_all(other.join("nested")).unwrap();
    fs::write(other.join("nested/value.h"), b"#pragma once\n").unwrap();
    let merged = dir.path().join("merged");
    merge_include_roots(&[include.clone(), other.clone()], &merged).unwrap();
    fs::write(other.join("nested/value.h"), b"conflicting definition\n").unwrap();
    assert!(merge_include_roots(&[include, other], &dir.path().join("conflict")).is_err());
}

#[test]
fn matching_attacker_archive_and_checksum_cannot_replace_expected_provenance() {
    let expected = BTreeMap::from([
        (
            "upstream_archive_sha256".into(),
            "7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f".into(),
        ),
        ("integration_patch_sha256".into(), "a".repeat(64)),
        ("header_recipe_sha256".into(), "b".repeat(64)),
        ("compatibility_policy_sha256".into(), "c".repeat(64)),
        ("upstream_files_sha256".into(), "d".repeat(64)),
    ]);
    verify_expected_provenance(&expected, &expected).unwrap();
    for field in expected.keys() {
        let mut forged = expected.clone();
        forged.insert(field.clone(), "0".repeat(64));
        assert!(
            verify_expected_provenance(&forged, &expected).is_err(),
            "{field}"
        );
        forged.remove(field);
        assert!(
            verify_expected_provenance(&forged, &expected).is_err(),
            "missing {field}"
        );
    }
    assert!(verify_expected_provenance(&BTreeMap::new(), &BTreeMap::new()).is_err());
}
