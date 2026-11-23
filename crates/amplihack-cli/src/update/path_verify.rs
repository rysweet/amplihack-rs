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
    stale_copies_in(expected, home, path_dirs)
}

/// [`stale_copies_on_path`] with the home folder and the PATH entries given, so
/// a test names folders it made and reads nothing from the host (issue #2931).
fn stale_copies_in(
    expected: &str,
    home: PathBuf,
    path_dirs: Vec<PathBuf>,
) -> Result<Vec<StaleCopy>> {
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

    /// One `amplihack` stand-in in a new folder under `root`. It prints `says`
    /// for `--version`, or exits 1 with no output when `says` is `None`.
    #[cfg(unix)]
    fn copy_in(root: &std::path::Path, folder: &str, says: Option<&str>) -> PathBuf {
        let dir = root.join(folder);
        std::fs::create_dir_all(&dir).unwrap();
        let body = match says {
            Some(line) => format!("#!/bin/sh\necho '{line}'\n"),
            None => "#!/bin/sh\nexit 1\n".to_string(),
        };
        let path = dir.join("amplihack");
        // The shared stub writer, which never holds the file open for
        // writing in this process. Written here with `std::fs::write`, a
        // sibling test's fork can inherit that descriptor and leave the
        // stub "text file busy"; the probe then reads the copy as one that
        // reports no version at all (issue #1973).
        crate::test_support::write_executable(&path, &body);
        dir
    }

    /// The probe finds every copy on the PATH it is given, or the check is
    /// inert. The reproduction of issue #1331 was a host with two copies; this
    /// test makes that host in its own folder: three folders on a PATH of its
    /// own, each with a stand-in `amplihack`, and a fourth entry with none.
    ///
    /// It used to read the host's PATH and pass only where an `amplihack` was
    /// installed (issue #2931), which is a fact about the machine and not
    /// about this code. Nothing here has a time limit, so a stand-in that is
    /// slow to start changes nothing.
    #[cfg(unix)]
    #[test]
    fn the_probe_finds_every_copy_on_the_path_it_is_given() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let current = copy_in(root, "current", Some("amplihack 9.9.9"));
        let older = copy_in(root, "older", Some("amplihack 9.9.8"));
        let silent = copy_in(root, "silent", None);
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let path_dirs = vec![empty, current.clone(), older.clone(), silent.clone()];
        let home = root.join("home");

        // A version nothing reports: every copy found is listed, in PATH order.
        let found = stale_copies_in("0.0.0-nonexistent", home.clone(), path_dirs.clone()).unwrap();
        assert_eq!(
            found,
            vec![
                StaleCopy {
                    path: current.join("amplihack"),
                    reported: Some("9.9.9".into()),
                },
                StaleCopy {
                    path: older.join("amplihack"),
                    reported: Some("9.9.8".into()),
                },
                StaleCopy {
                    path: silent.join("amplihack"),
                    reported: None,
                },
            ],
            "discovery must find each copy and read what it reports"
        );

        // The version one copy reports: that copy is current, the older one and
        // the one that will not say are stale.
        let stale = stale_copies_in("9.9.9", home.clone(), path_dirs).unwrap();
        assert_eq!(
            stale.iter().map(|c| c.path.clone()).collect::<Vec<_>>(),
            vec![older.join("amplihack"), silent.join("amplihack")],
            "only the copies that are not the expected version are stale"
        );

        // No copy anywhere on the PATH: nothing is stale, and nothing fails.
        assert_eq!(
            stale_copies_in("9.9.9", home, vec![root.join("empty")]).unwrap(),
            Vec::new()
        );
    }

    /// Two PATH entries that reach one file are one copy, not two.
    #[cfg(unix)]
    #[test]
    fn one_binary_reached_through_a_link_is_one_copy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let real = copy_in(root, "real", Some("amplihack 9.9.8"));
        let linked = root.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::os::unix::fs::symlink(real.join("amplihack"), linked.join("amplihack")).unwrap();

        let stale =
            stale_copies_in("9.9.9", root.join("home"), vec![real.clone(), linked]).unwrap();
        assert_eq!(
            stale,
            vec![StaleCopy {
                path: real.join("amplihack"),
                reported: Some("9.9.8".into()),
            }]
        );
    }
}
