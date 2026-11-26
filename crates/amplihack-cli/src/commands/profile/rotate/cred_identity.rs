//! Credential identity (issue #1886): whose login a folder holds, told by
//! the account Anthropic's OAuth profile endpoint reports for the folder's
//! own token (`usage_direct::account`), and compared with the account
//! recorded for each profile (#1845's expected account). Never by
//! `.claude.json`, `claude auth status` or usage reset times: accounts
//! created together share reset times (issue #1886, 3 November, nnorth and
//! mmorrison). See "Credential identity" in docs/reference/profile-command.md.
//!
//! Only directory paths are passed to `usage_direct`, the one module that
//! opens a login file. Reads never refresh (so never write a login), start
//! no process, and use no passive reading. An account read here is held in
//! memory for the cycle only: `Account` has a redacted `Debug`, and nothing
//! here writes or prints it. The one refresh of an expired live login is in
//! `live_refresh`, apart from these reads.

use super::identify::{Account, LoginKind, login_kind};
use crate::commands::profile::auto_switch::kit::{CREDS, State};
use crate::commands::profile::expected::{self, Record};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_direct::{self, Endpoints};
use std::path::Path;
use std::time::{Duration, Instant};

/// Each identity read ends within this time.
pub const IDENTITY_READ_TIMEOUT: Duration = Duration::from_secs(5);
/// No identity read of a cycle starts once this time has passed; one
/// already started may run its full [`IDENTITY_READ_TIMEOUT`] past it.
pub const IDENTITY_BUDGET: Duration = Duration::from_secs(30);

/// Debug builds only: raises [`IDENTITY_BUDGET`] for tests on a loaded host
/// (issue #1973; see `crate::debug_knob`). The name is outside
/// `AMPLIHACK_ROTATE*`, which the daemon removes from its own environment.
#[cfg(debug_assertions)]
pub(crate) const BUDGET_ENV: &str = "AMPLIHACK_TEST_IDENTITY_BUDGET_SECS";
/// Debug builds only: raises [`IDENTITY_READ_TIMEOUT`] for tests.
#[cfg(debug_assertions)]
pub(crate) const READ_TIMEOUT_ENV: &str = "AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS";

/// The identity budget of a cycle: [`IDENTITY_BUDGET`], unless a debug
/// build has the test knob set.
#[cfg(debug_assertions)]
pub fn identity_budget() -> Duration {
    crate::debug_knob::secs(BUDGET_ENV, IDENTITY_BUDGET)
}

/// The time limit of one identity read: [`IDENTITY_READ_TIMEOUT`], unless a
/// debug build has the test knob set.
#[cfg(debug_assertions)]
pub fn identity_read_timeout() -> Duration {
    crate::debug_knob::secs(READ_TIMEOUT_ENV, IDENTITY_READ_TIMEOUT)
}

/// Release builds: always [`IDENTITY_BUDGET`].
#[cfg(not(debug_assertions))]
pub fn identity_budget() -> Duration {
    IDENTITY_BUDGET
}

/// Release builds: always [`IDENTITY_READ_TIMEOUT`].
#[cfg(not(debug_assertions))]
pub fn identity_read_timeout() -> Duration {
    IDENTITY_READ_TIMEOUT
}

/// Reads the account of the login in a directory (`<home>/.claude` or
/// `<root>/<name>`); the error is one of `usage_direct::reason`.
pub type ReadFn = fn(&Path) -> Result<Account, &'static str>;

/// One identity read of the login in `creds_dir` against `ep`: one
/// `GET /api/oauth/profile`, no refresh, at most [`identity_read_timeout`]
/// ([`IDENTITY_READ_TIMEOUT`] unless a debug build sets the test knob).
pub fn read_with(ep: &Endpoints, creds_dir: &Path) -> Result<Account, &'static str> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    usage_direct::account(ep, creds_dir, identity_read_timeout(), now_ms)
}

/// The reader a new `Ctx` starts with: [`read_direct`], except in unit
/// tests, where only explicitly tagged task-owned fake identities are
/// accepted by a local fixture reader and no token is sent anywhere.
#[cfg(not(test))]
pub const DEFAULT_READ: ReadFn = read_direct;
#[cfg(test)]
pub const DEFAULT_READ: ReadFn = super::tests::tests_fixture_identity::read;

