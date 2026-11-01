//! `profile statusline` without `--owner` (issue #1745): the command
//! `amplihack install` writes into `~/.claude/settings.json`.
//!
//! [`mode`] picks the run from `CLAUDE_CONFIG_DIR`. In `~/.claude` (home
//! mode) it runs `~/.amplihack/.claude/tools/statusline.sh` with the same
//! input, then prints the login capacity line for the login the rotator
//! ledgers name, and records the reading when [`should_record`] allows it,
//! whatever the auto-switch setting. It never recovers or swaps, and never
//! writes the journal, a ledger or the hosts: the rotator owns `~/.claude`.
//! In a profile's own directory the run is that profile's, as with
//! `--owner`; anywhere else only `statusline.sh` runs. Every failure gives
//! less output, never an error on stdout.

use super::capacity_line;
use super::fsx;
use super::state::State;
use super::statusline::{self, LOCK_WAIT};
use super::usage::{self, RateLimits};
use crate::commands::profile::default_live;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::root_config;
use crate::commands::profile::store::ProfileStore;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// `statusline.sh`, relative to `$HOME`.
const SCRIPT: [&str; 4] = [".amplihack", ".claude", "tools", "statusline.sh"];

/// What a run without `--owner` does (D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Mode {
    /// `~/.claude`: the rotator's login.
    Home,
    /// A profile's own directory: the same as `--owner <owner>` under the
    /// canonical `root`.
    Owned { root: PathBuf, owner: String },
    /// Anything else: `statusline.sh` only.
    Passthrough,
}

/// The mode for `CLAUDE_CONFIG_DIR`. Unset, empty or `<home>/.claude`
/// (resolved where possible, a trailing slash ignored) is home mode.
/// Otherwise the directory is canonicalised: when its parent is the
/// canonical `root` and its name is a profile name, the run is that
/// profile's. Anything else, a symlink that resolves outside the root
/// included, is passthrough.
pub(super) fn mode(config_dir: Option<&OsStr>, home: Option<&Path>, root: Option<&Path>) -> Mode {
    let Some(dir) = config_dir.filter(|d| !d.is_empty()).map(Path::new) else {
        return Mode::Home;
    };
    let resolve = |p: &Path| {
        p.canonicalize()
            .unwrap_or_else(|_| p.components().collect::<PathBuf>())
    };
    if home.is_some_and(|h| resolve(dir) == resolve(&h.join(".claude"))) {
        return Mode::Home;
    }
    let (Ok(dir), Some(Ok(root))) = (dir.canonicalize(), root.map(Path::canonicalize)) else {
        return Mode::Passthrough;
    };
    match (dir.parent(), dir.file_name().and_then(OsStr::to_str)) {
        (Some(parent), Some(name)) if parent == root && ProfileName::parse(name).is_ok() => {
            Mode::Owned {
                owner: name.to_string(),
                root,
            }
        }
        _ => Mode::Passthrough,
    }
}

/// One home-mode update with `input` (already read): `statusline.sh`, then
/// the capacity line. Returns the exit code of `statusline.sh`, or 0 when it
/// did not run.
pub(super) fn run(root: Option<&Path>, input: &[u8]) -> i32 {
    let line = root.and_then(|root| {
        std::panic::catch_unwind(|| tick(root, input, fsx::now()))
            .ok()
            .flatten()
    });
    statusline::stream_command(script_command().as_deref(), input, &[line])
}

/// `statusline.sh` as a command for `sh -c`, when it may run; an
/// unquotable path means none.
pub(super) fn script_command() -> Option<String> {
    let home = root_config::home_from_env()?;
    let path = statusline_script(&home)?;
    statusline::quotable(&path).map(statusline::sh_quote).ok()
}

/// The profile root: `--root` when given, which must be absolute (a relative
/// one is no root, with no fallback); otherwise the root this environment
/// resolves to.
pub(super) fn root_for(arg: Option<&str>) -> Option<PathBuf> {
    let root = match arg {
        Some(r) => PathBuf::from(r),
        None => {
            let home = root_config::home_from_env();
            match root_config::resolve_with(|k| std::env::var_os(k), home.as_deref()) {
                Ok((root, _)) => root,
                Err(e) => {
                    eprintln!("auto-switch: status line: no profile root: {e}");
                    return None;
                }
            }
        }
    };
    root.is_absolute().then_some(root)
}

/// `statusline.sh`, if it may run: a regular file (not a symlink) owned by
/// the user or root that nobody else can write to, in directories up to
/// `home` that pass the same check.
pub(super) fn statusline_script(home: &Path) -> Option<PathBuf> {
    let path = SCRIPT.iter().fold(home.to_path_buf(), |p, c| p.join(c));
    let m = fs::symlink_metadata(&path).ok()?;
    if !m.is_file() || !trusted(&m) {
        return None;
    }
    let mut dir = path.parent()?;
    while dir != home {
        let m = fs::symlink_metadata(dir).ok()?;
        if !m.is_dir() || !trusted(&m) {
            return None;
        }
        dir = dir.parent()?;
    }
    // `$HOME` itself may be reached through a symlink.
    fs::metadata(home)
        .ok()
        .filter(|m| m.is_dir() && trusted(m))
        .map(|_| path)
}

fn trusted(m: &fs::Metadata) -> bool {
    (m.uid() == fsx::euid() || m.uid() == 0) && !fsx::writable_by_others(m)
}

