//! The profile root directory and the profiles inside it.

use super::name::ProfileName;
use super::{ProfileError, pointer};
use fs4::fs_std::FileExt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
const SHARED_ASSET_DIRS: &[&str] = &["agents", "commands", "output-styles", "rules", "skills"];
const SHARED_ASSET_FILES: &[&str] = &["CLAUDE.md"];
const SHARED_SETTINGS_KEYS: &[&str] = &[
    "alwaysThinkingEnabled",
    "attribution",
    "cleanupPeriodDays",
    "hooks",
    "includeCoAuthoredBy",
    "language",
    "model",
    "outputStyle",
    "permissions",
    "respectGitignore",
    "spinnerVerbs",
    "statusLine",
];

#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
    shared_assets: Option<PathBuf>,
}

impl ProfileStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            shared_assets: None,
        }
    }

    /// Use a fixture or user Claude config as the source of allowlisted assets.
    pub fn with_shared_assets(mut self, source: PathBuf) -> Self {
        self.shared_assets = Some(source);
        self
    }

    pub fn from_root_str(raw: &str) -> Result<Self, ProfileError> {
        let path = PathBuf::from(raw);
        if raw.is_empty() || !path.is_absolute() {
            return Err(ProfileError::InvalidRoot(raw.to_string()));
        }
        Ok(Self::new(path))
    }

    pub fn from_env() -> Result<Self, ProfileError> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        let root = if let Some(raw) = std::env::var_os(ROOT_ENV) {
            let raw = raw
                .into_string()
                .map_err(|_| ProfileError::InvalidRoot("non-UTF-8 path".into()))?;
            Self::from_root_str(&raw)?.root
        } else {
            home.as_ref()
                .ok_or_else(|| ProfileError::InvalidRoot("HOME is not set".into()))?
                .join(".amplihack")
                .join("claude-profiles")
        };
        let store = Self::new(root);
        Ok(match home {
            Some(home) => store.with_shared_assets(home.join(".claude")),
            None => store,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Non-creating check: a missing root is fine, a symlinked one is not.
    pub fn check_root_not_symlink(&self) -> Result<(), ProfileError> {
        check_root_not_symlink(&self.root)
    }

    fn ensure_root(&self) -> Result<(), ProfileError> {
        match fs::symlink_metadata(&self.root) {
            Ok(m) if m.file_type().is_symlink() => Err(ProfileError::SymlinkRefused(
                self.root.display().to_string(),
            )),
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(ProfileError::InvalidRoot("not a directory".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = self.root.parent() {
                    fs::create_dir_all(parent)?;
                }
                match private_dir_builder().create(&self.root) {
                    Ok(()) => Ok(()),
                    // Another process may have created the root after the first check.
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        match fs::symlink_metadata(&self.root) {
                            Ok(metadata)
                                if metadata.is_dir() && !metadata.file_type().is_symlink() =>
                            {
                                Ok(())
                            }
                            _ => Err(ProfileError::InvalidRoot(
                                "profile root changed during creation".into(),
                            )),
                        }
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn add(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.check_root_not_symlink()?;
        if let Some(source) = &self.shared_assets {
            reject_root_inside_source(&self.root, source)?;
        }
        self.ensure_root()?;
        let _lock = add_lock(&self.root)?;
        for existing in self.list()? {
            if ProfileName::parse(&existing).is_ok_and(|e| e.eq_fold(name)) {
                return Err(ProfileError::AlreadyExists(name.to_string()));
            }
        }
        let dir = self.root.join(name.as_str());
        match fs::symlink_metadata(&dir) {
            Ok(_) => return Err(ProfileError::AlreadyExists(name.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let staging = create_staging_dir(&self.root, name)?;
        let result = if let Some(source) = &self.shared_assets {
            seed_shared_assets(source, &staging)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            return Err(discard_staging(&staging, error));
        }
        if let Err(error) = rename_noreplace(&staging, &dir) {
            let error = if error.kind() == std::io::ErrorKind::AlreadyExists {
                ProfileError::AlreadyExists(name.to_string())
            } else {
                ProfileError::Io(error)
            };
            return Err(discard_staging(&staging, error));
        }
        Ok(dir)
    }

    pub fn list(&self) -> Result<Vec<String>, ProfileError> {
        self.check_root_not_symlink()?;
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // DirEntry::file_type does not follow symlinks.
            if entry.file_type()?.is_dir() && ProfileName::parse(&name).is_ok() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    /// Verifies the profile is a real directory directly inside the root and
    /// returns its canonical path.
    pub fn resolve(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.check_root_not_symlink()?;
        let path = self.root.join(name.as_str());
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(ProfileError::SymlinkRefused(name.to_string()));
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(ProfileError::NotFound(name.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProfileError::NotFound(name.to_string()));
            }
            Err(e) => return Err(e.into()),
        }
        let canonical = path.canonicalize()?;
        let root = self.root.canonicalize()?;
        if canonical.parent() != Some(root.as_path()) {
            return Err(ProfileError::SymlinkRefused(name.to_string()));
        }
        Ok(canonical)
    }

    pub fn remove(&self, name: &ProfileName, yes: bool) -> Result<(), ProfileError> {
        if !yes {
            return Err(ProfileError::ConfirmationRequired);
        }
        let dir = self.resolve(name)?;
        let was_default = matches!(pointer::read(&self.root), Ok(Some(ref d)) if d == name);
        fs::remove_dir_all(dir)?;
        if was_default {
            pointer::clear(&self.root)?;
        }
        Ok(())
    }
}

pub(super) fn check_root_not_symlink(root: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(root) {
        Ok(m) if m.file_type().is_symlink() => {
            Err(ProfileError::SymlinkRefused(root.display().to_string()))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn private_dir_builder() -> fs::DirBuilder {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b
}

fn add_lock(root: &Path) -> Result<fs::File, ProfileError> {
    let path = root.join(".add.lock");
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ProfileError::InvalidRoot(
            "profile add lock is not a regular file".into(),
        ));
    }
    file.lock_exclusive()?;
    Ok(file)
}

fn create_staging_dir(root: &Path, name: &ProfileName) -> Result<PathBuf, ProfileError> {
    static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let id = NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            ".seed-{}-{}-{id}",
            name.as_str(),
            std::process::id()
        ));
        match private_dir_builder().create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(ProfileError::InvalidRoot(
        "unable to allocate a private staging directory".into(),
    ))
}

fn discard_staging(path: &Path, seed_error: ProfileError) -> ProfileError {
    match fs::remove_dir_all(path) {
        Ok(()) => seed_error,
        Err(cleanup_error) => ProfileError::SeedCleanupFailed {
            path: path.display().to_string(),
            seed_error: seed_error.to_string(),
            cleanup_error: cleanup_error.to_string(),
        },
    }
}

fn reject_root_inside_source(root: &Path, source: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ProfileError::SymlinkRefused(source.display().to_string()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let source = canonicalize_with_missing(source)?;
    let root = canonicalize_with_missing(root)?;
    if root.starts_with(&source) {
        return Err(ProfileError::SharedSourceContainsRoot(
            root.display().to_string(),
        ));
    }
    Ok(())
}

fn canonicalize_with_missing(path: &Path) -> Result<PathBuf, ProfileError> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&existing) {
            Ok(_) => {
                let mut resolved = existing.canonicalize()?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(ProfileError::InvalidRoot(
                        "cannot resolve profile path".into(),
                    ));
                };
                missing.push(name.to_os_string());
                let Some(parent) = existing.parent() else {
                    return Err(ProfileError::InvalidRoot(
                        "cannot resolve profile path".into(),
                    ));
                };
                existing = parent.to_path_buf();
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_NOREPLACE
        // prevents replacing any concurrently-created destination.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
        // SAFETY: both paths are valid NUL-terminated strings; RENAME_EXCL
        // prevents replacing any concurrently-created destination.
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        // Windows MoveFile semantics fail when the destination already exists.
        fs::rename(source, destination)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (source, destination);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace directory rename is unsupported on this platform",
        ))
    }
}

fn seed_shared_assets(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(ProfileError::SymlinkRefused(source.display().to_string()));
        }
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let source_root = source.canonicalize()?;
    let profile_root = destination
        .parent()
        .ok_or_else(|| ProfileError::InvalidRoot("profile staging directory has no parent".into()))?
        .canonicalize()?;
    if profile_root.starts_with(&source_root) {
        return Err(ProfileError::SharedSourceContainsRoot(
            profile_root.display().to_string(),
        ));
    }

    for name in SHARED_ASSET_DIRS {
        copy_allowlisted_tree(&source.join(name), &destination.join(name))?;
    }
    for name in SHARED_ASSET_FILES {
        copy_regular_file(&source.join(name), &destination.join(name))?;
    }
    copy_filtered_settings(
        &source.join("settings.json"),
        &destination.join("settings.json"),
    )
}

/// Copies only regular files and directories. Symlinks and special files are
/// skipped so shared assets cannot pull credentials or arbitrary paths in.
fn copy_allowlisted_tree(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    private_dir_builder().create(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_allowlisted_tree(&source_path, &destination_path)?;
        } else if kind.is_file() {
            copy_regular_file(&source_path, &destination_path)?;
        }
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let Some(mut input) = open_regular_source_file(source)? else {
        return Ok(());
    };
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    Ok(())
}

fn copy_filtered_settings(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let Some(mut file) = open_regular_source_file(source)? else {
        return Ok(());
    };
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    let value: serde_json::Value = serde_json::from_slice(&raw)
        .map_err(|error| ProfileError::InvalidSharedSettings(error.to_string()))?;
    let Some(settings) = value.as_object() else {
        return Err(ProfileError::InvalidSharedSettings(
            "expected a JSON object".into(),
        ));
    };
    let safe: serde_json::Map<String, serde_json::Value> = settings
        .iter()
        .filter(|(key, _)| SHARED_SETTINGS_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if safe.is_empty() {
        return Ok(());
    }
    let contents = serde_json::to_vec_pretty(&safe)
        .map_err(|error| ProfileError::InvalidSharedSettings(error.to_string()))?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(destination)?.write_all(&contents)?;
    Ok(())
}

/// Reject a final-component symlink even if it appears between metadata and open.
/// Concurrent mutation of the same-user source tree is outside this feature's threat model.
fn open_regular_source_file(source: &Path) -> Result<Option<fs::File>, ProfileError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = match options.open(source) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Ok(None);
        }
    }
    Ok(Some(file))
}
