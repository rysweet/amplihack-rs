//! Stat-fingerprint cache for the installed framework-bundle compatibility check.
//!
//! Issue #1271: [`super::bundle_compat::validate_framework_bundle_compatibility`]
//! reads and fully parses five recipe YAML files (~58 KB on a stock install)
//! plus the JSON recipe manifest. `self_heal::run` needs its verdict on
//! *every* `amplihack` invocation, so that parse was paid on every launch,
//! even though the inputs change only when the bundle is re-staged.
//!
//! This module memoises the verdict in `~/.amplihack/.bundle-compat-cache.json`,
//! keyed on:
//!
//! - the running binary version (validation *rules* change with the binary), and
//! - a stat fingerprint of every path the validator looks at — file kind,
//!   length, and mtime to nanosecond resolution.
//!
//! Building the fingerprint is stat-only. Measured on a stock bundle, the
//! warm path (fingerprint + cache read) costs **0.013 ms** against
//! **0.837 ms** for the parse it replaces — 64x cheaper. Any change to any
//! input file (content, size, timestamp, a file appearing or disappearing, a
//! path becoming a symlink) changes the fingerprint and forces a full
//! revalidation.
//!
//! The cache is an optimisation, never an authority: if it cannot be read or
//! written the validator simply runs, so behaviour is identical either way.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::bundle_compat::{REQUIRED_SMART_RECIPES, validate_framework_bundle_compatibility};

/// Filename of the cache inside `~/.amplihack/`.
pub(crate) const CACHE_FILE: &str = ".bundle-compat-cache.json";

/// Sibling tempfile used for atomic writes.
const CACHE_TMP: &str = ".bundle-compat-cache.json.tmp";

/// Bumped whenever the on-disk cache shape changes; an older entry is
/// treated as a miss rather than misinterpreted.
const CACHE_FORMAT_VERSION: u32 = 1;

/// Files the validator reads, relative to a recipes directory.
fn validated_file_names() -> Vec<String> {
    let mut names = vec![
        "smart-orchestrator.yaml".to_string(),
        "_recipe_manifest.json".to_string(),
    ];
    names.extend(REQUIRED_SMART_RECIPES.iter().map(|r| format!("{r}.yaml")));
    names.sort();
    names
}

/// Every path whose state can change the validator's verdict.
///
/// Both candidate bundle roots are covered because
/// `validate_framework_bundle_compatibility` accepts either a bundle
/// directory or its parent; which one it resolves to is itself an input.
fn fingerprint_paths(root_or_bundle: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        root_or_bundle.to_path_buf(),
        root_or_bundle.join("recipes"),
        root_or_bundle.join("amplifier-bundle"),
        root_or_bundle.join("amplifier-bundle").join("recipes"),
    ];
    for recipes in [
        root_or_bundle.join("recipes"),
        root_or_bundle.join("amplifier-bundle").join("recipes"),
    ] {
        for name in validated_file_names() {
            paths.push(recipes.join(name));
        }
    }
    paths
}

/// One `lstat` rendered as a comparable string, or `"absent"`.
fn stat_entry(path: &Path) -> String {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return "absent".to_string();
    };
    let kind = if metadata.file_type().is_symlink() {
        "symlink"
    } else if metadata.is_dir() {
        "dir"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{kind}:{}:{}:{}:{}",
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ino()
        )
    }
    #[cfg(not(unix))]
    {
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{kind}:{}:{mtime}", metadata.len())
    }
}

/// Stat-only fingerprint of every input the validator reads.
pub(crate) fn fingerprint(root_or_bundle: &Path) -> String {
    let mut out = String::new();
    for path in fingerprint_paths(root_or_bundle) {
        out.push_str(&path.display().to_string());
        out.push('=');
        out.push_str(&stat_entry(&path));
        out.push('\n');
    }
    out
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    format_version: u32,
    binary_version: String,
    fingerprint: String,
    /// `None` = bundle was compatible; `Some(msg)` = the validator's message.
    issue: Option<String>,
}

