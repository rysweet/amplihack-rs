//! Where a run's temporary folder goes (issue #2291). The specification is
//! "Where a run's temp folder goes" in
//! `docs/howto/clean-up-after-recipe-runs.md`.
//!
//! [`choose_runtime_dir`] applies, in order, `AMPLIHACK_RUN_TMP_ROOT`, the
//! caller's `TMPDIR` and the default layout. It reads no environment itself:
//! [`TmpRootInputs::from_env`] collects the values once, so tests pass their
//! own. Every refusal falls back to the default layout inside the private
//! work root, with at most one warning.
//!
//! On macOS the per-user folder launchd gives every login as `TMPDIR` does
//! not count as the caller's choice: it sits on the same volume as `$HOME`,
//! so honouring it moves nothing off the work root's disk, and its 56-byte
//! resolved path would make every step's `TMPDIR` 77 bytes, too long for
//! common socket users under the 104-byte macOS limit.

use super::run_dir::{MAX_RUNTIME_TMP_BYTES, runtime_dir_for, short_id};
use super::work_root::create_private_dir_all;
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

pub(crate) const RUN_TMP_ROOT_ENV: &str = "AMPLIHACK_RUN_TMP_ROOT";
/// Short on purpose: every byte of the per-run name comes out of the 80-byte
/// budget for `<root>/<name>/tmp`. With `ahr-` the part after the root is 21
/// bytes, so a resolved root of up to 59 bytes fits.
pub(crate) const RUN_DIR_PREFIX: &str = "ahr-";
/// The dry-run folder, `<root>/ahd-<random>`. Its part after the root must
/// never be longer than the per-run folder's, because both are checked
/// against the same root budget.
pub(crate) const DRY_DIR_PREFIX: &str = "ahd-";
/// Random characters `tempfile` adds after [`DRY_DIR_PREFIX`].
pub(crate) const DRY_RAND_LEN: usize = 6;

const TMPDIR_ENV: &str = "TMPDIR";
const PARENT_RUNTIME_ENV: &str = "AMPLIHACK_WORKFLOW_RUNTIME_DIR";
const SHORT_ID_LEN: usize = 12;
/// `/<RUN_DIR_PREFIX><12 hex>/tmp`: the bytes a run adds after the root.
const RUN_SUFFIX_BYTES: usize = 1 + RUN_DIR_PREFIX.len() + SHORT_ID_LEN + "/tmp".len();
/// The longest resolved root whose run temp path fits the 80-byte limit.
const MAX_ROOT_BYTES: usize = MAX_RUNTIME_TMP_BYTES - RUN_SUFFIX_BYTES;

const _: () = assert!(
    1 + DRY_DIR_PREFIX.len() + DRY_RAND_LEN + "/tmp".len() <= RUN_SUFFIX_BYTES,
    "a dry-run folder must fit wherever a per-run folder fits"
);

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
    /// The per-user temporary folder macOS gives every login session as
    /// `TMPDIR` (`confstr(_CS_DARWIN_USER_TEMP_DIR)`); `None` elsewhere.
    /// Rule 2 skips a `TMPDIR` that resolves to it.
    pub os_user_tmpdir: Option<PathBuf>,
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
            os_user_tmpdir: os_user_tmpdir(),
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
    /// `<root>/ahr-<short id>` under the canonical `root`.
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

/// `ahr-<12 hex>`, or `None` when the run id has fewer than 12
/// hex digits.
pub(crate) fn external_dir_name(run_id: &str) -> Option<String> {
    let short = short_id(run_id);
    (short.len() == SHORT_ID_LEN).then(|| format!("{RUN_DIR_PREFIX}{short}"))
}

