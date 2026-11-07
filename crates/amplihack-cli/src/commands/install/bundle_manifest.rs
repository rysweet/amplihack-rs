//! Compare a staged bundle with its source (issue #1955).
//!
//! This shows that staging matches its source, not that the source is authentic.

use super::filesystem::{WalkEntryKind, WalkScope, walk_tree};
use super::paths::{sanitize_path, sanitize_text};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::path::Path;

/// Paths listed per category in a mismatch message (SEC-14).
const MAX_LISTED_PATHS: usize = 20;

/// Relative path (`/`-separated) to lowercase hex SHA-256 of each regular file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct BundleManifest {
    pub(super) files: BTreeMap<String, String>,
}

/// What `compare` found, each list sorted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ManifestDiff {
    pub(super) missing: Vec<String>,
    pub(super) extra: Vec<String>,
    pub(super) changed: Vec<String>,
}

impl ManifestDiff {
    pub(super) fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.changed.is_empty()
    }
}

/// Files the copy would take from a source bundle (`WalkScope::CopySet`).
pub(super) fn of_source(bundle: &Path) -> Result<BundleManifest> {
    build(bundle, WalkScope::CopySet)
}

/// Every regular file actually present in staging (`WalkScope::Everything`), so
/// a file with an excluded name still counts as extra.
pub(super) fn of_staged(bundle: &Path) -> Result<BundleManifest> {
    build(bundle, WalkScope::Everything)
}

fn build(bundle: &Path, scope: WalkScope) -> Result<BundleManifest> {
    let mut files = BTreeMap::new();
    for entry in walk_tree(bundle, scope)? {
        if entry.kind != WalkEntryKind::File {
            continue;
        }
        let rel = entry
            .rel
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        files.insert(rel, sha256_of_file(&entry.path)?);
    }
    Ok(BundleManifest { files })
}

fn sha256_of_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

/// Pure comparison.
pub(super) fn compare(source: &BundleManifest, staged: &BundleManifest) -> ManifestDiff {
    let mut diff = ManifestDiff::default();
    for (path, hash) in &source.files {
        match staged.files.get(path) {
            None => diff.missing.push(path.clone()),
            Some(other) if other != hash => diff.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    diff.extra = staged
        .files
        .keys()
        .filter(|path| !source.files.contains_key(*path))
        .cloned()
        .collect();
    diff
}

/// The user-facing failure text for a non-clean diff.
pub(super) fn render_mismatch(source: &Path, diff: &ManifestDiff) -> String {
    let mut text = format!(
        "the staged bundle does not match its source {} ({} missing, {} extra, {} changed):",
        sanitize_path(source),
        diff.missing.len(),
        diff.extra.len(),
        diff.changed.len()
    );
    for (label, paths) in [
        ("missing", &diff.missing),
        ("extra", &diff.extra),
        ("changed", &diff.changed),
    ] {
        for path in paths.iter().take(MAX_LISTED_PATHS) {
            let _ = write!(text, "\n  {label}: {}", sanitize_text(path));
        }
        if paths.len() > MAX_LISTED_PATHS {
            let _ = write!(
                text,
                "\n  {label}: and {} more",
                paths.len() - MAX_LISTED_PATHS
            );
        }
    }
    text
}
