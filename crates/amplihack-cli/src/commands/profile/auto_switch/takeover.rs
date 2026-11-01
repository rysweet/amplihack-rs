//! Capacity waits for steps another login could take over (issue #1801).
//!
//! [`could_take_over`] decides when a limit is seen whether a capacity wait
//! may start. It applies only the candidate rules that cannot change during a
//! wait, because a login over its threshold may recover.
//! [`capacity_candidate`] decides during the wait: it is the full auto-switch
//! choice over readings taken since the wait began.
//!
//! Everything here is read-only, makes no network or `/usage` call, and
//! prints nothing. Every read error gives `false` or `None`, so a step keeps
//! its skip or stays on its wait.
//!
//! The issue's other trigger, a rotator cycle on record, is left out on
//! purpose: rotation needs stored profiles, so the store check covers it. A
//! `$HOME/.claude` step also needs a running rotator: see [`home_check`].

use super::config::{self, Config, FileState};
use super::fsx;
use super::rerun::{holds_login, nested_candidate};
use super::state::State;
use crate::commands::profile::kind::Ctx;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate::ledger::{RotateState, active_blocks};
use crate::commands::profile::rotate::{self, platform_block};
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd;
use std::collections::BTreeSet;

/// How far ahead of the wait's clock a reading may be stamped.
const FUTURE_MARGIN: i64 = 60;

/// The profile the rotate ledger says is live in `$HOME/.claude`.
fn live_profile(store: &ProfileStore) -> Option<String> {
    RotateState::new(store.root()).load_ledger().ok()??.live
}

/// The takeover check for a step on the login in `$HOME/.claude`. Only a
/// rotator cycle or a login change can end such a step's wait, so the check
/// holds only while a rotator is running and another stored login could
/// take over from the profile live there. Without a rotator a far reset stays
/// `reset_too_far`, failing at once as before. The store is read only when the
/// check is called, which is when a limit is seen.
pub(crate) fn home_check(store: Option<ProfileStore>) -> Box<dyn Fn() -> bool> {
    Box::new(move || {
        store.as_ref().is_some_and(|s| {
            rotate::running(s.root()) && could_take_over(Some(s), live_profile(s).as_deref())
        })
    })
}

/// Another account could take over from a step whose own login is `own`: the
/// inherited profile, or the profile live in `$HOME/.claude`. With none, the
/// login in `$HOME/.claude` is unknown and two accounts must be eligible.
pub(crate) fn could_take_over(store: Option<&ProfileStore>, own: Option<&str>) -> bool {
    store.is_some_and(|s| another_login(s, own).unwrap_or(false))
}

/// A stored profile on another account than `own` that is not excluded, holds
/// its own login on this platform and has no active spend block. Compares
/// accounts as `nested_candidate` does, so a wait never starts for a login
/// the probe could not choose. `None` on any read error.
fn another_login(store: &ProfileStore, own: Option<&str>) -> Option<bool> {
    let cfg = match config::read(store.root()) {
        FileState::Valid(c) => c,
        FileState::Missing => Config::default(),
        FileState::Corrupt => {
            tracing::debug!("auto-switch config is unreadable; no takeover check");
            return None;
        }
    };
    let state = State::new(store.root()).ok()?;
    let ledger = RotateState::new(&state.root).load_ledger().ok()?;
    let hosts = state.load_ledger().ok()?;
    let names = store.list().ok()?;
    let now = fsx::now();
    let blocked = ledger
        .as_ref()
        .map(|l| active_blocks(l, now))
        .unwrap_or_default();
    // The API-key block covers every API-key profile.
    let key_blocked: Vec<ProfileName> = ledger
        .as_ref()
        .and_then(|l| platform_block::active(l, now))
        .map(|_| Ctx::load(store, usage_cmd::home_dir().as_deref()).platform_profiles(store))
        .unwrap_or_default();
    let accounts: BTreeSet<&str> = names
        .iter()
        .filter(|n| {
            !blocked.contains_key(*n)
                && !cfg.exclude.iter().any(|e| e.as_str() == n.as_str())
                && !key_blocked.iter().any(|k| k.as_str() == n.as_str())
                && holds_login(&state, n)
        })
        .map(|n| hosts.account_in(n))
        .collect();
    Some(match own {
        Some(o) => accounts.iter().any(|a| *a != hosts.account_in(o)),
        // One eligible account may be the very login in `$HOME/.claude`.
        None => accounts.len() >= 2,
    })
}

/// A reading stamped `observed_at` was taken during a wait that began at
/// `start`, and is not stamped implausibly far ahead of `now`.
pub(super) fn is_fresh(observed_at: i64, start: i64, now: i64) -> bool {
    start < observed_at && observed_at <= now.saturating_add(FUTURE_MARGIN)
}

/// The profile auto-switch would choose now for a step that inherited
/// `inherited`, from readings taken since `start` only: a profile with an
/// older reading, or none, has shown nothing during this wait.
pub(crate) fn capacity_candidate(
    store: &ProfileStore,
    state: &State,
    inherited: &ProfileName,
    start: i64,
    now: i64,
) -> Option<ProfileName> {
    let eff = config::effective(store.root());
    if !eff.on {
        return None;
    }
    let ledger = state.load_ledger().ok()?;
    let readings = state.try_load_usage().ok()?.accounts;
    let profiles: Vec<ProfileName> = store
        .list()
        .ok()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    let stale: Vec<String> = profiles
        .iter()
        .map(ProfileName::to_string)
        .filter(|n| {
            !readings
                .get(n)
                .is_some_and(|r| is_fresh(r.observed_at, start, now))
        })
        .collect();
    let account = ledger.account_in(inherited.as_str());
    nested_candidate(
        state,
        &ledger,
        &profiles,
        &readings,
        &eff.config,
        account,
        &stale,
    )
    .filter(|p| p != inherited)
}
