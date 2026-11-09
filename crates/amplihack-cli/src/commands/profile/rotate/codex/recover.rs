//! Codex recovery: an open journal is finished or rolled back under the
//! Codex lock. Journal paths are never trusted and never removed: the link
//! must be the configured one, both targets must pass the root rule, and
//! the temp path is worked out from the configured link.

use super::super::log::Sink;
use super::cycle::{CodexCtx, Env};
use super::link::{LinkState, inspect, swap_tmp};
use super::state::{CodexJournal, CodexPhase, load, save, try_lock};
use super::swap::{Refusal, RefuseCode, finish, state_unreadable};
use super::{account_of, lines};
use crate::commands::profile::ProfileError;
use std::path::Path;

/// Whether the journal's paths are the configured link and two accounts
/// under the root.
fn trusted(j: &CodexJournal, link: &Path, root_c: &Path) -> bool {
    let is_account = |p: &Path| account_of(p, root_c).is_some_and(|(_, canon)| canon == p);
    j.link == link && is_account(&j.old_target) && is_account(&j.new_target)
}

/// Recovers the open journal `j`, or in a dry run reports it.
pub(super) fn run(
    ctx: &CodexCtx<'_>,
    out: &mut Sink,
    env: &Env,
    j: &CodexJournal,
) -> Result<(), ProfileError> {
    let now = (ctx.hooks.now)();
    if ctx.read_only {
        out.emit(now, &lines::would_recover(&j.from, &j.to, j.phase.as_str()));
        return Ok(());
    }
    ctx.rs.ensure()?;
    let Some(_lock) = try_lock(&ctx.rs.dir)? else {
        Refusal::bare(RefuseCode::LockBusy).emit(out, now);
        return Ok(());
    };
    let mut state = match load(&ctx.rs.dir) {
        Ok(s) => s,
        Err(e) => {
            state_unreadable(&e).emit(out, now);
            return Ok(());
        }
    };
    // Another process may have closed it since this cycle looked.
    let Some(j) = state.journal.clone() else {
        return Ok(());
    };
    if !trusted(&j, ctx.link, &env.root_c) {
        Refusal::bare(RefuseCode::JournalMismatch).emit(out, now);
        return Ok(());
    }
    match inspect(ctx.link, &env.root_c) {
        LinkState::Link { target, ino, .. } if target == j.new_target && ino == j.to_ino => {
            finish(
                ctx,
                &mut state,
                &j.from,
                &j.to,
                j.phase == CodexPhase::Prepared,
                now,
            )?;
            out.emit(now, &lines::recover(&j.from, &j.to, false));
        }
        LinkState::Link { target, .. } if target == j.old_target => {
            let tmp = swap_tmp(ctx.link);
            if std::fs::symlink_metadata(&tmp).is_ok_and(|m| m.file_type().is_symlink()) {
                std::fs::remove_file(&tmp)?;
            }
            state.journal = None;
            save(&ctx.rs.dir, &state, now)?;
            out.emit(now, &lines::recover(&j.from, &j.to, true));
        }
        _ => Refusal::bare(RefuseCode::JournalMismatch).emit(out, now),
    }
    Ok(())
}
