//! `profile statusline` without `--owner` (issue #1745): the command
//! `amplihack install` writes into `~/.claude/settings.json`.
//!
//! [`mode`] picks the run from `CLAUDE_CONFIG_DIR`. In `~/.claude` (home
//! mode) it runs `~/.amplihack/.claude/tools/statusline.sh` with the same
//! input, then prints the login capacity line for the login the rotator
//! ledgers name and the no-capacity notice when it is due, and records the reading when [`should_record`] allows it,
//! whatever the auto-switch setting. It records into `home-usage.json`,
//! never `usage.json`: nothing here checks which account `~/.claude` really
//! holds, so these readings feed only the status line's spare count and
//! never a passive reading, `profile rotate` or auto-switch
//! (`State::try_load_home_usage`). It never recovers or swaps, and never
//! writes `usage.json`, the journal, a ledger, the hosts or the capacity
//! file: the rotator owns `~/.claude`. It opens no login credentials file, directly or
//! through anything it calls (SR1); the integration test
//! `a_recorded_home_tick_with_auto_switch_on_opens_no_credentials_file`
//! checks that with inotify while the binary runs.
//! In a profile's own directory the run is that profile's, as with
//! `--owner` but with no recovery, no swap check and no capacity-file
//! refresh, so it too opens and moves no login file; anywhere else only
//! `statusline.sh` runs. Every failure gives less output, never an error on
//! stdout.

use super::capacity;
use super::capacity_line;
use super::config;
use super::fsx;
use super::state::State;
use super::statusline::{self, LOCK_WAIT};
use super::usage::{self, RateLimits, Reading};
use crate::commands::profile::default_live;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::root_config;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeMap;
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

