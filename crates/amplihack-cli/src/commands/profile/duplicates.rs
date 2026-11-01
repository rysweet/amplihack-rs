//! Duplicate logins: two logins of the same account and organization, by
//! [`Account::same`]. Used by `profile login` to refuse one (failing closed
//! when the profiles cannot be read), and by `auto-switch status` and
//! `profile usage` to warn about one. Read-only:
//! only `.claude.json` files (each profile's and `$HOME`'s) and the saved
//! `oauthAccount` blocks are read; account values are never printed.
//!
//! Issue #1700: API-key profiles are left out on both sides, and so is
//! `~/.claude` while it holds a login without OAuth: they share one Claude
//! Platform credential by design.

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
    let kinds = super::kind::Ctx::load(store, home);
    let names: Vec<ProfileName> = store
        .list()
        .map_err(|e| anyhow::anyhow!("could not list profiles: {e}"))?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .filter(|n| kinds.kind(store, n) != super::kind::Kind::Platform)
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
    let home = home?;
    // On the API key, `$HOME/.claude.json` may still name the account of the
    // login that left: it is not the login in `~/.claude`.
    let creds = home.join(".claude").join(super::auto_switch::kit::CREDS);
    if super::rotate::identify::login_kind(&creds) == super::rotate::identify::LoginKind::NoOAuth {
        return None;
    }
    Account::of_dir(home).map(|a| (Holder::Home { live }, a))
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

/// Issue #1845 (D3): the holder the duplicate warning tells the user to
/// log in again. In a pair with `~/.claude`, the profile. In a pair of
/// profiles, the one profile whose current account is not the account
/// recorded for it; `None` when neither or both are, so amplihack cannot
/// tell. Without a state no record is read.
pub(super) fn wrong<'h>(
    state: Option<&State>,
    list: &[(Holder, Account)],
    a: &'h Holder,
    b: &'h Holder,
) -> Option<&'h Holder> {
    if matches!(a, Holder::Home { .. }) {
        return Some(b);
    }
    let state = state?;
    let mismatched = |h: &Holder| match h {
        Holder::Profile(n) => {
            list.iter()
                .find(|(x, _)| x == h)
                .and_then(|(_, acc)| super::expected::mismatched(state, n, acc))
                == Some(true)
        }
        Holder::Home { .. } => false,
    };
    match (mismatched(a), mismatched(b)) {
        (true, false) => Some(a),
        (false, true) => Some(b),
        _ => None,
    }
}

/// The duplicate-login warning, without a `note: ` or `warning: ` prefix.
/// The fix names `wrong`; without it, the warning says amplihack cannot
/// tell which holder has the wrong login.
pub(super) fn warning(a: &Holder, b: &Holder, wrong: Option<&Holder>) -> String {
    let (a, b) = (a.shown(), b.shown());
    match wrong {
        Some(w) => format!(
            "profiles '{a}' and '{b}' are signed in to the same account and organization \
             (duplicate login); fix: run 'amplihack profile login {}' with the browser signed \
             out of claude.ai or in a private window.",
            w.shown()
        ),
        None => format!(
            "profiles '{a}' and '{b}' are signed in to the same account and organization \
             (duplicate login); amplihack cannot tell which of them holds the wrong login; \
             fix: run 'amplihack profile login <name>' for the one that should hold a \
             different account, with the browser signed out of claude.ai or in a private \
             window."
        ),
    }
}

/// One warning per duplicate pair in `list`, in [`pairs`] order.
pub(super) fn warnings(state: Option<&State>, list: &[(Holder, Account)]) -> Vec<String> {
    pairs(list)
        .iter()
        .map(|(a, b)| warning(a, b, wrong(state, list, a, b)))
        .collect()
}

/// Issue #1845: one warning per profile whose login is not the account
/// recorded for it, in `list` order, whether or not it also forms a
/// duplicate pair. A session started with `CLAUDE_CONFIG_DIR` on a profile
/// folder can write another account's login there without a duplicate.
/// Profiles without a usable record are not checked.
pub(super) fn record_warnings(state: Option<&State>, list: &[(Holder, Account)]) -> Vec<String> {
    let Some(state) = state else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|(h, acc)| match h {
            Holder::Profile(n) if super::expected::mismatched(state, n, acc) == Some(true) => {
                Some(record_warning(h))
            }
            _ => None,
        })
        .collect()
}

/// The warning for a profile whose login is not its recorded account,
/// without a `warning: ` prefix. Names the profile, never an account.
pub(super) fn record_warning(h: &Holder) -> String {
    let shown = h.shown();
    format!(
        "profile '{shown}' holds a login that is not the account recorded for it; fix: run \
         'amplihack profile login {shown}' with the browser signed out of claude.ai or in a \
         private window."
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
