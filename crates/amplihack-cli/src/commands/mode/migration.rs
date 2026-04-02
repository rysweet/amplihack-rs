//! Migration logic for switching between local and plugin modes.

use crate::command_error::exit_error;
use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use super::{CHECK_MARK, ModeDetector};

#[derive(Debug, Clone)]
pub(super) struct MigrationHelper {
    pub(super) detector: ModeDetector,
}

#[derive(Debug)]
struct MigrationInfo {
    has_local: bool,
    has_plugin: bool,
    can_migrate_to_local: bool,
    plugin_path: Option<PathBuf>,
}

impl MigrationHelper {
    pub(super) fn new(detector: ModeDetector) -> Self {
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

pub(super) fn run_detect_with(detector: &ModeDetector, out: &mut impl Write) -> Result<()> {
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

pub(super) fn run_to_plugin_with(
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

pub(super) fn run_to_local_with(
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
pub(super) fn create_symlink(target: &Path, link: &Path) -> Result<()> {
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
pub(super) fn create_symlink(target: &Path, link: &Path) -> Result<()> {
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
