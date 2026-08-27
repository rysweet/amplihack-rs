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
//!
//! Also unlike the Copilot side, the target is **not** amplihack-private: it
//! is a namespace inside the user's own `~/.claude/commands/`, and its parent
//! is Claude's scan root. So the install records what it staged (via
//! [`crate::claude_plugin::record_claude_commands_ownership`]), preserves
//! anything it cannot prove it owns, and keeps its scratch directories out of
//! the scan root — see [`super::command_staging`].

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::command_staging::{StageRequest, command_source_dir_excluding, stage_command_files};
use super::paths::{global_claude_dir, staging_claude_dir};

/// Scratch root for the command swap.
///
/// A dotted sibling of `commands/` rather than `commands/amplihack.staging`:
/// every *subdirectory of `commands/`* is a command namespace, so scratch
/// state there is directly reachable as `/amplihack.staging:lock`. `~/.claude`
/// itself is not scanned for commands.
const SCRATCH_DIR: &str = ".amplihack-command-staging";

/// Result of staging the Claude Code command directory.
///
/// `copied == 0` means the source tree shipped no command markdown and
/// `target` was left untouched — the installer reports that as a skip rather
/// than a success, so a silently empty command set can't look like a win.
pub(super) struct StagedClaudeCommands {
    pub(super) copied: usize,
    pub(super) preserved: Vec<String>,
    pub(super) target: PathBuf,
}

/// The `~/.claude/commands/amplihack/` namespace directory.
pub(super) fn claude_commands_dir() -> Result<PathBuf> {
    Ok(global_claude_dir()?.join("commands").join("amplihack"))
}

/// The scratch root the command swap uses; amplihack-private, safe to delete.
pub(super) fn claude_commands_scratch_dir() -> Result<PathBuf> {
    Ok(global_claude_dir()?.join(SCRATCH_DIR))
}

/// Stage `<repo_root>`'s slash commands into `~/.claude/commands/amplihack/`.
pub(super) fn stage_claude_commands(repo_root: &Path) -> Result<StagedClaudeCommands> {
    stage_claude_commands_in(&global_claude_dir()?, &staging_claude_dir()?, repo_root)
}

/// Test-friendly variant: stages under an explicit `claude_home` and records
/// ownership under an explicit `staged_framework` root, so unit tests never
/// depend on (or mutate) the real `HOME`.
pub(super) fn stage_claude_commands_in(
    claude_home: &Path,
    staged_framework: &Path,
    repo_root: &Path,
) -> Result<StagedClaudeCommands> {
    let target = claude_home.join("commands").join("amplihack");
    let manifest = crate::claude_plugin::ownership_manifest_path(staged_framework);
    let Some(source) = command_source_dir_excluding(repo_root, Some(&target)) else {
        return Ok(StagedClaudeCommands {
            copied: 0,
            preserved: Vec::new(),
            target,
        });
    };

    let staged = stage_command_files(
        &StageRequest {
            source: &source,
            target: &target,
            scratch_root: &claude_home.join(SCRATCH_DIR),
            target_is_owned: crate::claude_plugin::claude_commands_are_owned(&manifest, &target)?,
        },
        |_path, body| Ok(body.to_string()),
    )?;

    if staged.copied > 0 {
        crate::claude_plugin::record_claude_commands_ownership(&manifest, &target)?;
    }
    Ok(StagedClaudeCommands {
        copied: staged.copied,
        preserved: staged.preserved,
        target,
    })
}

