//! Stage `.claude` assets into a temporary workspace for protected auto mode.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagingResult {
    pub temp_root: PathBuf,
    pub staged_claude: PathBuf,
    pub original_cwd: PathBuf,
}

pub struct AutoStager;

impl AutoStager {
    pub fn stage_for_nested_execution(
        original_cwd: &Path,
        session_id: &str,
    ) -> Result<StagingResult> {
        let temp_root = create_stage_root(session_id)?;
        let staged_claude = temp_root.join(".claude");
        fs::create_dir_all(&staged_claude)
            .with_context(|| format!("failed to create {}", staged_claude.display()))?;

        let source_claude = original_cwd.join(".claude");
        if source_claude.is_dir() {
            copy_claude_directory(&source_claude, &staged_claude)?;
        }

        Ok(StagingResult {
            temp_root,
            staged_claude,
            original_cwd: original_cwd
                .canonicalize()
                .unwrap_or_else(|_| original_cwd.to_path_buf()),
        })
    }
}

fn create_stage_root(session_id: &str) -> Result<PathBuf> {
    let safe_session_id = sanitize_session_id(session_id);
    let prefix = format!("amplihack-stage-{safe_session_id}-");
    let base = std::env::temp_dir();

    for attempt in 0..128u32 {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let candidate = base.join(format!("{prefix}{stamp:x}-{attempt}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to create {}", candidate.display()));
            }
        }
    }

    bail!(
        "failed to create a unique staged auto-mode directory in {}",
        base.display()
    )
}

fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' => ch,
            _ => '_',
        })
        .collect()
}

fn copy_claude_directory(source: &Path, dest: &Path) -> Result<()> {
    for dir_name in [
        "agents", "commands", "skills", "tools", "workflow", "context",
    ] {
        let source_dir = source.join(dir_name);
        if !source_dir.exists() {
            continue;
        }
        if !source_dir.is_dir() {
            continue;
        }
        if symlink_metadata(&source_dir)?.file_type().is_symlink() {
            tracing::warn!(
                path = %source_dir.display(),
                "skipping symlinked staged .claude directory"
            );
            continue;
        }

        copy_dir_recursive(&source_dir, &dest.join(dir_name))?;
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    // Guard: skip copy when source and dest resolve to the same directory.
    // Prevents infinite recursion / SameFileError (see issue #4296).
    if let (Ok(canon_src), Ok(canon_dst)) = (source.canonicalize(), dest.canonicalize()) {
        if canon_src == canon_dst {
            tracing::warn!(
                src = %source.display(),
                dst = %dest.display(),
                "skipping copy: source and destination are the same path"
            );
            return Ok(());
        }
    }

    fs::create_dir_all(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", source.display()))?;
        let entry_path = entry.path();
        let metadata = symlink_metadata(&entry_path)?;
        if metadata.file_type().is_symlink() {
            tracing::warn!(
                path = %entry_path.display(),
                "skipping symlink while staging .claude assets"
            );
            continue;
        }

        let destination = dest.join(entry.file_name());
        if metadata.is_dir() {
            copy_dir_recursive(&entry_path, &destination)?;
        } else if metadata.is_file() {
            fs::copy(&entry_path, &destination).with_context(|| {
                format!(
                    "failed to copy staged asset {} -> {}",
                    entry_path.display(),
                    destination.display()
                )
            })?;
        }
    }
    Ok(())
}

fn symlink_metadata(path: &Path) -> Result<fs::Metadata> {
    fs::symlink_metadata(path).with_context(|| format!("failed to inspect {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_for_nested_execution_copies_expected_claude_assets() {
        let project = tempfile::tempdir().unwrap();
        let source_claude = project.path().join(".claude");
        fs::create_dir_all(source_claude.join("agents")).unwrap();
        fs::create_dir_all(source_claude.join("runtime")).unwrap();
        fs::write(source_claude.join("agents").join("agent.md"), "agent").unwrap();
        fs::write(
            source_claude.join("runtime").join("sessions.jsonl"),
            "runtime",
        )
        .unwrap();

        let result =
            AutoStager::stage_for_nested_execution(project.path(), "nested-session").unwrap();

        assert!(result.temp_root.exists());
        assert!(
            result
                .staged_claude
                .join("agents")
                .join("agent.md")
                .exists()
        );
        assert!(!result.staged_claude.join("runtime").exists());
        assert_eq!(result.original_cwd, project.path().canonicalize().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn stage_for_nested_execution_skips_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().unwrap();
        let source_claude = project.path().join(".claude");
        fs::create_dir_all(source_claude.join("real-skills")).unwrap();
        symlink(
            source_claude.join("real-skills"),
            source_claude.join("skills"),
        )
        .unwrap();

        let result = AutoStager::stage_for_nested_execution(project.path(), "../unsafe").unwrap();

        assert!(!result.staged_claude.join("skills").exists());
        assert!(
            result
                .temp_root
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("___unsafe")
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