//! Workstream identity: the key that names a run's artefacts (issue #2553).
//!
//! Identity is separate from the issue a workstream *tracks*. The issue may be
//! absent; the key never is. Kept in its own brick because it is the security
//! boundary for four path and shell sinks, not merely a naming convenience.

use super::models::WorkstreamConfig;
use anyhow::{Result, bail};
use regex::Regex;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The charset a workstream identity may use, anchored and length-bounded.
///
/// Security control, not hygiene (issue #2553). Identity used to be an `i64`,
/// which could not carry a path separator, a parent reference or a quote. It
/// is now a string derived in part from model-produced text whose input may
/// include issue bodies, PR comments or repository content, and it reaches
/// four sinks: `fs::remove_dir_all`, work-directory creation, the
/// single-quoted `cd '{work_dir}'` in the generated run script, and the
/// state and progress file paths. Excluding `/`, `.`, `'` and whitespace is
/// what keeps those sinks closed by construction.
static KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").unwrap());

/// A workstream's identity: always present, unique within a run, and safe in
/// a path or inside single quotes.
///
/// Separate from the issue a workstream *tracks*, which may be absent. With
/// an issue present the key is the issue number, so `ws-123`, `log-123.txt`
/// and `ws-123.json` are byte-identical to the pre-#2553 paths and a running
/// workstream resumes with no migration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkstreamKey(String);

impl WorkstreamKey {
    /// Fails closed. A fallback key would silently merge two workstreams into
    /// one identity, which is the outcome the uniqueness check exists to
    /// prevent; there is deliberately no `From<String>`.
    pub fn new(raw: &str) -> Result<Self> {
        if !KEY_RE.is_match(raw) {
            bail!(
                "invalid workstream key {raw:?}: only ASCII letters, digits, `-` and `_` \
                 are allowed, between 1 and 64 characters"
            );
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkstreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identity of the `index`-th (one-based) workstream of a run.
///
/// Deterministic and derived only from the configuration, so a resumed run
/// reproduces the same key and finds the same state file. With an issue it is
/// the issue number; without one it is the slugified description — or the
/// slugified branch when there is no description — followed by the index, so
/// two workstreams sharing a description cannot share a work directory.
pub fn derive_key(config: &WorkstreamConfig, index: usize) -> Result<WorkstreamKey> {
    if let Some(key) = &config.key {
        return WorkstreamKey::new(key);
    }
    if let Some(issue) = config.issue()? {
        return WorkstreamKey::new(&issue.get().to_string());
    }
    let name = config
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(config.branch.as_str());
    let slug = crate::commands::orch::slugify(name, index);
    WorkstreamKey::new(&format!("{slug}-{index}"))
}

/// The one derivation of a workstream's work directory.
///
/// `Workstream::new` and `cleanup_merged` used to derive it independently and
/// only one of them sanitised; both now call this.
pub fn work_dir_for(base_dir: &Path, key: &WorkstreamKey) -> PathBuf {
    base_dir.join(format!("ws-{key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SR1/SR2 — the key is the only thing standing between model-produced
    /// text and four path and shell sinks, so the charset is enforced at
    /// construction and the work directory has exactly one derivation.
    #[test]
    fn a_derived_key_stays_inside_the_charset_and_under_the_base_directory() {
        let base = Path::new("/tmp/amplihack-workstreams");
        for hostile in ["../../etc", "a/b/c", "it's; rm -rf /", "..", ""] {
            let config: WorkstreamConfig = serde_json::from_value(serde_json::json!({
                "branch": "feat/orch-1-x",
                "description": hostile,
                "task": "t",
            }))
            .unwrap();
            let key = derive_key(&config, 1).expect("derivation never fails");
            assert!(
                WorkstreamKey::new(key.as_str()).is_ok(),
                "{key} left the charset"
            );
            assert!(work_dir_for(base, &key).starts_with(base));
        }
    }

    #[test]
    fn the_key_constructor_fails_closed() {
        for hostile in ["", "..", "a/b", "a b", "a'b", "a;b", &"x".repeat(65)] {
            assert!(WorkstreamKey::new(hostile).is_err(), "{hostile:?}");
        }
        assert!(WorkstreamKey::new("2553").is_ok());
        assert!(WorkstreamKey::new("triage-1").is_ok());
    }
}
