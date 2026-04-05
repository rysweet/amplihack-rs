//! Staging safety checks for directory deletion and file protection.
//!
//! Validates that directories are safe to remove (not git repos, not
//! symlinked, no unknown custom skills) and protects staged files
//! from accidental batch operations.

use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, warn};

/// Safety classification for a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyStatus {
    /// The directory is safe to delete.
    Safe,
    /// The directory must not be deleted.
    Unsafe,
    /// Safety could not be determined.
    Uncertain,
}

/// Result of a directory safety check.
#[derive(Debug, Clone)]
pub struct DirectorySafetyCheck {
    /// Overall safety classification.
    pub status: SafetyStatus,
    /// Human-readable reason for the classification.
    pub reason: String,
    /// Custom skill directories found (which may block deletion).
    pub custom_skills: Vec<PathBuf>,
}

/// Determine whether a directory is safe to delete.
///
/// A directory is **unsafe** if any of the following are true:
/// - It is a symlink (may point to important data)
/// - It contains a `.git` directory (it is a repository)
/// - It contains unknown/custom skill files not managed by amplihack
///
/// Returns [`SafetyStatus::Uncertain`] when the directory does not exist.
pub fn is_safe_to_delete(directory: &Path) -> DirectorySafetyCheck {
    if !directory.exists() {
        return DirectorySafetyCheck {
            status: SafetyStatus::Uncertain,
            reason: format!("Directory does not exist: {}", directory.display()),
            custom_skills: vec![],
        };
    }

    // Symlink check
    if directory
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return DirectorySafetyCheck {
            status: SafetyStatus::Unsafe,
            reason: format!("Directory is a symlink: {}", directory.display()),
            custom_skills: vec![],
        };
    }

    // Git repo check
    if directory.join(".git").exists() {
        return DirectorySafetyCheck {
            status: SafetyStatus::Unsafe,
            reason: format!(
                "Directory contains a git repository: {}",
                directory.display()
            ),
            custom_skills: vec![],
        };
    }

    // Custom skills check — look for non-amplihack skill directories
    let custom_skills = find_custom_skills(directory);
    if !custom_skills.is_empty() {
        return DirectorySafetyCheck {
            status: SafetyStatus::Unsafe,
            reason: format!(
                "Directory contains {} custom skill(s) not managed by amplihack",
                custom_skills.len()
            ),
            custom_skills,
        };
    }

    debug!(dir = %directory.display(), "Directory is safe to delete");
    DirectorySafetyCheck {
        status: SafetyStatus::Safe,
        reason: "No safety concerns found".into(),
        custom_skills: vec![],
    }
}

/// Return the list of staged files in a git repository.
///
/// Runs `git diff --cached --name-only` to capture files that are currently
/// staged for commit and must not be modified by batch operations.
pub fn capture_protected_staged_files(repo_path: &Path) -> Vec<String> {
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(|l| l.to_string())
                .collect()
        }
        Ok(out) => {
            warn!(
                code = ?out.status.code(),
                "git diff --cached returned non-zero"
            );
            vec![]
        }
        Err(e) => {
            warn!(error = %e, "Failed to run git diff --cached");
            vec![]
        }
    }
}

/// Validate that a batch of fix candidates does not overlap with protected files.
///
/// Returns only those candidates whose paths do not appear in the protected set.
pub fn validate_fix_batch(
    _repo_path: &Path,
    candidates: &[String],
    protected: &[String],
) -> Vec<String> {
    candidates
        .iter()
        .filter(|c| {
            let dominated = protected
                .iter()
                .any(|p| *c == p || c.starts_with(&format!("{p}/")));
            if dominated {
                debug!(
                    candidate = c.as_str(),
                    "Excluding candidate — overlaps with protected file"
                );
            }
            !dominated
        })
        .cloned()
        .collect()
}

