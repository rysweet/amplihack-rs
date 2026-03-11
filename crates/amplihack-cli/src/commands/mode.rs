//! Native mode detection and migration commands.

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

const ESSENTIAL_DIRS: [&str; 4] = ["agents", "commands", "skills", "tools"];
const CHECK_MARK: &str = "✓";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeMode {
    Local,
    Plugin,
    None,
}

impl ClaudeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Plugin => "plugin",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
struct ModeDetector {
    project_dir: PathBuf,
    home_dir: PathBuf,
}

impl ModeDetector {
    fn current() -> Result<Self> {
        Ok(Self {
            project_dir: std::env::current_dir().context("failed to read current directory")?,
            home_dir: home_dir()?,
        })
    }

    fn detect(&self) -> ClaudeMode {
        match std::env::var("AMPLIHACK_MODE") {
            Ok(value) if value.eq_ignore_ascii_case("local") && self.has_local_installation() => {
                ClaudeMode::Local
            }
            Ok(value) if value.eq_ignore_ascii_case("plugin") && self.has_plugin_installation() => {
                ClaudeMode::Plugin
            }
            _ if self.has_local_installation() => ClaudeMode::Local,
            _ if self.has_plugin_installation() => ClaudeMode::Plugin,
            _ => ClaudeMode::None,
        }
    }

    fn get_claude_dir(&self, mode: ClaudeMode) -> Option<PathBuf> {
        match mode {
            ClaudeMode::Local if self.has_local_installation() => Some(self.local_claude()),
            ClaudeMode::Plugin if self.has_plugin_installation() => Some(self.plugin_claude()),
            _ => None,
        }
    }

    fn local_claude(&self) -> PathBuf {
        self.project_dir.join(".claude")
    }

    fn plugin_root(&self) -> PathBuf {
        self.home_dir.join(".amplihack")
    }

    fn plugin_claude(&self) -> PathBuf {
        self.plugin_root().join(".claude")
    }

    fn plugin_manifest(&self) -> PathBuf {
        self.plugin_root()
            .join(".claude-plugin")
            .join("plugin.json")
    }

    fn has_local_installation(&self) -> bool {
        let local = self.local_claude();
        local.exists()
            && ESSENTIAL_DIRS
                .iter()
                .any(|entry| local.join(entry).exists())
    }

    fn has_plugin_installation(&self) -> bool {
        self.plugin_claude().exists() && self.plugin_manifest().exists()
    }
}

#[derive(Debug, Clone)]
struct MigrationHelper {
    detector: ModeDetector,
}

#[derive(Debug)]
struct MigrationInfo {
    has_local: bool,
    has_plugin: bool,
    can_migrate_to_local: bool,
    plugin_path: Option<PathBuf>,
}

impl MigrationHelper {
    fn new(detector: ModeDetector) -> Self {
        Self { detector }
    }

    fn can_migrate_to_plugin(&self) -> bool {
        self.detector.local_claude().exists() && self.detector.has_plugin_installation()
    }

    fn migrate_to_plugin(&self) -> Result<bool> {
        let local_claude = self.detector.local_claude();
        if !local_claude.exists() || !self.can_migrate_to_plugin() {
            return Ok(false);
        }
        fs::remove_dir_all(local_claude).context("failed to remove local .claude directory")?;
        Ok(true)
    }

    fn migrate_to_local(&self) -> Result<bool> {
        let local_claude = self.detector.local_claude();
        let plugin_claude = self.detector.plugin_claude();

        if local_claude.exists() || !plugin_claude.exists() {
            return Ok(false);
        }

        copy_dir_recursive(&plugin_claude, &local_claude)?;
        Ok(true)
    }

    fn get_migration_info(&self) -> MigrationInfo {
        let has_local = self.detector.local_claude().exists();
        let has_plugin = self.detector.plugin_claude().exists();

        MigrationInfo {
            has_local,
            has_plugin,
            can_migrate_to_local: has_plugin && !has_local,
            plugin_path: has_plugin.then(|| self.detector.plugin_claude()),
        }
    }
}

pub fn run_detect() -> Result<()> {
    let detector = ModeDetector::current()?;
    let mut stdout = io::stdout();
    run_detect_with(&detector, &mut stdout)
}

pub fn run_to_plugin() -> Result<()> {
    let detector = ModeDetector::current()?;
    let migrator = MigrationHelper::new(detector.clone());
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout();
    run_to_plugin_with(&detector, &migrator, &mut stdin, &mut stdout)
}

