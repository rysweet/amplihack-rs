//! The profile root directory and the profiles inside it.

use super::name::ProfileName;
use super::{ProfileError, pointer};
use std::fs;
use std::path::{Path, PathBuf};

pub const ROOT_ENV: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

#[derive(Debug, Clone)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn from_root_str(raw: &str) -> Result<Self, ProfileError> {
        let path = PathBuf::from(raw);
        if raw.is_empty() || !path.is_absolute() {
            return Err(ProfileError::InvalidRoot(raw.to_string()));
        }
        Ok(Self::new(path))
    }

    pub fn from_env() -> Result<Self, ProfileError> {
        if let Some(raw) = std::env::var_os(ROOT_ENV) {
            let raw = raw
                .into_string()
                .map_err(|_| ProfileError::InvalidRoot("non-UTF-8 path".into()))?;
            return Self::from_root_str(&raw);
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .filter(|h| !h.is_empty())
            .ok_or_else(|| ProfileError::InvalidRoot("HOME is not set".into()))?;
        Ok(Self::new(
            PathBuf::from(home)
                .join(".amplihack")
                .join("claude-profiles"),
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
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
            Ok(()) => Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(ProfileError::AlreadyExists(name.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    pub fn list(&self) -> Result<Vec<String>, ProfileError> {
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

fn private_dir_builder() -> fs::DirBuilder {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b
}