/// The capacity line for the login the rotator ledgers name, after
/// recording this reading when [`should_record`] allows it. `None` (and
/// nothing written) when the root is missing or not private, the input has
/// no usable `rate_limits`, or no trusted ledger names a usable profile.
pub(super) fn tick(root: &Path, input: &[u8], now: i64) -> Option<String> {
    // Before anything that could create the state directory.
    fsx::check_root_private(root).ok()?;
    let rl = usage::parse_statusline_input(input, now)?;
    let live = default_live::ledger_live(&ProfileStore::new(root.to_path_buf()))?;
    real_dir(&root.join(live.name.as_str())).then_some(())?;
    let state = match State::new(root) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(op = "home status line state", error = %e);
            return None;
        }
    };
    let spare = match should_record(&state, &live, &rl, now) {
        Some(account) => record(&state, &account, &rl, now, live.name.as_str()),
        None => unlocked_spare(&state, live.name.as_str(), now),
    };
    render(live.name.as_str(), &rl, spare)
}

/// `path` is a directory itself, not a symlink to one.
fn real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// The account to record this reading against, or `None` to record
/// nothing. In order, before any lock: the root is private, the winning
/// ledger has a `since` at least [`usage::COOLDOWN_SECS`] old (a login
/// swapped in more recently may still be showing the previous one's
/// figures), `<root>/<name>` is a real directory, and auto-switch
/// attribution gives an account.
pub(super) fn should_record(
    state: &State,
    live: &default_live::LedgerLive,
    rl: &RateLimits,
    now: i64,
) -> Option<String> {
    fsx::check_root_private(&state.root).ok()?;
    let since = live.since?;
    if (now as f64) - since < usage::COOLDOWN_SECS as f64 {
        return None;
    }
    if !real_dir(&state.profile_dir(live.name.as_str())) {
        return None;
    }
    let ledger = state
        .load_ledger()
        .inspect_err(|e| tracing::debug!(op = "home status line ledger", error = %e))
        .ok()?;
    let name = live.name.as_str();
    match usage::attribute(name, ledger.hosts.get(name), rl, now) {
        usage::Attribution::Record(account) => Some(account),
        usage::Attribution::Ignore(_) => None,
    }
}

/// Records `rl` against `account` under the state lock and returns the
/// spare count from the file just saved. When the lock is busy nothing is
/// recorded and the count is read without it. `None` when `usage.json` is
/// damaged or the save fails, so no count is shown from unsaved data.
fn record(state: &State, account: &str, rl: &RateLimits, now: i64, live: &str) -> Option<usize> {
    let _lock = match state.try_lock(LOCK_WAIT) {
        Ok(Some(lock)) => lock,
        Ok(None) => return unlocked_spare(state, live, now),
        Err(e) => {
            tracing::debug!(op = "home status line lock", error = %e);
            return unlocked_spare(state, live, now);
        }
    };
    // A damaged file is left for `profile usage` to report, never replaced.
    let mut file = match state.try_load_usage() {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(op = "home status line usage", error = %e);
            return None;
        }
    };
    let merged = usage::merge(file.accounts.get(account), usage::reading_from(rl, now));
    file.accounts.insert(account.to_string(), merged);
    if let Err(e) = state.save_usage(&file) {
        tracing::debug!(op = "home status line save", error = %e);
        return None;
    }
    match state.load_ledger() {
        Ok(ledger) => super::capacity::refresh(state, &file, &ledger, "statusline"),
        Err(e) => tracing::debug!(op = "home status line ledger", error = %e),
    }
    Some(spare(state, &file.accounts, live, now))
}

/// The spare count from `usage.json` read without the lock; `None` when it
/// is missing or damaged.
fn unlocked_spare(state: &State, live: &str, now: i64) -> Option<usize> {
    if !fsx::exists(&state.dir.join("usage.json")) {
        return None;
    }
    state
        .try_load_usage()
        .inspect_err(|e| tracing::debug!(op = "home status line usage", error = %e))
        .ok()
        .map(|f| spare(state, &f.accounts, live, now))
}

/// The capacity line in profile `owner`'s own session: the account
/// auto-switch has live in its directory (else `owner`), the bars from
/// `input`, and the spare count from recorded readings read without the
/// lock.
pub(super) fn owned_line(
    state: &State,
    owner: &ProfileName,
    input: &[u8],
    now: i64,
) -> Option<String> {
    let rl = usage::parse_statusline_input(input, now)?;
    let ledger = state
        .load_ledger()
        .inspect_err(|e| tracing::debug!(op = "profile status line ledger", error = %e))
        .ok();
    let login = ledger
        .as_ref()
        .map_or(owner.as_str(), |l| l.account_in(owner.as_str()));
    render(login, &rl, unlocked_spare(state, login, now))
}

fn spare(
    state: &State,
    accounts: &std::collections::BTreeMap<String, usage::Reading>,
    live: &str,
    now: i64,
) -> usize {
    capacity_line::spare(accounts, live, now, |name| {
        fs::symlink_metadata(state.profile_dir(name)).is_ok_and(|m| m.is_dir())
    })
}

fn render(login: &str, rl: &RateLimits, spare: Option<usize>) -> Option<String> {
    let pct = |w: Option<usage::Window>| w.map(|w| w.used_percentage);
    capacity_line::render(login, pct(rl.five_hour), pct(rl.seven_day), spare)
}
