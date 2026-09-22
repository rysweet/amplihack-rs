//! Every `~/.claude/…` destination `amplihack install` is responsible for.
//!
//! Claude Code reads none of `~/.amplihack/.claude/`. That tree is amplihack's
//! own staging area; the directories a Claude session actually discovers are
//! `~/.claude/commands/<namespace>/` and the direct children of
//! `~/.claude/skills/`. Anything staged but not *published* into those is
//! invisible to every session, with nothing in the install output to say so.
//!
//! That gap has now been shipped twice. Issue #1344: slash commands were
//! staged into the Copilot plugin and nowhere else, so `/lock`, `/auto` and
//! the rest never reached a Claude session. Issue #1438: `copytree_manifest`
//! copied `amplifier-bundle/skills` into the staging tree and printed
//! `✅ Copied skills -> skills`, while the only code that mirrors skills into
//! `~/.claude/skills` — [`crate::claude_plugin::ensure_claude_plugin_installed`]
//! — was reachable solely from `bootstrap::prepare_launcher("claude")`, which
//! additionally returns early in non-interactive mode. `install` therefore
//! reported success and exited 0 having never written the directory that
//! matters, and each skill added to the bundle stayed invisible until some
//! later interactive launch happened to sync it.
//!
//! Both were the same mistake — a per-asset-type code path that someone has to
//! remember to call — so the fan-out lives here as ONE enumerated list.
//! Adding an asset type to the framework is a new row in
//! [`CLAUDE_DESTINATIONS`], not a new call site to forget.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Outcome of publishing one asset type into `~/.claude`.
pub(super) struct Publication {
    /// Line printed under the install's Claude publication heading. Carries
    /// its own status glyph so a publisher can report a partial result
    /// without the caller having to guess.
    pub(super) detail: String,
    /// Entry for the install summary's feature list. `None` when nothing was
    /// published — a skip must never read as a win.
    pub(super) feature: Option<String>,
    /// Entries the publisher carried across because it could not prove
    /// amplihack had staged them.
    pub(super) preserved: Vec<String>,
    /// Destination directory, used to report `preserved`.
    pub(super) target: PathBuf,
}

/// One publishable asset type.
pub(super) struct ClaudeDestination {
    /// Asset type name, used in the failure line.
    pub(super) label: &'static str,
    /// Publishes this asset type from `repo_root` into the user's `~/.claude`.
    pub(super) publish: fn(&Path) -> Result<Publication>,
}

pub(super) const CLAUDE_DESTINATIONS: &[ClaudeDestination] = &[
    ClaudeDestination {
        label: "slash commands",
        publish: publish_slash_commands,
    },
    ClaudeDestination {
        label: "skills",
        publish: publish_skills,
    },
];

/// Stage the command markdowns into `~/.claude/commands/amplihack/` (#1344).
fn publish_slash_commands(repo_root: &Path) -> Result<Publication> {
    let staged = super::claude_commands::stage_claude_commands(repo_root)?;
    if staged.copied == 0 {
        return Ok(Publication {
            detail: format!(
                "↩️  No slash-command markdown found under {} — skipping",
                repo_root.display()
            ),
            feature: None,
            preserved: staged.preserved,
            target: staged.target,
        });
    }
    Ok(Publication {
        detail: format!(
            "✅ Staged {} slash command(s) at {}",
            staged.copied,
            staged.target.display()
        ),
        feature: Some(format!(
            "{} Claude Code slash commands (/amplihack:<name>)",
            staged.copied
        )),
        preserved: staged.preserved,
        target: staged.target,
    })
}

/// Mirror the staged skills into `~/.claude/skills/` (#1438).
///
/// The check afterwards is the invariant, not the action: every skill name
/// staged under `~/.amplihack/.claude/skills` must be discoverable under
/// `~/.claude/skills` when this returns. Containment rather than equality,
/// because `~/.claude/skills` is the user's own directory and may hold skills
/// amplihack neither staged nor may remove. A shortfall is reported as a
/// warning naming the missing skills — the whole cost of #1438 was that this
/// gap printed a green line.
fn publish_skills(_repo_root: &Path) -> Result<Publication> {
    let staged_root = super::paths::staging_claude_dir()?.join("skills");
    let target = super::paths::global_claude_dir()?.join("skills");
    let staged = crate::claude_plugin::discoverable_skill_names(&staged_root)?;

    if staged.is_empty() {
        return Ok(Publication {
            detail: format!(
                "↩️  No staged skills under {} — skipping",
                staged_root.display()
            ),
            feature: None,
            preserved: Vec::new(),
            target,
        });
    }

    crate::claude_plugin::ensure_claude_plugin_installed()?;

    let published = crate::claude_plugin::discoverable_skill_names(&target)?;
    let missing = staged.difference(&published).cloned().collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(Publication {
            detail: format!(
                "✅ Published {} skill(s) to {}",
                staged.len(),
                target.display()
            ),
            feature: Some(format!("{} Claude Code skills", staged.len())),
            preserved: Vec::new(),
            target,
        });
    }
    Ok(Publication {
        detail: format!(
            "⚠️  {} of {} staged skill(s) did not reach {} — Claude sessions will not see them: {}",
            missing.len(),
            staged.len(),
            target.display(),
            missing.join(", ")
        ),
        feature: None,
        preserved: Vec::new(),
        target,
    })
}
