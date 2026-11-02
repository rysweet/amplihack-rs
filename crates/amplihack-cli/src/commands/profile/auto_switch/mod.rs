//! Automatic switching between Claude login profiles (issue #1569).
//!
//! Off by default. When on, a status-line wrapper records each account's
//! usage, a session whose account reaches the threshold has its login swapped
//! for one with quota left (a move, never a copy), nested launches start
//! under a profile with quota left, and a `-p` step that hits its usage limit
//! is rerun under the next profile. See docs/reference/profile-command.md.
//!
//! This file is the interface the rest of amplihack uses. Everything that
//! touches the filesystem is Unix-only; other platforms get no-ops.

#[cfg(unix)]
mod candidate;
#[cfg(unix)]
pub(super) mod capacity;
#[cfg(unix)]
mod cli;
#[cfg(unix)]
pub(super) mod config;
#[cfg(unix)]
pub(super) mod fsx;
#[cfg(unix)]
pub(super) mod kit;
#[cfg(unix)]
mod liveness;
#[cfg(unix)]
mod mv;
#[cfg(unix)]
mod overflow_inputs;
#[cfg(unix)]
mod per_token;
#[cfg(unix)]
pub(super) mod rerun;
#[cfg(unix)]
mod state;
#[cfg(unix)]
mod statusline;
#[cfg(unix)]
mod swap;
#[cfg(unix)]
pub(super) mod takeover;
#[cfg(unix)]
mod usage;

#[cfg(all(test, unix))]
mod tests_candidate;
#[cfg(all(test, unix))]
mod tests_capacity;
#[cfg(all(test, unix))]
mod tests_config;
#[cfg(test)]
mod tests_contract;
#[cfg(all(test, unix))]
mod tests_exhausted_reset;
#[cfg(all(test, unix))]
mod tests_issue_1804;
#[cfg(all(test, unix))]
mod tests_liveness;
#[cfg(all(test, unix))]
mod tests_mv;
#[cfg(all(test, unix))]
mod tests_platform_candidate;
#[cfg(all(test, unix))]
mod tests_record;
#[cfg(all(test, unix))]
mod tests_takeover;
#[cfg(all(test, unix))]
mod tests_usage;

use super::ProfileError;
use super::name::ProfileName;
use super::store::ProfileStore;
use std::path::Path;

/// Set on a launch running under the rerun supervisor, so it does not
/// supervise itself. Removed before Claude starts.
pub const SUPERVISED_ENV: &str = "AMPLIHACK_AUTO_SWITCH_SUPERVISED";

/// For the rotate tests that check the rotator and auto-switch agree on
/// when overflow is used (issue #1606).
#[cfg(all(test, unix))]
pub(super) use candidate::{OverflowPool, Pool, select_with_overflow, way_off};
#[cfg(unix)]
pub use cli::AutoSwitchCommand;
/// File helpers `default_live` reuses for its read-only checks.
#[cfg(unix)]
pub(super) use fsx::{check_root_private, euid, exists, read_bounded_nofollow, writable_by_others};
/// For the shared-session liveness tests (issue #1634).
#[cfg(all(test, unix))]
pub(super) use liveness::scan;
#[cfg(all(test, unix))]
pub(super) use overflow_inputs::OverflowInputs;
#[cfg(unix)]
pub(super) use overflow_inputs::can_take_login;
#[cfg(unix)]
pub(super) use state::CREDS;
#[cfg(unix)]
pub(super) use swap::read_claude_json;
#[cfg(all(test, unix))]
pub(crate) use swap::rerun_owner;
#[cfg(unix)]
pub(super) use usage::Window;
#[cfg(all(test, unix))]
pub(super) use usage::{Reading as SwitchReading, ReadingSource};
#[cfg(unix)]
pub(crate) use {swap::sweep_rerun, usage::reset_or_default};

