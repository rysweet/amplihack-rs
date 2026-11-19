//! Issue #964, #1903, #2814 — the named checkout's `core.bare`, before and
//! after a run.
//!
//! #964 recorded a run leaving its caller's checkout with `core.bare=true`,
//! which makes every later `git status` there fail with "this operation must
//! be run in a work tree". #1903, the cause, is still open. While #2290 ran
//! each recipe against a private copy, a flip landed on the copy and the
//! caller never saw it; #2808 removed that copy, so a run now flips the
//! checkout an operator works in — a PR worktree, usually.
//!
//! This is not a fix for the cause. It is the observation the private copy
//! used to make unnecessary: read the setting before the run, read it again
//! after, and when it has moved say so on stderr and put the pre-run value
//! back. A checkout whose value never moved is never written to, and nothing
//! else about the checkout — its worktrees, its index, its files — is
//! touched.
//!
//! A reading that could not be taken at all ([`Bare::Unknown`]) is not a
//! value, on either side of the comparison: neither an unreadable checkout
//! before the run nor one after it is reported as a flip, and neither is
//! written to.
//!
//! The run calls [`CheckoutBare::restore`] as a release step, so a restore
//! that does not take becomes the run's error. Dropping the value runs the
//! same check once if that step never ran, which covers an early return on a
//! path the release list does not reach; there it can only report.

use std::cell::Cell;
use std::path::{Path, PathBuf};

/// `core.bare` in the repository `dir` selects, as `git config` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Bare {
    /// The setting is absent from the repository's own configuration.
    Unset,
    /// The setting is present, with this value.
    Set(String),
    /// `dir` is no repository, or git could not be run. Nothing to compare.
    Unknown,
}

fn read(dir: &Path) -> Bare {
    let output = amplihack_git::command_in(dir)
        .args(["config", "--local", "--get", "core.bare"])
        .output();
    match output {
        // Exit 1 with no output is git's "the key is not set"; any other
        // failure means the question could not be asked at all.
        Ok(out) if out.status.success() => Bare::Set(
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .to_ascii_lowercase(),
        ),
        Ok(out) if out.status.code() == Some(1) && out.stdout.is_empty() => Bare::Unset,
        _ => Bare::Unknown,
    }
}

/// The pre-run reading of one checkout's `core.bare`.
#[derive(Debug)]
pub(super) struct CheckoutBare {
    dir: PathBuf,
    before: Bare,
    done: Cell<bool>,
}

impl CheckoutBare {
    /// Read `core.bare` in `dir` before the run spawns.
    pub(super) fn snapshot(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            before: read(dir),
            done: Cell::new(false),
        }
    }

    /// Read it again, and restore the pre-run value if it moved. Returns the
    /// message to report when it moved and could not be put back; `None`
    /// when nothing moved or the restore succeeded. A restore that worked is
    /// still reported on stderr: a run that silently rewrites the operator's
    /// checkout configuration is the defect, not the symptom.
    pub(super) fn restore(&self) -> Option<String> {
        self.done.set(true);
        if self.before == Bare::Unknown {
            return None;
        }
        let after = read(&self.dir);
        if after == self.before {
            return None;
        }
        // The same rule as the pre-run value above, on the other side of the
        // comparison (#2814). `Unknown` is "the question could not be asked",
        // not a value git reported: the directory is gone, is no longer a
        // repository, or git could not run. There is nothing to compare it
        // with and nothing that could be written back, so this is said once
        // and left alone. Treating it as a flip warned about a change nobody
        // observed, attempted a `git config` that could not work, and turned
        // an otherwise successful run into an error naming the wrong cause.
        if after == Bare::Unknown {
            eprintln!(
                "WARNING: core.bare in the named checkout {} could not be read after the run \
                 (it read {:?} before it). Nothing is compared or written there; check that \
                 the directory is still a readable repository (issues #964, #2814).",
                self.dir.display(),
                self.before
            );
            return None;
        }
        let dir = self.dir.display();
        eprintln!(
            "WARNING: the run changed core.bare in the named checkout {dir} \
             from {:?} to {:?}. Nothing in amplihack sets it, and issue #1903 \
             has not found what does; the pre-run value is being put back so \
             the checkout stays usable (issues #964, #2814).",
            self.before, after
        );
        let restored = match &self.before {
            Bare::Set(value) => amplihack_git::command_in(&self.dir)
                .args(["config", "--local", "core.bare", value])
                .status(),
            // Unsetting a key that is absent exits 5; the run put it there,
            // so it is present here.
            Bare::Unset => amplihack_git::command_in(&self.dir)
                .args(["config", "--local", "--unset", "core.bare"])
                .status(),
            Bare::Unknown => unreachable!("an unknown pre-run value returns above"),
        };
        let now = read(&self.dir);
        if restored.map(|s| s.success()).unwrap_or(false) && now == self.before {
            return None;
        }
        Some(format!(
            "the run changed core.bare in the named checkout {dir} from {:?} to {:?} \
             and it could not be put back (it now reads {now:?}); that checkout's \
             git commands will keep failing until it is corrected by hand \
             (issues #964, #1903, #2814)",
            self.before, after
        ))
    }
}

impl Drop for CheckoutBare {
    fn drop(&mut self) {
        if self.done.get() {
            return;
        }
        if let Some(message) = self.restore() {
            eprintln!("ERROR: {message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with a born HEAD, or `None` when git is unavailable here.
    fn repo(dir: &Path) -> bool {
        let git = |args: &[&str]| {
            amplihack_git::command_in(dir)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        std::fs::create_dir_all(dir).is_ok() && git(&["init", "-q", "-b", "main", "."])
    }

    /// #2814: a checkout git can no longer read after the run has nothing to
    /// compare, so it is reported and left alone rather than reported as a
    /// flip whose restore then fails the run.
    #[test]
    fn an_unreadable_checkout_after_the_run_is_not_a_flip() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("checkout");
        if !repo(&dir) {
            eprintln!("skipping: git unavailable");
            return;
        }
        let snapshot = CheckoutBare::snapshot(&dir);
        assert_ne!(
            snapshot.before,
            Bare::Unknown,
            "the fixture must be readable before the run"
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            read(&dir),
            Bare::Unknown,
            "the fixture must now be unreadable"
        );
        assert!(
            snapshot.restore().is_none(),
            "an unreadable post-run checkout must not become the run's error"
        );
    }

    /// The flip the module exists for is still reported and put back.
    #[test]
    fn a_flipped_value_is_restored() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("checkout");
        if !repo(&dir) {
            eprintln!("skipping: git unavailable");
            return;
        }
        let snapshot = CheckoutBare::snapshot(&dir);
        let before = snapshot.before.clone();
        assert!(
            amplihack_git::command_in(&dir)
                .args(["config", "--local", "core.bare", "true"])
                .status()
                .unwrap()
                .success()
        );
        assert!(snapshot.restore().is_none(), "the restore must take");
        assert_eq!(read(&dir), before, "the pre-run value must be back");
    }
}
