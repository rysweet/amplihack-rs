//! Which profile's login is in `~/.claude` (D11, D13). Which profiles
//! may receive it (D16) is in `candidates`, re-exported here. With a
//! rotate ledger naming a live profile, the ledger decides (issue #1886);
//! without one, `adopt` matches trusted login identities, never logged.
//!
//! Issue #1700: this is also the one module that looks inside login and
//! settings files for a profile's kind ([`login_kind`],
//! [`api_key_setting_names`]). Those reads check which top-level keys are
//! present and return only an enum or a bool: no value leaves a function,
//! and a parse error is dropped inside it, so its text cannot quote a file.

use super::account::home_readable;
use super::adopt;
use super::ledger::RotateState;
use crate::commands::profile::ProfileError;
// Its own line: `tests_source_scan` refuses a read next to the login name.
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, MAX_CLAUDE_JSON, euid, read_bounded_nofollow, writable_by_others,
};
use crate::commands::profile::kind;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

pub use super::account::Account;
pub use super::candidates::{CandidateInput, Skip, candidates, refused_dirs};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveId {
    /// This profile's login is in `~/.claude`.
    Known(ProfileName),
    /// The login has no profile; a swap adopts it first (D12).
    Adopt,
    /// The rotate ledger names this profile, but hand-change recovery holds
    /// an open suspicion (`hand_suspect`, issue #1886): the credential in
    /// `~/.claude` answered as another profile's own login.
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
/// was. Passive routing preserves a known ledger location; active swaps
/// use `live_with` and require fresh trusted login evidence.
pub fn live(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<(LiveId, Option<&'static str>), ProfileError> {
    // Passive routing reports the ledger's location (#1886). Requiring a
    // network read here would hide a valid held login during an endpoint
    // outage. Active rotation always uses live_with and trusted evidence.
    if login_kind(&home.join(".claude").join(CREDS)) == LoginKind::OAuth
        && let Some(l) = rs.load_ledger()?
        && let Some(live) = l.live.as_deref()
    {
        let name = ProfileName::parse(live)?;
        let id = if l.hand_suspect.is_some() {
            LiveId::HandChanged(name)
        } else if !home_readable(home) {
            LiveId::HomeUnreadable(name)
        } else {
            LiveId::Known(name)
        };
        return Ok((id, None));
    }
    let budget = super::login_identity::Budget::new();
    let read = super::login_identity::default_read();
    let reader = super::login_identity::Reader {
        read: &read,
        budget: &budget,
    };
    let account = reader.oauth(&home.join(".claude")).ok();
    live_with(store, rs, home, account.as_ref())
}

pub(super) fn live_with(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    account: Option<&Account>,
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
    if let Some(l) = rs.load_ledger()?
        && let Some(live) = l.live.as_deref()
    {
        // Names in the ledger were checked when it was loaded.
        let name = ProfileName::parse(live)?;
        // Issue #1886: the ledger is taken at its word. Whose login is in
        // `~/.claude` is settled by hand-change recovery from what the
        // credential returns; an open suspicion blocks swaps and switches.
        if l.hand_suspect.is_some() {
            return Ok((LiveId::HandChanged(name), None));
        }
        if account.is_none() || !home_readable(home) {
            return Ok((LiveId::HomeUnreadable(name), None));
        }
        return Ok((LiveId::Known(name), None));
    }
    adopt::find_with(store, account)
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

/// Whether rotate live-profile detection names `name` as the login in
/// `<home>/.claude` (issue #1700: the launch guard's "held"). Any error, a
/// hand change, an ambiguous or an adopted login is not.
pub fn is_live(store: &ProfileStore, home: Option<&Path>, name: &ProfileName) -> bool {
    let Some(home) = home else {
        return false;
    };
    let rs = RotateState::new(store.root());
    matches!(live(store, &rs, home), Ok((LiveId::Known(n), _)) if &n == name)
}
