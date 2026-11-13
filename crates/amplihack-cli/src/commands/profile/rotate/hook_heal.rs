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
//!
//! Each file it writes is also counted in `<state>/hook-heals`, one Unix
//! time per line, kept for a day. `rotate status` prints the count as
//! `hooks repaired:`, so a repair on every cycle is visible instead of being
//! kept up quietly: a count above zero means something is still removing
//! the hooks.

use super::cycle::{Ctx, CycleOutcome};
use super::daemon::log_line;
use super::identify::{self, LiveId};
use super::ledger::RotateState;
use super::log::timestamp;
use crate::commands::install::hook_block::{
    FileDrift, FileMerge, HookTarget, block, drift_file, hook_target, merge_file,
};
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::settings_refresh::hook_path_refusal_here;
use std::path::{Path, PathBuf};

/// What one heal looks at. Production fills it in [`after_cycle`]; tests
/// build it directly.
pub(super) struct HealCtx<'a> {
    pub home_settings: &'a Path,
    pub live_profile_dir: Option<&'a Path>,
    pub read_only: bool,
    pub target: Option<&'a HookTarget>,
    /// Implicit publication is refused by origin, opt-out or version checks.
    pub publication_refused: bool,
    /// Why the live profile may not receive the hooks path (#1699).
    pub live_refusal: Option<String>,
}

/// Heals the files `ctx` names after a cycle that ended in `outcome`.
/// Returns the files written; each is logged by the caller.
pub(super) fn heal(outcome: CycleOutcome, ctx: &HealCtx<'_>) -> Vec<PathBuf> {
    if !applies(outcome, ctx.read_only, ctx.publication_refused) {
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
/// dry run, from an executable eligible for implicit hook publication.
fn applies(outcome: CycleOutcome, read_only: bool, publication_refused: bool) -> bool {
    matches!(outcome, CycleOutcome::Swapped | CycleOutcome::Stayed)
        && !read_only
        && !publication_refused
}

/// Writes `path` only when it exists and has drifted from both answers to
/// whether `StopFailure` is registered. The rotator reads `claude --version`
/// from its own PATH and keeps that answer until it restarts, so it may
/// disagree with the launch repair; a file current for either answer is
/// left alone, as the launch repair does for a non-Claude launch, so the two
/// never take turns rewriting the file and leaving backups. Whether it wrote.
fn heal_one(path: &Path, target: &HookTarget) -> bool {
    let other = HookTarget {
        limit_resume: !target.limit_resume,
        ..target.clone()
    };
    if drift_file(path, &other) == FileDrift::Clean {
        return false;
    }
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
/// ledger or the environment. A stamp that cannot be read refuses publication.
pub(super) fn after_cycle(outcome: CycleOutcome, ctx: &Ctx) {
    let publication_refused = !crate::commands::install::may_publish_implicitly();
    if !applies(outcome, ctx.read_only, publication_refused) {
        return;
    }
    let Some(target) = hook_target() else {
        return;
    };
    let live_dir = match identify::live(&ctx.store, &ctx.rs, &ctx.home) {
        Ok((LiveId::Known(name), _)) => match ctx.store.resolve(&name) {
            Ok(dir) => Some(dir),
            Err(error) => {
                log_line(
                    &ctx.rs,
                    &format!(
                        "hooks: live profile {name} not resolved ({error}); its settings.json \
                         not checked"
                    ),
                );
                None
            }
        },
        Ok(_) => None,
        Err(error) => {
            log_line(
                &ctx.rs,
                &format!(
                    "hooks: live profile not identified ({error}); its settings.json not checked"
                ),
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
        publication_refused,
        live_refusal,
    };
    let written = heal(outcome, &heal_ctx);
    if written.is_empty() {
        return;
    }
    record_heals(&ctx.rs, now(), written.len());
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

/// The record of recent repairs, in the rotate state directory.
const HEAL_RECORD: &str = "hook-heals";
/// How far back the record and the `hooks repaired:` count reach.
const HEAL_WINDOW: i64 = 86_400;

/// The repair times in the record that fall in the day before `now`. A
/// missing, unreadable or refused record counts as none.
fn recent_heals(rs: &RotateState, now: i64) -> Vec<i64> {
    let Ok(Some(raw)) = rs.read(&rs.dir.join(HEAL_RECORD), "hook repair record") else {
        return Vec::new();
    };
    String::from_utf8_lossy(&raw)
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .filter(|at| *at <= now && now - *at < HEAL_WINDOW)
        .collect()
}

/// Adds `files` repairs at `now` to the record and drops entries older than
/// a day, so the file stays small. A failed write is logged and changes
/// nothing else.
pub(super) fn record_heals(rs: &RotateState, now: i64, files: usize) {
    let mut times = recent_heals(rs, now);
    times.extend(std::iter::repeat_n(now, files));
    let body: String = times.iter().map(|at| format!("{at}\n")).collect();
    if let Err(error) = rs.write(HEAL_RECORD, body.as_bytes()) {
        log_line(rs, &format!("hooks: could not record the repair ({error})"));
    }
}

/// The `hooks repaired:` line for `rotate status`.
pub(super) fn status_line(rs: &RotateState, now: i64) -> String {
    let times = recent_heals(rs, now);
    match times.iter().max() {
        None => "hooks repaired: none in the last 24h".to_string(),
        Some(last) => format!(
            "hooks repaired: {} in the last 24h (last {}); something is still removing \
             amplihack's hooks",
            times.len(),
            timestamp(*last)
        ),
    }
}

#[cfg(test)]
#[path = "tests_issue_2279_heal.rs"]
mod tests;
