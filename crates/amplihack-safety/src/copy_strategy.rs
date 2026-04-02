//! Safe copy strategy for conflict-free file operations.
//!
//! Determines where to copy files based on conflict status. If conflicts
//! exist, offers overwrite / temp-dir / cancel options matching the Python
//! `SafeCopyStrategy`.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use thiserror::Error;

/// Error type for copy strategy operations.
#[derive(Debug, Error)]
pub enum CopyStrategyError {
    #[error("user cancelled")]
    Cancelled,
    #[error("io error: {0}")]
    Io(#[from] io::Error),
}

/// Where to copy files and how.
#[derive(Debug)]
pub struct CopyStrategy {
    pub target_dir: PathBuf,
    pub should_proceed: bool,
    pub use_temp: bool,
    /// Holds the temp directory alive. Dropping this removes the temp dir.
    pub _temp_handle: Option<TempDir>,
}

/// Determine safe copy target based on conflict detection.
///
/// ALWAYS stages to working directory. If conflicts exist, prompts user
/// to confirm overwrite (auto-approves in non-interactive mode).
pub struct SafeCopyStrategy;

/// User choice when conflicts are detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Overwrite,
    TempDir,
    Cancel,
}

impl SafeCopyStrategy {
    /// Determine where to copy files based on conflict status.
    ///
    /// - No conflicts → proceed to original target.
    /// - `auto_approve` → proceed regardless.
    /// - Otherwise → prompt user for choice.
    pub fn determine_target(
        original_target: impl AsRef<Path>,
        has_conflicts: bool,
        conflicting_files: &[String],
        auto_approve: bool,
    ) -> Result<CopyStrategy, CopyStrategyError> {
        let original = original_target.as_ref().to_path_buf();

        if !has_conflicts || auto_approve {
            return Ok(CopyStrategy {
                target_dir: original,
                should_proceed: true,
                use_temp: false,
                _temp_handle: None,
            });
        }

        let choice = Self::prompt_user(conflicting_files, &original)?;

        match choice {
            ConflictChoice::Cancel => Ok(CopyStrategy {
                target_dir: original,
                should_proceed: false,
                use_temp: false,
                _temp_handle: None,
            }),
            ConflictChoice::TempDir => {
                let tmp = TempDir::new()?;
                let claude_dir = tmp.path().join(".claude");
                std::fs::create_dir_all(&claude_dir)?;
                eprintln!("\n📁 Staging to temp directory: {}", tmp.path().display());
                eprintln!("   Your working directory .claude/ remains unchanged");
                Ok(CopyStrategy {
                    target_dir: claude_dir,
                    should_proceed: true,
                    use_temp: true,
                    _temp_handle: Some(tmp),
                })
            }
            ConflictChoice::Overwrite => Ok(CopyStrategy {
                target_dir: original,
                should_proceed: true,
                use_temp: false,
                _temp_handle: None,
            }),
        }
    }

    /// Determine target non-interactively (for tests and automation).
    pub fn determine_target_with_choice(
        original_target: impl AsRef<Path>,
        has_conflicts: bool,
        choice: ConflictChoice,
    ) -> Result<CopyStrategy, CopyStrategyError> {
        let original = original_target.as_ref().to_path_buf();

        if !has_conflicts {
            return Ok(CopyStrategy {
                target_dir: original,
                should_proceed: true,
                use_temp: false,
                _temp_handle: None,
            });
        }

        match choice {
            ConflictChoice::Cancel => Ok(CopyStrategy {
                target_dir: original,
                should_proceed: false,
                use_temp: false,
                _temp_handle: None,
            }),
            ConflictChoice::TempDir => {
                let tmp = TempDir::new()?;
                let claude_dir = tmp.path().join(".claude");
                std::fs::create_dir_all(&claude_dir)?;
                Ok(CopyStrategy {
                    target_dir: claude_dir,
                    should_proceed: true,
                    use_temp: true,
                    _temp_handle: Some(tmp),
                })
            }
            ConflictChoice::Overwrite => Ok(CopyStrategy {
                target_dir: original,
                should_proceed: true,
                use_temp: false,
                _temp_handle: None,
            }),
        }
    }

