//! The work root: one folder for run folders, targets, clones, the archive
//! and, when set explicitly, worktrees (issue #1808, D7, D9, R4).

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

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
        bail!("work root {} must be an absolute path", path.display());
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!(
            "work root {} must not contain `.` or `..` components",
            path.display()
        );
    }
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
            bail!("work root {} is the root folder", path.display());
        }
        if homes.iter().any(|h| h.starts_with(form)) {
            bail!(
                "work root {} is your home folder or one of its ancestors",
                path.display()
            );
        }
        if SYSTEM_PREFIXES.iter().any(|p| form.starts_with(p)) {
            bail!("work root {} is inside a system folder", path.display());
        }
        if SHARED_FOLDERS.iter().any(|p| form == Path::new(p)) || temps.contains(form) {
            bail!(
                "work root {} is a shared folder such as the system temporary folder",
                path.display()
            );
        }
        if form.join(".git").exists() {
            bail!("work root {} is the root of a git checkout", path.display());
        }
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let meta = if meta.file_type().is_symlink() {
                fs::metadata(path)
                    .with_context(|| format!("work root {} is a broken link", path.display()))?
            } else {
                meta
            };
            if !meta.is_dir() {
                bail!("work root {} is not a folder", path.display());
            }
            // SAFETY: geteuid has no preconditions and cannot fail.
            let euid = unsafe { libc::geteuid() };
            if meta.uid() != euid {
                bail!("work root {} is not owned by you", path.display());
            }
            if meta.mode() & 0o022 != 0 {
                bail!(
                    "work root {} is writable by group or others; fix it with `chmod 700 {}`",
                    path.display(),
                    path.display()
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            Err(error).with_context(|| format!("cannot inspect work root {}", path.display()))
        }
    }
}

impl WorkRoot {
    /// Creates the root with mode 0700, regardless of umask. Never called by a
    /// dry run.
    pub(crate) fn ensure_created(&self) -> Result<()> {
        if !self.path.is_dir() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.path)
                .with_context(|| format!("cannot create work root {}", self.path.display()))?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("cannot set mode 0700 on {}", self.path.display()))?;
        }
        Ok(())
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

    pub(crate) fn lock_path(&self) -> PathBuf {
        self.path.join(".cleanup.lock")
    }
}

/// `create_dir_all` with mode 0700 on every folder it creates.
pub(crate) fn create_private_dir_all(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("cannot set mode 0700 on {}", dir.display()))
}

#[cfg(test)]
#[path = "tests_work_root.rs"]
mod tests;
