//! Shared launcher context persistence for launcher commands and hooks.

use amplihack_types::ProjectDirs;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_STALE_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherKind {
    Claude,
    Copilot,
    Codex,
    Amplifier,
    Unknown,
}

impl LauncherKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Copilot => "copilot",
            Self::Codex => "codex",
            Self::Amplifier => "amplifier",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LauncherContext {
    pub launcher: LauncherKind,
    pub command: String,
    pub timestamp: String,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

pub fn write_launcher_context(
    project_root: &Path,
    launcher: LauncherKind,
    command: impl Into<String>,
    environment: BTreeMap<String, String>,
) -> Result<PathBuf> {
    let dirs = ProjectDirs::from_root(project_root);
    fs::create_dir_all(&dirs.runtime)
        .with_context(|| format!("failed to create {}", dirs.runtime.display()))?;
    let context_path = dirs.launcher_context_file();
    let context = LauncherContext {
        launcher,
        command: command.into(),
        timestamp: Utc::now().to_rfc3339(),
        environment,
    };
    let body =
        serde_json::to_string_pretty(&context).context("failed to encode launcher context")?;
    fs::write(&context_path, body)
        .with_context(|| format!("failed to write {}", context_path.display()))?;
    restrict_permissions(&context_path);
    Ok(context_path)
}

pub fn read_launcher_context(project_root: &Path) -> Option<LauncherContext> {
    let path = launcher_context_path(project_root);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                "failed reading launcher context: {error}"
            );
            return None;
        }
    };
    match serde_json::from_str::<LauncherContext>(&raw) {
        Ok(context) => Some(context),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                "invalid launcher context file: {error}"
            );
            None
        }
    }
}

pub fn launcher_context_path(project_root: &Path) -> PathBuf {
    ProjectDirs::from_root(project_root).launcher_context_file()
}

pub fn is_launcher_context_stale(context: &LauncherContext) -> bool {
    is_launcher_context_stale_with(context, DEFAULT_STALE_HOURS)
}

fn is_launcher_context_stale_with(context: &LauncherContext, max_age_hours: i64) -> bool {
    let Ok(timestamp) = DateTime::parse_from_rfc3339(&context.timestamp) else {
        return true;
    };
    let age = Utc::now().signed_duration_since(timestamp.with_timezone(&Utc));
    age > Duration::hours(max_age_hours)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        if let Err(e) = fs::set_permissions(path, permissions) {
            tracing::warn!(path = %path.display(), error = %e, "failed to restrict file permissions");
        }
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_launcher_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut environment = BTreeMap::new();
        environment.insert("AMPLIHACK_LAUNCHER".to_string(), "copilot".to_string());

        let path = write_launcher_context(
            dir.path(),
            LauncherKind::Copilot,
            "amplihack copilot --model opus",
            environment.clone(),
        )
        .unwrap();

        assert_eq!(path, launcher_context_path(dir.path()));
        let restored = read_launcher_context(dir.path()).unwrap();
        assert_eq!(restored.launcher, LauncherKind::Copilot);
        assert_eq!(restored.command, "amplihack copilot --model opus");
        assert_eq!(restored.environment, environment);
        assert!(!is_launcher_context_stale(&restored));
    }

    #[test]
    fn treats_old_context_as_stale() {
        let context = LauncherContext {
            launcher: LauncherKind::Copilot,
            command: "amplihack copilot".to_string(),
            timestamp: (Utc::now() - Duration::hours(25)).to_rfc3339(),
            environment: BTreeMap::new(),
        };
        assert!(is_launcher_context_stale(&context));
    }

    #[test]
    fn invalid_context_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = launcher_context_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{not-json").unwrap();

        assert!(read_launcher_context(dir.path()).is_none());
    }
}
