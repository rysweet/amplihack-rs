//! Which profiles may receive the login in `~/.claude` (D16), and why each
//! other profile is skipped. Split from `identify`, which re-exports it;
//! the kind and account checks it makes go through `kind` and
//! [`Account::of_dir`], so no file is looked inside here.

use super::identify::Account;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, Ledger, Liveness, exists, movable_login};
use crate::commands::profile::kind::{self, Kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;

/// Why a profile is not a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Live,
    /// The profile directory fails the ownership and mode check, so its
    /// login cannot be checked (issue #1698). A plan profile skipped for
    /// this holds a move onto overflow until the directory is fixed.
    Refused,
    NoLogin,
    Excluded,
    SessionRunning,
    Leased,
    Hosting,
    Parked,
    NoAccount,
    /// A subscription profile whose login file has no OAuth login or is
    /// unreadable: moved into `~/.claude` it would bill the API key.
    NotOAuth,
}

impl Skip {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Refused => "directory refused",
            Self::NoLogin => "no login",
            Self::Excluded => "excluded",
            Self::SessionRunning => "session running",
            Self::Leased => "leased",
            Self::Hosting => "hosting",
            Self::Parked => "parked",
            Self::NoAccount => "no account",
            Self::NotOAuth => "not oauth",
        }
    }
}

pub struct CandidateInput<'a> {
    pub live: Option<&'a ProfileName>,
    /// The auto-switch `exclude` list.
    pub exclude: Vec<ProfileName>,
    pub liveness: Liveness,
    /// The auto-switch ledger: leases and hosted directories.
    pub auto_switch: Ledger,
}

/// The candidates, then every skipped profile with its reason.
pub type Candidates = (Vec<ProfileName>, Vec<(ProfileName, Skip)>);

/// The profiles that may receive the login in `~/.claude`, sorted by name,
/// and every other profile with the first reason it is skipped. A profile
/// root that cannot be listed is an error, not an empty list.
pub fn candidates(
    store: &ProfileStore,
    input: &CandidateInput<'_>,
) -> Result<Candidates, ProfileError> {
    let (mut ok, mut skips) = (Vec::new(), Vec::new());
    let parked = store.root().join(".auto-switch.d").join("parked");
    let names = store.list()?;
    // Past `NoLogin` the login file is there: the home login plays no part.
    let kinds = kind::Ctx::load(store, None);
    for name in names.iter().filter_map(|n| ProfileName::parse(n).ok()) {
        let n = name.as_str();
        let dir = store.verify_profile_dir(&name).ok();
        let resolved = std::cell::OnceCell::new();
        let resolved = || resolved.get_or_init(|| kinds.resolve_kind(store, &name));
        let skip = if input.live == Some(&name) {
            Some(Skip::Live)
        } else if dir.is_none() {
            // An excluded profile is out of rotation whatever its directory,
            // so it holds nothing back.
            Some(if input.exclude.contains(&name) {
                Skip::Excluded
            } else {
                Skip::Refused
            })
        } else if !dir.as_ref().is_some_and(|d| movable_login(&d.join(CREDS))) {
            Some(Skip::NoLogin)
        } else if !resolved().may_be_live() {
            Some(Skip::NotOAuth)
        } else if input.exclude.contains(&name) {
            Some(Skip::Excluded)
        } else if input.liveness.is_live(&name) {
            Some(Skip::SessionRunning)
        } else if input.auto_switch.leased_to(n).is_some() {
            Some(Skip::Leased)
        } else if input.auto_switch.hosts.contains_key(n) {
            Some(Skip::Hosting)
        } else if exists(&parked.join(n)) {
            Some(Skip::Parked)
        } else if resolved().kind == Kind::Subscription
            && dir.as_deref().and_then(Account::of_dir).is_none()
        {
            Some(Skip::NoAccount)
        } else {
            None
        };
        match skip {
            Some(s) => skips.push((name, s)),
            None => ok.push(name),
        }
    }
    Ok((ok, skips))
}

/// The profiles skipped as [`Skip::Refused`], for the overflow rule.
pub fn refused_dirs(skips: &[(ProfileName, Skip)]) -> Vec<ProfileName> {
    skips
        .iter()
        .filter(|(_, s)| *s == Skip::Refused)
        .map(|(n, _)| n.clone())
        .collect()
}