/// Picks the run's temporary folder. `External` folders exist on return,
/// created non-recursively with mode 0700; `tmp/` and `artifacts/` are left
/// to the caller.
pub(crate) fn choose_runtime_dir(work_root: &Path, run_id: &str, inputs: &TmpRootInputs) -> Chosen {
    let candidate = match (inputs.explicit_root(), inputs.tmpdir.as_ref()) {
        (Some(root), _) => Some((Rule::Explicit, root)),
        (None, Some(tmpdir)) => Some((Rule::CallerTmpdir, tmpdir)),
        (None, None) => None,
    };
    let mut warning = None;
    if let (Some(name), Some((rule, value))) = (external_dir_name(run_id), candidate) {
        let value = Path::new(value);
        let attempt = usable_root(work_root, rule, value, inputs).and_then(|root| {
            create_run_dir(&root, &name, inputs.euid)
                .map(|dir| RuntimeChoice::External { dir, root })
                .map_err(Refused::Warn)
        });
        match attempt {
            Ok(choice) => {
                return Chosen {
                    choice,
                    warning: None,
                };
            }
            Err(Refused::Warn(reason)) => {
                tracing::warn!(variable = rule.var(), path = ?value, %reason, "run temp root refused");
                warning = Some(warning_line(rule, value, &reason));
            }
            Err(Refused::Silent) => {}
        }
    }
    Chosen {
        choice: RuntimeChoice::Default(runtime_dir_for(work_root, run_id)),
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
    let meta = match fs::metadata(value) {
        Ok(meta) => Some(meta),
        Err(_) if !explicit => return Err(Refused::Silent),
        Err(_) => {
            // Only the I/O cause: the anyhow context repeats the path
            // unescaped, which would let a crafted value forge stderr lines.
            if let Err(error) = create_private_dir_all(value) {
                return Err(Refused::Warn(format!(
                    "could not be created: {}",
                    error.root_cause()
                )));
            }
            match fs::metadata(value) {
                Ok(meta) => Some(meta),
                Err(error) => {
                    return Err(Refused::Warn(format!(
                        "could not be inspected after creation: {error}"
                    )));
                }
            }
        }
    };
    let meta = match meta {
        Some(meta) if meta.is_dir() => meta,
        _ => return Err(warn_or_silent("is not a folder".into())),
    };
    let root = match fs::canonicalize(value) {
        Ok(root) => root,
        Err(error) => return Err(warn_or_silent(format!("could not be resolved: {error}"))),
    };

    let work = fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    // An empty work root means neither HOME nor AMPLIHACK_WORK_ROOT is set, so there is no work root to protect.
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
    // The macOS login `TMPDIR` is the system's default, not a choice: skip it
    // without a word, before the length check, so macOS runs keep the short
    // default layout. A deliberately set `TMPDIR` is still honoured.
    if !explicit && is_os_user_tmpdir(&root, inputs) {
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
    // Warned under both rules: a root past [`MAX_ROOT_BYTES`] is one the
    // caller chose, and skipping it silently would put the run back on the
    // work root's disk without a word.
    if root.as_os_str().len() > MAX_ROOT_BYTES {
        let len = root.as_os_str().len() + RUN_SUFFIX_BYTES;
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

/// Whether the resolved `root` is the macOS per-user login temp folder. The
/// folder is resolved too, so `/var/folders/...` and `/private/var/folders/...`,
/// with or without a trailing `/`, compare equal. A folder below it does not
/// count.
fn is_os_user_tmpdir(root: &Path, inputs: &TmpRootInputs) -> bool {
    let Some(login) = inputs.os_user_tmpdir.as_ref().filter(|p| p.is_absolute()) else {
        return false;
    };
    let login = fs::canonicalize(login).unwrap_or_else(|_| login.clone());
    root == login
}

/// The folder launchd gives a macOS login session as `TMPDIR`, read with
/// `confstr(_CS_DARWIN_USER_TEMP_DIR)`, or `None` if the call fails.
#[cfg(target_os = "macos")]
fn os_user_tmpdir() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    // SAFETY: a null buffer with length 0 only asks for the size needed,
    // including the terminating NUL; 0 means the call failed.
    let needed = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if needed <= 1 {
        return None;
    }
    let mut buf = vec![0u8; needed];
    // SAFETY: `buf` is writable for `needed` bytes, the length passed.
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            needed,
        )
    };
    if written == 0 || written > needed {
        return None;
    }
    buf.truncate(written - 1);
    Some(PathBuf::from(OsString::from_vec(buf)))
}

/// Only macOS has a per-user login temp folder.
#[cfg(not(target_os = "macos"))]
fn os_user_tmpdir() -> Option<PathBuf> {
    None
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
