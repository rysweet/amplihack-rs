//! The checks before hand-change recovery (issue #1693) records a profile
//! as live or moves its own login aside. Each failed check is a refusal with
//! its reason and `fix` line. They fail closed: sessions that cannot be
//! seen, an unreadable auto-switch ledger or an unknown device refuse.

use super::guide::Fix;
use super::ledger::RotateState;
use super::swap::{self, DIFFERENT_DEVICES, LOGIN_NOT_MOVABLE};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CREDS, Ledger, Liveness, REFRESH_LOCK, State, check_root_private, exists, movable_login,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const STOPPED: &str = "an earlier swap stopped half way";
const LOCKED: &str = "Claude holds its refresh lock";
const UNSEEN: &str = "cannot see running sessions";

/// What recovery may not find out by itself: running sessions, the
/// auto-switch ledger (`None`: unreadable), devices, and a hook run after
/// the backup directory is created. Built only after a hand change.
#[derive(Clone)]
pub struct Env<'a> {
    pub liveness: Liveness,
    pub auto_switch: Option<Ledger>,
    pub dev: &'a dyn Fn(&Path) -> Option<u64>,
    pub after_create: &'a dyn Fn(),
}

fn real_dev(p: &Path) -> Option<u64> {
    std::fs::symlink_metadata(p).ok().map(|m| m.dev())
}

fn nothing() {}

impl Env<'static> {
    /// The sessions `scan` finds (the cycle's own `Ctx::scan`), the
    /// auto-switch ledger and real devices.
    pub fn system(root: &Path, state: &State, scan: fn(&Path) -> Liveness) -> Self {
        Self {
            liveness: scan(root),
            auto_switch: state.load_ledger().ok(),
            dev: &real_dev,
            after_create: &nothing,
        }
    }
}

/// The checks before recording: a private root, no pending journal, and
/// the swap's home checks.
pub fn record_check(store: &ProfileStore, rs: &RotateState, home: &Path) -> Option<(String, Fix)> {
    if let Err(e) = check_root_private(store.root()) {
        return Some((format!("profile root {e}"), Fix::Root));
    }
    if exists(&rs.journal) {
        return Some((STOPPED.into(), Fix::Recovery));
    }
    match swap::check_home(home) {
        Ok(_) => None,
        Err(e) => {
            let reason = match e {
                ProfileError::RotateRefused(r) => r,
                e => e.to_string(),
            };
            let fix = if reason.starts_with("HOME") {
                Fix::Home
            } else {
                Fix::DotClaude
            };
            Some((reason, fix))
        }
    }
}

/// The extra checks before `m`'s own copy is moved. Fails closed: sessions
/// that cannot be seen, an unreadable auto-switch ledger or an unknown
/// device each refuse.
pub fn move_check(
    store: &ProfileStore,
    home: &Path,
    m: &ProfileName,
    env: &Env<'_>,
) -> Option<(String, Fix)> {
    if env.liveness.is_live(m) {
        return Some(match env.liveness {
            Liveness::Known(_) => (format!("a claude session is running on {m}"), Fix::Close),
            Liveness::Everything | Liveness::Unsupported => (UNSEEN.into(), Fix::CloseThenMove),
        });
    }
    let Ok(dir) = store.verify_profile_dir(m) else {
        return Some((
            format!("profile {m} failed the directory check"),
            Fix::ByHand,
        ));
    };
    let dot = home.join(".claude");
    if exists(&dot.join(REFRESH_LOCK)) || exists(&dir.join(REFRESH_LOCK)) {
        return Some((LOCKED.into(), Fix::Wait));
    }
    let lent = env
        .auto_switch
        .as_ref()
        .is_none_or(|a| a.leased_to(m.as_str()).is_some() || a.hosts.contains_key(m.as_str()));
    if lent {
        return Some((
            format!("{m}'s login is lent out or it hosts another login"),
            Fix::Lease,
        ));
    }
    if !movable_login(&dir.join(CREDS)) {
        return Some((LOGIN_NOT_MOVABLE.into(), Fix::ByHand));
    }
    let devs = [dot.as_path(), dir.as_path(), store.root()].map(|p| (env.dev)(p));
    if devs[0].is_none() || devs.iter().any(|d| *d != devs[0]) {
        return Some((DIFFERENT_DEVICES.into(), Fix::ByHand));
    }
    None
}