pub fn run_to_local() -> Result<()> {
    let detector = ModeDetector::current()?;
    let migrator = MigrationHelper::new(detector.clone());
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout();
    run_to_local_with(&migrator, &mut stdin, &mut stdout)
}

fn run_detect_with(detector: &ModeDetector, out: &mut impl Write) -> Result<()> {
    let mode = detector.detect();
    writeln!(out, "Claude installation mode: {}", mode.as_str())?;

    if let Some(path) = detector.get_claude_dir(mode) {
        writeln!(out, "Using .claude directory: {}", path.display())?;
    } else {
        writeln!(out, "No .claude installation found")?;
        writeln!(out, "Install amplihack with: amplihack install")?;
    }

    Ok(())
}

fn run_to_plugin_with(
    detector: &ModeDetector,
    migrator: &MigrationHelper,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<()> {
    if !migrator.can_migrate_to_plugin() {
        writeln!(out, "Cannot migrate to plugin mode:")?;
        if !detector.has_local_installation() {
            writeln!(out, "  - No local .claude/ directory found")?;
        }
        if !detector.has_plugin_installation() {
            writeln!(out, "  - Plugin not installed (run: amplihack install)")?;
        }
        return Err(exit_error(1));
    }

    writeln!(
        out,
        "This will remove local .claude/ directory: {}",
        detector.local_claude().display()
    )?;
    writeln!(out, "Plugin installation will be used instead.")?;

    if !confirm(input, out)? {
        writeln!(out, "Migration cancelled")?;
        return Ok(());
    }

    if migrator.migrate_to_plugin()? {
        writeln!(out, "{CHECK_MARK} Migrated to plugin mode successfully")?;
        writeln!(out, "Local .claude/ removed, using plugin installation")?;
        Ok(())
    } else {
        writeln!(out, "Migration failed")?;
        Err(exit_error(1))
    }
}

fn run_to_local_with(
    migrator: &MigrationHelper,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<()> {
    let info = migrator.get_migration_info();
    let project_dir = &migrator.detector.project_dir;

    if !info.can_migrate_to_local {
        writeln!(out, "Cannot create local .claude/ directory:")?;
        if info.has_local {
            writeln!(out, "  - Local .claude/ already exists")?;
        }
        if !info.has_plugin {
            writeln!(out, "  - Plugin not installed (run: amplihack install)")?;
        }
        return Err(exit_error(1));
    }

    writeln!(
        out,
        "This will create local .claude/ directory in: {}",
        project_dir.display()
    )?;
    writeln!(
        out,
        "Copying from plugin: {}",
        info.plugin_path
            .as_ref()
            .context("plugin path missing for migration")?
            .display()
    )?;

    if !confirm(input, out)? {
        writeln!(out, "Migration cancelled")?;
        return Ok(());
    }

    if migrator.migrate_to_local()? {
        writeln!(out, "{CHECK_MARK} Local .claude/ created successfully")?;
        writeln!(out, "Now using project-local installation")?;
        Ok(())
    } else {
        writeln!(out, "Migration failed")?;
        Err(exit_error(1))
    }
}

fn confirm(input: &mut impl BufRead, out: &mut impl Write) -> Result<bool> {
    write!(out, "Continue? (y/N): ")?;
    out.flush()?;

    let mut line = String::new();
    input
        .read_line(&mut line)
        .context("failed to read confirmation input")?;
    Ok(line.trim().eq_ignore_ascii_case("y"))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)
        .with_context(|| format!("failed to create directory {}", dst.display()))?;

    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_symlink() {
            tracing::debug!("Skipping symlink: {}", source_path.display());
            continue;
        } else if file_type.is_dir() {
            copy_dir_recursive(&source_path, &target_path)?;
        } else if file_type.is_file() {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }

    Ok(())
}

#[cfg(unix)]
#[allow(dead_code)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            link.display(),
            target.display()
        )
    })?;
    Ok(())
}