/// Validate and return a worktree path for a given stage name.
///
/// Ensures the worktree directory either does not exist yet (will be created
/// by git) or is already a valid git worktree.
pub fn require_isolated_worktree(
    stage_name: &str,
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<PathBuf, String> {
    // If the worktree path already exists, verify it's a valid worktree
    if worktree_path.exists() {
        let git_dir = worktree_path.join(".git");
        if !git_dir.exists() {
            return Err(format!(
                "Worktree path {} exists but is not a git worktree",
                worktree_path.display()
            ));
        }
        debug!(
            stage = stage_name,
            path = %worktree_path.display(),
            "Using existing worktree"
        );
        return Ok(worktree_path.to_path_buf());
    }

    // Verify the parent repo exists
    if !repo_path.join(".git").exists() && !repo_path.join(".git").is_file() {
        return Err(format!(
            "Repository path {} does not contain a git repository",
            repo_path.display()
        ));
    }

    debug!(
        stage = stage_name,
        path = %worktree_path.display(),
        "Worktree path is available"
    );
    Ok(worktree_path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Scan a directory for skill subdirectories not managed by amplihack.
fn find_custom_skills(directory: &Path) -> Vec<PathBuf> {
    let known_dirs = ["amplihack", ".amplihack"];
    let mut customs = Vec::new();

    let entries = match std::fs::read_dir(directory) {
        Ok(e) => e,
        Err(_) => return customs,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !known_dirs.contains(&name_str.as_ref()) {
            customs.push(path);
        }
    }

    customs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_directory_is_uncertain() {
        let result = is_safe_to_delete(Path::new("/nonexistent/phantom/staging/dir"));
        assert_eq!(result.status, SafetyStatus::Uncertain);
        assert!(result.reason.contains("does not exist"));
    }

    #[test]
    fn directory_with_git_is_unsafe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        let result = is_safe_to_delete(dir.path());
        assert_eq!(result.status, SafetyStatus::Unsafe);
        assert!(result.reason.contains("git repository"));
    }

    #[test]
    fn empty_directory_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        let result = is_safe_to_delete(dir.path());
        assert_eq!(result.status, SafetyStatus::Safe);
    }

    #[test]
    fn directory_with_known_subdirs_is_safe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("amplihack")).unwrap();
        let result = is_safe_to_delete(dir.path());
        assert_eq!(result.status, SafetyStatus::Safe);
    }

    #[test]
    fn directory_with_custom_skill_is_unsafe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("my-custom-skill")).unwrap();
        let result = is_safe_to_delete(dir.path());
        assert_eq!(result.status, SafetyStatus::Unsafe);
        assert!(!result.custom_skills.is_empty());
    }

    #[test]
    fn validate_fix_batch_filters_protected() {
        let candidates = vec!["a.rs".into(), "b.rs".into(), "c.rs".into()];
        let protected = vec!["b.rs".into()];
        let result = validate_fix_batch(Path::new("."), &candidates, &protected);
        assert_eq!(result, vec!["a.rs", "c.rs"]);
    }

    #[test]
    fn validate_fix_batch_prefix_overlap() {
        let candidates = vec!["src/lib.rs".into(), "src/main.rs".into()];
        let protected = vec!["src/lib.rs".into()];
        let result = validate_fix_batch(Path::new("."), &candidates, &protected);
        assert_eq!(result, vec!["src/main.rs"]);
    }

    #[test]
    fn validate_fix_batch_no_protected() {
        let candidates = vec!["a.rs".into(), "b.rs".into()];
        let result = validate_fix_batch(Path::new("."), &candidates, &[]);
        assert_eq!(result, vec!["a.rs", "b.rs"]);
    }

    #[test]
    fn require_isolated_worktree_nonexistent_parent_repo() {
        let result = require_isolated_worktree(
            "test",
            Path::new("/nonexistent/repo"),
            Path::new("/nonexistent/wt"),
        );
        assert!(result.is_err());
    }
}
