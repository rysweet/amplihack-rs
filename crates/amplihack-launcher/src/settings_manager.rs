//! Settings manager for agent settings backup and restoration.
//!
//! Matches Python `amplihack/launcher/settings_manager.py`:
//! - Timestamped backup of settings.json
//! - Session state persistence for recovery
//! - Restore on exit

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Manages settings.json backup and restoration.
pub struct SettingsManager {
    pub settings_path: PathBuf,
    pub session_id: String,
    pub non_interactive: bool,
    pub backup_path: Option<PathBuf>,
    session_state_dir: PathBuf,
    session_state_file: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionState {
    session_id: String,
    backup_path: Option<String>,
    settings_path: String,
    timestamp: u64,
}

impl SettingsManager {
    pub fn new(settings_path: PathBuf, session_id: String, non_interactive: bool) -> Self {
        let home = dirs_or_home();
        let session_state_dir = home
            .join(".claude")
            .join("runtime")
            .join("sessions");
        let session_state_file = session_state_dir.join(format!("{session_id}_backup.json"));

        Self {
            settings_path,
            session_id,
            non_interactive,
            backup_path: None,
            session_state_dir,
            session_state_file,
        }
    }

    /// Create a timestamped backup of settings.json.
    /// Returns `(success, backup_path)`.
    pub fn create_backup(&mut self) -> (bool, Option<PathBuf>) {
        if !self.settings_path.exists() {
            return (false, None);
        }

        let timestamp = unix_timestamp();
        let file_name = self
            .settings_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let backup_name = format!("{file_name}.backup.{timestamp}");
        let backup_path = self
            .settings_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(backup_name);

        match fs::copy(&self.settings_path, &backup_path) {
            Ok(_) => {
                self.backup_path = Some(backup_path.clone());
                let _ = self.save_session_state();
                (true, Some(backup_path))
            }
            Err(e) => {
                tracing::warn!("Failed to create backup: {e}");
                (false, None)
            }
        }
    }

    /// Restore settings.json from backup.
    pub fn restore_backup(&mut self) -> bool {
        // Try loading from session state if no backup_path
        if self.backup_path.is_none() {
            let _ = self.load_session_state();
        }

        let backup = match &self.backup_path {
            Some(p) => p.clone(),
            None => return false,
        };

        if !backup.exists() {
            tracing::warn!("Backup file not found: {}", backup.display());
            return false;
        }

        match fs::copy(&backup, &self.settings_path) {
            Ok(_) => {
                let _ = fs::remove_file(&backup);
                let _ = self.cleanup_session_state();
                true
            }
            Err(e) => {
                tracing::warn!("Failed to restore backup: {e}");
                false
            }
        }
    }

    /// Persist backup info to session state file.
    pub fn save_session_state(&self) -> bool {
        if let Err(e) = fs::create_dir_all(&self.session_state_dir) {
            tracing::warn!("Failed to create session state dir: {e}");
            return false;
        }

        let state = SessionState {
            session_id: self.session_id.clone(),
            backup_path: self.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            settings_path: self.settings_path.to_string_lossy().into_owned(),
            timestamp: unix_timestamp(),
        };

        match serde_json::to_string_pretty(&state) {
            Ok(json) => fs::write(&self.session_state_file, json).is_ok(),
            Err(_) => false,
        }
    }

    /// Load backup info from session state file.
    pub fn load_session_state(&mut self) -> bool {
        if !self.session_state_file.exists() {
            return false;
        }

        let content = match fs::read_to_string(&self.session_state_file) {
            Ok(c) => c,
            Err(_) => return false,
        };

        let state: SessionState = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(_) => return false,
        };

        if let Some(ref bp) = state.backup_path {
            self.backup_path = Some(PathBuf::from(bp));
        }

        true
    }

    /// Remove session state file.
    pub fn cleanup_session_state(&self) -> bool {
        if self.session_state_file.exists() {
            fs::remove_file(&self.session_state_file).is_ok()
        } else {
            true
        }
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, SettingsManager) {
        let dir = tempfile::tempdir().unwrap();
        let settings_path = dir.path().join("settings.json");
        fs::write(&settings_path, r#"{"key": "value"}"#).unwrap();

        let mut mgr = SettingsManager::new(
            settings_path,
            "test-session-123".into(),
            true,
        );
        // Override session state dir to use temp
        mgr.session_state_dir = dir.path().join("state");
        mgr.session_state_file = dir.path().join("state").join("test-session-123_backup.json");
        (dir, mgr)
    }

    #[test]
    fn create_and_restore_backup() {
        let (_dir, mut mgr) = setup();

        let (ok, path) = mgr.create_backup();
        assert!(ok);
        assert!(path.is_some());
        let backup = path.unwrap();
        assert!(backup.exists());

        // Modify settings
        fs::write(&mgr.settings_path, r#"{"key": "modified"}"#).unwrap();

        // Restore
        assert!(mgr.restore_backup());
        let restored = fs::read_to_string(&mgr.settings_path).unwrap();
        assert!(restored.contains("value"));

        // Backup should be cleaned up
        assert!(!backup.exists());
    }

    #[test]
    fn create_backup_no_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SettingsManager::new(
            dir.path().join("nonexistent.json"),
            "s1".into(),
            true,
        );
        let (ok, path) = mgr.create_backup();
        assert!(!ok);
        assert!(path.is_none());
    }

    #[test]
    fn restore_without_backup() {
        let (_dir, mut mgr) = setup();
        assert!(!mgr.restore_backup());
    }

    #[test]
    fn session_state_save_and_load() {
        let (_dir, mut mgr) = setup();
        mgr.backup_path = Some(PathBuf::from("/fake/path/backup.json"));
        assert!(mgr.save_session_state());

        // Reset and reload
        mgr.backup_path = None;
        assert!(mgr.load_session_state());
        assert_eq!(
            mgr.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            Some("/fake/path/backup.json".to_string())
        );
    }

    #[test]
    fn cleanup_session_state() {
        let (_dir, mut mgr) = setup();
        mgr.backup_path = Some(PathBuf::from("/fake"));
        mgr.save_session_state();
        assert!(mgr.session_state_file.exists());
        assert!(mgr.cleanup_session_state());
        assert!(!mgr.session_state_file.exists());
    }

    #[test]
    fn load_session_state_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = SettingsManager::new(
            dir.path().join("settings.json"),
            "s1".into(),
            true,
        );
        mgr.session_state_file = dir.path().join("nonexistent.json");
        assert!(!mgr.load_session_state());
    }
}
