//! Which profile's login is in `~/.claude` (D11, D13), and which profiles
//! may receive it (D16). Only `.claude.json` files and metadata are read;
//! account values are compared, never logged.

use super::ledger::{RotateState, safe_account};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, Ledger, Liveness, MAX_CLAUDE_JSON, exists, movable_login,
    read_bounded_nofollow, read_claude_json,
};
use crate::commands::profile::default_live::{self, Prototype};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// An `oauthAccount`: its user (`accountUuid`), its organization
/// (`organizationUuid`) and its e-mail (lower-cased). `Debug` hides all
/// three. [`Account::same`] is the only identity comparison in amplihack.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    uuid: Option<String>,
    org: Option<String>,
    email: Option<String>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Account(..)")
    }
}

impl Account {
    #[cfg(test)]
    pub fn from_claude_json(raw: &[u8]) -> Option<Self> {
        Self::from_value(&serde_json::from_slice(raw).ok()?)
    }

    /// From a parsed `.claude.json`.
    pub fn from_value(v: &serde_json::Value) -> Option<Self> {
        Self::from_block(v.get("oauthAccount")?)
    }

    /// From an `oauthAccount` block itself (as saved by auto-switch). A
    /// value over 320 bytes, with control characters, empty or not a string
    /// counts as missing; a block with neither a uuid nor an e-mail is no
    /// account.
    pub fn from_block(block: &serde_json::Value) -> Option<Self> {
        let field = |k: &str| {
            block
                .get(k)
                .and_then(serde_json::Value::as_str)
                .filter(|s| safe_account(s))
                .map(str::to_string)
        };
        let a = Self {
            uuid: field("accountUuid"),
            org: field("organizationUuid"),
            email: field("emailAddress").map(|e| e.to_lowercase()),
        };
        (a.uuid.is_some() || a.email.is_some()).then_some(a)
    }

    /// The account in `dir/.claude.json`.
    pub fn of_dir(dir: &Path) -> Option<Self> {
        Self::from_value(&read_claude_json(dir)?.0)
    }

    /// Two logins are one account when they are the same user in the same
    /// organization: one user in two organizations has two subscriptions,
    /// each with its own limits. With a uuid on both sides the uuids and
    /// the organizations (both present and equal, or both missing) decide;
    /// with a uuid on neither side the e-mails do; with a uuid on one side
    /// only they are different accounts.
    pub fn same(&self, other: &Self) -> bool {
        match (&self.uuid, &other.uuid) {
            (Some(a), Some(b)) => a == b && self.org == other.org,
            (None, None) => matches!((&self.email, &other.email), (Some(a), Some(b)) if a == b),
            _ => false,
        }
    }

    /// The value stored in the ledger: the uuid, else the e-mail.
    pub fn key(&self) -> &str {
        self.uuid.as_deref().or(self.email.as_deref()).unwrap_or("")
    }

    /// Whether a stored key names this account.
    pub fn matches_key(&self, key: &str) -> bool {
        self.uuid.as_deref() == Some(key)
            || self
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(key))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveId {
    /// This profile's login is in `~/.claude`.
    Known(ProfileName),
    /// The login has no profile; a swap adopts it first (D12).
    Adopt,
    /// The rotate ledger names this profile, but `~/.claude` holds another
    /// account: someone changed the login by hand.
    HandChanged(ProfileName),
    /// The rotate ledger names this profile, `~/.claude` has a login file,
    /// but `~/.claude.json` has no account that can be read. Claude rewrites
    /// that file often, so one bad read is not a hand change: nothing is
    /// recovered or swapped, and the next cycle reads it again (#1702).
    HomeUnreadable(ProfileName),
    /// Several login-less profiles match the account in `~/.claude`.
    Ambiguous,
    /// `~/.claude` has no login file (D13).
    NoLogin,
}

/// A profile directory without anything at `.credentials.json` (a dangling
/// symlink counts as something) that passes the spawn-time check.
fn login_less(store: &ProfileStore, name: &ProfileName) -> Option<std::path::PathBuf> {
    let dir = store.verify_profile_dir(name).ok()?;
    (!exists(&dir.join(CREDS))).then_some(dir)
}

/// The prototype ledger's profile if it may be trusted, else the reason it
/// is ignored. `Ok(None)`: no ledger, or the idle state.
fn from_prototype(
    store: &ProfileStore,
    home: Option<&Account>,
) -> Result<Option<ProfileName>, &'static str> {
    let value = match default_live::prototype(store.root()) {
        Prototype::Absent | Prototype::Live(None | Some(serde_json::Value::Null)) => {
            return Ok(None);
        }
        Prototype::Untrusted => return Err("untrusted"),
        Prototype::Live(Some(v)) => v,
    };
    let name = value
        .as_str()
        .and_then(|s| ProfileName::parse(s).ok())
        .ok_or("invalid name")?;
    let Ok(dir) = store.verify_profile_dir(&name) else {
        return Err("untrusted");
    };
    if exists(&dir.join(CREDS)) {
        return Err("profile has a login");
    }
    match (Account::of_dir(&dir), home) {
        (Some(a), Some(h)) if !a.same(h) => Err("account does not match"),
        (Some(_), None) => Err("account does not match"),
        _ => Ok(Some(name)),
    }
}

