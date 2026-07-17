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
        // SEC: Sanitize session_id to prevent path traversal — only allow
        // alphanumeric, hyphens, and underscores.
        let safe_session_id: String = session_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let home = dirs_or_home();
        let session_state_dir = home.join(".claude").join("runtime").join("sessions");
        let session_state_file = session_state_dir.join(format!("{safe_session_id}_backup.json"));

        Self {
            settings_path,
            session_id: safe_session_id,
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
                if !self.save_session_state() {
                    tracing::warn!(
                        backup = %backup_path.display(),
                        "created settings backup but failed to persist its pointer; a later restore may not find this backup"
                    );
                }
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
            tracing::warn!(
                dir = %self.session_state_dir.display(),
                error = %e,
                "failed to create session state dir; backup pointer will not be recoverable"
            );
            return false;
        }

        let state = SessionState {
            session_id: self.session_id.clone(),
            backup_path: self
                .backup_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            settings_path: self.settings_path.to_string_lossy().into_owned(),
            timestamp: unix_timestamp(),
        };

        // Metadata-only diagnostics: session state records point at settings.json,
        // which can hold tokens — never log record contents.
        let json = match serde_json::to_string_pretty(&state) {
            Ok(json) => json,
            Err(e) => {
                tracing::error!(
                    path = %self.session_state_file.display(),
                    error = %e,
                    "failed to serialize session state (internal error); backup pointer not saved"
                );
                return false;
            }
        };
        match fs::write(&self.session_state_file, json) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    path = %self.session_state_file.display(),
                    error = %e,
                    "failed to write session state file; backup pointer not saved"
                );
                false
            }
        }
    }

    /// Load backup info from session state file.
    pub fn load_session_state(&mut self) -> bool {
        // Missing != corrupt: an absent record is the normal "no backup this
        // session" case and stays silent; a present-but-broken record is surfaced.
        if !self.session_state_file.exists() {
            return false;
        }

        let content = match fs::read_to_string(&self.session_state_file) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    path = %self.session_state_file.display(),
                    error = %e,
                    "session state record exists but could not be read"
                );
                return false;
            }
        };

        // Log metadata only (path + serde position); the record may reference
        // secret-bearing paths, so never echo `content`.
        let state: SessionState = match serde_json::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    path = %self.session_state_file.display(),
                    error = %e,
                    "session state record is present but corrupt; cannot recover backup pointer (this is NOT the same as no record)"
                );
                return false;
            }
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

        let mut mgr = SettingsManager::new(settings_path, "test-session-123".into(), true);
        // Override session state dir to use temp
        mgr.session_state_dir = dir.path().join("state");
        mgr.session_state_file = dir
            .path()
            .join("state")
            .join("test-session-123_backup.json");
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
        let mut mgr = SettingsManager::new(dir.path().join("nonexistent.json"), "s1".into(), true);
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
            mgr.backup_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
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
        let mut mgr = SettingsManager::new(dir.path().join("settings.json"), "s1".into(), true);
        mgr.session_state_file = dir.path().join("nonexistent.json");
        assert!(!mgr.load_session_state());
    }
}

