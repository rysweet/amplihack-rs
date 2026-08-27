//! Stage the amplihack slash commands for Claude Code sessions.
//!
//! Claude Code discovers user-level slash commands under `~/.claude/commands/`,
//! namespaced by subdirectory: a file at `~/.claude/commands/amplihack/lock.md`
//! is invoked as `/amplihack:lock`.
//!
//! Before issue #1344 the only thing that staged the command markdowns was
//! [`super::copilot_plugin::stage_commands`], whose target is always
//! `<plugin_dir>/commands` — so on a fully installed host
//! `~/.copilot/installed-plugins/amplihack@local/commands/` held all 23
//! commands while `~/.claude/commands/` did not exist at all. `/lock`
//! ("Enable continuous work mode without stopping"), `/unlock`, `/auto`,
//! `/ultrathink` and the rest were unavailable in exactly the sessions a user
//! reaches for them, with nothing in the install output to make the gap
//! visible.
//!
//! Unlike the Copilot side, the markdown is staged **verbatim**. Copilot
//! flattens every plugin's commands into a single namespace and rejects a
//! colon in the frontmatter `name:`, which is why that path rewrites
//! `name: amplihack:lock` to `name: lock`. Claude derives the command name
//! from the path instead (`amplihack/` + file stem), so the frontmatter is
//! left exactly as the source of truth wrote it.

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::command_staging::{command_source_dir, stage_command_files};
use super::paths::global_claude_dir;

/// Result of staging the Claude Code command directory.
///
/// `copied == 0` means the source tree shipped no command markdown and
/// `target` was left untouched — the installer reports that as a skip rather
/// than a success, so a silently empty command set can't look like a win.
pub(super) struct StagedClaudeCommands {
    pub(super) copied: usize,
    pub(super) target: PathBuf,
}

/// Stage `<repo_root>`'s slash commands into `~/.claude/commands/amplihack/`.
pub(super) fn stage_claude_commands(repo_root: &Path) -> Result<StagedClaudeCommands> {
    stage_claude_commands_in(&global_claude_dir()?, repo_root)
}

/// Test-friendly variant: stages under an explicit `claude_home` instead of
/// resolving `~/.claude` from the environment, so unit tests never depend on
/// (or mutate) the real `HOME`.
pub(super) fn stage_claude_commands_in(
    claude_home: &Path,
    repo_root: &Path,
) -> Result<StagedClaudeCommands> {
    let target = claude_home.join("commands").join("amplihack");
    let Some(source) = command_source_dir(repo_root) else {
        return Ok(StagedClaudeCommands { copied: 0, target });
    };
    let copied = stage_command_files(&source, &target, |_path, body| Ok(body.to_string()))?;
    Ok(StagedClaudeCommands { copied, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn repo_with_commands(root: &Path, commands: &[&str]) {
        let dir = root
            .join("docs")
            .join("claude")
            .join("commands")
            .join("amplihack");
        fs::create_dir_all(&dir).unwrap();
        for command in commands {
            fs::write(
                dir.join(format!("{command}.md")),
                format!("---\nname: amplihack:{command}\n---\n# /{command}\n"),
            )
            .unwrap();
        }
    }

    #[test]
    fn stages_commands_into_the_amplihack_namespace_dir() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("repo");
        repo_with_commands(&repo, &["lock", "unlock", "auto"]);
        let claude_home = td.path().join(".claude");

        let staged = stage_claude_commands_in(&claude_home, &repo).unwrap();

        assert_eq!(staged.copied, 3);
        assert_eq!(
            staged.target,
            claude_home.join("commands").join("amplihack")
        );
        for command in ["lock", "unlock", "auto"] {
            assert!(
                staged.target.join(format!("{command}.md")).is_file(),
                "{command}.md must be discoverable as /amplihack:{command}"
            );
        }
    }

    #[test]
    fn stages_claude_command_bodies_verbatim() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("repo");
        repo_with_commands(&repo, &["lock"]);
        let claude_home = td.path().join(".claude");

        let staged = stage_claude_commands_in(&claude_home, &repo).unwrap();

        let body = fs::read_to_string(staged.target.join("lock.md")).unwrap();
        assert_eq!(
            body, "---\nname: amplihack:lock\n---\n# /lock\n",
            "Claude namespaces by directory; the Copilot frontmatter rewrite must not leak here"
        );
    }

    #[test]
    fn missing_command_source_is_reported_as_zero_not_an_error() {
        let td = TempDir::new().unwrap();
        let claude_home = td.path().join(".claude");

        let staged = stage_claude_commands_in(&claude_home, &td.path().join("repo")).unwrap();

        assert_eq!(staged.copied, 0);
        assert!(!staged.target.exists());
    }

    #[test]
    fn restaging_is_idempotent() {
        let td = TempDir::new().unwrap();
        let repo = td.path().join("repo");
        repo_with_commands(&repo, &["lock", "unlock"]);
        let claude_home = td.path().join(".claude");

        stage_claude_commands_in(&claude_home, &repo).unwrap();
        let staged = stage_claude_commands_in(&claude_home, &repo).unwrap();

        assert_eq!(staged.copied, 2);
        let entries = fs::read_dir(&staged.target).unwrap().count();
        assert_eq!(entries, 2, "a second install must not duplicate commands");
    }
}
