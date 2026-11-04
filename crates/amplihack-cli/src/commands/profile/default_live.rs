//! Which profile's login is live in the default directory `~/.claude`
//! (issue #1578). A login rotator outside amplihack can move one account's
//! login there, leaving that profile's own directory without one; `profile
//! usage` then probes the profile against `~/.claude`.
//!
//! A rotator ledger decides when one exists: the external prototype's
//! (`<root>/.rotator/ledger.json`) or that of `profile rotate`
//! (`<root>/.auto-switch.d/rotate/ledger.json`, issue #1592); with both, the
//! one with the later `since` (D14). Without one, the single login-less
//! profile whose `oauthAccount` matches `~/.claude.json` is live. Read-only:
//! neither ledger is written or locked here. Linux-only in practice: it needs the login file
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
    /// The same, from the `profile rotate` ledger.
    Rotate(Option<serde_json::Value>),
    /// The account in `~/.claude.json` the profile matched.
    #[cfg(unix)]
    Account(super::rotate::identify::Account),
}

#[path = "usage_live.rs"]
mod usage_live;
pub(super) use usage_live::still_live;

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

/// The live profile's name, without the warning: `None` when nothing is
/// live, detection is ambiguous or fails, `home` is `None`, or on platforms
/// without the login file.
pub(super) fn live_name(
    store: &ProfileStore,
    home: Option<&Path>,
) -> Option<super::name::ProfileName> {
    detect(store, home).0.map(|l| l.name)
}

/// The live login by the rotator ledgers alone, for the status line in
/// `~/.claude` (issue #1745).
#[cfg(unix)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct LedgerLive {
    pub(super) name: super::name::ProfileName,
    /// The winning ledger's `since`, in epoch seconds.
    pub(super) since: Option<f64>,
}

/// The login the rotator ledgers name (issue #1745). Reads the `profile
/// rotate` ledger and the prototype's once each, under the same trust rules
/// as [`detect`], and nothing else: no login file, no `~/.claude.json`, no
/// profile listing. A ledger that is absent, untrusted or names nobody
/// (`live` missing, null or empty) counts as absent. With two, the later `since` wins; on a tie the rotate
/// ledger wins, and a ledger without `since` is older. `None` when the
/// winner's `live` is not a valid profile name.
#[cfg(unix)]
pub(super) fn ledger_live(store: &ProfileStore) -> Option<LedgerLive> {
    let named = |l: unix::Ledger| match l {
        unix::Ledger::Valid(l)
            if l.live
                .as_ref()
                .is_some_and(|v| !v.is_null() && v.as_str() != Some("")) =>
        {
            Some(l)
        }
        _ => None,
    };
    let rotate = named(unix::rotate(store.root()));
    let proto = named(unix::prototype(store.root()));
    let winner = match (rotate, proto) {
        (Some(r), Some(p)) => {
            if newer(unix::since(&p), unix::since(&r)) {
                p
            } else {
                r
            }
        }
        (r, p) => r.or(p)?,
    };
    let name = winner
        .live
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .and_then(|s| super::name::ProfileName::parse(s).ok())?;
    Some(LedgerLive {
        name,
        since: unix::since(&winner),
    })
}

