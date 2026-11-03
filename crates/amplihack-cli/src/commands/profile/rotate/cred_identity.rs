//! Credential identity (issue #1886): whose login `~/.claude` holds, told
//! by what each credential returns from a direct usage read, never by
//! `.claude.json`. A login's *fingerprint* is the reset time of each window
//! the reply reports; two logins of one account reset at the same moments.
//! See "Credential identity" in docs/reference/profile-command.md.
//!
//! Only directory paths are passed to `usage_direct`, the one module that
//! opens a login file. Reads never refresh (so never write a login), start
//! no process, and use no passive reading. A fingerprint is held in memory
//! for the cycle only: it has a redacted `Debug` and nothing that writes it.

use super::identify::{LoginKind, login_kind};
use crate::commands::profile::auto_switch::kit::CREDS;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd::PROBE_CANCEL;
use crate::commands::profile::usage_direct::{self, Endpoints, Request};
use crate::commands::profile::usage_parse::{UsageReport, UsageWindow};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

/// Each identity read ends within this time.
pub const IDENTITY_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// No identity read of a cycle starts once this time has passed; one
/// already started may run its full [`IDENTITY_READ_TIMEOUT`] past it.
pub const IDENTITY_BUDGET: Duration = Duration::from_secs(30);
/// A window label longer than this is left out of a fingerprint.
const MAX_LABEL: usize = 64;
/// At most this many windows are kept in a fingerprint.
const MAX_WINDOWS: usize = 32;
/// A reset time more than this long before the read is left out (SEC-14).
const RESET_PAST: i64 = 86_400;
/// A reset time more than this long after the read is left out (SEC-14).
const RESET_FUTURE: i64 = 400 * 86_400;

/// Reads the fingerprint of the login in a directory (`<home>/.claude` or
/// `<root>/<name>`); the error is one of `usage_direct::reason`.
pub type ReadFn = fn(&Path) -> Result<Fingerprint, &'static str>;

/// The reset times of a login's windows, keyed by window, in whole seconds.
#[derive(Clone, PartialEq, Eq)]
pub struct Fingerprint(BTreeMap<String, i64>);

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fingerprint({} windows)", self.0.len())
    }
}

impl Fingerprint {
    /// From a usage report read at `now` (Unix seconds, the unit of
    /// `resets_at`): `session`, `week_all` and each `week_models` window
    /// whose reset time is after the epoch and within `now - 1 day ..= now +
    /// 400 days`. Model labels over 64 bytes are left out, and at most 32
    /// windows are kept (SEC-14).
    pub fn from_report(r: &UsageReport, now: i64) -> Self {
        let plausible = now.saturating_sub(RESET_PAST)..=now.saturating_add(RESET_FUTURE);
        let mut map = BTreeMap::new();
        let mut put = |key: String, w: &UsageWindow| {
            if let Some(at) = w.resets_at.filter(|at| *at > 0 && plausible.contains(at))
                && map.len() < MAX_WINDOWS
            {
                map.entry(key).or_insert(at);
            }
        };
        if let Some(w) = &r.session {
            put("session".into(), w);
        }
        if let Some(w) = &r.week_all {
            put("week_all".into(), w);
        }
        for w in r.week_models.iter().filter(|w| w.label.len() <= MAX_LABEL) {
            put(format!("model:{}", w.label), w);
        }
        Self(map)
    }

    /// The number of windows kept.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// How two readings compare (D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Same,
    Different,
    Unsure,
}

/// Different when a window both report resets at another time; Same when
/// both report the same windows, at least two, with equal reset times;
/// Unsure otherwise. A missing window may be a session started between
/// the reads, so it is never Different on its own.
pub fn compare(a: &Fingerprint, b: &Fingerprint) -> Verdict {
    let differs =
        a.0.iter()
            .any(|(k, at)| b.0.get(k).is_some_and(|bt| bt != at));
    if differs {
        Verdict::Different
    } else if a.0.len() >= 2 && a.0.keys().eq(b.0.keys()) {
        Verdict::Same
    } else {
        Verdict::Unsure
    }
}

/// One identity read of the login in `creds_dir` against `ep`: no refresh,
/// no swap journal needed (a refresh is the only thing it guards), at most
/// [`IDENTITY_READ_TIMEOUT`].
pub fn read_with(ep: &Endpoints, creds_dir: &Path) -> Result<Fingerprint, &'static str> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let req = Request {
        creds_dir,
        journals: &[],
        allow_refresh: false,
        cancel: &PROBE_CANCEL,
        timeout: IDENTITY_READ_TIMEOUT,
        now_ms,
    };
    let now = now_ms.div_euclid(1000);
    usage_direct::read(ep, &req).map(|d| Fingerprint::from_report(&d.report, now))
}

