//! Which profile's login is in `~/.claude` (D11, D13). Which profiles
//! may receive it (D16) is in `candidates`, re-exported here.
//! `.claude.json` files and metadata are read; account values are
//! compared, never logged.
//!
//! Issue #1700: this is also the one module that looks inside login and
//! settings files for a profile's kind ([`login_kind`],
//! [`api_key_setting_names`]). Those reads check which top-level keys are
//! present and return only an enum or a bool: no value leaves a function,
//! and a parse error is dropped inside it, so its text cannot quote a file.

use super::ledger::{RotateState, safe_account};
use crate::commands::profile::ProfileError;
// Its own line: `tests_source_scan` refuses a read next to the login name.
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, euid, exists, read_bounded_nofollow, read_claude_json,
    writable_by_others,
};
use crate::commands::profile::default_live::{self, Prototype};
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub use super::candidates::{CandidateInput, Skip, candidates};
pub use super::holders::{Holders, home_holders, home_key, home_still_matches};

/// The largest login or settings file read for its keys; the top-level key
/// of a claude.ai login; the API-key setting names, checked for presence.
const MAX_KIND_READ: u64 = 64 * 1024;
const OAUTH_KEY: &str = "claudeAiOauth";
const ENV_API_KEY: &str = "ANTHROPIC_API_KEY";
const API_KEY_HELPER: &str = "apiKeyHelper";
const PRIMARY_API_KEY: &str = "primaryApiKey";

/// What a login file is, by its top-level keys only: a readable JSON object
/// with `claudeAiOauth` (`OAuth`) or without it, `{}` included (`NoOAuth`:
/// Claude Code falls back to the Claude Platform credential); anything else
/// there is `Unreadable`, never proof of an API key; nothing is `Missing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
    OAuth,
    NoOAuth,
    Unreadable,
    Missing,
}

/// The kind of the login file at `path`. Read without following a
/// symlink, without blocking on a FIFO, at most 64 KiB, and only from a
/// regular file owned by the current user that group and others cannot
/// write.
pub fn login_kind(path: &Path) -> LoginKind {
    let (raw, meta) = match read_bounded_nofollow(path, MAX_KIND_READ) {
        Ok(None) => return LoginKind::Missing,
        Ok(Some(found)) => found,
        Err(_) => return LoginKind::Unreadable,
    };
    // SR-6: whoever could write the file could put `{}` there and make
    // the profile bill the API key, so its contents prove nothing.
    if meta.uid() != euid() || writable_by_others(&meta) || meta.mode() & 0o020 != 0 {
        return LoginKind::Unreadable;
    }
    match serde_json::from_slice::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Object(map)) if map.contains_key(OAUTH_KEY) => LoginKind::OAuth,
        Ok(serde_json::Value::Object(_)) => LoginKind::NoOAuth,
        _ => LoginKind::Unreadable,
    }
}

/// The JSON object at `path` when it may count as evidence: a regular file
/// of the current user that group and others cannot write, at most `limit`
/// bytes. `None` otherwise.
fn evidence_object(path: &Path, limit: u64) -> Option<serde_json::Map<String, serde_json::Value>> {
    let (raw, meta) = read_bounded_nofollow(path, limit).ok()??;
    // Group-writable counts too: anyone who could write it could plant a
    // setting name and turn the profile into an API-key profile.
    if meta.uid() != euid() || writable_by_others(&meta) || meta.mode() & 0o020 != 0 {
        return None;
    }
    match serde_json::from_slice::<serde_json::Value>(&raw).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// Whether `dir` names an API-key setting: `env.ANTHROPIC_API_KEY` or
/// `apiKeyHelper` in `settings.json` (at most 64 KiB), or `primaryApiKey`
/// in `.claude.json` (which Claude Code lets grow larger). Presence only;
/// a value is never looked at, and `apiKeyHelper` is never run.
pub fn api_key_setting_names(dir: &Path) -> bool {
    let in_settings = evidence_object(&dir.join("settings.json"), MAX_KIND_READ).is_some_and(|m| {
        m.contains_key(API_KEY_HELPER)
            || m.get("env")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|env| env.contains_key(ENV_API_KEY))
    });
    in_settings
        || evidence_object(
            &dir.join(CLAUDE_JSON),
            crate::commands::profile::auto_switch::kit::MAX_CLAUDE_JSON,
        )
        .is_some_and(|m| m.contains_key(PRIMARY_API_KEY))
}

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
    /// `~/.claude` has no login file (D13), an unreadable one, or one
    /// without OAuth while no profile is an API-key profile.
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

/// The live profile when `~/.claude` holds a login without OAuth (issue
/// #1700): the ledger's `live` when it is an API-key profile (no account
/// check: an API-key profile need not have an `oauthAccount`), else the
/// only API-key profile; several are `Ambiguous`, none `NoLogin`.
fn platform_live(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<LiveId, ProfileError> {
    let platforms = kind::platform_profiles(store, home);
    if let Some(l) = rs.load_ledger()?
        && let Some(live) = l.live.as_deref()
    {
        let name = ProfileName::parse(live)?;
        if platforms.contains(&name) {
            return Ok(LiveId::Known(name));
        }
    }
    Ok(match platforms.as_slice() {
        [] => LiveId::NoLogin,
        [one] => LiveId::Known(one.clone()),
        _ => LiveId::Ambiguous,
    })
}

/// The live profile, and the reason the prototype ledger was ignored, if it
/// was. Reads `$HOME/.claude.json`, the ledgers, each profile's
/// `.claude.json`, and which top-level keys the login files have.
pub fn live(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<(LiveId, Option<&'static str>), ProfileError> {
    let creds = home.join(".claude").join(CREDS);
    if !std::fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
        return Ok((LiveId::NoLogin, None));
    }
    match login_kind(&creds) {
        LoginKind::OAuth => {}
        LoginKind::NoOAuth => return Ok((platform_live(store, rs, home)?, None)),
        // A damaged file is never an API-key login.
        LoginKind::Unreadable | LoginKind::Missing => return Ok((LiveId::NoLogin, None)),
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
