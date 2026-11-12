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
use super::super::identify::{LoginKind, login_kind};
use super::super::log::{Sink, msg};
use crate::commands::profile::auto_switch::kit::{CREDS, now};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::usage_cmd::PROBE_CANCEL;
use crate::commands::profile::usage_direct::{self, Endpoints};
use std::path::Path;
use std::time::Duration;

/// Refreshes the login in a directory once when its token has expired;
/// `Ok(true)` when the file now holds a fresh token. Arguments: the profile
/// root, whose swap journals forbid a refresh, and the directory.
pub type RefreshFn = fn(&Path, &Path) -> Result<bool, &'static str>;

/// Each refresh of an expired live login ends within this time.
pub const LIVE_REFRESH_TIMEOUT: Duration = Duration::from_secs(10);

/// The refresh a cycle uses: [`refresh_direct`], except in unit tests,
/// where nothing is sent and no login is written.
#[cfg(not(test))]
const DEFAULT_REFRESH: RefreshFn = refresh_direct;
#[cfg(test)]
const DEFAULT_REFRESH: RefreshFn = |_, _| Ok(false);

/// `usage_direct::refresh_expired` against the endpoints the environment
/// sets, with the direct read's refusals: a pending swap journal under
/// `root`, a directory others can write, or a cancelled probe.
#[cfg_attr(test, allow(dead_code))]
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
pub fn refresh_expired_live(ctx: &Ctx, out: &mut Sink) {
    if ctx.read_only {
        return;
    }
    let dir = ctx.home.join(".claude");
    match excluded_live(ctx) {
        // Logged only when a refresh was due; the expiry is read locally.
        Ok(Some(name)) => {
            if usage_direct::expired(&dir, chrono::Utc::now().timestamp_millis()) {
                out.emit(now(), &msg::excluded_not_refreshed(&name));
            }
            return;
        }
        Ok(None) => {}
        // Issue #2218: an unknown exclude list refreshes nothing.
        Err(_) => return,
    }
    if login_kind(&dir.join(CREDS)) != LoginKind::OAuth {
        return;
    }
    match DEFAULT_REFRESH(ctx.store.root(), &dir) {
        Ok(false) => {}
        Ok(true) => out.emit(now(), msg::LIVE_REFRESHED),
        Err(why) => out.emit(now(), &format!("{} ({why})", msg::LIVE_REFRESH_FAILED)),
    }
}

/// The excluded profile whose login is in `~/.claude`, by any rule of the
/// local proof (`super::super::live_excluded_offline`); no request is made.
fn excluded_live(ctx: &Ctx) -> Result<Option<ProfileName>, crate::commands::profile::ProfileError> {
    let found = super::super::live_excluded_offline(&ctx.store, &ctx.rs, &ctx.home)?;
    Ok(found.map(|(n, _)| n))
}
