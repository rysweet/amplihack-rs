use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Default staleness threshold in hours.
const DEFAULT_STALENESS_HOURS: i64 = 24;

/// Launcher identity for adaptive hook injection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherType {
    Claude,
    Copilot,
    Unknown,
}

impl std::fmt::Display for LauncherType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Copilot => write!(f, "copilot"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Persisted launcher context written by the bootstrap layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LauncherContext {
    pub launcher_type: LauncherType,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub environment: Option<HashMap<String, String>>,
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

    /// Detect the current launcher, defaulting to `Claude` when the file is
    /// missing, malformed, or stale (> 24 h) — matching Python's fail-safe.
    pub fn detect(&self) -> LauncherType {
        self.detect_with_staleness(DEFAULT_STALENESS_HOURS)
    }

    /// Detect with a custom staleness threshold.
    pub fn detect_with_staleness(&self, max_age_hours: i64) -> LauncherType {
        match self.read_context() {
            Ok(ctx) => {
                if self.is_stale_ctx(&ctx, max_age_hours) {
                    tracing::debug!("launcher context is stale, defaulting to claude");
                    LauncherType::Claude
                } else {
                    ctx.launcher_type
                }
            }
            Err(_) => {
                tracing::debug!("no launcher context found, defaulting to claude");
                LauncherType::Claude
            }
        }
    }

    /// Persist a new launcher context to disk.
    pub fn write_context(
        &self,
        launcher_type: LauncherType,
        command: Option<String>,
        environment: Option<HashMap<String, String>>,
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

        // Best-effort chmod 600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                &self.context_path,
                std::fs::Permissions::from_mode(0o600),
            );
        }

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
    fn detect_defaults_to_claude_when_no_file() {
        let (_dir, det) = setup();
        assert_eq!(det.detect(), LauncherType::Claude);
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
        let env = HashMap::from([("editor".to_string(), "vscode".to_string())]);
        det.write_context(LauncherType::Copilot, None, Some(env))
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
        assert!(det.is_stale(0));
    }

    #[test]
    fn detect_stale_defaults_to_claude() {
        let (_dir, det) = setup();
        let ctx = LauncherContext {
            launcher_type: LauncherType::Copilot,
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
        // Stale context defaults to Claude (Python parity)
        assert_eq!(det.detect(), LauncherType::Claude);
    }

    #[test]
    fn detect_with_custom_staleness() {
        let (_dir, det) = setup();
        det.write_context(LauncherType::Copilot, None, None)
            .unwrap();
        // Fresh with 24h threshold
        assert_eq!(det.detect_with_staleness(24), LauncherType::Copilot);
        // Stale with 0h threshold — defaults to Claude
        assert_eq!(det.detect_with_staleness(0), LauncherType::Claude);
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
        det.cleanup().unwrap();
    }

    #[test]
    fn roundtrip_serialization_with_environment() {
        let (_dir, det) = setup();
        let env = HashMap::from([
            ("editor".to_string(), "vscode".to_string()),
            ("shell".to_string(), "bash".to_string()),
        ]);
        det.write_context(LauncherType::Copilot, Some("copilot-cli".into()), Some(env))
            .unwrap();
        let raw = std::fs::read_to_string(det.context_path()).unwrap();
        let ctx: LauncherContext = serde_json::from_str(&raw).unwrap();
        assert_eq!(ctx.launcher_type, LauncherType::Copilot);
        assert_eq!(ctx.command.as_deref(), Some("copilot-cli"));
        let env = ctx.environment.unwrap();
        assert_eq!(env.get("editor").unwrap(), "vscode");
        assert_eq!(env.get("shell").unwrap(), "bash");
    }

    #[test]
    fn launcher_type_display() {
        assert_eq!(LauncherType::Claude.to_string(), "claude");
        assert_eq!(LauncherType::Copilot.to_string(), "copilot");
        assert_eq!(LauncherType::Unknown.to_string(), "unknown");
    }

    #[cfg(unix)]
    #[test]
    fn write_context_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, det) = setup();
        det.write_context(LauncherType::Claude, None, None).unwrap();
        let meta = std::fs::metadata(det.context_path()).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
