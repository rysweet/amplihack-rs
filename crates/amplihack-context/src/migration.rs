//! Migration helper for amplihack installation modes.
//!
//! Ported from `amplihack/mode_detector/migration.py`.
//!
//! Provides simple directory copy/move operations so users can
//! switch between Local (per-project `.claude/`) and Plugin
//! (`~/.amplihack/.claude/`) installation modes.

use std::fs;
use std::path::{Path, PathBuf};

/// Information about the available migration paths for a project.
#[derive(Debug, Clone)]
pub struct MigrationInfo {
    /// Whether the project has a local `.claude/` directory.
    pub has_local: bool,
    /// Whether the global plugin `.claude/` directory exists.
    pub has_plugin: bool,
    /// Whether migration from local to plugin mode is possible.
    pub can_migrate_to_plugin: bool,
    /// Whether migration from plugin to local mode is possible.
    pub can_migrate_to_local: bool,
    /// Path to the local `.claude/` directory, if it exists.
    pub local_path: Option<PathBuf>,
    /// Path to the plugin `.claude/` directory, if it exists.
    pub plugin_path: Option<PathBuf>,
}

/// Helps users migrate between amplihack installation modes.
///
/// Supports two migration directions:
/// - **Local → Plugin**: removes the project-local `.claude/` directory
///   (the user should confirm before calling).
/// - **Plugin → Local**: copies the global plugin `.claude/` into the project.
pub struct MigrationHelper {
    plugin_claude: PathBuf,
    plugin_root: PathBuf,
}

impl MigrationHelper {
    /// Create a new helper using the default plugin paths under `~/.amplihack/`.
    ///
    /// Returns `None` when the home directory cannot be determined.
    pub fn new() -> Option<Self> {
        let home = dirs_path()?;
        let plugin_root = home.join(".amplihack");
        let plugin_claude = plugin_root.join(".claude");
        Some(Self {
            plugin_claude,
            plugin_root,
        })
    }

    /// Create a helper with explicit plugin paths (useful for testing).
    pub fn with_paths(plugin_root: PathBuf) -> Self {
        let plugin_claude = plugin_root.join(".claude");
        Self {
            plugin_claude,
            plugin_root,
        }
    }

    /// Migrate a project from local to plugin mode.
    ///
    /// This **removes** the project-local `.claude/` directory.
    /// The caller should confirm with the user before invoking this.
    ///
    /// Returns `true` on success, `false` if the migration cannot proceed.
    pub fn migrate_to_plugin(&self, project_dir: &Path) -> bool {
        let local_claude = project_dir.join(".claude");

        if !local_claude.exists() {
            return false;
        }
        if !self.can_migrate_to_plugin(project_dir) {
            return false;
        }

        fs::remove_dir_all(&local_claude).is_ok()
    }

    /// Create a local `.claude/` directory by copying from the plugin.
    ///
    /// Returns `true` on success, `false` if local already exists or the
    /// plugin directory is missing.
    pub fn migrate_to_local(&self, project_dir: &Path) -> bool {
        let local_claude = project_dir.join(".claude");

        if local_claude.exists() {
            return false; // don't overwrite existing
        }
        if !self.plugin_claude.exists() {
            return false;
        }

        copy_dir_recursive(&self.plugin_claude, &local_claude).is_ok()
    }

    /// Check whether the project can migrate to plugin mode.
    ///
    /// Requires a local `.claude/`, a plugin `.claude/`, and a plugin manifest.
    pub fn can_migrate_to_plugin(&self, project_dir: &Path) -> bool {
        let local_claude = project_dir.join(".claude");

        if !local_claude.exists() {
            return false;
        }
        if !self.plugin_claude.exists() {
            return false;
        }

        let manifest = self.plugin_root.join(".claude-plugin").join("plugin.json");
        manifest.exists()
    }

    /// Gather information about migration options for a project.
    pub fn get_migration_info(&self, project_dir: &Path) -> MigrationInfo {
        let local_claude = project_dir.join(".claude");
        let has_local = local_claude.exists();
        let has_plugin = self.plugin_claude.exists();

        MigrationInfo {
            has_local,
            has_plugin,
            can_migrate_to_plugin: self.can_migrate_to_plugin(project_dir),
            can_migrate_to_local: has_plugin && !has_local,
            local_path: if has_local {
                Some(local_claude)
            } else {
                None
            },
            plugin_path: if has_plugin {
                Some(self.plugin_claude.clone())
            } else {
                None
            },
        }
    }
}

/// Resolve the user's home directory.
fn dirs_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_helper(tmp: &TempDir) -> (MigrationHelper, PathBuf) {
        let plugin_root = tmp.path().join("plugin_root");
        fs::create_dir_all(plugin_root.join(".claude")).unwrap();
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin").join("plugin.json"),
            "{}",
        )
        .unwrap();
        let helper = MigrationHelper::with_paths(plugin_root);
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        (helper, project)
    }

    #[test]
    fn migrate_to_plugin_removes_local() {
        let tmp = TempDir::new().unwrap();
        let (helper, project) = setup_helper(&tmp);
        fs::create_dir_all(project.join(".claude")).unwrap();

        assert!(helper.migrate_to_plugin(&project));
        assert!(!project.join(".claude").exists());
    }

    #[test]
    fn migrate_to_plugin_fails_without_local() {
        let tmp = TempDir::new().unwrap();
        let (helper, project) = setup_helper(&tmp);
        assert!(!helper.migrate_to_plugin(&project));
    }

    #[test]
    fn migrate_to_local_copies_plugin() {
        let tmp = TempDir::new().unwrap();
        let (helper, project) = setup_helper(&tmp);
        // Put a file in the plugin .claude/ so we can verify it's copied.
        fs::write(
            helper.plugin_claude.join("settings.json"),
            r#"{"ok":true}"#,
        )
        .unwrap();

        assert!(helper.migrate_to_local(&project));
        assert!(project.join(".claude").join("settings.json").exists());
    }

    #[test]
    fn migrate_to_local_fails_if_local_exists() {
        let tmp = TempDir::new().unwrap();
        let (helper, project) = setup_helper(&tmp);
        fs::create_dir_all(project.join(".claude")).unwrap();
        assert!(!helper.migrate_to_local(&project));
    }

    #[test]
    fn can_migrate_to_plugin_requires_manifest() {
        let tmp = TempDir::new().unwrap();
        let plugin_root = tmp.path().join("no_manifest");
        fs::create_dir_all(plugin_root.join(".claude")).unwrap();
        // No .claude-plugin/plugin.json
        let helper = MigrationHelper::with_paths(plugin_root);
        let project = tmp.path().join("proj");
        fs::create_dir_all(project.join(".claude")).unwrap();

        assert!(!helper.can_migrate_to_plugin(&project));
    }

    #[test]
    fn get_migration_info_reports_status() {
        let tmp = TempDir::new().unwrap();
        let (helper, project) = setup_helper(&tmp);
        fs::create_dir_all(project.join(".claude")).unwrap();

        let info = helper.get_migration_info(&project);
        assert!(info.has_local);
        assert!(info.has_plugin);
        assert!(info.can_migrate_to_plugin);
        assert!(!info.can_migrate_to_local); // local already exists
        assert!(info.local_path.is_some());
        assert!(info.plugin_path.is_some());
    }
}
