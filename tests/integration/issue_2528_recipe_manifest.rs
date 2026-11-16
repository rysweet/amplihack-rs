//! Issue #2528 (S3/M3): `amplifier-bundle/recipes/_recipe_manifest.json` is a
//! map from recipe stem to the first 16 hexadecimal characters of the
//! SHA-256 of `amplifier-bundle/recipes/<stem>.yaml`.
//!
//! The manifest is not an integrity control. `bundle_compat` reads it only to
//! check that the file is a JSON object and that each required recipe has a
//! non-empty string; nothing in the shipped code compares an entry against
//! the bytes of the recipe it names. These tests are the only place that
//! comparison happens, so an entry left behind by an edited recipe is a
//! silent inconsistency everywhere else.
//!
//! `issue_1966_consensus_criticality::manifest_hashes_match_edited_recipes`
//! already covers the seven recipes that change edited. That list has to be
//! grown by hand, and a merge that edits a recipe nobody listed leaves its
//! entry stale unnoticed: at the time this file was written `origin/main`
//! carried 27 such entries, one of them (`code-philosophy-audit`) a value no
//! `sha256sum` ever produced. These tests cover every entry instead of a
//! list, so no edit can outrun them.
//!
//! Regenerate the whole manifest with, from the repository root:
//!
//! ```text
//! for f in amplifier-bundle/recipes/*.yaml; do
//!     printf '%s %s\n' "$(basename "$f" .yaml)" \
//!         "$(sha256sum "$f" | cut -c1-16)"
//! done
//! ```
//!
//! and write each pair into the JSON object, keeping its key order.
//!
//! Not every recipe needs an entry: `recipes/` holds more `*.yaml` files than
//! the manifest has keys, and an un-keyed recipe is not a fault. These tests
//! therefore constrain the entries that exist, never the set of files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MANIFEST: &str = "amplifier-bundle/recipes/_recipe_manifest.json";

/// How to put a stale manifest right; printed with every failure.
const REGENERATE: &str = "regenerate the entry with: \
     sha256sum amplifier-bundle/recipes/<stem>.yaml | cut -c1-16";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

/// The manifest, as a sorted map so failures read in a stable order.
fn manifest() -> BTreeMap<String, String> {
    let path = repo_root().join(MANIFEST);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not a JSON object of strings: {error}", MANIFEST))
}

fn recipe_path(stem: &str) -> PathBuf {
    repo_root()
        .join("amplifier-bundle/recipes")
        .join(format!("{stem}.yaml"))
}

/// The first 16 characters of `sha256sum <path>`, through `isolated(..)` like
/// every other command these tests run.
fn sha256_prefix(path: &Path) -> String {
    let mut command = crate::isolated_amplihack::isolated("sha256sum");
    command.arg(path);
    let out = command.output().expect("run sha256sum");
    assert!(
        out.status.success(),
        "sha256sum {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).expect("sha256sum prints ASCII");
    let digest = text
        .split_whitespace()
        .next()
        .expect("sha256sum prints a digest");
    assert_eq!(
        digest.len(),
        64,
        "sha256sum printed {digest:?} for {}",
        path.display()
    );
    digest[..16].to_owned()
}

/// The enforcing test: every entry, not a hand-kept list of names.
///
/// Every mismatch is collected and reported together, so one run names the
/// whole set a merge has to put right rather than the first of them.
#[test]
fn every_manifest_entry_matches_the_sha256_prefix_of_its_recipe() {
    let manifest = manifest();
    assert!(
        !manifest.is_empty(),
        "{MANIFEST} has no entries; these tests would pass vacuously"
    );
    let mut stale = Vec::new();
    for (stem, entry) in &manifest {
        let path = recipe_path(stem);
        if !path.is_file() {
            // Reported by the companion test; skipped here so one missing
            // file does not hide every genuine mismatch behind a panic.
            continue;
        }
        let actual = sha256_prefix(&path);
        if *entry != actual {
            stale.push(format!("  {stem}: manifest {entry}, recipe {actual}"));
        }
    }
    assert!(
        stale.is_empty(),
        "{} of {} {MANIFEST} entries do not match their recipe:\n{}\n{REGENERATE}",
        stale.len(),
        manifest.len(),
        stale.join("\n")
    );
}

/// An entry for a recipe that no longer exists can never be checked against
/// anything, so it must not be left behind by a rename or a removal.
#[test]
fn every_manifest_entry_names_a_recipe_that_exists() {
    let manifest = manifest();
    let missing: Vec<&String> = manifest
        .keys()
        .filter(|stem| !recipe_path(stem).is_file())
        .collect();
    assert!(
        missing.is_empty(),
        "{MANIFEST} keys name recipes that are not in amplifier-bundle/recipes: {missing:?}; \
         drop the entry or restore the recipe"
    );
}

/// The shape a regenerated entry has. This does not catch a hand-typed value
/// that happens to look like a digest; only the comparison above does that.
#[test]
fn every_manifest_value_is_sixteen_lowercase_hex_characters() {
    let manifest = manifest();
    let malformed: Vec<String> = manifest
        .iter()
        .filter(|(_, value)| {
            value.len() != 16
                || !value
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
        .map(|(stem, value)| format!("{stem}: {value:?}"))
        .collect();
    assert!(
        malformed.is_empty(),
        "{MANIFEST} values must be 16 lowercase hexadecimal characters: {}\n{REGENERATE}",
        malformed.join(", ")
    );
}
