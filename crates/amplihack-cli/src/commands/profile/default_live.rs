//! Which profile's login is live in the default directory `~/.claude`
//! (issue #1578). A login rotator outside amplihack can move one account's
//! login there, leaving that profile's own directory without one; `profile
//! usage` then probes the profile against `~/.claude`.
//!
//! The rotator's ledger (`<root>/.rotator/ledger.json`) decides when it
//! exists. Without one, the single login-less profile whose `oauthAccount`
//! matches `~/.claude.json` is live. Read-only: nothing under `.rotator/` is
//! written or locked. Linux-only in practice: it needs the login file
//! `~/.claude/.credentials.json`, which Claude on macOS does not write (the
//! login is kept in the Keychain), and the ownership checks are Unix-only.

use super::store::ProfileStore;
use std::path::Path;

/// The directory label shown in output; never the expanded home path.
pub(super) const LABEL: &str = "~/.claude";

pub(super) const LEDGER_WARNING: &str =
    "warning: could not read the login rotator's ledger; no profile is marked live in ~/.claude";
pub(super) const OWN_LOGIN_WARNING: &str = "warning: the login rotator's ledger names a profile that still has its own login; it is not marked live in ~/.claude";
pub(super) const LIST_WARNING: &str =
    "warning: could not list profiles; no profile is marked live in ~/.claude";
pub(super) const AMBIGUOUS_WARNING: &str =
    "warning: more than one profile matches the account in ~/.claude; none is marked live";

/// The live profile and what showed it, so the check can be repeated after
/// the probes to see whether the login in `~/.claude` was swapped.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Live {
    pub(super) name: super::name::ProfileName,
    #[cfg_attr(not(unix), allow(dead_code))]
    evidence: Evidence,
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
enum Evidence {
    /// The ledger's `since` field (any JSON value) when it named the profile.
    Ledger(Option<serde_json::Value>),
    /// The account in `~/.claude.json` the profile matched.
    Account(Account),
}

#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
struct Account {
    uuid: Option<String>,
    /// Lower-cased.
    email: Option<String>,
}

#[cfg_attr(not(unix), allow(dead_code))]
impl Account {
    /// The uuids decide when both have one; otherwise the e-mails do.
    fn same(&self, other: &Self) -> bool {
        match (&self.uuid, &other.uuid) {
            (Some(a), Some(b)) => a == b,
            _ => matches!((&self.email, &other.email), (Some(a), Some(b)) if a == b),
        }
    }
}

/// Whether `live` is still the live profile, by the same evidence. A
/// profile that became live after `live` was detected, or a swap away and
/// back under a ledger without `since`, is not seen.
pub(super) fn still_live(store: &ProfileStore, home: Option<&Path>, live: &Live) -> bool {
    detect(store, home).0.as_ref() == Some(live)
}

