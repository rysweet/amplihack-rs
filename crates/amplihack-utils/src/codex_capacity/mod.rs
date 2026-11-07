//! Live Codex rate-limit readings (issue #2001).
//!
//! Each Codex account is a directory used as `CODEX_HOME`. Its figures are
//! read live by starting `codex app-server` with that home and asking it
//! `account/rateLimits/read` over JSON-RPC on stdin and stdout; no model call
//! is made. Nothing is taken from session files or any earlier reading, and
//! an account that cannot be read gives its reason, never an older value.
//!
//! `amplihack profile usage` prints these readings. The Codex rotator
//! (#2005) and Codex load balancing (#2006) are meant to use the same
//! [`list_accounts`], [`read_all`] and [`next_choice`].
//!
//! Nothing here prints, and no file inside an account directory is opened:
//! `auth.json` and `config.toml` are not read; Codex reads its own login.
//! Directories are only listed and checked for type. `CODEX_HOME` is set on
//! the child only, and `OPENAI_API_KEY`, `CODEX_API_KEY` and `RUST_LOG` are
//! removed from the child's environment.

mod parse;
#[cfg(unix)]
mod rpc;
#[cfg(all(test, unix))]
mod tests;

use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The variable naming the Codex profiles root.
pub const ROOT_ENV: &str = "CODEX_PROFILES_ROOT";
/// The variable naming the Codex program.
pub const BIN_ENV: &str = "CODEX_BIN";
/// The variable setting the per-account deadline in seconds.
pub const TIMEOUT_ENV: &str = "AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS";
/// The per-account deadline when [`TIMEOUT_ENV`] is unset or invalid.
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
/// The largest deadline [`TIMEOUT_ENV`] may set.
pub const MAX_TIMEOUT_SECS: u64 = 300;
/// The credit balance above which a full account still counts as usable,
/// as in `codex-pick`.
pub const DEFAULT_FLOOR: f64 = 3000.0;
/// The label of the `$HOME/.codex` account.
pub const HOME_LABEL: &str = "~/.codex";
/// The reason every account gets where the live read cannot run.
pub const UNSUPPORTED: &str = "codex usage read is supported only on Unix";

/// Which rate-limit window Codex reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowName {
    /// `primary`, usually the five-hour window.
    Primary,
    /// `secondary`, usually the weekly window.
    Secondary,
}

impl WindowName {
    /// The field name, `primary` or `secondary`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }
}

/// One reported window. Each figure is `None` when it was missing or not of
/// the documented kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub name: WindowName,
    pub used_percent: Option<f64>,
    pub window_minutes: Option<u64>,
    /// Unix epoch seconds as received, unchecked.
    pub resets_at: Option<i64>,
}

/// The credits Codex reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Credits {
    /// True only when Codex sent the JSON value `true`.
    pub has_credits: bool,
    pub balance: Option<f64>,
}

/// One live reading.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Reading {
    /// The windows reported, primary first.
    pub windows: Vec<Window>,
    pub credits: Option<Credits>,
    /// Cut to 200 characters and cleaned of control characters.
    pub plan_type: Option<String>,
    /// Cut to 200 characters and cleaned of control characters.
    pub rate_limit_reached_type: Option<String>,
    pub ordinary_usage_allowed: Option<bool>,
}

/// What reading one account gave.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Outcome {
    /// The account answered with its figures.
    Ok(Reading),
    /// Codex said the account is not signed in.
    NotSignedIn,
    /// No answer within the deadline.
    Timeout,
    /// Anything else, with its reason (at most 200 characters of any
    /// server or stderr text, cleaned).
    Error(String),
}

/// An account to read: its label and its directory as built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The directory name, or [`HOME_LABEL`].
    pub label: String,
    /// `<root>/<name>` or `$HOME/.codex`, not resolved through symlinks.
    pub path: PathBuf,
}

/// An account and what reading it gave.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct AccountResult {
    pub label: String,
    pub path: PathBuf,
    pub outcome: Outcome,
    /// Unix epoch seconds at which the answer arrived or the read ended.
    pub read_at: i64,
}

impl AccountResult {
    pub fn new(label: impl Into<String>, path: PathBuf, outcome: Outcome, read_at: i64) -> Self {
        Self {
            label: label.into(),
            path,
            outcome,
            read_at,
        }
    }
}

/// The Codex program: [`BIN_ENV`] when set and not empty, else `codex`.
pub fn codex_bin() -> OsString {
    std::env::var_os(BIN_ENV)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("codex"))
}

/// The Codex profiles root: [`ROOT_ENV`] when set and not empty, else
/// `<home>/.codex-profiles`; `None` with neither.
pub fn profiles_root(home: Option<&Path>) -> Option<PathBuf> {
    std::env::var_os(ROOT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".codex-profiles")))
}

/// The per-account deadline in seconds from [`TIMEOUT_ENV`], and the one
/// warning to print when its value is not a whole number from 1 to 300.
pub fn timeout_from_env() -> (u64, Option<String>) {
    timeout_secs(std::env::var_os(TIMEOUT_ENV).as_deref())
}