#[cfg(not(unix))]
#[derive(Debug, clap::Subcommand)]
pub enum AutoSwitchCommand {
    /// Not available on this platform
    Status,
}

/// What a lease check is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// A launch; `join` when it inherited the profile from the session whose
    /// `CLAUDE_CONFIG_DIR` is this profile's directory.
    Launch {
        join: bool,
    },
    Login,
    Remove,
}

pub fn handle(store: &ProfileStore, cmd: AutoSwitchCommand) -> anyhow::Result<()> {
    #[cfg(unix)]
    return cli::handle(store, cmd);
    #[cfg(not(unix))]
    {
        let _ = (store, cmd);
        Err(ProfileError::AutoSwitchUnsupported.into())
    }
}

/// The hidden `profile statusline` command; returns the exit code.
pub fn statusline(root: &str, owner: &str) -> i32 {
    #[cfg(unix)]
    return statusline::run(root, owner);
    #[cfg(not(unix))]
    {
        let _ = (root, owner);
        0
    }
}

/// Refuses an action on `name` while its login is away from home, after
/// finishing any interrupted swap and returning ended sessions. Without a
/// state directory this is one metadata check.
pub fn guard(store: &ProfileStore, name: &ProfileName, action: Action) -> Result<(), ProfileError> {
    #[cfg(unix)]
    {
        if !store.root().join(".auto-switch.d").exists() {
            return Ok(());
        }
        let state = state::State::new(store.root())?;
        swap::guard(&state, name, action)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, name, action);
        Ok(())
    }
}

/// Whether auto-switch holds `name`'s login for it (issue #1700): leased
/// to another session's directory, or recorded as hosted in its own. A
/// state directory or ledger that cannot be read holds nothing.
#[cfg(unix)]
pub fn login_held_elsewhere(store: &ProfileStore, name: &ProfileName) -> bool {
    if !store.root().join(".auto-switch.d").exists() {
        return false;
    }
    let Ok(state) = state::State::new(store.root()) else {
        return false;
    };
    state
        .load_ledger()
        .is_ok_and(|l| l.leased_to(name.as_str()).is_some() || l.hosts.contains_key(name.as_str()))
}

/// Keeps swaps out of a profile between the pre-spawn lease check and the
/// moment Claude's process exists, so liveness scans see the session. Drop it
/// once the spawn returns.
#[must_use = "dropping the hold re-opens the window before the spawn"]
pub struct LaunchHold {
    #[cfg(unix)]
    _lock: Option<state::Lock>,
}

/// [`guard`] for a launch, keeping the state lock held until the returned
/// value is dropped.
pub fn guard_launch(
    store: &ProfileStore,
    name: &ProfileName,
    join: bool,
) -> Result<LaunchHold, ProfileError> {
    #[cfg(unix)]
    {
        if !store.root().join(".auto-switch.d").exists() {
            return Ok(LaunchHold { _lock: None });
        }
        let state = state::State::new(store.root())?;
        let lock = swap::guard_held(&state, name, Action::Launch { join })?;
        Ok(LaunchHold { _lock: lock })
    }
    #[cfg(not(unix))]
    {
        let _ = (store, name, join);
        Ok(LaunchHold {})
    }
}