/// The reader a new `Ctx` starts with: [`read_direct`], except in unit
/// tests, where a cycle that a test did not give its own reader must never
/// send a fixture's token anywhere; there it reads nothing and fails.
#[cfg(not(test))]
pub const DEFAULT_READ: ReadFn = read_direct;
#[cfg(test)]
pub const DEFAULT_READ: ReadFn = |_| Err(usage_direct::reason::TRANSPORT);

/// [`read_with`] against the endpoints the environment sets. Their
/// warnings are dropped here: the cycle's measurement resolves the same
/// endpoints and logs them. Unusable endpoints send nothing (SEC-3).
#[cfg_attr(test, allow(dead_code))]
pub fn read_direct(creds_dir: &Path) -> Result<Fingerprint, &'static str> {
    let (ep, _warnings) = Endpoints::from_env();
    read_with(&ep, creds_dir)
}

/// Why recovery cannot tell; profile lists are sorted by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// `~/.claude` could not be read.
    HomeFailed,
    /// `~/.claude` reported fewer than two windows.
    HomeTooFew,
    /// `~/.claude` answers the same as several profiles.
    Many(Vec<ProfileName>),
    /// These profiles could not be read (empty: the profile list itself).
    Unread(Vec<ProfileName>),
    /// These profiles report other windows than `~/.claude`.
    Windows(Vec<ProfileName>),
    /// The cycle's read budget ran out.
    OutOfTime,
    /// The ledger's `swapped_at` is more than `SWAP_GRACE` in the future.
    ClockBack,
}

/// What the readings say about the login in `~/.claude` (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// No other profile's own login answers the same.
    Clear,
    /// Exactly this other profile's own login answers the same.
    Suspect(ProfileName),
    Unsure(Why),
}

/// The profiles that may be compared: each that passes the spawn-time
/// directory check and has its own OAuth login, sorted by name. `None` when
/// the profile root cannot be listed.
fn candidates(store: &ProfileStore) -> Option<Vec<(ProfileName, std::path::PathBuf)>> {
    let mut names: Vec<ProfileName> = store
        .list()
        .ok()?
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect();
    names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Some(
        names
            .into_iter()
            .filter_map(|n| {
                // A profile that fails the check is left out without a note.
                // That can make two holders of the copy read as `Suspect`, not
                // `Many`; moving `M`'s copy is still safe, the copies being equal.
                let dir = store.verify_profile_dir(&n).ok()?;
                (login_kind(&dir.join(CREDS)) == LoginKind::OAuth).then_some((n, dir))
            })
            .collect(),
    )
}

/// Reads `<home>/.claude`, then (only when that gave at least two windows)
/// each candidate profile, within `budget`, and returns the first finding
/// of "Findings" in docs/reference/profile-command.md that holds. `live` is
/// the profile the ledger names.
///
/// `budget` is checked before each read, and a read started inside it may
/// still take a full [`IDENTITY_READ_TIMEOUT`]; so `assess` ends within
/// `budget + IDENTITY_READ_TIMEOUT` (35 seconds for [`IDENTITY_BUDGET`]).
pub fn assess(
    store: &ProfileStore,
    home: &Path,
    live: &ProfileName,
    read: &dyn Fn(&Path) -> Result<Fingerprint, &'static str>,
    budget: Duration,
) -> Finding {
    let start = Instant::now();
    let Ok(here) = read(&home.join(".claude")) else {
        return Finding::Unsure(Why::HomeFailed);
    };
    if here.len() < 2 {
        return Finding::Unsure(Why::HomeTooFew);
    }
    let Some(profiles) = candidates(store) else {
        return Finding::Unsure(Why::Unread(Vec::new()));
    };
    let (mut same, mut unread, mut windows) = (Vec::new(), Vec::new(), Vec::new());
    let mut out_of_time = false;
    for (name, dir) in profiles {
        if start.elapsed() >= budget {
            out_of_time = true;
            break;
        }
        match read(&dir).map(|fp| compare(&here, &fp)) {
            Ok(Verdict::Same) => same.push(name),
            Ok(Verdict::Different) => {}
            Ok(Verdict::Unsure) => windows.push(name),
            Err(_) => unread.push(name),
        }
    }
    match same.as_slice() {
        [_, _, ..] => return Finding::Unsure(Why::Many(same)),
        [one] if one == live => return Finding::Clear,
        [one] => return Finding::Suspect(one.clone()),
        [] => {}
    }
    if out_of_time {
        Finding::Unsure(Why::OutOfTime)
    } else if !unread.is_empty() {
        Finding::Unsure(Why::Unread(unread))
    } else if !windows.is_empty() {
        Finding::Unsure(Why::Windows(windows))
    } else {
        Finding::Clear
    }
}
