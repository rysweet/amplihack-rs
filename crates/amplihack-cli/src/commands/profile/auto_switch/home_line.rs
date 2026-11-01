//! `profile statusline` without `--owner` (issue #1745): the command
//! `amplihack install` writes into `~/.claude/settings.json`.
//!
//! It runs `~/.amplihack/.claude/tools/statusline.sh` with the same input as
//! the first line, then prints the login capacity line for the login the
//! rotator ledger names in `~/.claude`, and records the reading for that
//! login whatever the auto-switch setting. It never recovers or swaps, and
//! never writes the journal, a ledger or the hosts: the rotator owns
//! `~/.claude`. Every failure gives less output, never an error on stdout.
//!
//! Run by a profile's wrapper (`CLAUDE_CONFIG_DIR` naming anything but
//! `~/.claude`), it prints the first line only; the wrapper prints the
//! capacity line once, for its own owner ([`wrapper_line`]).

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

/// One no-owner status-line update with `input` (already read); returns
/// the exit code of `statusline.sh`, or 0 when it did not run.
pub fn run(root_arg: Option<&str>, input: &[u8]) -> i32 {
    let home = root_config::home_from_env();
    // An unquotable path means no first line.
    let first = home
        .as_deref()
        .and_then(statusline_script)
        .and_then(|p| statusline::quotable(&p).map(statusline::sh_quote).ok());
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR");
    let line = home
        .as_deref()
        .filter(|h| !inner_run(config_dir.as_deref(), h))
        .and_then(|_| {
            let root = root_for(root_arg)?;
            std::panic::catch_unwind(|| tick(&root, input, fsx::now()))
                .ok()
                .flatten()
        });
    statusline::stream_command(first.as_deref(), input, &[line])
}

/// Whether this run is inside a profile: `CLAUDE_CONFIG_DIR` is set and
/// names anything but `<home>/.claude` (resolved where possible, a trailing
/// slash ignored).
pub(super) fn inner_run(config_dir: Option<&OsStr>, home: &Path) -> bool {
    let Some(dir) = config_dir.filter(|d| !d.is_empty()) else {
        return false;
    };
    let resolve = |p: &Path| {
        p.canonicalize()
            .unwrap_or_else(|_| p.components().collect::<PathBuf>())
    };
    resolve(Path::new(dir)) != resolve(&home.join(".claude"))
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
                    tracing::debug!(op = "home status line root", error = %e);
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

/// The capacity line a profile's wrapper adds after the no-owner command
/// (which printed only its first line there): the login live in `owner`'s
/// directory, the percentages from `input`, the spare count from recorded
/// readings, read without the lock. No spare segment without `usage.json`.
pub(super) fn wrapper_line(
    state: &State,
    owner: &ProfileName,
    input: &[u8],
    now: i64,
) -> Option<String> {
    let rl = usage::parse_statusline_input(input, now)?;
    let ledger = state
        .load_ledger()
        .inspect_err(|e| tracing::debug!(op = "wrapper status line ledger", error = %e))
        .ok();
    let login = ledger
        .as_ref()
        .map_or(owner.as_str(), |l| l.account_in(owner.as_str()));
    let spare = if fsx::exists(&state.dir.join("usage.json")) {
        state
            .try_load_usage()
            .inspect_err(|e| tracing::debug!(op = "wrapper status line usage", error = %e))
            .ok()
            .map(|f| spare(state, &f.accounts, login, now))
    } else {
        None
    };
    render(login, &rl, spare)
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