#[cfg(windows)]
#[allow(dead_code)]
fn create_symlink(target: &Path, link: &Path) -> Result<()> {
    let metadata = fs::metadata(target)
        .with_context(|| format!("failed to inspect symlink target {}", target.display()))?;
    if metadata.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
    .with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            link.display(),
            target.display()
        )
    })?;
    Ok(())
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::tempdir;

    fn setup_detector(local: bool, plugin: bool) -> (tempfile::TempDir, ModeDetector) {
        let root = tempdir().unwrap();
        let project_dir = root.path().join("project");
        let home_dir = root.path().join("home");
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();

        if local {
            fs::create_dir_all(project_dir.join(".claude").join("skills")).unwrap();
        }

        if plugin {
            fs::create_dir_all(home_dir.join(".amplihack/.claude/tools")).unwrap();
            fs::create_dir_all(home_dir.join(".amplihack/.claude-plugin")).unwrap();
            fs::write(home_dir.join(".amplihack/.claude-plugin/plugin.json"), "{}").unwrap();
        }

        (
            root,
            ModeDetector {
                project_dir,
                home_dir,
            },
        )
    }

    #[test]
    fn detect_prefers_local_installation() {
        let (_tmp, detector) = setup_detector(true, true);
        assert_eq!(detector.detect(), ClaudeMode::Local);
    }

    #[test]
    fn detect_falls_back_to_plugin() {
        let (_tmp, detector) = setup_detector(false, true);
        assert_eq!(detector.detect(), ClaudeMode::Plugin);
    }

    #[test]
    fn detect_reports_none() {
        let (_tmp, detector) = setup_detector(false, false);
        assert_eq!(detector.detect(), ClaudeMode::None);
    }

    #[test]
    fn run_detect_matches_python_output_shape() {
        let (_tmp, detector) = setup_detector(true, false);
        let mut output = Vec::new();
        run_detect_with(&detector, &mut output).unwrap();
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Claude installation mode: local"));
        assert!(rendered.contains("Using .claude directory:"));
    }

    #[test]
    fn to_plugin_cancels_cleanly() {
        let (_tmp, detector) = setup_detector(true, true);
        let migrator = MigrationHelper::new(detector.clone());
        let mut input = io::Cursor::new("n\n");
        let mut output = Vec::new();

        run_to_plugin_with(&detector, &migrator, &mut input, &mut output).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Continue? (y/N): "));
        assert!(rendered.contains("Migration cancelled"));
        assert!(detector.local_claude().exists());
    }

    #[test]
    fn to_plugin_removes_local_directory() {
        let (_tmp, detector) = setup_detector(true, true);
        let migrator = MigrationHelper::new(detector.clone());
        let mut input = io::Cursor::new("y\n");
        let mut output = Vec::new();

        run_to_plugin_with(&detector, &migrator, &mut input, &mut output).unwrap();

        assert!(!detector.local_claude().exists());
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Migrated to plugin mode successfully"));
    }

    #[test]
    fn to_local_copies_plugin_directory() {
        let (_tmp, detector) = setup_detector(false, true);
        fs::create_dir_all(detector.plugin_claude().join("skills")).unwrap();
        fs::write(detector.plugin_claude().join("skills/example.txt"), "hello").unwrap();
        let migrator = MigrationHelper::new(detector.clone());
        let mut input = io::Cursor::new("y\n");
        let mut output = Vec::new();

        run_to_local_with(&migrator, &mut input, &mut output).unwrap();

        assert_eq!(
            fs::read_to_string(detector.local_claude().join("skills/example.txt")).unwrap(),
            "hello"
        );
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Local .claude/ created successfully"));
    }

    #[test]
    fn to_local_reports_missing_plugin() {
        let (_tmp, detector) = setup_detector(false, false);
        let migrator = MigrationHelper::new(detector);
        let mut input = io::Cursor::new("");
        let mut output = Vec::new();

        let err = run_to_local_with(&migrator, &mut input, &mut output).unwrap_err();
        assert!(crate::command_error::exit_code(&err).is_some());
        assert!(
            String::from_utf8(output)
                .unwrap()
                .contains("Plugin not installed")
        );
    }

    #[test]
    fn confirm_accepts_yes_only() {
        let mut yes = io::Cursor::new("y\n");
        let mut out = Vec::new();
        assert!(confirm(&mut yes, &mut out).unwrap());

        let mut no = io::Cursor::new("yes\n");
        let mut out = Vec::new();
        assert!(!confirm(&mut no, &mut out).unwrap());
    }

    #[test]
    fn home_dir_reads_home_env() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", OsString::from("/tmp/example-home"));
        }
        let value = home_dir().unwrap();
        if let Some(old) = previous {
            unsafe {
                std::env::set_var("HOME", old);
            }
        } else {
            unsafe {
                std::env::remove_var("HOME");
            }
        }
        assert_eq!(value, PathBuf::from("/tmp/example-home"));
    }
}
