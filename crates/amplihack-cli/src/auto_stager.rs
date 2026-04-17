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

/// Returns `true` for directory names that should be excluded from staging copies.
fn is_excluded_dir(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some("__pycache__" | ".pytest_cache" | "node_modules")
    )
}

/// Returns `true` for file extensions that should be excluded from staging copies.
fn is_excluded_file(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .map(|s| s.ends_with(".pyc") || s.ends_with(".pyo"))
        .unwrap_or(false)
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    // Same-path guard: bail if source and destination resolve to the same location.
    if let (Ok(src_canon), Ok(dst_canon)) = (source.canonicalize(), dest.canonicalize())
        && src_canon == dst_canon
    {
        anyhow::bail!(
            "source and destination are the same path: {}",
            src_canon.display()
        );
    }

    fs::create_dir_all(dest).with_context(|| format!("failed to create {}", dest.display()))?;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", source.display()))?;
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let metadata = symlink_metadata(&entry_path)?;
        if metadata.file_type().is_symlink() {
            tracing::warn!(
                path = %entry_path.display(),
                "skipping symlink while staging .claude assets"
            );
            continue;
        }

        let destination = dest.join(&file_name);
        if metadata.is_dir() {
            if is_excluded_dir(&file_name) {
                continue;
            }
            copy_dir_recursive(&entry_path, &destination)?;
        } else if metadata.is_file() {
            if is_excluded_file(&file_name) {
                continue;
            }
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
    fn stage_for_nested_execution_skips_pycache() {
        let project = tempfile::tempdir().unwrap();
        let source_claude = project.path().join(".claude");
        let agents_dir = source_claude.join("agents");
        let pycache = agents_dir.join("__pycache__");
        fs::create_dir_all(&pycache).unwrap();
        fs::write(agents_dir.join("agent.md"), "agent").unwrap();
        fs::write(agents_dir.join("helper.pyc"), "bytecode").unwrap();
        fs::write(pycache.join("agent.cpython-312.pyc"), "cached").unwrap();

        let result =
            AutoStager::stage_for_nested_execution(project.path(), "pycache-test").unwrap();

        assert!(
            result
                .staged_claude
                .join("agents")
                .join("agent.md")
                .exists()
        );
        assert!(
            !result
                .staged_claude
                .join("agents")
                .join("__pycache__")
                .exists()
        );
        assert!(
            !result
                .staged_claude
                .join("agents")
                .join("helper.pyc")
                .exists()
        );
    }

    #[test]
    fn copy_dir_recursive_rejects_same_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file.txt"), "data").unwrap();

        let result = copy_dir_recursive(&dir, &dir);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("same"),
            "expected same-path error, got: {err_msg}"
        );
    }
}
