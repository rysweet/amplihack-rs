//! Persisted dashboard configuration (`LocalFleetDashboardSummary`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::FleetLocalError;

fn default_version() -> u8 {
    1
}

/// Persisted configuration for the local session dashboard.
///
/// Saved to `~/.claude/runtime/fleet_dashboard.json` with `0o600` permissions
/// via an atomic temp-file + rename write (SEC-03).
///
/// All fields use `#[serde(default)]` so that forward-compatible JSON files
/// with unknown keys can be round-tripped without data loss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalFleetDashboardSummary {
    /// Tracked project directories.  Each path is canonicalized via
    /// `fs::canonicalize()` before insertion (SEC-02).
    #[serde(default)]
    pub projects: Vec<PathBuf>,

    /// Unix timestamp (seconds) of the last full session refresh, or `None`
    /// if no refresh has completed since the file was created.
    #[serde(default)]
    pub last_full_refresh: Option<i64>,

    /// Schema version.  Starts at 1; bump only on breaking changes.
    #[serde(default = "default_version")]
    pub version: u8,

    /// Forward-compatibility bucket for fields added in future versions.
    /// Unknown keys from newer serializations land here instead of being
    /// silently dropped.
    #[serde(default)]
    pub extras: HashMap<String, serde_json::Value>,
}

impl Default for LocalFleetDashboardSummary {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
            last_full_refresh: None,
            version: default_version(),
            extras: HashMap::new(),
        }
    }
}

impl LocalFleetDashboardSummary {
    /// Load the summary from `path`, or return the default if the file does
    /// not exist.
    ///
    /// On parse failure the existing file is renamed to `<path>.bak` and a
    /// fresh default is returned (fail-open: never blocks the dashboard).
    pub fn load(path: Option<&Path>) -> Result<Self, FleetLocalError> {
        let path = match path {
            None => return Ok(Self::default()),
            Some(p) => p,
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(FleetLocalError::Io(e)),
        };

        match serde_json::from_str::<Self>(&content) {
            Ok(summary) => Ok(summary),
            Err(_) => {
                // Fail-open: rename corrupt file and return default.
                let bak = path.with_extension("json.bak");
                let _ = std::fs::rename(path, bak);
                Ok(Self::default())
            }
        }
    }

    /// Persist the summary to `path` atomically (temp file + rename).
    ///
    /// Creates the file with `0o600` permissions on Unix (SEC-03).
    pub fn save(&self, path: &Path) -> Result<(), FleetLocalError> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let json = serde_json::to_string(self)?;

        // Write to a temp file in the same directory, then rename atomically.
        let tmp_path = parent.join(format!(".fleet_dashboard_tmp_{}.json", std::process::id()));

        // Set 0o600 permissions before writing content (SEC-03).
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o600))?;
            }

            file.write_all(json.as_bytes())?;
        }

        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Add a project directory.
    ///
    /// - Calls `fs::canonicalize()` and `is_dir()` before inserting (SEC-02).
    /// - Deduplicates: if the canonical path already exists, no-op.
    /// - Returns an error if canonicalization fails or path is not a directory.
    pub fn add_project(&mut self, path: PathBuf) -> Result<(), FleetLocalError> {
        let canonical = std::fs::canonicalize(&path)?;
        if !canonical.is_dir() {
            return Err(FleetLocalError::Io(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                "path is not a directory",
            )));
        }
        if !self.projects.contains(&canonical) {
            self.projects.push(canonical);
        }
        Ok(())
    }

    /// Remove a project directory.
    ///
    /// Uses `retain()` to remove all entries matching `path`.  No-op if the
    /// path is not tracked.
    pub fn remove_project(&mut self, path: &Path) {
        self.projects.retain(|p| p != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_dashboard_summary_default_is_sensible() {
        let s = LocalFleetDashboardSummary::default();
        assert!(s.projects.is_empty(), "default projects must be empty");
        assert!(
            s.last_full_refresh.is_none(),
            "default last_full_refresh must be None"
        );
        assert_eq!(s.version, 1, "default version must be 1");
        assert!(s.extras.is_empty(), "default extras must be empty");
    }

    /// TC-11: `LocalFleetDashboardSummary` round-trips through `serde_json`
    /// without any data loss.
    #[test]
    fn tc11_fleet_dashboard_summary_serde_roundtrip_all_fields() {
        let original = LocalFleetDashboardSummary {
            projects: vec![
                PathBuf::from("/workspace/alpha"),
                PathBuf::from("/workspace/beta"),
            ],
            last_full_refresh: Some(1_700_000_000_i64),
            version: 1,
            extras: {
                let mut m = HashMap::new();
                m.insert("custom_key".to_string(), serde_json::json!("custom_value"));
                m.insert("count".to_string(), serde_json::json!(42));
                m
            },
        };

        let json = serde_json::to_string(&original).expect("serialize must not fail");
        let restored: LocalFleetDashboardSummary =
            serde_json::from_str(&json).expect("deserialize must not fail");

        assert_eq!(
            restored.projects, original.projects,
            "projects field not preserved"
        );
        assert_eq!(
            restored.last_full_refresh, original.last_full_refresh,
            "last_full_refresh not preserved"
        );
        assert_eq!(restored.version, original.version, "version not preserved");
        assert_eq!(
            restored.extras, original.extras,
            "extras field not preserved"
        );
    }

    #[test]
    fn tc11_fleet_dashboard_summary_serde_roundtrip_empty() {
        let original = LocalFleetDashboardSummary::default();
        let json = serde_json::to_string(&original).expect("serialize");
        let restored: LocalFleetDashboardSummary =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.projects, original.projects);
        assert_eq!(restored.last_full_refresh, original.last_full_refresh);
        assert_eq!(restored.version, original.version);
    }

    #[test]
    fn fleet_dashboard_summary_deserializes_with_only_required_version_field() {
        let json = r#"{"version": 2}"#;
        let s: LocalFleetDashboardSummary = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.version, 2);
        assert!(s.projects.is_empty());
        assert!(s.last_full_refresh.is_none());
        assert!(s.extras.is_empty());
    }

    #[test]
    fn fleet_dashboard_summary_deserializes_missing_version_uses_default() {
        let json = r#"{"projects": ["/tmp/foo"]}"#;
        let s: LocalFleetDashboardSummary = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.version, 1, "missing version should default to 1");
        assert_eq!(s.projects, vec![PathBuf::from("/tmp/foo")]);
    }

    #[test]
    fn fleet_dashboard_summary_preserves_extras_on_roundtrip() {
        let json = r#"{"projects":[],"version":1,"last_full_refresh":null,"extras":{"future_flag":true,"count":7}}"#;
        let s: LocalFleetDashboardSummary = serde_json::from_str(json).expect("deserialize");
        assert_eq!(s.extras["future_flag"], serde_json::json!(true));
        assert_eq!(s.extras["count"], serde_json::json!(7));

        let back = serde_json::to_string(&s).expect("re-serialize");
        let s2: LocalFleetDashboardSummary = serde_json::from_str(&back).expect("re-deserialize");
        assert_eq!(s2.extras, s.extras);
    }
}
