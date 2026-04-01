use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Launcher identity for adaptive hook injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherType {
    Claude,
    Copilot,
    Unknown,
}

/// Persisted launcher context written by the bootstrap layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherContext {
    pub launcher_type: LauncherType,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Reads/writes `.claude/runtime/launcher_context.json` to detect
/// which agent launcher (Claude Code, Copilot, etc.) started the session.
pub struct LauncherDetector {
    context_path: PathBuf,
}

impl LauncherDetector {
    pub fn new(project_dir: &Path) -> Self {
        Self {
            context_path: project_dir.join(".claude/runtime/launcher_context.json"),
        }
    }

    /// Detect the current launcher, returning `Unknown` when the file is
    /// missing or stale (> 24 h).
    pub fn detect(&self) -> LauncherType {
        match self.read_context() {
            Ok(ctx) => {
                if self.is_stale_ctx(&ctx, 24) {
                    tracing::debug!("launcher context is stale");
                    LauncherType::Unknown
                } else {
                    ctx.launcher_type
                }
            }
            Err(_) => LauncherType::Unknown,
        }
    }

    /// Persist a new launcher context to disk.
    pub fn write_context(
        &self,
        launcher_type: LauncherType,
        command: Option<String>,
        environment: Option<String>,
    ) -> Result<()> {
        let ctx = LauncherContext {
            launcher_type,
            command,
            environment,
            timestamp: Utc::now(),
        };
        if let Some(parent) = self.context_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir: {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(&ctx)?;
        std::fs::write(&self.context_path, json)
            .with_context(|| format!("write: {}", self.context_path.display()))?;
        Ok(())
    }

    /// Whether the persisted context is older than `max_age_hours`.
    pub fn is_stale(&self, max_age_hours: i64) -> bool {
        match self.read_context() {
            Ok(ctx) => self.is_stale_ctx(&ctx, max_age_hours),
            Err(_) => true,
        }
    }

    /// Remove the context file.
    pub fn cleanup(&self) -> Result<()> {
        if self.context_path.exists() {
            std::fs::remove_file(&self.context_path)?;
        }
        Ok(())
    }

    /// Return the on-disk path for inspection/testing.
    pub fn context_path(&self) -> &Path {
        &self.context_path
    }

    fn read_context(&self) -> Result<LauncherContext> {
        let content = std::fs::read_to_string(&self.context_path)?;
        let ctx: LauncherContext = serde_json::from_str(&content)?;
        Ok(ctx)
    }

    fn is_stale_ctx(&self, ctx: &LauncherContext, max_age_hours: i64) -> bool {
        let age = Utc::now() - ctx.timestamp;
        age.num_hours() >= max_age_hours
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, LauncherDetector) {
        let dir = TempDir::new().unwrap();
        let det = LauncherDetector::new(dir.path());
        (dir, det)
    }

    #[test]
    fn detect_unknown_when_no_file() {
        let (_dir, det) = setup();
        assert_eq!(det.detect(), LauncherType::Unknown);
    }

    #[test]
    fn detect_claude_after_write() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Claude, Some("claude".into()), None)
            .unwrap();
        assert_eq!(det.detect(), LauncherType::Claude);
    }

    #[test]
    fn detect_copilot_after_write() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Copilot, None, Some("vscode".into()))
            .unwrap();
        assert_eq!(det.detect(), LauncherType::Copilot);
    }

    #[test]
    fn write_context_creates_directories() {
        let (_dir, det) = setup();
        assert!(!det.context_path().exists());
        det.write_context(LauncherType::Claude, None, None).unwrap();
        assert!(det.context_path().exists());
    }

    #[test]
    fn is_stale_returns_true_when_no_file() {
        let (_dir, det) = setup();
        assert!(det.is_stale(24));
    }

    #[test]
    fn is_stale_returns_false_for_fresh_context() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Claude, None, None).unwrap();
        assert!(!det.is_stale(24));
    }

    #[test]
    fn is_stale_returns_true_for_zero_max_age() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Claude, None, None).unwrap();
        // max_age_hours=0 means anything >= 0 hours old is stale
        assert!(det.is_stale(0));
    }

    #[test]
    fn detect_stale_context_returns_unknown() {
        let (_dir, det) = setup();
        // Write a context with a timestamp far in the past
        let ctx = LauncherContext {
            launcher_type: LauncherType::Claude,
            command: None,
            environment: None,
            timestamp: Utc::now() - chrono::Duration::hours(48),
        };
        std::fs::create_dir_all(det.context_path().parent().unwrap()).unwrap();
        std::fs::write(
            det.context_path(),
            serde_json::to_string_pretty(&ctx).unwrap(),
        )
        .unwrap();
        assert_eq!(det.detect(), LauncherType::Unknown);
    }

    #[test]
    fn cleanup_removes_file() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Claude, None, None).unwrap();
        assert!(det.context_path().exists());
        det.cleanup().unwrap();
        assert!(!det.context_path().exists());
    }

    #[test]
    fn cleanup_noop_when_missing() {
        let (_dir, det) = setup();
        det.cleanup().unwrap(); // should not error
    }

    #[test]
    fn roundtrip_serialization() {
        let (_dir, det) = setup();
        det.write_context(
            LauncherType::Copilot,
            Some("copilot-cli".into()),
            Some("terminal".into()),
        )
        .unwrap();
        let raw = std::fs::read_to_string(det.context_path()).unwrap();
        let ctx: LauncherContext = serde_json::from_str(&raw).unwrap();
        assert_eq!(ctx.launcher_type, LauncherType::Copilot);
        assert_eq!(ctx.command.as_deref(), Some("copilot-cli"));
        assert_eq!(ctx.environment.as_deref(), Some("terminal"));
    }
}