/// [`read_with`] against the endpoints the environment sets. Their
/// warnings are dropped here: the cycle's measurement resolves the same
/// endpoints and logs them. Unusable endpoints send nothing (SEC-3).
#[cfg_attr(test, allow(dead_code))]
pub fn read_direct(creds_dir: &Path) -> Result<Account, &'static str> {
    let (ep, _warnings) = Endpoints::from_env();
    read_with(&ep, creds_dir)
}

/// The usable #1845 record of `name`, if any.
pub fn record_of(store: &ProfileStore, name: &ProfileName) -> Option<Record> {
    let state = State::new(store.root()).ok()?;
    expected::lookup(&state, name).ok().flatten()
}

/// Why recovery cannot tell; profile lists are sorted by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The read of `~/.claude` failed, for this cause.
    Home(&'static str),
    /// `~/.claude` holds the same account as several profiles' own logins.
    Many(Vec<ProfileName>),
    /// These profiles' reads failed, each for its cause (empty: the profile
    /// list itself).
    Unread(Vec<(ProfileName, &'static str)>),
    /// The cycle's read budget ran out.
    OutOfTime,
    /// The ledger's `swapped_at` is more than `SWAP_GRACE` in the future.
    ClockBack,
    /// This profile's own login holds the account in `~/.claude`, but its
    /// record names another account: the folder itself is crossed.
    NotRecorded(ProfileName),
    /// `~/.claude` holds an account other than the one recorded for the
    /// live profile, and no profile's own login holds it.
    NotLive,
}

/// What the readings say about the login in `~/.claude` (D4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// `~/.claude` holds the live profile's account, or one no other
    /// profile's own login holds.
    Clear,
    /// Exactly this other profile's own login holds the same account.
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

/// Reads `<home>/.claude`, then (unless that is the account recorded for
/// `live`) each candidate profile, within `budget`, and returns the first
/// finding of "Findings" in docs/reference/profile-command.md that holds.
/// `live` is the profile the ledger names. Two logins are one account by
/// [`Account::same`]: the same account uuid in the same organization.
///
/// `budget` is checked before each read, and a read started inside it may
/// still take a full [`IDENTITY_READ_TIMEOUT`]; so `assess` ends within
/// `budget + IDENTITY_READ_TIMEOUT` (35 seconds for [`IDENTITY_BUDGET`]).
pub fn assess(
    store: &ProfileStore,
    home: &Path,
    live: &ProfileName,
    read: &dyn Fn(&Path) -> Result<Account, &'static str>,
    budget: Duration,
) -> Finding {
    let start = Instant::now();
    let here = match read(&home.join(".claude")) {
        Ok(here) => here,
        Err(cause) => return Finding::Unsure(Why::Home(cause)),
    };
    let live_record = record_of(store, live);
    if live_record.as_ref().is_some_and(|r| r.names(&here)) {
        return Finding::Clear;
    }
    let Some(profiles) = candidates(store) else {
        return Finding::Unsure(Why::Unread(Vec::new()));
    };
    let (mut same, mut unread) = (Vec::new(), Vec::new());
    let mut out_of_time = false;
    for (name, dir) in profiles {
        if start.elapsed() >= budget {
            out_of_time = true;
            break;
        }
        match read(&dir) {
            Ok(a) if a.same(&here) => same.push(name),
            Ok(_) => {}
            Err(cause) => unread.push((name, cause)),
        }
    }
    match same.as_slice() {
        [_, _, ..] => return Finding::Unsure(Why::Many(same)),
        [one] if one == live => return Finding::Clear,
        [one] if record_of(store, one).is_some_and(|r| !r.names(&here)) => {
            return Finding::Unsure(Why::NotRecorded(one.clone()));
        }
        [one] => return Finding::Suspect(one.clone()),
        [] => {}
    }
    if out_of_time {
        Finding::Unsure(Why::OutOfTime)
    } else if !unread.is_empty() {
        Finding::Unsure(Why::Unread(unread))
    } else if live_record.is_some() {
        Finding::Unsure(Why::NotLive)
    } else {
        Finding::Clear
    }
}
