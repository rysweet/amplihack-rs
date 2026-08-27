//! Shared staging for the amplihack slash-command markdown files.
//!
//! Exactly one command set — `docs/claude/commands/amplihack/*.md` — feeds two
//! independent surfaces:
//!
//! * the Copilot CLI plugin, at `<plugin_dir>/commands/` (flat, with the
//!   `amplihack:` namespace stripped from each file's frontmatter `name:`
//!   because Copilot flattens every plugin's commands into one namespace and
//!   rejects the colon), and
//! * Claude Code, at `~/.claude/commands/amplihack/` (verbatim; Claude derives
//!   `/amplihack:<name>` from the directory plus the file stem).
//!
//! Only the target directory and the per-file transform differ, so the source
//! probing and the atomic staging-then-swap live here once. Issue #1344 was
//! precisely the cost of having only the Copilot half: the commands existed in
//! the repo, were documented, and never arrived in a Claude session.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Locate the slash-command markdown source directory inside `repo_root`.
///
/// In the bundle layout the canonical command markdowns live at
/// `<repo>/docs/claude/commands/amplihack/`; in legacy `.claude` checkouts
/// they live at `<repo>/.claude/commands/amplihack/` (or one parent up, for
/// nested checkouts). All three are probed in that order; the first existing
/// directory wins. Returns `None` when the source tree ships no commands.
pub(super) fn command_source_dir(repo_root: &Path) -> Option<PathBuf> {
    [
        repo_root
            .join("docs")
            .join("claude")
            .join("commands")
            .join("amplihack"),
        repo_root.join(".claude").join("commands").join("amplihack"),
        repo_root
            .parent()
            .map(|parent| parent.join(".claude").join("commands").join("amplihack"))
            .unwrap_or_default(),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

/// Copy every `*.md` file in `source` into `target`, passing each file's body
/// through `transform` first, and swap the result into place atomically.
///
/// Files land in a sibling `<target>.staging` directory and are only renamed
/// over `target` once every file has been written, so a reader never observes
/// a half-populated command directory and a failed copy leaves the previously
/// staged command set intact. An existing `target` is moved aside to
/// `<target>.old` for the duration of the swap and restored if the swap fails.
///
/// Non-`.md` entries and subdirectories are ignored. Returns the number of
/// files copied; `Ok(0)` means nothing was staged and `target` was left
/// exactly as it was.
pub(super) fn stage_command_files(
    source: &Path,
    target: &Path,
    transform: impl Fn(&Path, &str) -> Result<String>,
) -> Result<usize> {
    let staging = staging_dir(target);
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(&staging)
        .with_context(|| format!("failed to create {}", staging.display()))?;

    let mut copied = 0_usize;
    for entry in
        fs::read_dir(source).with_context(|| format!("failed to read {}", source.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry.path().extension().and_then(|ext| ext.to_str()) == Some("md")
        {
            let dst = staging.join(entry.file_name());
            let body = fs::read_to_string(entry.path())
                .with_context(|| format!("failed to read {}", entry.path().display()))?;
            let transformed = transform(&entry.path(), &body)?;
            fs::write(&dst, transformed).with_context(|| {
                format!(
                    "failed to write command {} to {}",
                    entry.path().display(),
                    dst.display()
                )
            })?;
            copied += 1;
        }
    }

    if copied == 0 {
        let _ = fs::remove_dir_all(&staging);
        return Ok(0);
    }

    if target.exists() {
        let backup = backup_dir(target);
        if backup.exists() {
            let _ = fs::remove_dir_all(&backup);
        }
        fs::rename(target, &backup).with_context(|| {
            format!(
                "failed to back up existing {} to {}",
                target.display(),
                backup.display()
            )
        })?;
        if let Err(err) = fs::rename(&staging, target) {
            let _ = fs::rename(&backup, target);
            let _ = fs::remove_dir_all(&staging);
            return Err(err)
                .with_context(|| format!("failed to swap commands into {}", target.display()));
        }
        let _ = fs::remove_dir_all(&backup);
    } else {
        fs::rename(&staging, target)
            .with_context(|| format!("failed to move commands into {}", target.display()))?;
    }

    Ok(copied)
}

fn staging_dir(target: &Path) -> PathBuf {
    sibling_with_suffix(target, ".staging")
}

fn backup_dir(target: &Path) -> PathBuf {
    sibling_with_suffix(target, ".old")
}

fn sibling_with_suffix(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn verbatim(_path: &Path, body: &str) -> Result<String> {
        Ok(body.to_string())
    }

    #[test]
    fn prefers_docs_source_over_legacy_claude_source() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("repo");
        let docs = repo
            .join("docs")
            .join("claude")
            .join("commands")
            .join("amplihack");
        let legacy = repo.join(".claude").join("commands").join("amplihack");
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(&legacy).unwrap();

        assert_eq!(command_source_dir(&repo).as_deref(), Some(docs.as_path()));
    }

    #[test]
    fn falls_back_to_legacy_claude_source() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("repo");
        let legacy = repo.join(".claude").join("commands").join("amplihack");
        fs::create_dir_all(&legacy).unwrap();

        assert_eq!(command_source_dir(&repo).as_deref(), Some(legacy.as_path()));
    }

    #[test]
    fn no_source_directory_yields_none() {
        let td = TempDir::new().unwrap();
        assert!(command_source_dir(&td.path().join("repo")).is_none());
    }

    #[test]
    fn stages_only_markdown_files() {
        let td = TempDir::new().unwrap();
        let source = td.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("lock.md"), "# /lock\n").unwrap();
        fs::write(source.join("notes.txt"), "ignored\n").unwrap();

        let target = td.path().join("out").join("commands");
        let copied = stage_command_files(&source, &target, verbatim).unwrap();

        assert_eq!(copied, 1);
        assert!(target.join("lock.md").is_file());
        assert!(!target.join("notes.txt").exists());
    }

    #[test]
    fn empty_source_leaves_target_untouched_and_removes_staging() {
        let td = TempDir::new().unwrap();
        let source = td.path().join("source");
        fs::create_dir_all(&source).unwrap();
        let target = td.path().join("commands");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("previous.md"), "keep me\n").unwrap();

        let copied = stage_command_files(&source, &target, verbatim).unwrap();

        assert_eq!(copied, 0);
        assert!(
            target.join("previous.md").is_file(),
            "an empty source must not wipe an already-staged command set"
        );
        assert!(
            !staging_dir(&target).exists(),
            "staging dir must be cleaned"
        );
    }

    #[test]
    fn restaging_replaces_stale_commands_and_leaves_no_scratch_dirs() {
        let td = TempDir::new().unwrap();
        let source = td.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("lock.md"), "v2\n").unwrap();

        let target = td.path().join("commands");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("lock.md"), "v1\n").unwrap();
        fs::write(target.join("removed.md"), "stale\n").unwrap();

        let copied = stage_command_files(&source, &target, verbatim).unwrap();

        assert_eq!(copied, 1);
        assert_eq!(fs::read_to_string(target.join("lock.md")).unwrap(), "v2\n");
        assert!(
            !target.join("removed.md").exists(),
            "the swap must replace the directory, not merge into it"
        );
        assert!(!staging_dir(&target).exists());
        assert!(!backup_dir(&target).exists());
    }

    #[test]
    fn transform_is_applied_to_every_staged_file() {
        let td = TempDir::new().unwrap();
        let source = td.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("a.md"), "body\n").unwrap();
        fs::write(source.join("b.md"), "body\n").unwrap();

        let target = td.path().join("commands");
        let copied = stage_command_files(&source, &target, |path, body| {
            Ok(format!(
                "{}:{body}",
                path.file_stem().unwrap().to_str().unwrap()
            ))
        })
        .unwrap();

        assert_eq!(copied, 2);
        assert_eq!(fs::read_to_string(target.join("a.md")).unwrap(), "a:body\n");
        assert_eq!(fs::read_to_string(target.join("b.md")).unwrap(), "b:body\n");
    }
}