/// Whether `a` is strictly later than `b`; a missing `since` is older than
/// any.
#[cfg(unix)]
fn newer(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
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

/// The prototype rotator's ledger, for `profile rotate` (issue #1592).
#[cfg(unix)]
pub(super) enum Prototype {
    Absent,
    /// Present but failing the trust rules or not valid JSON.
    Untrusted,
    /// Its `live` value; `None` or JSON null is the idle state.
    Live(Option<serde_json::Value>),
}

#[cfg(unix)]
pub(super) fn prototype(root: &Path) -> Prototype {
    match unix::prototype(root) {
        unix::Ledger::Absent => Prototype::Absent,
        unix::Ledger::Bad(_) => Prototype::Untrusted,
        unix::Ledger::Valid(l) => Prototype::Live(l.live),
    }
}

#[cfg(unix)]
#[path = "default_live_since.rs"]
mod since;

#[cfg(unix)]
mod unix {
    use super::super::auto_switch::{
        CREDS, check_root_private, euid, exists, read_bounded_nofollow, writable_by_others,
    };
    use super::super::name::ProfileName;
    use super::super::rotate::identify::Account;
    use super::super::store::ProfileStore;
    use super::{
        AMBIGUOUS_WARNING, Evidence, LEDGER_WARNING, LIST_WARNING, Live, OWN_LOGIN_WARNING,
    };
    use serde::Deserialize;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    const LEDGER_DIR: &str = ".rotator";
    const ROTATE_DIRS: [&str; 2] = [".auto-switch.d", "rotate"];
    const LEDGER_FILE: &str = "ledger.json";
    const MAX_LEDGER: u64 = 64 * 1024;

    /// The fields used; any others are ignored.
    #[derive(Deserialize)]
    pub(super) struct RotatorLedger {
        pub(super) live: Option<serde_json::Value>,
        since: Option<serde_json::Value>,
    }

    pub(super) enum Ledger {
        Absent,
        /// Present but unusable; the warning to print.
        Bad(&'static str),
        Valid(RotatorLedger),
    }

    pub(super) fn detect(
        store: &ProfileStore,
        home: &Path,
    ) -> (Option<Live>, Option<&'static str>) {
        let creds = home.join(".claude").join(CREDS);
        if !fs::symlink_metadata(&creds).is_ok_and(|m| m.is_file()) {
            return (None, None);
        }
        let proto = || from(store, home, prototype(store.root()), Evidence::Ledger);
        // A rotate ledger that names nobody (an alert, an idle cycle) says
        // nothing about who is live, so it counts as absent. One that names
        // a profile is taken at its word (issue #1886): `~/.claude.json`
        // can still name the previous account for minutes after a swap.
        let rotate = match rotate(store.root()) {
            Ledger::Valid(r) if r.live.as_ref().is_none_or(serde_json::Value::is_null) => {
                Ledger::Absent
            }
            l => l,
        };
        match rotate {
            Ledger::Absent => proto(),
            // A damaged rotate ledger is reported; the prototype's decides.
            Ledger::Bad(w) => match prototype(store.root()) {
                Ledger::Valid(l) => (
                    from_ledger(store, l, Evidence::Ledger).ok().flatten(),
                    Some(w),
                ),
                _ => (None, Some(w)),
            },
            Ledger::Valid(r) => match prototype(store.root()) {
                // The later `since` wins; a tie goes to the rotate ledger, and
                // a ledger without `since` is older.
                Ledger::Valid(p) if since(&p) > since(&r) => {
                    from(store, home, Ledger::Valid(p), Evidence::Ledger)
                }
                _ => from(store, home, Ledger::Valid(r), Evidence::Rotate),
            },
        }
    }

    pub(super) fn since(l: &RotatorLedger) -> Option<f64> {
        super::since::epoch_secs(l.since.as_ref()?)
    }

    /// The live profile a ledger gives; without one, the account match.
    fn from(
        store: &ProfileStore,
        home: &Path,
        l: Ledger,
        evidence: fn(Option<serde_json::Value>) -> Evidence,
    ) -> (Option<Live>, Option<&'static str>) {
        match l {
            Ledger::Absent => from_account(store, home),
            Ledger::Bad(w) => (None, Some(w)),
            Ledger::Valid(l) => match from_ledger(store, l, evidence) {
                Ok(live) => (live, None),
                Err(w) => (None, Some(w)),
            },
        }
    }

    /// The prototype's ledger. Only a ledger that is not there falls back to
    /// the account match. A root that cannot be searched cannot be used at
    /// all. Anything at `.rotator` other than a real directory (a symlink,
    /// even a dangling one) is an untrusted ledger, and so is one inside a
    /// `.rotator` that cannot be searched.
    pub(super) fn prototype(root: &Path) -> Ledger {
        let dir = root.join(LEDGER_DIR);
        match fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ledger::Absent,
            Err(_) => return Ledger::Bad(LIST_WARNING),
            Ok(m) if !m.is_dir() => return Ledger::Bad(LEDGER_WARNING),
            Ok(_) => {}
        }
        match fs::symlink_metadata(dir.join(LEDGER_FILE)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::Absent,
            Err(_) => Ledger::Bad(LEDGER_WARNING),
            Ok(_) => {
                read_ledger(root, &[LEDGER_DIR]).map_or(Ledger::Bad(LEDGER_WARNING), Ledger::Valid)
            }
        }
    }

    /// The `profile rotate` ledger, under the same trust rules.
    pub(super) fn rotate(root: &Path) -> Ledger {
        match fs::symlink_metadata(root.join(ROTATE_DIRS[0])) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ledger::Absent,
            Err(_) => return Ledger::Bad(LIST_WARNING),
            Ok(_) => {}
        }
        let path = root
            .join(ROTATE_DIRS[0])
            .join(ROTATE_DIRS[1])
            .join(LEDGER_FILE);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::Absent,
            Err(_) => Ledger::Bad(LEDGER_WARNING),
            Ok(_) => {
                read_ledger(root, &ROTATE_DIRS).map_or(Ledger::Bad(LEDGER_WARNING), Ledger::Valid)
            }
        }
    }

    /// The ledger, if the profile root is private, each of `dirs` (such as
    /// `.rotator`) is a real directory owned by the user that nobody else can write to (a group
    /// holding only the user counts as private, as for the root), and
    /// the file is a regular file owned by the user, not writable by group
    /// or others, at most 64 KiB, and valid JSON.
    fn read_ledger(root: &Path, dirs: &[&str]) -> Option<RotatorLedger> {
        check_root_private(root).ok()?;
        let mut dir = root.to_path_buf();
        for d in dirs {
            dir.push(d);
            let m = fs::symlink_metadata(&dir).ok()?;
            if !m.is_dir() || m.uid() != euid() || writable_by_others(&m) {
                return None;
            }
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
    fn from_ledger(
        store: &ProfileStore,
        l: RotatorLedger,
        evidence: fn(Option<serde_json::Value>) -> Evidence,
    ) -> Result<Option<Live>, &'static str> {
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
            evidence: evidence(l.since),
        }))
    }

    fn from_account(store: &ProfileStore, home: &Path) -> (Option<Live>, Option<&'static str>) {
        let Some(home_account) = Account::of_dir(home) else {
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
                    .and_then(|dir| Account::of_dir(&dir))
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
