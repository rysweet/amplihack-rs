//! Where a run's temporary folder goes (issue #2291). The specification is
//! "Where a run's temp folder goes" in
//! `docs/howto/clean-up-after-recipe-runs.md`.
//!
//! [`choose_runtime_dir`] applies, in order, `AMPLIHACK_RUN_TMP_ROOT`, the
//! caller's `TMPDIR` and the default layout. It reads no environment itself:
//! [`TmpRootInputs::from_env`] collects the values once, so tests pass their
//! own. Every refusal falls back to the default layout inside the private
//! work root, with at most one warning.

use super::run_dir::{MAX_RUNTIME_TMP_BYTES, runtime_dir_for, short_id};
use super::work_root::create_private_dir_all;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

pub(crate) const RUN_TMP_ROOT_ENV: &str = "AMPLIHACK_RUN_TMP_ROOT";
pub(crate) const RUN_DIR_PREFIX: &str = "amplihack-run-";

const TMPDIR_ENV: &str = "TMPDIR";
const PARENT_RUNTIME_ENV: &str = "AMPLIHACK_WORKFLOW_RUNTIME_DIR";
const SHORT_ID_LEN: usize = 12;

/// The values the chooser looks at, taken from the environment by
/// [`TmpRootInputs::from_env`] and passed in so tests need no process
/// environment.
#[derive(Debug, Clone, Default)]
pub(crate) struct TmpRootInputs {
    /// `AMPLIHACK_RUN_TMP_ROOT`.
    pub run_tmp_root: Option<OsString>,
    /// The caller's `TMPDIR`.
    pub tmpdir: Option<OsString>,
    /// The caller's `AMPLIHACK_WORKFLOW_RUNTIME_DIR`.
    pub parent_runtime_dir: Option<OsString>,
    /// `$HOME`, for the `$HOME/.amplihack` check.
    pub home: Option<PathBuf>,
    /// The uid the root (or uid 0) and the per-run folder must belong to.
    pub euid: u32,
}

impl TmpRootInputs {
    pub(crate) fn from_env() -> Self {
        Self {
            run_tmp_root: std::env::var_os(RUN_TMP_ROOT_ENV),
            tmpdir: std::env::var_os(TMPDIR_ENV),
            parent_runtime_dir: std::env::var_os(PARENT_RUNTIME_ENV),
            home: std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(PathBuf::from),
            // SAFETY: geteuid has no preconditions and cannot fail.
            euid: unsafe { libc::geteuid() },
        }
    }

    /// `AMPLIHACK_RUN_TMP_ROOT` when it is set and not empty.
    fn explicit_root(&self) -> Option<&OsString> {
        self.run_tmp_root.as_ref().filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeChoice {
    /// `runtime_dir_for(work_root, run_id)`.
    Default(PathBuf),
    /// `<root>/amplihack-run-<short id>` under the canonical `root`.
    External { dir: PathBuf, root: PathBuf },
}

#[derive(Debug, Clone)]
pub(crate) struct Chosen {
    pub choice: RuntimeChoice,
    pub warning: Option<String>,
}

/// Which rule a root came from; names the variable in warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rule {
    Explicit,
    CallerTmpdir,
}

impl Rule {
    fn var(self) -> &'static str {
        match self {
            Self::Explicit => RUN_TMP_ROOT_ENV,
            Self::CallerTmpdir => TMPDIR_ENV,
        }
    }
}

/// Why a root was not used. `Silent` is the normal "rule does not apply"
/// case for `TMPDIR`; `Warn` carries the reason for the one-line warning.
enum Refused {
    Silent,
    Warn(String),
}

/// One warning line. The path is printed with `{:?}`, so a newline or a
/// terminal control code in a variable cannot forge another output line.
fn warning_line(rule: Rule, value: &Path, reason: &str) -> String {
    format!(
        "amplihack: {}={:?} {reason}; using the default run folder",
        rule.var(),
        value
    )
}

/// `amplihack-run-<12 hex>`, or `None` when the run id has fewer than 12
/// hex digits.
pub(crate) fn external_dir_name(run_id: &str) -> Option<String> {
    let short = short_id(run_id);
    (short.len() == SHORT_ID_LEN).then(|| format!("{RUN_DIR_PREFIX}{short}"))
}

/// Picks the run's temporary folder. `External` folders exist on return,
/// created non-recursively with mode 0700; `tmp/` and `artifacts/` are left
/// to the caller.
pub(crate) fn choose_runtime_dir(work_root: &Path, run_id: &str, inputs: &TmpRootInputs) -> Chosen {
    let default = || RuntimeChoice::Default(runtime_dir_for(work_root, run_id));
    let Some(name) = external_dir_name(run_id) else {
        return Chosen {
            choice: default(),
            warning: None,
        };
    };
    let (rule, value) = match (inputs.explicit_root(), inputs.tmpdir.as_ref()) {
        (Some(root), _) => (Rule::Explicit, root),
        (None, Some(tmpdir)) => (Rule::CallerTmpdir, tmpdir),
        (None, None) => {
            return Chosen {
                choice: default(),
                warning: None,
            };
        }
    };
    let value = Path::new(value);
    let refused = match usable_root(work_root, rule, value, inputs) {
        Ok(root) => match create_run_dir(&root, &name, inputs.euid) {
            Ok(dir) => {
                return Chosen {
                    choice: RuntimeChoice::External { dir, root },
                    warning: None,
                };
            }
            Err(reason) => Refused::Warn(reason),
        },
        Err(refused) => refused,
    };
    let warning = match refused {
        Refused::Silent => None,
        Refused::Warn(reason) => {
            let line = warning_line(rule, value, &reason);
            tracing::warn!(variable = rule.var(), path = ?value, %reason, "run temp root refused");
            Some(line)
        }
    };
    Chosen {
        choice: default(),
        warning,
    }
}

