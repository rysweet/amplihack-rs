//! What `profile usage` marks live in `~/.claude` (issue #1693): after a
//! manual `/login` the rotate ledger may name a profile whose account is not
//! the one in `~/.claude`; `usage` then goes by who holds that account.
//! Read-only.

use super::super::name::ProfileName;
use super::super::store::ProfileStore;
use super::{Evidence, Live, detect};
use std::path::Path;

/// What `profile usage` marks live in `~/.claude`.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::commands::profile) enum UsageLive {
    /// The rotate ledger names a profile whose account is not in
    /// `~/.claude`, and exactly this profile holds that account, with or
    /// without its own login.
    Profile(ProfileName),
    /// As above, and no profile holds the account: `~/.claude` gets its own
    /// row.
    Home,
    /// Anything else: [`detect`]'s result and warning.
    Detected(Option<Live>, Option<&'static str>),
}

/// [`detect`], except while the rotate ledger disagrees with `~/.claude`
/// after a manual `/login`: then the profile holding the account, or
/// `~/.claude` itself. `detect` and `live_name` (the `profile login`
/// refusal and duplicate warnings) keep going by the ledger. Read-only.
pub(in crate::commands::profile) fn detect_for_usage(
    store: &ProfileStore,
    home: Option<&Path>,
) -> UsageLive {
    #[cfg(unix)]
    if let Some(h) = home
        && super::unix::rotate_hand_changed(store, h)
    {
        use super::super::rotate::identify::{Holders, home_holders};
        match home_holders(store, h) {
            Ok(Holders::One { name, .. }) => return UsageLive::Profile(name),
            Ok(Holders::None) => return UsageLive::Home,
            _ => {}
        }
    }
    let (live, warning) = detect(store, home);
    UsageLive::Detected(live, warning)
}

/// Whether `live` is still the live profile, by the same evidence. A
/// profile that became live after `live` was detected, or a swap away and
/// back under a ledger without `since`, is not seen. When either side was
/// found as the account's holder, the name decides: the rotator recording
/// that holder as live meanwhile is not a swap.
pub(in crate::commands::profile) fn still_live(
    store: &ProfileStore,
    home: Option<&Path>,
    live: &Live,
) -> bool {
    let now = match detect_for_usage(store, home) {
        UsageLive::Profile(name) => Live::holder(name),
        UsageLive::Home | UsageLive::Detected(None, _) => return false,
        UsageLive::Detected(Some(now), _) => now,
    };
    if now.evidence == Evidence::Holder || live.evidence == Evidence::Holder {
        now.name == live.name
    } else {
        now == *live
    }
}