/// One home-mode update with `input` (already read): `statusline.sh`, the
/// capacity line, then the no-capacity notice. The notice reads only the
/// capacity file, under the same private root as the line. Returns the exit
/// code of `statusline.sh`, or 0 when it did not run.
pub(super) fn run(root: Option<&Path>, input: &[u8]) -> i32 {
    if let Some(root) = root {
        statusline::publish_observation(root, input);
    }
    let line = std::panic::catch_unwind(|| tick(root, input, fsx::now()))
        .ok()
        .flatten();
    // After the reading, as in a profile's session.
    let notice = root
        .filter(|r| fsx::check_root_private(r).is_ok())
        .and_then(|r| {
            std::panic::catch_unwind(|| capacity::statusline_line(r))
                .ok()
                .flatten()
        });
    statusline::stream_command(script_command().as_deref(), input, &[line, notice])
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

/// The home-mode capacity line. The login is the one the rotator ledgers
/// name, when `<root>/<name>` is a real directory; the reading is recorded
/// first when [`should_record`] allows it. Without a usable root (missing,
/// a symlink, or not private) only the bars are left: nothing is read or
/// written under it. `None` when no segment remains.
pub(super) fn tick(root: Option<&Path>, input: &[u8], now: i64) -> Option<String> {
    let rl = usage::parse_statusline_input(input, now);
    // Before anything that could create the state directory.
    let root = root.filter(|r| match fsx::check_root_private(r) {
        Ok(()) => true,
        Err(e) => {
            if fs::symlink_metadata(r).is_ok() {
                eprintln!(
                    "auto-switch: status line: profile root {}: {e}",
                    r.display()
                );
            }
            false
        }
    });
    let (login, spare) = match root.map(State::new) {
        Some(Ok(state)) => home_login_and_spare(&state, rl.as_ref(), now),
        Some(Err(e)) => {
            tracing::debug!(op = "home status line state", error = %e);
            (None, None)
        }
        None => (None, None),
    };
    let pct = |w: Option<usage::Window>| w.map(|w| w.used_percentage);
    capacity_line::render(
        login.as_ref().map(ProfileName::as_str),
        rl.as_ref().and_then(|r| pct(r.five_hour)),
        rl.as_ref().and_then(|r| pct(r.seven_day)),
        spare,
    )
}

/// The ledgers' login and the spare count, recording `rl` on the way when
/// it can be trusted.
fn home_login_and_spare(
    state: &State,
    rl: Option<&RateLimits>,
    now: i64,
) -> (Option<ProfileName>, Option<usize>) {
    let live = default_live::ledger_live(&ProfileStore::new(state.root.clone()))
        .filter(|l| real_dir(&state.profile_dir(l.name.as_str())));
    let name = live.as_ref().map(|l| l.name.as_str());
    let spare = match (&live, rl) {
        (Some(l), Some(rl)) => match should_record(state, l, rl, now) {
            Some(account) => record(state, &account, rl, now, name),
            None => unlocked_spare(state, name, now),
        },
        _ => unlocked_spare(state, name, now),
    };
    (live.map(|l| l.name), spare)
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

/// Records `rl` against `account` in `home-usage.json` under the state lock
/// and returns the spare count from the readings just saved. When the lock
/// is busy nothing is recorded and the count is read without it. `None`
/// when either readings file is damaged or the save fails, so no count is
/// shown from unsaved data.
///
/// Issue #1745 asked for these readings to feed the rotator through
/// `usage.json`. They do not, because home mode cannot confirm which account
/// `~/.claude` holds; see "Differences from issue #1745" in
/// `docs/reference/STATUSLINE.md`.
fn record(
    state: &State,
    account: &str,
    rl: &RateLimits,
    now: i64,
    live: Option<&str>,
) -> Option<usize> {
    let _lock = match state.try_lock(LOCK_WAIT) {
        Ok(Some(lock)) => lock,
        Ok(None) => return unlocked_spare(state, live, now),
        Err(e) => {
            tracing::debug!(op = "home status line lock", error = %e);
            return unlocked_spare(state, live, now);
        }
    };
    // A damaged file is left for the user to see, never replaced.
    let mut file = match state.try_load_home_usage() {
        Ok(f) => f,
        Err(e) => {
            tracing::debug!(op = "home status line usage", error = %e);
            return None;
        }
    };
    let merged = usage::merge(file.accounts.get(account), usage::reading_from(rl, now));
    file.accounts.insert(account.to_string(), merged);
    if let Err(e) = state.save_home_usage(&file) {
        tracing::debug!(op = "home status line save", error = %e);
        return None;
    }
    // No `capacity::refresh` here: with auto-switch on it resolves profile
    // kinds, which opens the login credentials file in `~/.claude` and in
    // every profile (`kind::Ctx::load`), and this path never reads credentials (SR1). The
    // capacity file is kept current by `profile usage`, the rotator and the
    // `--owner` wrapper once a reading reaches the threshold.
    let checked = state
        .try_load_usage()
        .inspect_err(|e| tracing::debug!(op = "status line usage", error = %e))
        .ok()?;
    let accounts = combined(checked.accounts, file.accounts, now);
    spare(state, &accounts, live, now)
}

/// The spare count from both readings files read without the lock; `None`
/// when either is damaged.
fn unlocked_spare(state: &State, live: Option<&str>, now: i64) -> Option<usize> {
    let checked = state.try_load_usage();
    let home = state.try_load_home_usage();
    let (checked, home) = match (checked, home) {
        (Ok(c), Ok(h)) => (c, h),
        (Err(e), _) | (_, Err(e)) => {
            tracing::debug!(op = "status line usage", error = %e);
            return None;
        }
    };
    let accounts = combined(checked.accounts, home.accounts, now);
    spare(state, &accounts, live, now)
}

/// `usage.json` readings with the `~/.claude` readings merged in, for the
/// spare count only: a person reads it, nothing acts on it. Per account the
/// newer reading wins, as in [`usage::merge`], except that a usage-limit
/// failure stands until its windows reset: a later percentage says nothing
/// about a limit that has been hit (#1846).
fn combined(
    mut checked: BTreeMap<String, Reading>,
    home: BTreeMap<String, Reading>,
    now: i64,
) -> BTreeMap<String, Reading> {
    for (account, reading) in home {
        if checked
            .get(&account)
            .is_some_and(|r| capacity_line::is_limited(r, now))
        {
            continue;
        }
        let merged = usage::merge(checked.get(&account), reading);
        checked.insert(account, merged);
    }
    checked
}

/// The spare count (D7): the profiles `profile list` shows, less `live`,
/// the auto-switch `exclude` list and the overflow profiles, whose reading
/// (under the account auto-switch has live in their directory, else their
/// own name) passes [`capacity_line::is_spare`]. `None` when the list, the
/// auto-switch settings or its ledger cannot be read.
fn spare(
    state: &State,
    accounts: &BTreeMap<String, Reading>,
    live: Option<&str>,
    now: i64,
) -> Option<usize> {
    let cfg = match config::read(&state.root) {
        config::FileState::Valid(c) => c,
        config::FileState::Missing => config::Config::default(),
        config::FileState::Corrupt => return None,
    };
    let ledger = state
        .load_ledger()
        .inspect_err(|e| tracing::debug!(op = "status line spare ledger", error = %e))
        .ok()?;
    let names = ProfileStore::new(state.root.clone())
        .list()
        .inspect_err(|e| tracing::debug!(op = "status line spare list", error = %e))
        .ok()?;
    let count = names
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .filter(|n| Some(n.as_str()) != live && !cfg.exclude.contains(n) && !cfg.is_overflow(n))
        .filter(|n| {
            accounts
                .get(ledger.account_in(n.as_str()))
                .is_some_and(|r| capacity_line::is_spare(r, now))
        })
        .count();
    Some(count)
}

/// The capacity line in profile `owner`'s own session: the account
/// auto-switch has live in its directory (else `owner`), the bars from
/// `input`, and the spare count from readings read without the lock,
/// `owner` excluded.
pub(super) fn owned_line(
    state: &State,
    owner: &ProfileName,
    input: &[u8],
    now: i64,
) -> Option<String> {
    let rl = usage::parse_statusline_input(input, now);
    let ledger = state
        .load_ledger()
        .inspect_err(|e| tracing::debug!(op = "profile status line ledger", error = %e))
        .ok();
    let login = ledger
        .as_ref()
        .map_or(owner.as_str(), |l| l.account_in(owner.as_str()));
    let pct = |w: Option<usage::Window>| w.map(|w| w.used_percentage);
    capacity_line::render(
        Some(login),
        rl.as_ref().and_then(|r| pct(r.five_hour)),
        rl.as_ref().and_then(|r| pct(r.seven_day)),
        unlocked_spare(state, Some(owner.as_str()), now),
    )
}
