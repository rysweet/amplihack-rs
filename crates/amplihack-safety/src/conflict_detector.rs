//! Git conflict detection for safe file copying.
//!
//! Detects uncommitted changes in `.claude/` directories before overwriting,
//! filtering out system-generated metadata that is safe to overwrite.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Result of git conflict detection.
#[derive(Debug, Clone)]
pub struct ConflictDetectionResult {
    pub has_conflicts: bool,
    pub conflicting_files: Vec<String>,
    pub is_git_repo: bool,
}

/// Detect git conflicts for safe file copying.
///
/// Runs `git status --porcelain` and checks for uncommitted changes
/// in `.claude/` directories, excluding system-managed metadata files.
pub struct GitConflictDetector {
    target_dir: PathBuf,
    system_metadata: HashSet<&'static str>,
}

impl GitConflictDetector {
    /// System-generated files excluded from conflict detection.
    /// These are auto-managed by the framework and safe to overwrite.
    const SYSTEM_FILES: &[&str] = &[
        ".version",
        "settings.json",
        "context/PROJECT.md",
        "context/PROJECT.md.bak",
    ];

    pub fn new(target_dir: impl AsRef<Path>) -> Self {
        let system_metadata: HashSet<&'static str> = Self::SYSTEM_FILES.iter().copied().collect();
        Self {
            target_dir: target_dir.as_ref().to_path_buf(),
            system_metadata,
        }
    }

    /// Detect conflicts between `essential_dirs` and uncommitted changes.
    pub fn detect_conflicts(&self, essential_dirs: &[&str]) -> ConflictDetectionResult {
        if !self.is_git_repo() {
            return ConflictDetectionResult {
                has_conflicts: false,
                conflicting_files: Vec::new(),
                is_git_repo: false,
            };
        }

        let uncommitted = self.get_uncommitted_files();
        let conflicts = self.filter_conflicts(&uncommitted, essential_dirs);

        ConflictDetectionResult {
            has_conflicts: !conflicts.is_empty(),
            conflicting_files: conflicts,
            is_git_repo: true,
        }
    }