    fn prompt_user(
        conflicting_files: &[String],
        target_path: &Path,
    ) -> Result<ConflictChoice, CopyStrategyError> {
        let stderr = io::stderr();
        let mut err = stderr.lock();

        writeln!(err, "\n⚠️  Uncommitted changes detected in .claude/")?;
        writeln!(err, "{}", "=".repeat(70))?;
        writeln!(err, "\n📁 Files with uncommitted changes:")?;

        let display_count = conflicting_files.len().min(20);
        for f in &conflicting_files[..display_count] {
            writeln!(err, "  ⚠️  {f}")?;
        }
        if conflicting_files.len() > 20 {
            writeln!(err, "  ... and {} more", conflicting_files.len() - 20)?;
        }

        writeln!(err, "\n📝 Choose how to proceed:")?;
        writeln!(
            err,
            "   Working directory: {}",
            target_path.parent().unwrap_or(target_path).display()
        )?;
        writeln!(err, "\n💡 Options:")?;
        writeln!(
            err,
            "  • Y (default): Overwrite .claude/ in working directory"
        )?;
        writeln!(err, "  • t: Stage to temp directory instead")?;
        writeln!(err, "  • n: Cancel and exit")?;
        writeln!(err, "{}", "=".repeat(70))?;

        // Check non-interactive mode
        if std::env::args().any(|a| a == "--auto" || a == "-p") {
            writeln!(
                err,
                "\n🚀 Non-interactive mode detected - auto-approving overwrite"
            )?;
            return Ok(ConflictChoice::Overwrite);
        }

        eprint!("\nHow to proceed? [Y/t/n]: ");
        io::stderr().flush()?;

        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let response = line.trim().to_lowercase();

        match response.as_str() {
            "" | "y" | "yes" => Ok(ConflictChoice::Overwrite),
            "t" | "temp" => Ok(ConflictChoice::TempDir),
            "n" | "no" => {
                writeln!(
                    err,
                    "\n❌ User cancelled - keeping existing .claude/ directory"
                )?;
                Ok(ConflictChoice::Cancel)
            }
            _ => {
                writeln!(
                    err,
                    "\n⚠️  Invalid choice '{response}'. Defaulting to overwrite."
                )?;
                Ok(ConflictChoice::Overwrite)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_conflicts_proceeds() {
        let result =
            SafeCopyStrategy::determine_target("/tmp/test/.claude", false, &[], false).unwrap();
        assert!(result.should_proceed);
        assert!(!result.use_temp);
    }

    #[test]
    fn auto_approve_overrides_conflicts() {
        let files = vec![".claude/commands/dev.md".to_string()];
        let result =
            SafeCopyStrategy::determine_target("/tmp/test/.claude", true, &files, true).unwrap();
        assert!(result.should_proceed);
        assert!(!result.use_temp);
    }

    #[test]
    fn cancel_choice_does_not_proceed() {
        let result = SafeCopyStrategy::determine_target_with_choice(
            "/tmp/test/.claude",
            true,
            ConflictChoice::Cancel,
        )
        .unwrap();
        assert!(!result.should_proceed);
    }

    #[test]
    fn overwrite_choice_proceeds() {
        let result = SafeCopyStrategy::determine_target_with_choice(
            "/tmp/test/.claude",
            true,
            ConflictChoice::Overwrite,
        )
        .unwrap();
        assert!(result.should_proceed);
        assert!(!result.use_temp);
    }

    #[test]
    fn temp_choice_creates_temp_dir() {
        let result = SafeCopyStrategy::determine_target_with_choice(
            "/tmp/test/.claude",
            true,
            ConflictChoice::TempDir,
        )
        .unwrap();
        assert!(result.should_proceed);
        assert!(result.use_temp);
        assert!(result.target_dir.ends_with(".claude"));
        assert!(result._temp_handle.is_some());
    }

    #[test]
    fn no_conflicts_ignores_choice() {
        let result = SafeCopyStrategy::determine_target_with_choice(
            "/tmp/test/.claude",
            false,
            ConflictChoice::Cancel,
        )
        .unwrap();
        assert!(result.should_proceed, "no conflicts should always proceed");
    }
}