/// Restage the command namespace if it has gone missing since the install.
///
/// Deliberately *not* an entry in `settings::missing_framework_paths`: that
/// list drives `framework_restage_needed`, and a gap a restage cannot close
/// makes every launch restage the entire framework, forever — issue #1266
/// verbatim. A host whose only command source resolves to the staging target
/// (review finding 4) is exactly such a gap. So this is a single bounded
/// top-up instead: one attempt per launch, warn and continue on failure.
pub(super) fn ensure_claude_commands_staged() -> Result<()> {
    let target = claude_commands_dir()?;
    if target.is_dir() {
        return Ok(());
    }
    let Some(repo_root) = super::clone::find_bundled_framework_root() else {
        tracing::warn!(
            path = %target.display(),
            "amplihack slash commands are missing and no framework source was found to restage them"
        );
        return Ok(());
    };
    match stage_claude_commands(&repo_root) {
        Ok(staged) if staged.copied > 0 => println!(
            "🔧 Restaged {} amplihack slash command(s) at {}",
            staged.copied,
            target.display()
        ),
        Ok(_) => tracing::warn!(
            path = %target.display(),
            source = %repo_root.display(),
            "framework source ships no slash-command markdown; /amplihack:* stays unavailable"
        ),
        Err(error) => tracing::warn!(
            path = %target.display(),
            "could not restage amplihack slash commands: {error:#}"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _td: TempDir,
        repo: PathBuf,
        claude_home: PathBuf,
        staged_framework: PathBuf,
    }

    impl Fixture {
        fn new(commands: &[&str]) -> Self {
            let td = TempDir::new().unwrap();
            let repo = td.path().join("repo");
            repo_with_commands(&repo, commands);
            Self {
                claude_home: td.path().join(".claude"),
                staged_framework: td.path().join(".amplihack").join(".claude"),
                repo,
                _td: td,
            }
        }

        fn stage(&self) -> Result<StagedClaudeCommands> {
            stage_claude_commands_in(&self.claude_home, &self.staged_framework, &self.repo)
        }
    }

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
        let fixture = Fixture::new(&["lock", "unlock", "auto"]);

        let staged = fixture.stage().unwrap();

        assert_eq!(staged.copied, 3);
        assert_eq!(
            staged.target,
            fixture.claude_home.join("commands").join("amplihack")
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
        let fixture = Fixture::new(&["lock"]);

        let staged = fixture.stage().unwrap();

        let body = fs::read_to_string(staged.target.join("lock.md")).unwrap();
        assert_eq!(
            body, "---\nname: amplihack:lock\n---\n# /lock\n",
            "Claude namespaces by directory; the Copilot frontmatter rewrite must not leak here"
        );
    }

    #[test]
    fn missing_command_source_is_reported_as_zero_not_an_error() {
        let fixture = Fixture::new(&[]);
        fs::remove_dir_all(fixture.repo.join("docs")).unwrap();

        let staged = fixture.stage().unwrap();

        assert_eq!(staged.copied, 0);
        assert!(!staged.target.exists());
    }

    #[test]
    fn restaging_is_idempotent() {
        let fixture = Fixture::new(&["lock", "unlock"]);

        fixture.stage().unwrap();
        let staged = fixture.stage().unwrap();

        assert_eq!(staged.copied, 2);
        let entries = fs::read_dir(&staged.target).unwrap().count();
        assert_eq!(entries, 2, "a second install must not duplicate commands");
    }

    /// A command dropped from the source is dropped from the namespace — but
    /// only because the recorded digest proves nothing else has touched it.
    #[test]
    fn a_verified_namespace_drops_commands_the_source_no_longer_ships() {
        let fixture = Fixture::new(&["lock", "retired"]);
        fixture.stage().unwrap();
        fs::remove_file(
            fixture
                .repo
                .join("docs/claude/commands/amplihack/retired.md"),
        )
        .unwrap();

        let staged = fixture.stage().unwrap();

        assert_eq!(staged.copied, 1);
        assert!(staged.preserved.is_empty());
        assert!(!staged.target.join("retired.md").exists());
    }

    /// Issue #1344 review finding 2: the swap used to rename the whole
    /// directory aside and delete it, so a user's own command file in the same
    /// namespace was destroyed while the installer printed a green success.
    #[test]
    fn install_preserves_a_users_own_command_file_in_the_namespace() {
        let fixture = Fixture::new(&["lock"]);
        let target = fixture.claude_home.join("commands").join("amplihack");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("my-thing.md"), "mine\n").unwrap();

        let staged = fixture.stage().unwrap();

        assert_eq!(staged.copied, 1);
        assert_eq!(staged.preserved, vec!["my-thing.md"]);
        assert_eq!(
            fs::read_to_string(target.join("my-thing.md")).unwrap(),
            "mine\n",
            "/amplihack:my-thing is the user's, not amplihack's to delete"
        );
        assert!(target.join("lock.md").is_file());
    }

    /// A file added after a clean install invalidates the digest, so the next
    /// install preserves it too.
    #[test]
    fn a_file_added_after_install_survives_the_next_install() {
        let fixture = Fixture::new(&["lock"]);
        let staged = fixture.stage().unwrap();
        fs::write(staged.target.join("my-thing.md"), "mine\n").unwrap();

        let staged = fixture.stage().unwrap();

        assert_eq!(staged.preserved, vec!["my-thing.md"]);
        assert_eq!(
            fs::read_to_string(staged.target.join("my-thing.md")).unwrap(),
            "mine\n"
        );
    }

    /// Issue #1344 review finding 4: `find_bundled_framework_root` resolves
    /// `repo_root` to `~/.amplihack` on any host with a prior staged install,
    /// and the parent probe then evaluates to the staging target itself.
    #[test]
    fn a_repo_root_whose_probe_resolves_to_the_target_stages_nothing() {
        let td = TempDir::new().unwrap();
        let home = td.path();
        let repo = home.join(".amplihack");
        fs::create_dir_all(&repo).unwrap();
        let claude_home = home.join(".claude");
        let target = claude_home.join("commands").join("amplihack");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("lock.md"), "already staged\n").unwrap();

        let staged = stage_claude_commands_in(&claude_home, &repo.join(".claude"), &repo).unwrap();

        assert_eq!(
            staged.copied, 0,
            "staging the target from its own contents reports a green command \
             count that can never refresh (issue #1344 review finding 4)"
        );
        assert_eq!(
            fs::read_to_string(target.join("lock.md")).unwrap(),
            "already staged\n"
        );
    }

    #[test]
    fn scratch_directories_never_land_in_claudes_command_scan_root() {
        let fixture = Fixture::new(&["lock", "unlock"]);

        fixture.stage().unwrap();

        let namespaces: Vec<String> = fs::read_dir(fixture.claude_home.join("commands"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            namespaces,
            vec!["amplihack".to_string()],
            "an orphan scratch dir under commands/ surfaces every command a \
             second time as /amplihack.staging:*"
        );
    }
}
