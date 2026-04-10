//! Auto-staging for nested amplihack sessions.
//!
//! When amplihack runs nested (inside an active session) in the amplihack
//! source repository, we stage `.claude/` to a temporary directory to avoid
//! self-modification.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Result of a staging operation.
#[derive(Debug, Clone)]
pub struct StagingResult {
    /// Root of the temporary directory.
    pub temp_root: PathBuf,
    /// Path to the staged `.claude` directory inside `temp_root`.
    pub staged_claude: PathBuf,
    /// Original working directory that was staged from.
    pub original_cwd: PathBuf,
}

/// Directories that are safe to copy during staging.
const DIRS_TO_COPY: &[&str] = &[
    "agents", "commands", "skills", "tools", "workflow", "context",
];

/// Stage `.claude` directory to a temp location when nested.
pub struct AutoStager;

impl AutoStager {
    /// Create a temp dir, copy `.claude` components, and set `AMPLIHACK_IS_STAGED=1`.
    ///
    /// # Errors
    /// Returns an error if the temp directory or copy operations fail.
    pub fn stage_for_nested_execution(
        &self,
        original_cwd: &Path,
        session_id: &str,
    ) -> anyhow::Result<StagingResult> {
        let safe_id = sanitize_session_id(session_id);
        let temp_root = std::env::temp_dir().join(format!("amplihack-stage-{safe_id}"));
        fs::create_dir_all(&temp_root)?;

        let staged_claude = temp_root.join(".claude");
        fs::create_dir_all(&staged_claude)?;

        let source_claude = original_cwd.join(".claude");
        if source_claude.exists() {
            copy_claude_directory(&source_claude, &staged_claude);
        }

        // SAFETY: We are the only thread modifying this variable during staging.
        unsafe { std::env::set_var("AMPLIHACK_IS_STAGED", "1") };

        let original_cwd = original_cwd
            .canonicalize()
            .unwrap_or_else(|_| original_cwd.to_path_buf());

        Ok(StagingResult {
            temp_root,
            staged_claude,
            original_cwd,
        })
    }
}

/// Replace any character that is not `[a-zA-Z0-9_-]` with `_`.
fn sanitize_session_id(id: &str) -> String {
    let re = Regex::new(r"[^a-zA-Z0-9_-]").expect("static regex");
    re.replace_all(id, "_").into_owned()
}

/// Copy safe subdirectories of `.claude`, skipping symlinks and `runtime/`.
fn copy_claude_directory(source: &Path, dest: &Path) {
    for dir_name in DIRS_TO_COPY {
        let src_dir = source.join(dir_name);
        if !src_dir.exists() || !src_dir.is_dir() {
            continue;
        }
        if src_dir.read_link().is_ok() {
            tracing::warn!("Skipping symlinked directory {dir_name} (security protection)");
            continue;
        }
        let dest_dir = dest.join(dir_name);
        if let Err(e) = copy_dir_recursive(&src_dir, &dest_dir) {
            tracing::warn!("Failed to copy {dir_name} directory: {e}");
        }
    }
}

/// Recursively copy a directory, skipping symlinks.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Guard: skip copy when source and dest resolve to the same directory (issue #4296).
    if let (Ok(canon_src), Ok(canon_dst)) = (src.canonicalize(), dst.canonicalize()) {
        if canon_src == canon_dst {
            tracing::warn!(
                src = %src.display(),
                dst = %dst.display(),
                "skipping copy: source and destination are the same path"
            );
            return Ok(());
        }
    }

    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_symlink() {
            continue;
        } else if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sanitize_strips_bad_chars() {
        assert_eq!(sanitize_session_id("ses/../../x"), "ses_______x");
        assert_eq!(sanitize_session_id("ok-id_1"), "ok-id_1");
        assert_eq!(sanitize_session_id(""), "");
    }

    #[test]
    fn stage_creates_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("project");
        let claude_dir = cwd.join(".claude").join("agents");
        fs::create_dir_all(&claude_dir).unwrap();
        fs::write(claude_dir.join("a.md"), "agent").unwrap();

        let stager = AutoStager;
        let result = stager.stage_for_nested_execution(&cwd, "test-123").unwrap();

        assert!(result.staged_claude.exists());
        assert!(result.staged_claude.join("agents").join("a.md").exists());
        // Clean up env var
        unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") };
        // Clean up temp
        let _ = fs::remove_dir_all(&result.temp_root);
    }

    #[test]
    fn stage_skips_missing_source() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("no-claude");
        fs::create_dir_all(&cwd).unwrap();

        let stager = AutoStager;
        let result = stager.stage_for_nested_execution(&cwd, "empty").unwrap();

        assert!(result.staged_claude.exists());
        assert!(
            fs::read_dir(&result.staged_claude)
                .unwrap()
                .next()
                .is_none()
        );
        unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") };
        let _ = fs::remove_dir_all(&result.temp_root);
    }

    #[test]
    fn stage_skips_runtime_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("proj");
        let runtime_dir = cwd.join(".claude").join("runtime");
        fs::create_dir_all(&runtime_dir).unwrap();
        fs::write(runtime_dir.join("log.json"), "{}").unwrap();

        let stager = AutoStager;
        let result = stager.stage_for_nested_execution(&cwd, "rt").unwrap();

        assert!(!result.staged_claude.join("runtime").exists());
        unsafe { std::env::remove_var("AMPLIHACK_IS_STAGED") };
        let _ = fs::remove_dir_all(&result.temp_root);
    }

    #[test]
    fn copy_dir_recursive_works() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let nested = src.join("sub");
        fs::create_dir_all(&nested).unwrap();
        fs::write(src.join("a.txt"), "hello").unwrap();
        fs::write(nested.join("b.txt"), "world").unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(dst.join("sub").join("b.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn copy_dir_recursive_same_path_returns_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file.txt"), "content").unwrap();

        copy_dir_recursive(&dir, &dir).unwrap();

        assert_eq!(fs::read_to_string(dir.join("file.txt")).unwrap(), "content");
    }
}