/// Legacy mode only: the canonical `AMPLIHACK_RUN_TMP_ROOT` when rule 1
/// accepts it, `Ok(None)` when it is unset or empty, and the warning line
/// otherwise. `TMPDIR` is not looked at; `tempdir()` already follows it.
pub(crate) fn usable_explicit_root(
    work_root: &Path,
    inputs: &TmpRootInputs,
) -> Result<Option<PathBuf>, String> {
    let Some(value) = inputs.explicit_root() else {
        return Ok(None);
    };
    let value = Path::new(value);
    match usable_root(work_root, Rule::Explicit, value, inputs) {
        Ok(root) => Ok(Some(root)),
        Err(Refused::Warn(reason)) => Err(warning_line(Rule::Explicit, value, &reason)),
        Err(Refused::Silent) => Err(warning_line(Rule::Explicit, value, "cannot be used")),
    }
}

/// Validates a root under `rule` and returns its canonical path. Only rule 1
/// may create a missing root; failures that only show `TMPDIR` does not
/// apply are silent.
fn usable_root(
    work_root: &Path,
    rule: Rule,
    value: &Path,
    inputs: &TmpRootInputs,
) -> Result<PathBuf, Refused> {
    let explicit = rule == Rule::Explicit;
    let warn_or_silent = |reason: String| {
        if explicit {
            Refused::Warn(reason)
        } else {
            Refused::Silent
        }
    };
    if !value.is_absolute() {
        return Err(warn_or_silent("is not an absolute path".into()));
    }
    if fs::metadata(value).is_err() {
        if !explicit {
            return Err(Refused::Silent);
        }
        if let Err(error) = create_private_dir_all(value) {
            return Err(Refused::Warn(format!("could not be created: {error:#}")));
        }
    }
    let meta = match fs::metadata(value) {
        Ok(meta) if meta.is_dir() => meta,
        _ => return Err(warn_or_silent("is not a folder".into())),
    };
    let root = match fs::canonicalize(value) {
        Ok(root) => root,
        Err(error) => return Err(warn_or_silent(format!("could not be resolved: {error}"))),
    };

    let work = fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    if !work.as_os_str().is_empty() && root.starts_with(&work) {
        return Err(warn_or_silent("is inside the work root".into()));
    }
    if let Some(home) = &inputs.home {
        let state = home.join(".amplihack");
        let state = fs::canonicalize(&state).unwrap_or(state);
        if root.starts_with(&state) {
            return Err(warn_or_silent("is inside $HOME/.amplihack".into()));
        }
    }
    if !explicit && inside_parent_runtime(value, &root, inputs) {
        return Err(Refused::Silent);
    }

    if meta.uid() != inputs.euid && meta.uid() != 0 {
        return Err(Refused::Warn("is owned by another user".into()));
    }
    if meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0 {
        return Err(Refused::Warn(
            "is writable by other users without the sticky bit".into(),
        ));
    }
    if root.to_str().is_none() {
        return Err(Refused::Warn("is not valid UTF-8".into()));
    }
    // `/amplihack-run-<12 hex>/tmp`.
    let len = root.as_os_str().len() + 1 + RUN_DIR_PREFIX.len() + SHORT_ID_LEN + "/tmp".len();
    if len > MAX_RUNTIME_TMP_BYTES {
        return Err(Refused::Warn(format!(
            "would make the run temp path {len} bytes (limit {MAX_RUNTIME_TMP_BYTES})"
        )));
    }
    Ok(root)
}

/// Whether `TMPDIR` lies inside the caller's
/// `AMPLIHACK_WORKFLOW_RUNTIME_DIR`, compared by whole components both as
/// given and resolved: a sub-recipe's `TMPDIR` is its parent's temp folder.
fn inside_parent_runtime(value: &Path, root: &Path, inputs: &TmpRootInputs) -> bool {
    let Some(parent) = inputs.parent_runtime_dir.as_ref().filter(|p| !p.is_empty()) else {
        return false;
    };
    let parent = Path::new(parent);
    if !parent.is_absolute() {
        return false;
    }
    if value.starts_with(parent) {
        return true;
    }
    fs::canonicalize(parent).is_ok_and(|p| root.starts_with(p))
}

/// Creates `<root>/<name>` with one non-recursive `mkdir` at mode 0700 and
/// checks the result is a private folder owned by `euid`. An existing entry
/// of that name is never reused.
fn create_run_dir(root: &Path, name: &str, euid: u32) -> Result<PathBuf, String> {
    let dir = root.join(name);
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(&dir) {
        return Err(if error.kind() == ErrorKind::AlreadyExists {
            format!("per-run folder {dir:?} already exists")
        } else {
            format!("per-run folder {dir:?} could not be created: {error}")
        });
    }
    let private = fs::symlink_metadata(&dir)
        .is_ok_and(|m| m.file_type().is_dir() && m.uid() == euid && m.mode() & 0o077 == 0);
    if !private {
        if let Err(error) = fs::remove_dir(&dir) {
            tracing::warn!(path = ?dir, %error, "could not remove a per-run folder that is not private");
        }
        return Err(format!("per-run folder {dir:?} is not private"));
    }
    Ok(dir)
}

#[cfg(test)]
#[path = "tests_run_tmp_root.rs"]
mod tests;
