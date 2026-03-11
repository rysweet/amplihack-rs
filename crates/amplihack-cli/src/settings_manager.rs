//! Atomic settings.json CRUD using `AtomicJsonFile` from amplihack-state.
//!
//! Provides a high-level interface for reading and writing amplihack settings
//! (both global `~/.claude/settings.json` and project-local `.claude/settings.json`).

use amplihack_state::AtomicJsonFile;
use amplihack_types::{ProjectDirs, Settings};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Manages settings.json files for amplihack.
pub struct SettingsManager {
    global: AtomicJsonFile,
    local: Option<AtomicJsonFile>,
}

impl SettingsManager {
    /// Create a settings manager for the global settings only.
    pub fn global_only() -> Result<Self> {
        let global_path = global_settings_path()
            .context("could not determine global settings path (HOME not set)")?;
        Ok(Self {
            global: AtomicJsonFile::new(global_path),
            local: None,
        })
    }

    /// Create a settings manager for both global and project-local settings.
    pub fn with_project(project_dirs: &ProjectDirs) -> Result<Self> {
        let global_path = global_settings_path()
            .context("could not determine global settings path (HOME not set)")?;
        let local_path = project_dirs.claude.join("settings.json");
        Ok(Self {
            global: AtomicJsonFile::new(global_path),
            local: Some(AtomicJsonFile::new(local_path)),
        })
    }

    /// Read global settings. Returns default if file doesn't exist.
    pub fn read_global(&self) -> Result<Settings> {
        self.global
            .read_or_default()
            .context("failed to read global settings")
    }

    /// Read project-local settings. Returns default if file doesn't exist.
    pub fn read_local(&self) -> Result<Option<Settings>> {
        match &self.local {
            Some(local) => local.read().context("failed to read local settings"),
            None => Ok(None),
        }
    }

    /// Write global settings atomically.
    pub fn write_global(&self, settings: &Settings) -> Result<()> {
        self.global
            .write(settings)
            .context("failed to write global settings")
    }

    /// Write project-local settings atomically.
    pub fn write_local(&self, settings: &Settings) -> Result<()> {
        let local = self
            .local
            .as_ref()
            .context("no project-local settings configured")?;
        local
            .write(settings)
            .context("failed to write local settings")
    }

    /// Update global settings with a mutation function.
    pub fn update_global<F>(&self, f: F) -> Result<Settings>
    where
        F: FnOnce(&mut Settings),
    {
        self.global
            .update(f)
            .context("failed to update global settings")
    }

    /// Return the global settings file path.
    pub fn global_path(&self) -> &std::path::Path {
        self.global.path()
    }
}

fn global_settings_path() -> Option<PathBuf> {
    ProjectDirs::global_settings()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_global_returns_default_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("settings.json"));
        let mgr = SettingsManager {
            global: file,
            local: None,
        };
        let settings = mgr.read_global().unwrap();
        assert!(settings.hooks.is_empty());
    }

    #[test]
    fn write_and_read_global_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("settings.json"));
        let mgr = SettingsManager {
            global: file,
            local: None,
        };

        let mut settings = Settings::default();
        settings
            .extra
            .insert("test_key".to_string(), serde_json::json!("test_value"));
        mgr.write_global(&settings).unwrap();

        let read = mgr.read_global().unwrap();
        assert_eq!(read.extra.get("test_key").unwrap(), "test_value");
    }

    #[test]
    fn update_global_modifies_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("settings.json"));
        let mgr = SettingsManager {
            global: file,
            local: None,
        };

        let result = mgr
            .update_global(|s| {
                s.extra
                    .insert("updated".to_string(), serde_json::json!(true));
            })
            .unwrap();
        assert_eq!(result.extra.get("updated").unwrap(), true);
    }

    #[test]
    fn read_local_returns_none_without_project() {
        let dir = tempfile::tempdir().unwrap();
        let file = AtomicJsonFile::new(dir.path().join("settings.json"));
        let mgr = SettingsManager {
            global: file,
            local: None,
        };
        assert!(mgr.read_local().unwrap().is_none());
    }

    #[test]
    fn local_settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let global_file = AtomicJsonFile::new(dir.path().join("global.json"));
        let local_file = AtomicJsonFile::new(dir.path().join("local.json"));
        let mgr = SettingsManager {
            global: global_file,
            local: Some(local_file),
        };

        let settings = Settings::default();
        mgr.write_local(&settings).unwrap();
        let read = mgr.read_local().unwrap();
        assert!(read.is_some());
    }
}