/// Whether this process's `CLAUDE_CONFIG_DIR` is `dir` (whole canonical
/// paths), i.e. it runs inside the session using that directory.
pub fn inside_session(dir: &Path) -> bool {
    let Some(own) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) else {
        return false;
    };
    match (
        std::path::PathBuf::from(own).canonicalize(),
        dir.canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Whether `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` turns the switch on here: the only
/// way to know it is on when the profile root itself cannot be opened.
pub fn env_forces_on() -> bool {
    #[cfg(unix)]
    return config::env_toggle(std::env::var_os(config::TOGGLE_ENV).as_deref())
        == config::EnvToggle::On;
    #[cfg(not(unix))]
    false
}

fn supervised() -> bool {
    std::env::var_os(SUPERVISED_ENV).is_some_and(|v| !v.is_empty())
}

/// For a launch that inherited `inherited`: another profile to use when the
/// switch is on and the account live there is exhausted. Prints one note.
pub fn substitute_nested(store: &ProfileStore, inherited: &ProfileName) -> Option<ProfileName> {
    #[cfg(unix)]
    {
        if supervised() {
            return None; // the supervisor already chose
        }
        let eff = config::effective(store.root());
        if !eff.on {
            return None;
        }
        let state = match state::State::new(store.root()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "warning: auto-switch could not check profile {:?}: {e}",
                    inherited.as_str()
                );
                return None;
            }
        };
        let sub = rerun::substitute(store, &state, &eff.config, inherited)?;
        eprintln!(
            "{}",
            rerun::substitution_note(inherited, &sub.reason, &sub.profile)
        );
        per_token::record(&state.root, sub.per_token.as_deref());
        Some(sub.profile)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, inherited);
        None
    }
}

/// `substitute_nested` by name, for `multitask` workstream scripts.
pub fn substitute_name(inherited: &str) -> Option<String> {
    let store = match ProfileStore::from_env() {
        Ok(store) => store,
        Err(e) => {
            if env_forces_on() {
                eprintln!("warning: auto-switch could not open the profile root: {e}");
            }
            return None;
        }
    };
    let name = ProfileName::parse(inherited).ok()?;
    substitute_nested(&store, &name).map(|n| n.to_string())
}

/// Returns logins once a launched Claude has exited. Problems are warnings:
/// the next launch, login or `status` retries.
pub fn return_after_session(dir: &Path) {
    #[cfg(unix)]
    {
        let Some(root) = dir.parent() else {
            return;
        };
        if !root.join(".auto-switch.d").exists() {
            return;
        }
        let result = state::State::new(root).and_then(|s| {
            let _lock = s.lock()?;
            swap::recover(&s).map(|_| ())
        });
        if let Err(e) = result {
            eprintln!("warning: auto-switch could not return logins: {e}");
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// `amplihack claude` / launch: the one-line no-capacity warning on stderr
/// when every account is over the threshold. Never changes the launch.
pub fn launch_warning(args: &[String]) {
    #[cfg(unix)]
    capacity::launch_warning(args, &mut std::io::stderr());
    #[cfg(not(unix))]
    let _ = args;
}

/// `amplihack recipe run`: the one-line no-capacity warning, with the reset
/// time, to `out`. Never refuses the run.
pub fn recipe_warning(out: &mut dyn std::io::Write) {
    #[cfg(unix)]
    capacity::recipe_warning(out);
    #[cfg(not(unix))]
    let _ = out;
}

/// Who holds which login, from the ledger: directory → account live there.
/// Read for marks and attribution in `profile usage`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaseView {
    pub hosts: std::collections::BTreeMap<String, String>,
}

impl LeaseView {
    /// The account whose login is in directory `dir`.
    pub fn account_in<'a>(&'a self, dir: &'a str) -> &'a str {
        self.hosts.get(dir).map_or(dir, String::as_str)
    }

    /// The directory account `name` is live in, other than its own.
    pub fn leased_to(&self, name: &str) -> Option<&str> {
        self.hosts
            .iter()
            .find(|(dir, live)| live.as_str() == name && dir.as_str() != name)
            .map(|(dir, _)| dir.as_str())
    }
}

/// One window as `profile usage` read it.
// Only the Unix recorder reads the fields.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservedWindow {
    pub used_percentage: f64,
    pub resets_at: Option<i64>,
}

/// What `profile usage` saw in one profile directory, and the account it
/// believed was live there when it probed.
#[cfg_attr(not(unix), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub profile: String,
    pub account: String,
    pub session: Option<ObservedWindow>,
    pub week: Option<ObservedWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Auto-switch is off and has no state directory: nothing was created.
    NotSetUp,
    /// Another process held the state lock.
    Busy,
    /// A swap journal exists; recovery is left to launches and the status line.
    SwapInProgress,
    /// `written`: accounts whose reading was updated. `moved`: profiles whose
    /// live account changed while they were probed, so were not recorded.
    Recorded {
        written: Vec<String>,
        moved: Vec<String>,
    },
}