// ---------------------------------------------------------------------------
// Issue #871 — session-state failures must be observable (missing != corrupt),
// and diagnostics must not leak file contents (settings.json may hold tokens).
//
// Previously `load_session_state` collapsed both an absent and a corrupt record
// to `false` with no diagnostic, and `restore_backup`'s
// `let _ = self.load_session_state()` (line ~90) discarded that failure. These
// tests pin: absent record -> silent false; corrupt record -> error! (redacted)
// -> false; restore surfaces a corrupt record.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod issue_871_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    const FAKE_SECRET: &str = "ghp_FAKE_SECRET_do_not_log_0123456789";

    #[derive(Default)]
    struct CaptureSubscriber {
        lines: Arc<Mutex<Vec<String>>>,
        next_id: Arc<AtomicU64>,
    }

    impl Subscriber for CaptureSubscriber {
        fn enabled(&self, meta: &Metadata<'_>) -> bool {
            *meta.level() <= tracing::Level::WARN
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(self.next_id.fetch_add(1, Ordering::Relaxed) + 1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut grabber = FieldGrabber::default();
            event.record(&mut grabber);
            let mut line = event.metadata().level().to_string();
            line.push(' ');
            line.push_str(&grabber.fields.join(" "));
            self.lines
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(line);
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
        fn register_callsite(
            &self,
            _: &'static Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }
        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::WARN)
        }
    }

    #[derive(Default)]
    struct FieldGrabber {
        fields: Vec<String>,
    }
    impl Visit for FieldGrabber {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn capture<T>(op: impl FnOnce() -> T) -> (T, Vec<String>) {
        let sub = CaptureSubscriber::default();
        let lines = Arc::clone(&sub.lines);
        let out = tracing::subscriber::with_default(sub, op);
        let captured = lines.lock().unwrap_or_else(|p| p.into_inner()).clone();
        (out, captured)
    }

    fn has_level(lines: &[String], level: &str) -> bool {
        lines.iter().any(|l| l.starts_with(level))
    }

    fn joined(lines: &[String]) -> String {
        lines.join("\n")
    }

    fn mgr_with_state(dir: &Path) -> SettingsManager {
        let settings_path = dir.join("settings.json");
        fs::write(&settings_path, r#"{"key":"value"}"#).unwrap();
        let mut mgr = SettingsManager::new(settings_path, "sess-871".into(), true);
        mgr.session_state_dir = dir.join("state");
        mgr.session_state_file = dir.join("state").join("sess-871_backup.json");
        mgr
    }

    #[test]
    fn load_session_state_missing_is_silent() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = mgr_with_state(dir.path());
        // No session-state file on disk: the normal "no backup this session" case.
        let (ok, logs) = capture(|| mgr.load_session_state());
        assert!(!ok);
        assert!(
            logs.is_empty(),
            "an absent session-state record must be silent; got: {logs:?}"
        );
    }

    #[test]
    fn load_session_state_corrupt_logs_error_without_leaking_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = mgr_with_state(dir.path());
        fs::create_dir_all(&mgr.session_state_dir).unwrap();
        fs::write(
            &mgr.session_state_file,
            format!("{{ corrupt json {FAKE_SECRET}"),
        )
        .unwrap();

        let (ok, logs) = capture(|| mgr.load_session_state());
        assert!(!ok, "a corrupt record must load as false");
        assert!(
            has_level(&logs, "ERROR"),
            "a corrupt session-state record must be surfaced at error!; got: {logs:?}"
        );
        assert!(
            joined(&logs).contains("backup.json"),
            "the diagnostic must reference the record path; got: {logs:?}"
        );
        assert!(
            !joined(&logs).contains(FAKE_SECRET),
            "diagnostics must be metadata-only and must not leak file contents; got: {logs:?}"
        );
    }

    #[test]
    fn restore_backup_surfaces_corrupt_session_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut mgr = mgr_with_state(dir.path());
        mgr.backup_path = None;
        fs::create_dir_all(&mgr.session_state_dir).unwrap();
        fs::write(&mgr.session_state_file, format!("not-json {FAKE_SECRET}")).unwrap();

        // restore_backup falls back to load_session_state to recover the path;
        // a corrupt record there was previously swallowed by `let _ = ...`.
        let (restored, logs) = capture(|| mgr.restore_backup());
        assert!(!restored, "restore with a corrupt record must return false");
        assert!(
            has_level(&logs, "ERROR"),
            "restore must surface the corrupt session-state record; got: {logs:?}"
        );
        assert!(
            !joined(&logs).contains(FAKE_SECRET),
            "diagnostics must not leak file contents; got: {logs:?}"
        );
    }
}
