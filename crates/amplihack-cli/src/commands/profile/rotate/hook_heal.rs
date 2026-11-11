//! Issue #2279 (C8, D6): after a cycle that kept or moved the live login,
//! the rotator puts amplihack's hooks block back into
//! `~/.claude/settings.json` and the live profile's `settings.json` when it
//! has drifted.
//!
//! It reacts to the cycle, so it adds no interval. It runs outside the swap
//! lock, writes only a file that already exists and has drifted, never
//! touches `.claude.json` or `.credentials.json`, and never changes the
//! cycle's outcome or exit code. A binary older than the install stamp
//! writes nothing (#1526).

use super::cycle::{Ctx, CycleOutcome};
use super::daemon::log_line;
use super::identify::{self, LiveId};
use crate::commands::install::hook_block::{
    FileDrift, FileMerge, HookTarget, block, drift_file, hook_target, merge_file,
};
use crate::commands::profile::settings_refresh::hook_path_refusal_here;
use std::path::{Path, PathBuf};

/// What one heal looks at. Production fills it in [`after_cycle`]; tests
/// build it directly.
pub(super) struct HealCtx<'a> {
    pub home_settings: &'a Path,
    pub live_profile_dir: Option<&'a Path>,
    pub read_only: bool,
    pub target: Option<&'a HookTarget>,
    pub newer_stamp: bool,
    /// Why the live profile may not receive the hooks path (#1699).
    pub live_refusal: Option<String>,
}

/// Heals the files `ctx` names after a cycle that ended in `outcome`.
/// Returns the files written; each is logged by the caller.
pub(super) fn heal(outcome: CycleOutcome, ctx: &HealCtx<'_>) -> Vec<PathBuf> {
    if !applies(outcome, ctx.read_only, ctx.newer_stamp) {
        return Vec::new();
    }
    let Some(target) = ctx.target else {
        return Vec::new();
    };
    let mut files = vec![ctx.home_settings.to_path_buf()];
    match (ctx.live_profile_dir, &ctx.live_refusal) {
        (Some(dir), None) => files.push(dir.join("settings.json")),
        (Some(_), Some(reason)) => {
            tracing::info!("live profile settings.json not healed: {reason}");
        }
        (None, _) => {}
    }
    files
        .into_iter()
        .filter(|path| heal_one(path, target))
        .collect()
}

/// The heal's only gate: a cycle that kept or moved the live login, not a
/// dry run, from a binary no older than the install stamp.
fn applies(outcome: CycleOutcome, read_only: bool, newer_stamp: bool) -> bool {
    matches!(outcome, CycleOutcome::Swapped | CycleOutcome::Stayed) && !read_only && !newer_stamp
}

/// Writes `path` only when it exists and has drifted. Whether it wrote.
fn heal_one(path: &Path, target: &HookTarget) -> bool {
    match drift_file(path, target) {
        FileDrift::Clean | FileDrift::Absent => false,
        FileDrift::Unparseable(reason) | FileDrift::Refused(reason) => {
            tracing::warn!("{} not healed: {reason}", path.display());
            false
        }
        FileDrift::Drift(_) => match merge_file(path, target) {
            FileMerge::Written => true,
            FileMerge::AlreadyCurrent | FileMerge::Absent => false,
            FileMerge::Unparseable(reason) | FileMerge::Refused(reason) => {
                tracing::warn!("{} not healed: {reason}", path.display());
                false
            }
        },
    }
}

/// The production heal after `run_cycle` returned `outcome`, outside the
/// swap lock. [`heal`] applies the gate; this asks [`applies`] first only
/// to avoid work: resolving the hooks binary (which may run
/// `claude --version`) and identifying the live login (which may read the
/// network). The live profile comes from the rotator's own detection, never from the
/// ledger or the environment. A stamp that cannot be read counts as newer.
pub(super) fn after_cycle(outcome: CycleOutcome, ctx: &Ctx) {
    let newer_stamp = !matches!(crate::commands::install::newer_install_stamp(), Ok(None));
    if !applies(outcome, ctx.read_only, newer_stamp) {
        return;
    }
    let Some(target) = hook_target() else {
        return;
    };
    let live_dir = match identify::live(&ctx.store, &ctx.rs, &ctx.home) {
        Ok((LiveId::Known(name), _)) => match ctx.store.resolve(&name) {
            Ok(dir) => Some(dir),
            Err(_) => {
                log_line(
                    &ctx.rs,
                    &format!(
                        "hooks: live profile {name} not resolved; its settings.json not checked"
                    ),
                );
                None
            }
        },
        Ok(_) => None,
        Err(_) => {
            log_line(
                &ctx.rs,
                "hooks: live profile not identified; its settings.json not checked",
            );
            None
        }
    };
    let live_refusal = live_dir
        .as_ref()
        .and_then(|_| hook_path_refusal_here(&target.bin, ctx.store.root()));
    let home_settings = ctx.home.join(".claude").join("settings.json");
    let heal_ctx = HealCtx {
        home_settings: &home_settings,
        live_profile_dir: live_dir.as_deref(),
        read_only: ctx.read_only,
        target: Some(&target),
        newer_stamp,
        live_refusal,
    };
    let written = heal(outcome, &heal_ctx);
    if written.is_empty() {
        return;
    }
    let events = block(&target)
        .as_object()
        .map(|hooks| hooks.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    for path in written {
        log_line(
            &ctx.rs,
            &format!(
                "hooks: restored amplihack's hooks block in {} ({events})",
                path.display()
            ),
        );
    }
}

#[cfg(test)]
#[path = "tests_issue_2279_heal.rs"]
mod tests;