/// Whether readings may be recorded under `root`: the effective setting is
/// on, or the state directory already exists (the status line's gate).
pub fn recording_enabled(root: &Path) -> bool {
    #[cfg(unix)]
    return config::effective(root).on || fsx::exists(&root.join(state::DIR));
    #[cfg(not(unix))]
    {
        let _ = root;
        false
    }
}

/// The ledger's leases, read without the lock. `None` without a state
/// directory. The root and state directory must pass the same trust checks
/// as every other state read.
pub fn lease_view(root: &Path) -> Result<Option<LeaseView>, ProfileError> {
    #[cfg(unix)]
    {
        if !fsx::exists(&root.join(state::DIR)) {
            return Ok(None);
        }
        let state = state::State::new(root)?;
        fsx::check_root_private(&state.root)
            .map_err(|e| ProfileError::AutoSwitchInvariant(format!("profile root {e}")))?;
        fsx::check_private_dir(&state.dir)
            .map_err(|e| ProfileError::AutoSwitchInvariant(format!("{}: {e}", state::DIR)))?;
        let ledger = state.load_ledger()?;
        Ok(Some(LeaseView {
            hosts: ledger
                .hosts
                .into_iter()
                .map(|(dir, h)| (dir, h.live))
                .collect(),
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        Ok(None)
    }
}

/// Issue #1844: the stored reading for the account whose login is in
/// directory `dir`, when it was observed no more than `max_age` seconds
/// before `now` (and not after it), with each window that passes
/// [`usage::plausible`]. `None` without auto-switch state, a reading, or a
/// plausible window, and when anything cannot be read. Read-only and
/// silent: it never creates state, takes the lock or prints.
#[cfg(unix)]
pub fn fresh_reading(root: &Path, dir: &str, max_age: i64, now: i64) -> Option<FreshReading> {
    if max_age <= 0 {
        return None;
    }
    let state = state::State::new(root).ok()?;
    if !state.exists() {
        return None;
    }
    fsx::check_root_private(&state.root).ok()?;
    fsx::check_private_dir(&state.dir).ok()?;
    let ledger = state.load_ledger().ok()?;
    let account = ledger.account_in(dir);
    let file = state.try_load_usage().ok()?;
    let r = file.accounts.get(account)?;
    let age = now.checked_sub(r.observed_at)?;
    if !(0..=max_age).contains(&age) {
        return None;
    }
    let ok =
        |w: &Option<Window>| w.and_then(|w| usage::plausible(w.used_percentage, w.resets_at, now));
    let (session, week) = (ok(&r.five_hour), ok(&r.seven_day));
    (session.is_some() || week.is_some()).then_some(FreshReading {
        session,
        week,
        // The same test as [`no_session_window_profiles`]; a stored session
        // window dropped as implausible is unknown, not absent.
        no_session_window: r.no_session_window() && week.is_some(),
    })
}

/// The plausible windows of a fresh stored reading (issue #1844).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreshReading {
    pub session: Option<Window>,
    pub week: Option<Window>,
    /// Issue #1818: the reading has a week window and no session window, so
    /// it has no session limit.
    pub no_session_window: bool,
}

/// Issue #1818: the profiles among `names` whose stored reading has a week
/// window and no session window, sorted. Read-only: it never creates state,
/// takes the lock or writes, and any failure (no state, an unsafe or damaged
/// `usage.json`) gives an empty list without printing anything.
pub fn no_session_window_profiles(root: &Path, names: &[&str]) -> Vec<String> {
    #[cfg(unix)]
    {
        let Ok(state) = state::State::new(root) else {
            return Vec::new();
        };
        if !state.exists() {
            return Vec::new();
        }
        let Ok(file) = state.try_load_usage() else {
            return Vec::new();
        };
        let mut out: Vec<String> = file
            .accounts
            .iter()
            .filter(|(name, r)| r.no_session_window() && names.contains(&name.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        out.sort();
        out
    }
    #[cfg(not(unix))]
    {
        let _ = (root, names);
        Vec::new()
    }
}

/// The `rotate status` line `no session window: <names>` for the profiles
/// among `names` that [`no_session_window_profiles`] lists, or `None` when
/// there are none. Read-only. It does not say "no session limit": the stored
/// reading cannot tell a plan with none from a session window that was not
/// recorded.
pub fn no_session_window_line(root: &Path, names: &[String]) -> Option<String> {
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let free = no_session_window_profiles(root, &names);
    if free.is_empty() {
        return None;
    }
    let free: Vec<String> = free
        .iter()
        .map(|n| super::usage_cmd::sanitize_display(n, false))
        .collect();
    Some(format!("no session window: {}", free.join(", ")))
}

/// Records `profile usage` observations as status-line readings. Takes the
/// lock for a bounded wait, never runs swap recovery, re-checks each
/// profile's live account under the lock, applies the status-line bounds,
/// keeps a stored window that has not reset when only the other one was
/// read, and writes `usage.json` at most once.
pub fn record_usage(root: &Path, observed: &[Observation]) -> Result<RecordOutcome, ProfileError> {
    #[cfg(unix)]
    {
        use usage::{Reading, ReadingSource, Window};
        if !recording_enabled(root) {
            return Ok(RecordOutcome::NotSetUp);
        }
        let state = state::State::new(root)?;
        let Some(_lock) = state.try_lock(statusline::LOCK_WAIT)? else {
            return Ok(RecordOutcome::Busy);
        };
        if state.load_journal()?.is_some() {
            return Ok(RecordOutcome::SwapInProgress);
        }
        let ledger = state.load_ledger()?;
        let now = fsx::now();
        let bounded = |w: Option<ObservedWindow>| {
            w.and_then(|w| usage::plausible(w.used_percentage, w.resets_at?, now))
        };
        let still_valid = |w: Option<Window>| w.filter(|w| w.resets_at > now);
        let mut file = None;
        let (mut written, mut moved) = (Vec::new(), Vec::new());
        for o in observed {
            if ledger.account_in(&o.profile) != o.account {
                moved.push(o.profile.clone());
                continue;
            }
            let (five_hour, seven_day) = (bounded(o.session), bounded(o.week));
            if five_hour.is_none() && seven_day.is_none() {
                continue;
            }
            let file = file.get_or_insert_with(|| state.load_usage_or_warn());
            let old = file.accounts.get(&o.account);
            let reading = Reading {
                // Issue #1818: a reading with no session limit keeps a stored
                // session window until its reset passes. A 0%, reset-less
                // reading may be stale, and a live window that contradicts
                // it is the cautious answer; the cost is at most one
                // five-hour window in which the login is passed over.
                five_hour: five_hour.or_else(|| still_valid(old.and_then(|r| r.five_hour))),
                seven_day: seven_day.or_else(|| still_valid(old.and_then(|r| r.seven_day))),
                observed_at: now,
                // Deliberate: the usage file's format is fixed, and a
                // `/usage` reading means the same as a status-line one.
                source: ReadingSource::Statusline,
            };
            let merged = usage::merge(old, reading);
            file.accounts.insert(o.account.clone(), merged);
            written.push(o.account.clone());
        }
        if let Some(file) = file {
            state.save_usage(&file)?;
            capacity::refresh(&state, &file, &ledger, "usage");
        }
        Ok(RecordOutcome::Recorded { written, moved })
    }
    #[cfg(not(unix))]
    {
        let _ = (root, observed);
        Ok(RecordOutcome::NotSetUp)
    }
}