/// The live profile, and the reason the prototype ledger was ignored, if it
/// was. Reads `$HOME/.claude.json`, the ledgers and each profile's
/// `.claude.json`; never a login file.
pub fn live(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<(LiveId, Option<&'static str>), ProfileError> {
    let creds = home.join(".claude").join(CREDS);
    if !std::fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
        return Ok((LiveId::NoLogin, None));
    }
    let account = Account::of_dir(home);
    if let Some(l) = rs.load_ledger()?
        && let (Some(live), Some(key)) = (l.live.as_deref(), l.account.as_deref())
    {
        // Names in the ledger were checked when it was loaded.
        let name = ProfileName::parse(live)?;
        return Ok(match &account {
            Some(a) if a.matches_key(key) => (LiveId::Known(name), None),
            Some(_) => (LiveId::HandChanged(name), None),
            None => (LiveId::HomeUnreadable(name), None),
        });
    }
    let ignored = match from_prototype(store, account.as_ref()) {
        Ok(Some(name)) => return Ok((LiveId::Known(name), None)),
        Ok(None) => None,
        Err(reason) => Some(reason),
    };
    let Some(account) = account else {
        return Ok((LiveId::Adopt, ignored));
    };
    let mut matched = store.list()?.into_iter().filter_map(|n| {
        let name = ProfileName::parse(&n).ok()?;
        let dir = login_less(store, &name)?;
        Account::of_dir(&dir)
            .is_some_and(|a| a.same(&account))
            .then_some(name)
    });
    let id = match (matched.next(), matched.next()) {
        (Some(name), None) => LiveId::Known(name),
        (Some(_), Some(_)) => LiveId::Ambiguous,
        (None, _) => LiveId::Adopt,
    };
    Ok((id, ignored))
}

/// Why a profile is not a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    Live,
    NoLogin,
    Excluded,
    SessionRunning,
    Leased,
    Hosting,
    Parked,
    NoAccount,
}

impl Skip {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::NoLogin => "no login",
            Self::Excluded => "excluded",
            Self::SessionRunning => "session running",
            Self::Leased => "leased",
            Self::Hosting => "hosting",
            Self::Parked => "parked",
            Self::NoAccount => "no account",
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
    for name in names.iter().filter_map(|n| ProfileName::parse(n).ok()) {
        let n = name.as_str();
        let dir = store.verify_profile_dir(&name).ok();
        let skip = if input.live == Some(&name) {
            Some(Skip::Live)
        } else if !dir.as_ref().is_some_and(|d| movable_login(&d.join(CREDS))) {
            Some(Skip::NoLogin)
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
        } else if dir.as_deref().and_then(Account::of_dir).is_none() {
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

/// Who holds the account in `~/.claude` (issue #1693). Lists are sorted by
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holders {
    /// No profile has the account.
    None,
    /// Exactly one profile has it; `own_login`: something is at its
    /// `.credentials.json` (by `lstat`).
    One {
        name: ProfileName,
        own_login: bool,
    },
    Many(Vec<ProfileName>),
    /// Profiles with a login whose account cannot be read: any of them could
    /// hold the account, so no match can be trusted.
    Unreadable(Vec<ProfileName>),
    /// `~/.claude.json` has no usable account.
    HomeUnreadable,
    /// `~/.claude/.credentials.json` is missing or not a regular file.
    HomeNoLogin,
}

/// Every profile that passes the directory check, with or without its own
/// login, compared by [`Account::same`] with the account in `~/.claude`.
/// Reads `.claude.json` files and metadata only; a login-less profile with
/// no account is skipped.
pub fn home_holders(store: &ProfileStore, home: &Path) -> Result<Holders, ProfileError> {
    let creds = home.join(".claude").join(CREDS);
    if !std::fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
        return Ok(Holders::HomeNoLogin);
    }
    let Some(account) = Account::of_dir(home) else {
        return Ok(Holders::HomeUnreadable);
    };
    let mut names: Vec<ProfileName> = store
        .list()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let (mut matched, mut unreadable) = (Vec::new(), Vec::new());
    for name in names {
        let Ok(dir) = store.verify_profile_dir(&name) else {
            continue;
        };
        let own = exists(&dir.join(CREDS));
        match Account::of_dir(&dir) {
            Some(a) if a.same(&account) => matched.push((name, own)),
            Some(_) => {}
            None if own => unreadable.push(name),
            None => {}
        }
    }
    if !unreadable.is_empty() {
        return Ok(Holders::Unreadable(unreadable));
    }
    Ok(match matched.len() {
        0 => Holders::None,
        1 => {
            let (name, own_login) = matched.remove(0);
            Holders::One { name, own_login }
        }
        _ => Holders::Many(matched.into_iter().map(|(n, _)| n).collect()),
    })
}

/// The ledger key (uuid, else e-mail) of the account in `~/.claude.json`.
pub fn home_key(home: &Path) -> Option<String> {
    Account::of_dir(home).map(|a| a.key().to_string())
}

/// Whether `~/.claude.json`, read again, still holds the account `key`.
pub fn home_still_matches(home: &Path, key: &str) -> bool {
    Account::of_dir(home).is_some_and(|a| a.matches_key(key))
}

/// Why `~/.claude.json` has no usable account, in words for the escalated
/// `home-unreadable` guidance (#1702 review). Reads the file the same way
/// [`Account::of_dir`] does; never returns any of its content.
pub fn home_problem(home: &Path) -> &'static str {
    let raw = match read_bounded_nofollow(&home.join(CLAUDE_JSON), MAX_CLAUDE_JSON) {
        Ok(None) => return "it does not exist",
        Err(_) => {
            return "it cannot be read (a symlink, not a regular file, over 32 MiB or not readable)";
        }
        Ok(Some((raw, _))) => raw,
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return "it is not valid JSON (empty or cut short)";
    };
    if !v.is_object() {
        return "it is not a JSON object";
    }
    match v.get("oauthAccount") {
        None | Some(serde_json::Value::Null) => "it has no oauthAccount block",
        Some(_) => "its oauthAccount block has no usable accountUuid or emailAddress",
    }
}
