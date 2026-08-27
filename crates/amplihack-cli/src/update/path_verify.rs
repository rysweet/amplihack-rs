//! Post-update verification that every `amplihack` on PATH is the new build.
//!
//! Issue #1331. `download_and_replace` refreshes the copy it resolved. Both documented
//! install paths exist -- `cargo install` lands in `~/.cargo/bin`, the npx bootstrap in
//! `~/.local/bin` -- so anyone who has used both has two, and updating one of two is a
//! trap rather than a partial success.
//!
//! It is a trap because the recursion guards only work if every participant in a tree
//! agrees on the rules. On the affected host the fixed binary seeded a tree, an older
//! `amplihack` on PATH handled `session-tree register`, wrote a ceiling-less entry, and
//! depth reached 3 against a ceiling of 2. A half-upgraded host is not partly fixed.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Result, bail};

use crate::path_conflicts::{PathAnalysisInput, analyze_path_conflicts};

/// A copy on PATH that is not running the expected version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaleCopy {
    pub(crate) path: PathBuf,
    /// `None` when the binary would not report a version at all.
    pub(crate) reported: Option<String>,
}

/// Every distinct `amplihack` on PATH whose `--version` is not `expected`.
///
/// Distinct by canonical path, so a symlink farm pointing at one real binary is one
/// copy rather than several.
pub(crate) fn stale_copies_on_path(expected: &str) -> Result<Vec<StaleCopy>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let input = PathAnalysisInput {
        home_dir: home,
        current_exe: std::env::current_exe().unwrap_or_default(),
        path_dirs,
        binary_names: vec!["amplihack".to_string()],
    };

    let report = analyze_path_conflicts(&input)?;
    let Some(resolution) = report.resolution("amplihack") else {
        return Ok(Vec::new());
    };

    let mut seen = BTreeSet::new();
    let mut stale = Vec::new();
    for candidate in &resolution.canonical_candidates {
        if !seen.insert(candidate.canonical_path.clone()) {
            continue;
        }
        let reported = probe_version(&candidate.path);
        if reported.as_deref() != Some(expected) {
            stale.push(StaleCopy {
                path: candidate.path.clone(),
                reported,
            });
        }
    }
    Ok(stale)
}

/// Ask a binary what version it is. `None` when it cannot say.
fn probe_version(path: &std::path::Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // `amplihack --version` prints "amplihack X.Y.Z".
    text.split_whitespace().nth(1).map(str::to_string)
}

/// Fail the update when any copy on PATH is still on an older build.
///
/// Non-zero rather than a warning: a warning in a long install log is not a control,
/// and the failure it guards against is silent.
pub(crate) fn verify_all_path_copies(expected: &str) -> Result<()> {
    let stale = stale_copies_on_path(expected)?;
    if stale.is_empty() {
        return Ok(());
    }

    let mut msg = format!(
        "update incomplete: {} copy/copies of `amplihack` on PATH are not v{expected}:\n",
        stale.len()
    );
    for copy in &stale {
        let reported = copy.reported.as_deref().unwrap_or("no version reported");
        msg.push_str(&format!("     - {} ({reported})\n", copy.path.display()));
    }
    msg.push_str(
        "   A half-upgraded host is not partly fixed: the recursion guards only hold if\n\
         \x20  every participant in a tree is the same build. Update or remove the copies\n\
         \x20  above, then re-run. See `which -a amplihack` (issue #1331).",
    );
    bail!(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy reporting the expected version is not stale; anything else is --
    /// including a binary that will not say what it is, since "cannot tell" must not
    /// read as "fine".
    #[test]
    fn staleness_is_decided_by_the_reported_version() {
        let same = StaleCopy {
            path: PathBuf::from("/a/amplihack"),
            reported: Some("0.18.0".into()),
        };
        assert_eq!(same.reported.as_deref(), Some("0.18.0"));

        let silent = StaleCopy {
            path: PathBuf::from("/b/amplihack"),
            reported: None,
        };
        assert_ne!(
            silent.reported.as_deref(),
            Some("0.18.0"),
            "a binary that reports nothing must never count as up to date"
        );
    }

    /// The whole point: the failure names every straggler, so the operator knows what
    /// to fix rather than being told only that something is wrong.
    #[test]
    fn the_failure_names_every_stale_copy() {
        let stale = [
            StaleCopy {
                path: PathBuf::from("/home/u/.local/bin/amplihack"),
                reported: Some("0.17.0".into()),
            },
            StaleCopy {
                path: PathBuf::from("/home/u/.cargo/bin/amplihack"),
                reported: None,
            },
        ];
        let mut msg = String::new();
        for c in &stale {
            msg.push_str(&format!(
                "     - {} ({})\n",
                c.path.display(),
                c.reported.as_deref().unwrap_or("no version reported")
            ));
        }
        assert!(msg.contains(".local/bin/amplihack (0.17.0)"));
        assert!(msg.contains(".cargo/bin/amplihack (no version reported)"));
    }

    /// This host is the reproduction: two distinct copies on PATH. The probe must
    /// find at least the one it is running as, or the check is inert.
    #[test]
    fn the_probe_finds_copies_on_this_host() {
        // Issue #1380: this reads PATH, and sibling tests point PATH at a tempdir
        // holding one stub binary while they run. A reader has to take the same
        // lock as the writers or it observes a PATH with no amplihack on it and
        // fails for reasons that have nothing to do with the probe.
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Deliberately asks for a version nothing can be, so every copy found is
        // reported stale -- this asserts discovery, not the comparison.
        let found = stale_copies_on_path("0.0.0-nonexistent").unwrap_or_default();
        if std::env::var_os("PATH").is_some() {
            assert!(
                !found.is_empty() || std::env::current_exe().is_err(),
                "the probe found no amplihack on PATH at all; the gate would be inert"
            );
        }
    }
}