/// Whether `name` could be the live profile: only a profile directory
/// without its own login can be. Used to skip warnings that cannot concern
/// any selected profile.
pub(super) fn could_be_live(store: &ProfileStore, name: &super::name::ProfileName) -> bool {
    #[cfg(unix)]
    {
        !super::auto_switch::exists(
            &store
                .root()
                .join(name.as_str())
                .join(super::auto_switch::CREDS),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = (store, name);
        false
    }
}

/// The live profile, if any, and at most one warning to print. `home` is
/// `$HOME`; `None` when it is unset or empty.
pub(super) fn detect(
    store: &ProfileStore,
    home: Option<&Path>,
) -> (Option<Live>, Option<&'static str>) {
    #[cfg(unix)]
    {
        match home {
            Some(home) => unix::detect(store, home),
            None => (None, None),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (store, home);
        (None, None)
    }
}

/// The rotator ledger's `live` field, for the usage-limit wait's login
/// fingerprint (#1618). Only a ledger that passes the trust checks counts;
/// nothing is printed.
#[cfg(unix)]
pub(super) fn rotator_live(root: &Path) -> Option<String> {
    unix::read_ledger(root)?.live?.as_str().map(str::to_string)
}

#[cfg(unix)]
mod unix {
    use super::super::auto_switch::{
        CREDS, check_root_private, euid, exists, read_bounded_nofollow, read_claude_json,
        writable_by_others,
    };
    use super::super::name::ProfileName;
    use super::super::store::ProfileStore;
    use super::{
        AMBIGUOUS_WARNING, Account, Evidence, LEDGER_WARNING, LIST_WARNING, Live, OWN_LOGIN_WARNING,
    };
    use serde::Deserialize;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    const LEDGER_DIR: &str = ".rotator";
    const LEDGER_FILE: &str = "ledger.json";
    const MAX_LEDGER: u64 = 64 * 1024;

    /// The fields used; any others are ignored.
    #[derive(Deserialize)]
    pub(super) struct RotatorLedger {
        pub(super) live: Option<serde_json::Value>,
        since: Option<serde_json::Value>,
    }

    pub(super) fn detect(
        store: &ProfileStore,
        home: &Path,
    ) -> (Option<Live>, Option<&'static str>) {
        let creds = home.join(".claude").join(CREDS);
        if !fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
            return (None, None);
        }
        // Only a ledger that is not there falls back to the account match.
        // A root that cannot be searched cannot be used at all. Anything at
        // `.rotator` other than a real directory (a symlink, even a dangling
        // one) is an untrusted ledger, and so is one inside a `.rotator`
        // that cannot be searched.
        let dir = store.root().join(LEDGER_DIR);
        match fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return from_account(store, home),
            Err(_) => return (None, Some(LIST_WARNING)),
            Ok(m) if !m.is_dir() => return (None, Some(LEDGER_WARNING)),
            Ok(_) => {}
        }
        match fs::symlink_metadata(dir.join(LEDGER_FILE)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => from_account(store, home),
            Err(_) => (None, Some(LEDGER_WARNING)),
            Ok(_) => match read_ledger(store.root()) {
                Some(l) => match from_ledger(store, l) {
                    Ok(live) => (live, None),
                    Err(w) => (None, Some(w)),
                },
                None => (None, Some(LEDGER_WARNING)),
            },
        }
    }

    /// The ledger, if the profile root is private, `.rotator` is a real
    /// directory owned by the user that nobody else can write to (a group
    /// holding only the user counts as private, as for the root), and
    /// the file is a regular file owned by the user, not writable by group
    /// or others, at most 64 KiB, and valid JSON.
    pub(super) fn read_ledger(root: &Path) -> Option<RotatorLedger> {
        check_root_private(root).ok()?;
        let dir = root.join(LEDGER_DIR);
        let m = fs::symlink_metadata(&dir).ok()?;
        if !m.is_dir() || m.uid() != euid() || writable_by_others(&m) {
            return None;
        }
        let (raw, meta) = read_bounded_nofollow(&dir.join(LEDGER_FILE), MAX_LEDGER).ok()??;
        if meta.uid() != euid() || meta.mode() & 0o022 != 0 {
            return None;
        }
        serde_json::from_slice(&raw).ok()
    }

    /// For the account match: a profile directory that passes the spawn-time
    /// check and has nothing at `.credentials.json` (a dangling symlink
    /// counts as something).
    fn login_less_dir(store: &ProfileStore, name: &ProfileName) -> Option<std::path::PathBuf> {
        let dir = store.verify_profile_dir(name).ok()?;
        (!exists(&dir.join(CREDS))).then_some(dir)
    }

    /// A null or missing `live` is the rotator's idle state: nobody is live.
    /// A name that is not a valid, usable profile is an unreadable ledger.
    fn from_ledger(store: &ProfileStore, l: RotatorLedger) -> Result<Option<Live>, &'static str> {
        let live = match l.live {
            None | Some(serde_json::Value::Null) => return Ok(None),
            Some(v) => v,
        };
        let name = live
            .as_str()
            .and_then(|s| ProfileName::parse(s).ok())
            .ok_or(LEDGER_WARNING)?;
        let dir = store
            .verify_profile_dir(&name)
            .map_err(|_| LEDGER_WARNING)?;
        if exists(&dir.join(CREDS)) {
            return Err(OWN_LOGIN_WARNING);
        }
        Ok(Some(Live {
            name,
            evidence: Evidence::Ledger(l.since),
        }))
    }

    /// `oauthAccount` in `dir/.claude.json`. An empty or non-string value
    /// counts as missing.
    fn account(dir: &Path) -> Option<Account> {
        let (v, _) = read_claude_json(dir)?;
        let block = v.get("oauthAccount")?;
        let field = |k: &str| {
            block
                .get(k)
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Account {
            uuid: field("accountUuid"),
            email: field("emailAddress").map(|e| e.to_lowercase()),
        })
    }

    fn from_account(store: &ProfileStore, home: &Path) -> (Option<Live>, Option<&'static str>) {
        let Some(home_account) = account(home) else {
            return (None, None);
        };
        let Ok(names) = store.list() else {
            return (None, Some(LIST_WARNING));
        };
        let mut matched = names
            .iter()
            .filter_map(|n| ProfileName::parse(n).ok())
            .filter(|n| {
                login_less_dir(store, n)
                    .and_then(|dir| account(&dir))
                    .is_some_and(|a| a.same(&home_account))
            });
        match (matched.next(), matched.next()) {
            (Some(name), None) => (
                Some(Live {
                    name,
                    evidence: Evidence::Account(home_account),
                }),
                None,
            ),
            (Some(_), Some(_)) => (None, Some(AMBIGUOUS_WARNING)),
            (None, _) => (None, None),
        }
    }
}