/// [`timeout_from_env`] for a given raw value. Unset or empty is the
/// default with no warning.
pub(crate) fn timeout_secs(raw: Option<&OsStr>) -> (u64, Option<String>) {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return (DEFAULT_TIMEOUT_SECS, None);
    };
    let text = raw.to_string_lossy();
    let parsed = Some(text.as_ref())
        .filter(|t| t.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|t| t.parse::<u64>().ok())
        .filter(|s| (1..=MAX_TIMEOUT_SECS).contains(s));
    match parsed {
        Some(s) => (s, None),
        None => (
            DEFAULT_TIMEOUT_SECS,
            Some(format!(
                "warning: {TIMEOUT_ENV}={:?} is not a whole number from 1 to {MAX_TIMEOUT_SECS}; using {DEFAULT_TIMEOUT_SECS}",
                text.as_ref()
            )),
        ),
    }
}

/// The accounts to read: each subdirectory of `root` whose name does not
/// start with `.`, in byte order of its name (symlinks to directories are
/// followed), then `$HOME/.codex` as [`HOME_LABEL`] when it is a directory
/// whose canonical path is neither the root's nor any listed account's.
///
/// A root that is missing or not a directory gives no profile accounts.
/// Any other error listing the root, or reading one of its entries, is
/// returned. Only directory listings and file types are read.
pub fn list_accounts(root: &Path, home: Option<&Path>) -> std::io::Result<Vec<Account>> {
    let mut names: Vec<OsString> = Vec::new();
    if root.is_dir() {
        for entry in std::fs::read_dir(root)? {
            let name = entry?.file_name();
            if !name.as_encoded_bytes().starts_with(b".") && root.join(&name).is_dir() {
                names.push(name);
            }
        }
    }
    names.sort_by(|a, b| a.as_encoded_bytes().cmp(b.as_encoded_bytes()));
    let mut accounts: Vec<Account> = names
        .into_iter()
        .map(|n| Account {
            label: n.to_string_lossy().into_owned(),
            path: root.join(&n),
        })
        .collect();
    if let Some(home) = home {
        let path = home.join(".codex");
        if path.is_dir() && !resolves_to_listed(&path, root, &accounts) {
            accounts.push(Account {
                label: HOME_LABEL.to_string(),
                path,
            });
        }
    }
    Ok(accounts)
}

/// Whether `path` resolves to `root` or to one of `accounts`.
fn resolves_to_listed(path: &Path, root: &Path, accounts: &[Account]) -> bool {
    let Ok(canon) = std::fs::canonicalize(path) else {
        return false;
    };
    std::iter::once(root)
        .chain(accounts.iter().map(|a| a.path.as_path()))
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .any(|p| p == canon)
}

/// Reads one account live with `bin` and the account directory as
/// `CODEX_HOME`, within `timeout` for the whole exchange.
pub fn read_account(bin: &OsStr, path: &Path, timeout: Duration) -> Outcome {
    #[cfg(unix)]
    {
        rpc::read(bin, path, timeout)
    }
    #[cfg(not(unix))]
    {
        let _ = (bin, path, timeout);
        Outcome::Error(UNSUPPORTED.to_string())
    }
}

/// Reads every account at once, one thread each, and returns the results
/// in the order of `accounts`. A [`HOME_LABEL`] account that is not signed
/// in is dropped; every other result is kept.
pub fn read_all(bin: &OsStr, accounts: &[Account], timeout: Duration) -> Vec<AccountResult> {
    std::thread::scope(|s| {
        let handles: Vec<_> = accounts
            .iter()
            .map(|a| {
                s.spawn(move || {
                    let outcome = read_account(bin, &a.path, timeout);
                    (outcome, chrono::Utc::now().timestamp())
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(accounts)
            .map(|(h, a)| {
                let (outcome, read_at) = h.join().unwrap_or_else(|_| {
                    (
                        Outcome::Error("the read failed".to_string()),
                        chrono::Utc::now().timestamp(),
                    )
                });
                AccountResult::new(a.label.clone(), a.path.clone(), outcome, read_at)
            })
            .filter(|r| !(r.label == HOME_LABEL && r.outcome == Outcome::NotSignedIn))
            .collect()
    })
}

/// An account's room, as `codex-pick` works it out: `100 - max(used
/// percentage)` over its windows (no percentage is room 0). Room of 0 or
/// less becomes `0.5` when it has credits with a balance above `floor`, and
/// `-1` otherwise.
pub fn headroom(r: &Reading, floor: f64) -> f64 {
    let fullest = r
        .windows
        .iter()
        .filter_map(|w| w.used_percent)
        .fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
    let room = fullest.map_or(0.0, |m| 100.0 - m);
    if room > 0.0 {
        return room;
    }
    let credited = r
        .credits
        .as_ref()
        .is_some_and(|c| c.has_credits && c.balance.is_some_and(|b| b > floor));
    if credited { 0.5 } else { -1.0 }
}

/// The index of the account `codex-pick`'s rule picks from these live
/// readings (not always `codex-pick`'s own choice, which reads possibly
/// older session files): among signed-in rows
/// with room above 0, the most room, then the greatest path as bytes.
pub fn next_choice(rows: &[AccountResult], floor: f64) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| match &r.outcome {
            Outcome::Ok(reading) => Some((i, headroom(reading, floor), r)),
            _ => None,
        })
        .filter(|(_, room, _)| *room > 0.0)
        .max_by(|(_, ra, a), (_, rb, b)| {
            ra.total_cmp(rb).then_with(|| {
                a.path
                    .as_os_str()
                    .as_encoded_bytes()
                    .cmp(b.path.as_os_str().as_encoded_bytes())
            })
        })
        .map(|(i, _, _)| i)
}