/// Compatibility verdict for `root_or_bundle`, reusing `cache_path` when the
/// inputs are byte-for-byte unchanged since the last full validation.
///
/// Returns `None` when the bundle is compatible and `Some(message)` with the
/// validator's error text when it is not — exactly what a direct call to
/// [`validate_framework_bundle_compatibility`] yields.
pub(crate) fn compatibility_issue_cached(
    root_or_bundle: &Path,
    cache_path: &Path,
) -> Option<String> {
    let current = fingerprint(root_or_bundle);

    if let Some(entry) = read_cache(cache_path)
        && entry.format_version == CACHE_FORMAT_VERSION
        && entry.binary_version == crate::VERSION
        && entry.fingerprint == current
    {
        tracing::debug!("bundle_compat: cache hit; skipping recipe YAML parse");
        return entry.issue;
    }

    tracing::debug!("bundle_compat: cache miss; validating installed bundle");
    let issue = compatibility_issue_uncached(root_or_bundle);
    write_cache(
        cache_path,
        &CacheEntry {
            format_version: CACHE_FORMAT_VERSION,
            binary_version: crate::VERSION.to_string(),
            fingerprint: current,
            issue: issue.clone(),
        },
    );
    issue
}

/// Compatibility verdict computed from scratch, ignoring any cache.
pub(crate) fn compatibility_issue_uncached(root_or_bundle: &Path) -> Option<String> {
    match validate_framework_bundle_compatibility(root_or_bundle) {
        Ok(()) => None,
        Err(err) => Some(err.to_string()),
    }
}

