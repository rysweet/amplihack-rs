//! Duplicate logins: two logins of the same account and organization, by
//! [`Account::same`]. Used by `profile login` to refuse one (failing closed
//! when the profiles cannot be read), and by `auto-switch status` and
//! `profile usage` to warn about one. Read-only:
//! only `.claude.json` files (each profile's and `$HOME`'s) and the saved
//! `oauthAccount` blocks are read; account values are never printed.

use super::auto_switch::kit::{Ledger, State, account};
use super::default_live;
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::store::ProfileStore;
use super::usage_cmd::sanitize_display;
use std::path::Path;

/// How the login in `~/.claude` is named in messages.
pub(super) const HOME_LABEL: &str = "~/.claude";

/// Where a login is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Holder {
    Profile(ProfileName),
    /// `~/.claude`; `live` is the rotator's live profile when it is known.
    Home {
        live: Option<ProfileName>,
    },
}

impl Holder {
    /// The name shown in the warning: the profile, or `~/.claude`.
    fn shown(&self) -> String {
        match self {
            Self::Profile(n) => sanitize_display(n.as_str(), false),
            Self::Home { .. } => HOME_LABEL.to_string(),
        }
    }
}

/// Every profile whose account is known, in `store.list()` order, then
/// `~/.claude` when `$HOME/.claude.json` names an account. Opens the
/// auto-switch state read-only to find directories hosting another login.
/// For the warnings: when the profiles cannot be read, one `warning:` line
/// is printed and nothing is compared.
pub(super) fn accounts(store: &ProfileStore, home: Option<&Path>) -> Vec<(Holder, Account)> {
    try_accounts(store, home).unwrap_or_else(|e| {
        eprintln!("warning: could not check for duplicate logins: {e:#}");
        Vec::new()
    })
}

/// [`accounts`], with an error when the profiles cannot be listed or the
/// auto-switch state cannot be opened, so `profile login` fails closed.
pub(super) fn try_accounts(
    store: &ProfileStore,
    home: Option<&Path>,
) -> anyhow::Result<Vec<(Holder, Account)>> {
    let names: Vec<ProfileName> = store
        .list()
        .map_err(|e| anyhow::anyhow!("could not list profiles: {e}"))?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    let live = default_live::live_name(store, home);
    let state = State::new(store.root())
        .map_err(|e| anyhow::anyhow!("could not open the auto-switch state: {e}"))?;
    // A damaged ledger only means a hosting directory is read as its own;
    // `auto-switch status` reports the damage itself.
    let ledger = state.exists().then(|| state.load_ledger().ok()).flatten();
    Ok(accounts_with(&state, ledger.as_ref(), &names, home, live))
}

/// [`accounts`] with the state, ledger, names and live profile already
/// read, as `auto-switch status` has them.
pub(super) fn accounts_with(
    state: &State,
    ledger: Option<&Ledger>,
    names: &[ProfileName],
    home: Option<&Path>,
    live: Option<ProfileName>,
) -> Vec<(Holder, Account)> {
    let mut out: Vec<(Holder, Account)> = names
        .iter()
        .filter_map(|n| {
            let hosting = ledger.is_some_and(|l| l.hosts.contains_key(n.as_str()));
            account(state, n.as_str(), hosting).map(|a| (Holder::Profile(n.clone()), a))
        })
        .collect();
    out.extend(home_entry(home, live));
    out
}

fn home_entry(home: Option<&Path>, live: Option<ProfileName>) -> Option<(Holder, Account)> {
    Account::of_dir(home?).map(|a| (Holder::Home { live }, a))
}

/// Every pair of holders with the same account: profile pairs in list
/// order, then pairs with `~/.claude` as `(Home, Profile)`. The rotator's
/// live profile is never paired with `~/.claude`: that login is its own.
pub(super) fn pairs(list: &[(Holder, Account)]) -> Vec<(Holder, Holder)> {
    let profiles: Vec<&(Holder, Account)> = list
        .iter()
        .filter(|(h, _)| matches!(h, Holder::Profile(_)))
        .collect();
    let mut out = Vec::new();
    for (i, (a, aa)) in profiles.iter().enumerate() {
        for (b, ba) in &profiles[i + 1..] {
            if aa.same(ba) {
                out.push((a.clone(), b.clone()));
            }
        }
    }
    for (h, ha) in list {
        let Holder::Home { live } = h else {
            continue;
        };
        for (p, pa) in &profiles {
            let is_live = matches!(p, Holder::Profile(n) if live.as_ref() == Some(n));
            if !is_live && ha.same(pa) {
                out.push((h.clone(), p.clone()));
            }
        }
    }
    out
}

/// The duplicate-login warning, without a `note: ` or `warning: ` prefix.
/// The fix names `b`, which is always a profile.
pub(super) fn warning(a: &Holder, b: &Holder) -> String {
    let (a, b) = (a.shown(), b.shown());
    format!(
        "profiles '{a}' and '{b}' are signed in to the same account and organization \
         (duplicate login); fix: run 'amplihack profile login {b}' with the browser signed \
         out of claude.ai or in a private window."
    )
}

/// The holder as named in the `profile login` refusal.
pub(super) fn holder_label(h: &Holder) -> String {
    match h {
        Holder::Profile(n) => format!("profile '{}'", sanitize_display(n.as_str(), false)),
        Holder::Home { live: Some(n) } => format!(
            "{HOME_LABEL} (rotator live profile '{}')",
            sanitize_display(n.as_str(), false)
        ),
        Holder::Home { live: None } => HOME_LABEL.to_string(),
    }
}