    fn is_git_repo(&self) -> bool {
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(&self.target_dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn get_uncommitted_files(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&self.target_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        let output = match output {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut uncommitted = Vec::new();

        for line in stdout.lines() {
            if line.len() < 4 {
                continue;
            }
            let status = &line[..2];
            let filename = line[3..].to_string();

            // Only Modified, Added, Renamed are conflicts.
            // Deleted files are NOT conflicts — we're copying fresh files anyway.
            if status.contains('M') || status.contains('A') || status.contains('R') {
                uncommitted.push(filename);
            }
        }

        uncommitted
    }

    fn filter_conflicts(&self, uncommitted: &[String], essential_dirs: &[&str]) -> Vec<String> {
        let mut conflicts = Vec::new();

        for file_path in uncommitted {
            let Some(relative) = file_path.strip_prefix(".claude/") else {
                continue;
            };

            // Skip system-generated metadata — safe to overwrite
            if self.system_metadata.contains(relative) {
                continue;
            }

            for dir in essential_dirs {
                let prefix = format!("{dir}/");
                if relative.starts_with(&prefix) || relative == *dir {
                    conflicts.push(file_path.clone());
                    break;
                }
            }
        }

        conflicts
    }
}

/// Check for git conflicts with a 5-second timeout.
///
/// Convenience wrapper that creates a detector and runs it with a timeout guard.
pub fn check_conflicts_with_timeout(
    target_dir: impl AsRef<Path>,
    essential_dirs: &[&str],
    timeout: Duration,
) -> ConflictDetectionResult {
    let detector = GitConflictDetector::new(target_dir);
    let (tx, rx) = std::sync::mpsc::channel();
    let dirs: Vec<String> = essential_dirs.iter().map(|s| s.to_string()).collect();
    let det = detector;
    std::thread::spawn(move || {
        let dir_refs: Vec<&str> = dirs.iter().map(|s| s.as_str()).collect();
        let _ = tx.send(det.detect_conflicts(&dir_refs));
    });
    rx.recv_timeout(timeout).unwrap_or(ConflictDetectionResult {
        has_conflicts: false,
        conflicting_files: Vec::new(),
        is_git_repo: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        dir
    }

    #[test]
    fn non_git_repo_no_conflicts() {
        let dir = TempDir::new().unwrap();
        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["commands", "context"]);
        assert!(!result.has_conflicts);
        assert!(!result.is_git_repo);
        assert!(result.conflicting_files.is_empty());
    }

    #[test]
    fn clean_repo_no_conflicts() {
        let dir = setup_git_repo();
        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["commands", "context"]);
        assert!(!result.has_conflicts);
        assert!(result.is_git_repo);
    }

    #[test]
    fn detects_modified_claude_files() {
        let dir = setup_git_repo();
        let claude_dir = dir.path().join(".claude/commands");
        fs::create_dir_all(&claude_dir).unwrap();
        let file = claude_dir.join("dev.md");
        fs::write(&file, "original").unwrap();

        // Stage and commit
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        // Modify file
        fs::write(&file, "modified").unwrap();

        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["commands"]);
        assert!(result.has_conflicts);
        assert_eq!(result.conflicting_files.len(), 1);
        assert!(result.conflicting_files[0].contains("commands/dev.md"));
    }

    #[test]
    fn ignores_system_metadata() {
        let dir = setup_git_repo();
        let claude_dir = dir.path().join(".claude");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("settings.json"), "{}").unwrap();
        fs::write(claude_dir.join(".version"), "1.0").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        fs::write(claude_dir.join("settings.json"), "{}updated").unwrap();
        fs::write(claude_dir.join(".version"), "2.0").unwrap();

        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["."]);
        assert!(!result.has_conflicts, "system metadata should be excluded");
    }

    #[test]
    fn ignores_deleted_files() {
        let dir = setup_git_repo();
        let claude_dir = dir.path().join(".claude/commands");
        fs::create_dir_all(&claude_dir).unwrap();
        let file = claude_dir.join("old.md");
        fs::write(&file, "content").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        fs::remove_file(&file).unwrap();

        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["commands"]);
        assert!(!result.has_conflicts, "deleted files are not conflicts");
    }

    #[test]
    fn ignores_non_claude_files() {
        let dir = setup_git_repo();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("main.rs"), "fn main(){}").unwrap();

        let detector = GitConflictDetector::new(dir.path());
        let result = detector.detect_conflicts(&["commands"]);
        assert!(!result.has_conflicts);
    }

    #[test]
    fn filters_by_essential_dir() {
        let dir = setup_git_repo();
        let cmds = dir.path().join(".claude/commands");
        let ctx = dir.path().join(".claude/context");
        fs::create_dir_all(&cmds).unwrap();
        fs::create_dir_all(&ctx).unwrap();
        fs::write(cmds.join("a.md"), "a").unwrap();
        fs::write(ctx.join("b.md"), "b").unwrap();

        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add"])
            .current_dir(dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();

        fs::write(cmds.join("a.md"), "changed").unwrap();
        fs::write(ctx.join("b.md"), "changed").unwrap();

        let detector = GitConflictDetector::new(dir.path());

        // Only check "commands" — should find 1 conflict
        let result = detector.detect_conflicts(&["commands"]);
        assert_eq!(result.conflicting_files.len(), 1);

        // Check both — should find 2
        let result = detector.detect_conflicts(&["commands", "context"]);
        assert_eq!(result.conflicting_files.len(), 2);
    }

    #[test]
    fn timeout_convenience_function() {
        let dir = TempDir::new().unwrap();
        let result =
            check_conflicts_with_timeout(dir.path(), &["commands"], Duration::from_secs(5));
        assert!(!result.has_conflicts);
    }
}
