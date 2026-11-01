//! Read-only view of the profile root for the capacity governor (issue
//! #1811): which profiles exist, which one the default pointer names, which
//! may take ordinary work, and the latest reading of the account live in
//! each.
//!
//! Nothing here creates, writes, moves or locks anything. Account keys are
//! used only to look a reading up and never leave this module: a
//! [`ProfileSnapshot`] holds profile names and numbers only, and derives
//! neither `Debug` nor `Serialize`. This is the only governor file that
//! knows how readings are stored, so a change to the reading format (#1698,
//! #1818) is made here.

use super::auto_switch::{lease_view, usage_view};
use super::name::ProfileName;
use super::store::ProfileStore;
use super::{overflow, pointer};
use std::path::Path;

/// One profile as the governor sees it. Each window is `(used percentage,
/// resets_at)`.
pub struct ProfileSnapshot {
    pub name: String,
    /// The default pointer names this profile.
    pub active: bool,
    /// A subscription profile that is not marked as overflow: work the
    /// governor admits may run on it without spending money per use.
    pub eligible: bool,
    pub five_hour: Option<(f64, i64)>,
    pub seven_day: Option<(f64, i64)>,
    /// When the reading was taken; 0 without a reading.
    pub observed_at: i64,
    /// The reading has a week window and no session window (issue #1818's
    /// `Reading::no_session_window`). Not "no session limit": the stored
    /// reading cannot tell the two apart.
    pub no_session_window: bool,
}

/// The profile root for this process's environment, the same one every
/// profile command uses. Nothing is created.
pub fn root_from_env() -> Option<std::path::PathBuf> {
    ProfileStore::from_env()
        .map(|store| store.root().to_path_buf())
        .inspect_err(|error| tracing::debug!(%error, "governor: no profile root"))
        .ok()
}

/// Every profile under `root`, sorted by name, with its latest reading.
/// `home` is the user's home directory, used only to tell an API-key profile
/// from a subscription one. A root that cannot be read gives no profiles; a
/// damaged reading file gives profiles without readings.
pub fn snapshot(root: &Path, home: Option<&Path>) -> Vec<ProfileSnapshot> {
    let store = ProfileStore::new(root.to_path_buf());
    let names = match store.list() {
        Ok(names) => names,
        Err(error) => {
            tracing::debug!(%error, "governor: profile root unreadable");
            return Vec::new();
        }
    };
    let active = match pointer::read(root) {
        Ok(name) => name.map(|n| n.to_string()),
        Err(error) => {
            tracing::debug!(%error, "governor: default pointer unreadable");
            None
        }
    };
    let leases = lease_view(root).unwrap_or_else(|error| {
        tracing::debug!(%error, "governor: lease ledger unreadable");
        None
    });
    let usage = usage_view(root).unwrap_or_else(|error| {
        tracing::debug!(%error, "governor: usage readings unreadable");
        None
    });
    let overflow = overflow::overflow_names(root);
    // Issue #1804: a root whose kinds cannot be resolved must not read as
    // "no API-key profile", or a per-token profile would join the pool.
    // Without the list, no profile is eligible.
    let platform = platform_names(&store, home);
    if platform.is_none() {
        tracing::debug!("governor: profile kinds unreadable; no profile is eligible");
    }
    names
        .into_iter()
        .map(|name| {
            let account = leases
                .as_ref()
                .map_or(name.as_str(), |l| l.account_in(&name))
                .to_string();
            let reading = usage.as_ref().and_then(|u| u.get(&account)).copied();
            let parsed = ProfileName::parse(&name).ok();
            let eligible = parsed
                .as_ref()
                .zip(platform.as_ref())
                .is_some_and(|(n, platform)| !overflow.contains(n) && !platform.contains(n));
            ProfileSnapshot {
                active: active.as_deref() == Some(name.as_str()),
                eligible,
                five_hour: reading.and_then(|r| r.five_hour),
                seven_day: reading.and_then(|r| r.seven_day),
                observed_at: reading.map_or(0, |r| r.observed_at),
                no_session_window: reading.is_some_and(|r| r.no_session_window),
                name,
            }
        })
        .collect()
}

/// The API-key profiles under `store`, or `None` when the root cannot be
/// listed (issue #1804's `try_platform_profiles`).
#[cfg(unix)]
fn platform_names(store: &ProfileStore, home: Option<&Path>) -> Option<Vec<ProfileName>> {
    super::kind::Ctx::load(store, home).try_platform_profiles(store)
}

/// No login kinds off Unix: no API-key profiles.
#[cfg(not(unix))]
fn platform_names(_store: &ProfileStore, _home: Option<&Path>) -> Option<Vec<ProfileName>> {
    Some(Vec::new())
}
