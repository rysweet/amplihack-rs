//! Issue #1753, Codex review R1: one refresh of an expired login in
//! `~/.claude`, before anything reads its identity. Identity reads
//! (`cred_identity`) never refresh, so without this a valid login that no
//! running session refreshed would be refused on every cycle and rotation
//! would stall. The refresh is not identity evidence: the snapshot reads the
//! refreshed credential's identity afresh, and only that reading can let a
//! login move. Issue #1962: the target is `~/.claude` alone, so neither the
//! rotate ledger nor the lease view stops it: without this refresh no identity
//! can be read and rotation stalls. A barred profile folder is never refreshed
//! here. See "An expired live login" in docs/reference/profile-command.md.

use super::super::cycle::Ctx;
use super::super::log::{Sink, msg};
#[cfg(test)]
use crate::commands::profile::usage_cmd::PROBE_CANCEL;
use crate::commands::profile::usage_direct;
#[cfg(test)]
use crate::commands::profile::usage_direct::Endpoints;
#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::time::Duration;

/// Each refresh of an expired live login ends within this time.
#[cfg(test)]
pub const LIVE_REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

/// `usage_direct::refresh_expired` against the endpoints the environment
/// sets, with the direct read's refusals: a pending swap journal under
/// `root`, a directory others can write, or a cancelled probe.
#[cfg(test)]
#[allow(dead_code)]
pub fn refresh_direct(root: &Path, creds_dir: &Path) -> Result<bool, &'static str> {
    let (ep, _warnings) = Endpoints::from_env();
    let state = root.join(".auto-switch.d");
    let journals = [
        state.join("journal.json"),
        state.join("rotate").join("journal.json"),
    ];
    let req = usage_direct::Request {
        creds_dir,
        journals: &journals,
        allow_refresh: true,
        cancel: &PROBE_CANCEL,
        timeout: LIVE_REFRESH_TIMEOUT,
        now_ms: chrono::Utc::now().timestamp_millis(),
    };
    usage_direct::refresh_expired(&ep, &req)
}

/// Refreshes the OAuth login in `~/.claude` once when its token has
/// expired. Never in a read-only run (`--dry-run`), and never when a rule
/// of the local proof names an excluded profile's login there (issue
/// #2218): its refresh token is shared with other hosts, which a refresh
/// here would log out. A pending journal, a held
/// lock, a directory others can write and an unexpired token are refused by
/// `usage_direct`. A refresh that fails is logged; the snapshot then finds no
/// trusted identity, and the cycle alerts and moves nothing.
pub fn refresh_expired_live(
    ctx: &Ctx,
    out: &mut Sink,
) -> Result<(), crate::commands::profile::ProfileError> {
    if ctx.read_only {
        return Ok(());
    }
    let dir = ctx.home.join(".claude");
    let expired = usage_direct::expired(&dir, (ctx.now)().saturating_mul(1000));
    let excluded = super::super::live_excluded_offline(&ctx.store, &ctx.rs, &ctx.home)?;
    if let Some((name, _)) = &excluded
        && expired
    {
        out.emit((ctx.now)(), &msg::excluded_not_refreshed(name));
    }
    // Structured assessment refreshes at most once and persists definitive
    // failures before any identity reader can attempt this credential again.
    super::super::login_state::assess_with(
        &ctx.store,
        &ctx.home,
        (ctx.now)(),
        ctx.identity_reader(),
    )?;
    if expired
        && excluded.is_none()
        && !usage_direct::expired(&dir, (ctx.now)().saturating_mul(1000))
    {
        out.emit((ctx.now)(), msg::LIVE_REFRESHED);
    }
    Ok(())
}