/// Drop the cache so the next invocation revalidates from scratch.
///
/// Called after a re-stage: the bundle just changed under us, and the
/// fingerprint we would write here could race the installer's own writes.
pub(crate) fn invalidate(cache_path: &Path) {
    match fs::remove_file(cache_path) {
        Ok(()) => tracing::debug!("bundle_compat: cache invalidated after re-stage"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::debug!("bundle_compat: could not invalidate cache: {err}"),
    }
}

fn read_cache(cache_path: &Path) -> Option<CacheEntry> {
    let raw = fs::read_to_string(cache_path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Best-effort atomic cache write. Failure only costs a future cache miss,
/// so it is logged at debug and never propagated.
fn write_cache(cache_path: &Path, entry: &CacheEntry) {
    let Some(parent) = cache_path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(encoded) = serde_json::to_string(entry) else {
        return;
    };
    let tmp = parent.join(CACHE_TMP);
    if let Err(err) = fs::write(&tmp, encoded) {
        tracing::debug!("bundle_compat: could not write cache tempfile: {err}");
        return;
    }
    restrict_permissions(&tmp);
    if let Err(err) = fs::rename(&tmp, cache_path) {
        tracing::debug!("bundle_compat: could not install cache file: {err}");
        let _ = fs::remove_file(&tmp);
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::install::bundle_compat::read_counter;

    /// Number of bundle files a full validation of a healthy bundle reads:
    /// smart-orchestrator + four companions + the recipe manifest.
    const FULL_VALIDATION_READS: usize = 6;

    fn write_compatible_bundle(bundle: &Path) {
        let recipes = bundle.join("recipes");
        fs::create_dir_all(&recipes).unwrap();
        fs::write(recipes.join("smart-orchestrator.yaml"), compatible_smart()).unwrap();
        for recipe in REQUIRED_SMART_RECIPES {
            fs::write(
                recipes.join(format!("{recipe}.yaml")),
                format!(
                    "name: \"{recipe}\"\nsteps:\n  - id: smoke\n    type: bash\n    command: 'true'\n"
                ),
            )
            .unwrap();
        }
        fs::write(
            recipes.join("_recipe_manifest.json"),
            r#"{
  "smart-classify-route": "250c8da0ee348745",
  "smart-execute-routing": "11612506ae846a47",
  "smart-orchestrator": "8d55ee4817dbc815",
  "smart-reflect-loop": "7b8101dfce096480",
  "smart-validate-summarize": "007548c49e9654fb"
}
"#,
        )
        .unwrap();
    }

    fn compatible_smart() -> &'static str {
        r#"name: "smart-orchestrator"
description: "Composable smart task orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
"#
    }

    /// Cold path: nothing cached yet, so the validator runs in full and the
    /// cache file is created.
    #[test]
    fn cold_cache_validates_and_primes() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);

        let (issue, reads) =
            read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert_eq!(issue, None, "healthy bundle must be reported compatible");
        assert_eq!(
            reads, FULL_VALIDATION_READS,
            "cold cache must perform the full validation"
        );
        assert!(cache.is_file(), "cold validation must prime the cache file");
    }

    /// Warm path — the point of issue #1271. A second call over an unchanged
    /// bundle must read zero bundle files.
    #[test]
    fn warm_cache_reads_no_bundle_files() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);

        assert_eq!(compatibility_issue_cached(&bundle, &cache), None);

        let (issue, reads) =
            read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert_eq!(issue, None, "cached verdict must match the cold verdict");
        assert_eq!(
            reads, 0,
            "an unchanged bundle must not be read or parsed again"
        );
    }

    /// Stale path: the bundle changed, so the fingerprint no longer matches
    /// and the verdict must be recomputed rather than served from cache.
    #[test]
    fn changed_bundle_invalidates_cache_and_recomputes() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);

        assert_eq!(compatibility_issue_cached(&bundle, &cache), None);

        // Break the bundle: a stale monolithic smart-orchestrator.
        fs::write(
            bundle.join("recipes/smart-orchestrator.yaml"),
            "name: \"smart-orchestrator\"\ndescription: \"uses orch_helper.py\"\nsteps: []\n",
        )
        .unwrap();

        let (issue, reads) =
            read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert!(
            reads > 0,
            "a changed bundle must be revalidated, not cached"
        );
        let issue = issue.expect("stale smart-orchestrator must be reported incompatible");
        assert!(
            issue.contains("orch_helper.py"),
            "stale verdict must be the validator's real message, got: {issue}"
        );
    }

    /// A cached *incompatible* verdict must be served from cache too — the
    /// cache stores the verdict, not just the happy answer.
    #[test]
    fn incompatible_verdict_is_cached_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        fs::remove_file(bundle.join("recipes/smart-reflect-loop.yaml")).unwrap();
        let cache = temp.path().join(CACHE_FILE);

        let cold = compatibility_issue_cached(&bundle, &cache)
            .expect("missing companion recipe must be incompatible");
        let (warm, reads) =
            read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert_eq!(reads, 0, "unchanged broken bundle must not be re-read");
        assert_eq!(
            warm.as_deref(),
            Some(cold.as_str()),
            "cached failure message must be identical to the computed one"
        );
    }

    /// A deleted file changes the fingerprint even though nothing was edited.
    #[test]
    fn deleting_a_required_recipe_changes_the_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);

        let before = fingerprint(&bundle);
        fs::remove_file(bundle.join("recipes/smart-classify-route.yaml")).unwrap();
        let after = fingerprint(&bundle);

        assert_ne!(
            before, after,
            "removing a validated input must change the stat fingerprint"
        );
    }

    /// A corrupt or truncated cache file is a miss, never a wrong answer.
    #[test]
    fn corrupt_cache_falls_back_to_full_validation() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);
        fs::write(&cache, "{not json at all").unwrap();

        let (issue, reads) =
            read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert_eq!(issue, None);
        assert_eq!(
            reads, FULL_VALIDATION_READS,
            "an unreadable cache must fall back to validating"
        );
    }

    /// A cache written by a different binary version must not be trusted:
    /// the validation rules themselves ship with the binary.
    #[test]
    fn cache_from_another_binary_version_is_ignored() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);

        assert_eq!(compatibility_issue_cached(&bundle, &cache), None);
        let mut entry: CacheEntry = serde_json::from_str(&fs::read_to_string(&cache).unwrap())
            .expect("primed cache must be readable");
        entry.binary_version = "0.0.0-not-this-binary".to_string();
        fs::write(&cache, serde_json::to_string(&entry).unwrap()).unwrap();

        let (_, reads) = read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));

        assert_eq!(
            reads, FULL_VALIDATION_READS,
            "a cache entry from another binary version must be revalidated"
        );
    }

    #[test]
    fn invalidate_removes_the_cache_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        let cache = temp.path().join(CACHE_FILE);

        assert_eq!(compatibility_issue_cached(&bundle, &cache), None);
        assert!(cache.is_file());
        invalidate(&cache);
        assert!(!cache.exists(), "invalidate must remove the cache file");
        invalidate(&cache);

        let (_, reads) = read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));
        assert_eq!(
            reads, FULL_VALIDATION_READS,
            "after invalidation the next call must revalidate"
        );
    }

    /// An unwritable cache directory must degrade to "always validate",
    /// never to a wrong verdict or an error.
    #[test]
    fn unwritable_cache_location_still_returns_the_correct_verdict() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("amplifier-bundle");
        write_compatible_bundle(&bundle);
        // A path whose parent is a *file*, so create_dir_all/rename must fail.
        let blocker = temp.path().join("blocker");
        fs::write(&blocker, b"not a directory").unwrap();
        let cache = blocker.join(CACHE_FILE);

        for _ in 0..2 {
            let (issue, reads) =
                read_counter::count_reads(|| compatibility_issue_cached(&bundle, &cache));
            assert_eq!(
                issue, None,
                "verdict must be correct without a usable cache"
            );
            assert_eq!(
                reads, FULL_VALIDATION_READS,
                "without a usable cache every call validates"
            );
        }
    }
}
