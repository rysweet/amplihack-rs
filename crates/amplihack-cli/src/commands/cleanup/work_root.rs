//! The work root: one folder for run folders, targets, clones, the archive
//! and, when set explicitly, worktrees (issue #1808, D7, D9, R4).

use super::recovery_path::{describe, display_path, inspect_components};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The folder of stable execution storage under the work root.
pub(crate) const EXEC_DIR: &str = "exec";

pub(crate) const WORK_ROOT_ENV: &str = "AMPLIHACK_WORK_ROOT";
pub(crate) const WORK_ROOT_EXPLICIT_ENV: &str = "AMPLIHACK_WORK_ROOT_EXPLICIT";

/// System folders that may never hold, or be, a work root.
const SYSTEM_PREFIXES: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/lib", "/lib32", "/lib64", "/libx32", "/proc", "/run",
    "/sbin", "/sys", "/usr",
];

/// Shared folders that may hold a work root but may not be one.
const SHARED_FOLDERS: &[&str] = &[
    "/home", "/media", "/mnt", "/opt", "/srv", "/tmp", "/var", "/var/tmp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkRoot {
    pub path: PathBuf,
    /// Whether worktrees move under the work root (see the how-to page).
    pub explicit: bool,
}

/// Resolves the work root from the two variables and `home`. Validates it and
/// never creates anything.
pub(crate) fn resolve(
    work_root_var: Option<&OsStr>,
    explicit_var: Option<&OsStr>,
    home: &Path,
) -> Result<WorkRoot> {
    let set = work_root_var.filter(|v| !v.is_empty());
    let path = match set {
        Some(value) => PathBuf::from(value),
        None => home.join(".amplihack").join("work"),
    };
    let explicit = match explicit_var.and_then(OsStr::to_str) {
        Some("1") => true,
        Some("0") => false,
        _ => set.is_some(),
    };
    validate(&path, home)?;
    Ok(WorkRoot { path, explicit })
}

/// Resolves the work root from this process's environment.
pub(crate) fn resolve_from_env() -> Result<WorkRoot> {
    let work_root_var = std::env::var_os(WORK_ROOT_ENV).filter(|v| !v.is_empty());
    let home = match crate::user_config::home_dir() {
        Some(home) => home,
        // With the root given explicitly, HOME is only needed to refuse the
        // home folder and its ancestors, so the raw variable is enough.
        None if work_root_var.is_some() => std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set, so the work root cannot be checked")?,
        None => bail!("HOME is not set, so the default work root cannot be resolved"),
    };
    resolve(
        work_root_var.as_deref(),
        std::env::var_os(WORK_ROOT_EXPLICIT_ENV).as_deref(),
        &home,
    )
}

/// Refuses a work root that cleanup could not safely manage.
pub(crate) fn validate(path: &Path, home: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("work root {} must be an absolute path", display_path(path));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!(
            "work root {} must not contain `.` or `..` components",
            display_path(path)
        );
    }
    // Classify every component before any ownership/mode advice or resolved
    // location checks. A link's target is never used to suggest chmod.
    let meta = inspect_components(path, |_, _| {})
        .map_err(|fault| anyhow::anyhow!(describe(path, &[fault])))?;
    // Compare both the path as given and its resolved form, so a symlink
    // cannot hide a refused folder.
    let mut forms = vec![path.to_path_buf()];
    if let Ok(canonical) = fs::canonicalize(path) {
        forms.push(canonical);
    }
    let mut homes = vec![home.to_path_buf()];
    if let Ok(canonical) = fs::canonicalize(home) {
        homes.push(canonical);
    }
    let temp = std::env::temp_dir();
    let mut temps = vec![temp.clone(), PathBuf::from("/tmp")];
    if let Ok(canonical) = fs::canonicalize(&temp) {
        temps.push(canonical);
    }
    for form in &forms {
        if form == Path::new("/") {
            bail!("work root {} is the root folder", display_path(path));
        }
        if homes.iter().any(|h| h.starts_with(form)) {
            bail!(
                "work root {} is your home folder or one of its ancestors",
                display_path(path)
            );
        }
        if SYSTEM_PREFIXES.iter().any(|p| form.starts_with(p)) {
            bail!("work root {} is inside a system folder", display_path(path));
        }
        if SHARED_FOLDERS.iter().any(|p| form == Path::new(p)) || temps.contains(form) {
            bail!(
                "work root {} is a shared folder such as the system temporary folder",
                display_path(path)
            );
        }
        if form.join(".git").exists() {
            bail!(
                "work root {} is the root of a git checkout",
                display_path(path)
            );
        }
    }
    if let Some(meta) = meta
        && let Some(fault) = super::recovery_path::private_directory_fault(path, &meta)
    {
        bail!(super::recovery_path::root_line(path, &fault));
    }
    Ok(())
}

impl WorkRoot {
    /// Creates the root with mode 0700, regardless of umask. Never called by a
    /// dry run.
    pub(crate) fn ensure_created(&self) -> Result<()> {
        create_private_dir_all(&self.path)
            .with_context(|| format!("cannot create work root {}", display_path(&self.path)))
    }

    pub(crate) fn runs_dir(&self) -> PathBuf {
        self.path.join("runs")
    }

    pub(crate) fn targets_dir(&self) -> PathBuf {
        self.path.join("targets")
    }

    pub(crate) fn worktrees_dir(&self) -> PathBuf {
        self.path.join("worktrees")
    }

    pub(crate) fn short_runtime_dir(&self) -> PathBuf {
        self.path.join("r")
    }

    /// `<work root>/exec/`: the stable execution storage of runs with a
    /// recovery identity (issue #2096, #2493).
    pub(crate) fn exec_dir(&self) -> PathBuf {
        self.path.join(EXEC_DIR)
    }

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.path.join(".cleanup.lock")
    }
}

/// `create_dir_all` with mode 0700 on every folder it creates.
pub(crate) fn create_private_dir_all(dir: &Path) -> Result<()> {
    amplihack_utils::secure_files::create_private_directories(dir)
        .with_context(|| format!("cannot create {}", display_path(dir)))
}

#[cfg(test)]
#[path = "tests_work_root.rs"]
mod tests;
