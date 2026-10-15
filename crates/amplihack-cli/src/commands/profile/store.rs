//! The profile root directory and the profiles inside it.

use super::name::ProfileName;
use super::{ProfileError, pointer};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";
const SHARED_ASSET_DIRS: &[&str] = &["agents", "commands", "output-styles", "rules", "skills"];
const MAX_COPY_DEPTH: usize = 16;
const MAX_COPY_ENTRIES: usize = 5_000;
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
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
                private_dir_builder().create(&self.root)?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn add(&self, name: &ProfileName) -> Result<PathBuf, ProfileError> {
        self.ensure_root()?;
        for existing in self.list()? {
            if ProfileName::parse(&existing).is_ok_and(|e| e.eq_fold(name)) {
                return Err(ProfileError::AlreadyExists(name.to_string()));
            }
        }
        let dir = self.root.join(name.as_str());
        match private_dir_builder().create(&dir) {
            Ok(()) => {
                if let Some(source) = &self.shared_assets
                    && let Err(error) = seed_shared_assets(source, &dir)
                {
                    // A failed seed must not leave a profile that appears ready.
                    let _ = fs::remove_dir_all(&dir);
                    return Err(error);
                }
                Ok(dir)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ProfileError::AlreadyExists(name.to_string()))
            }
            Err(e) => Err(e.into()),
        }
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

    let mut entries = 0usize;
    for name in SHARED_ASSET_DIRS {
        copy_allowlisted_tree(&source.join(name), &destination.join(name), 0, &mut entries)?;
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
fn copy_allowlisted_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    entries: &mut usize,
) -> Result<(), ProfileError> {
    if depth > MAX_COPY_DEPTH {
        return Err(ProfileError::SharedAssetLimit(format!(
            "directory nesting deeper than {MAX_COPY_DEPTH}"
        )));
    }
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    private_dir_builder().create(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        *entries += 1;
        if *entries > MAX_COPY_ENTRIES {
            return Err(ProfileError::SharedAssetLimit(format!(
                "more than {MAX_COPY_ENTRIES} entries"
            )));
        }
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_allowlisted_tree(&source_path, &destination_path, depth + 1, entries)?;
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
    let mut input = open_source_nofollow(source)?;
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

/// Opens without following a symlink swapped in after the metadata check, and
/// without blocking on a FIFO swapped in the same way.
fn open_source_nofollow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn copy_filtered_settings(source: &Path, destination: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let mut raw = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(open_source_nofollow(source)?, MAX_SETTINGS_BYTES + 1),
        &mut raw,
    )?;
    if raw.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(ProfileError::SharedAssetLimit(
            "settings.json larger than 1 MiB".into(),
        ));
    }
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
