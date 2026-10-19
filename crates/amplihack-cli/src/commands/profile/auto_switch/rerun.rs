//! Usage-limit reruns for nested `-p` launches that inherited a profile
//! (recipe agent steps). A supervising amplihack runs the launch as a child
//! (see `profile::attempt`), and when the attempt fails with what the recipe
//! runner's own classifier calls a usage limit, reruns it under the next
//! eligible profile while auto-switch is on (#1569). When none is left, or
//! auto-switch is off, the usage-limit wait may run the step once more under
//! the inherited profile (#1618). Only the final attempt's stdout is printed.

use super::candidate::{self, Pool};
use super::config::{self, Config};
use super::fsx;
use super::state::{Ledger, State};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::attempt::{Limited, Signals, run_attempt, usage_limited};
use crate::commands::profile::limit_wait::scratch::{self, CopyError, StdinCopy};
use crate::commands::profile::limit_wait::{AfterLimit, Waiter};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(test)]
pub(super) use crate::commands::profile::attempt::limit_reset;

/// An eligible profile for a nested launch: not excluded, not exhausted, not
/// in `skip`, holding its own login at home, and not hosting another account.
pub fn nested_candidate(
    state: &State,
    ledger: &Ledger,
    profiles: &[ProfileName],
    readings: &BTreeMap<String, Reading>,
    cfg: &Config,
    current: &str,
    skip: &[String],
) -> Option<ProfileName> {
    let pool = Pool {
        profiles,
        readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now: fsx::now(),
    };
    candidate::select(&pool, current, |c| {
        let n = c.as_str();
        !skip.iter().any(|s| s == n)
            && !ledger.hosts.contains_key(n)
            && ledger.leased_to(n).is_none()
            // macOS keeps logins in the Keychain: no file to look for.
            && (cfg!(target_os = "macos") || fsx::exists(&state.live_creds(n)))
    })
    .cloned()
}

pub fn profile_names(store: &ProfileStore) -> Vec<ProfileName> {
    let names = store.list().unwrap_or_else(|e| {
        eprintln!("auto-switch: cannot list profiles, so none can be substituted: {e}");
        Vec::new()
    });
    names
        .iter()
        .filter_map(|n| ProfileName::parse(n).ok())
        .collect()
}

/// `at 93% of its five-hour limit`: the window that made `r` exhausted.
fn limit_reason(r: &Reading, now: i64) -> String {
    let five = candidate::adjusted(r.five_hour.as_ref(), now);
    let seven = candidate::adjusted(r.seven_day.as_ref(), now);
    let (pct, window) = if five >= seven {
        (five, "five-hour")
    } else {
        (seven, "seven-day")
    };
    format!("at {pct:.0}% of its {window} limit")
}

/// The note printed when a nested launch is moved to another profile.
pub fn substitution_note(from: &ProfileName, reason: &str, to: &ProfileName) -> String {
    format!(
        "amplihack: profile {:?} is {reason}; auto-switch is using profile {:?} for this launch",
        from.as_str(),
        to.as_str()
    )
}

/// The profile a nested launch inheriting `inherited` should use, when the
/// account live there is exhausted and another profile has quota left, with
/// the reason for the note.
pub fn substitute(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    inherited: &ProfileName,
) -> Option<(ProfileName, String)> {
    if !state.exists() {
        return None; // no readings yet
    }
    let ledger = match state.load_ledger() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("auto-switch: no substitute for this launch: {e}");
            return None;
        }
    };
    let account = ledger.account_in(inherited.as_str()).to_string();
    let readings = state.load_usage_or_warn().accounts;
    let now = fsx::now();
    let reading = readings.get(&account)?;
    if !candidate::is_exhausted(Some(reading), cfg.threshold, now) {
        return None;
    }
    let reason = limit_reason(reading, now);
    let profiles = profile_names(store);
    let skip = vec![inherited.to_string()];
    nested_candidate(state, &ledger, &profiles, &readings, cfg, &account, &skip)
        .map(|c| (c, reason))
}

fn record_limit(state: &State, account: &str, reset: i64) {
    let _lock = match state.lock() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("auto-switch: usage limit for this account not recorded: {e}");
            return;
        }
    };
    let mut file = state.load_usage_or_warn();
    let now = fsx::now();
    let seven_day = file.accounts.get(account).and_then(|r| r.seven_day);
    file.accounts.insert(
        account.to_string(),
        Reading {
            five_hour: Some(Window {
                used_percentage: 100.0,
                resets_at: reset,
            }),
            seven_day,
            observed_at: now,
            source: ReadingSource::UsageLimitFailure,
        },
    );
    if let Err(e) = state.save_usage(&file) {
        eprintln!("auto-switch: {e}");
    }
}

/// The account the auto-switch ledger says is live in profile `name`, for
/// the wait's login fingerprint (#1618). `None` without a trusted ledger.
pub(crate) fn ledger_account(root: &std::path::Path, name: &str) -> Option<String> {
    let state = State::new(root).ok()?;
    fsx::check_private_dir(&state.dir).ok()?;
    let ledger = state.load_ledger().ok()?;
    Some(ledger.account_in(name).to_string())
}

/// Runs the launch under supervision and returns its final exit code.
/// `Ok(None)` when the supervisor could not start, so the caller launches
/// unsupervised; once an attempt has run, errors are returned instead, so a
/// step is never run twice by accident. `None` too when neither auto-switch
/// nor the wait applies.
pub fn supervise(
    store: &ProfileStore,
    inherited: ProfileName,
    mut waiter: Waiter,
) -> Result<Option<i32>> {
    let eff = config::effective(store.root());
    if !eff.on && !waiter.enabled() {
        return Ok(None);
    }
    // Auto-switch off creates nothing under the profile root (#1618).
    let state = if eff.on {
        Some(State::new(store.root())?)
    } else {
        None
    };
    let setup = scratch_dir(state.as_ref()).and_then(|dir| {
        Ok((
            dir,
            std::env::current_exe().context("cannot find the amplihack binary")?,
        ))
    });
    let (rerun_dir, exe) = match setup {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, eff.on, &e),
    };
    super::sweep_rerun(&rerun_dir);
    let signals = match Signals::register() {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, eff.on, &format!("cannot catch stop signals: {e}")),
    };
    let stdin = match StdinCopy::take(&rerun_dir) {
        Ok(c) => c,
        Err(CopyError::BeforeRead(e)) => return unsupervised(&waiter, eff.on, &e),
        Err(e @ CopyError::AfterRead(_)) => {
            eprintln!(
                "error: could not copy this step's input for its supervisor ({e}); not running it"
            );
            return Ok(Some(1));
        }
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    let mut profile = inherited.clone();
    if let Some(state) = &state
        && let Some((c, reason)) = substitute(store, state, &eff.config, &inherited)
    {
        eprintln!("{}", substitution_note(&inherited, &reason, &c));
        profile = c;
    }
    let mut tried: Vec<String> = Vec::new();
    let mut n = 0u32;
    loop {
        n += 1;
        let switch_dir = state
            .as_ref()
            .filter(|_| profile != inherited)
            .map(|s| s.profile_dir(profile.as_str()));
        let env = switch_dir.as_deref().map(|d| (&profile, d));
        let attempt = match run_attempt(&exe, &args, &rerun_dir, env, &stdin, n, &signals) {
            Ok(a) => a,
            // Only before any input was consumed can a plain launch follow.
            Err(e) if n == 1 && profile == inherited && stdin.path().is_none() => {
                return unsupervised(&waiter, eff.on, &e);
            }
            Err(e) => return Err(e),
        };
        tried.push(profile.to_string());
        let Some(lim) = usage_limited(&attempt, waiter.now()) else {
            return attempt.finish(None);
        };
        if let Some(state) = &state {
            let eff = config::effective(&state.root);
            if eff.on
                && let Some(next) =
                    next_profile(store, state, &eff.config, &profile, &tried, &lim, &waiter)
            {
                if let Some(sig) = signals.received() {
                    eprintln!("auto-switch: no rerun: this step was told to stop");
                    return attempt.finish(Some(128 + sig));
                }
                eprintln!(
                    "amplihack: profile {:?} hit its usage limit; rerunning this step under profile {:?} (auto-switch)",
                    profile.as_str(),
                    next.as_str()
                );
                attempt.discard();
                profile = next;
                continue;
            }
        }
        match waiter.after_limit(&lim, n, &signals) {
            AfterLimit::Finish(code) => return attempt.finish(code),
            AfterLimit::Retry => {
                attempt.discard();
                profile = inherited.clone();
            }
        }
    }
}

/// `<root>/.auto-switch.d/rerun` with auto-switch on, else
/// `$HOME/.amplihack/rerun`.
fn scratch_dir(state: Option<&State>) -> Result<PathBuf> {
    match state {
        Some(state) => {
            let dir = state.dir.join("rerun");
            state.ensure().and_then(|()| state.ensure_sub(&dir))?;
            Ok(dir)
        }
        None => {
            let home = std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .context("HOME is not set")?;
            scratch::home_rerun_dir(std::path::Path::new(&home))
                .context("no private scratch directory")
        }
    }
}

fn unsupervised(waiter: &Waiter, on: bool, why: &dyn std::fmt::Display) -> Result<Option<i32>> {
    let who = if on {
        "auto-switch"
    } else {
        "the usage-limit wait"
    };
    eprintln!("warning: {who} cannot supervise this step ({why}); running it once");
    waiter.not_supervised();
    Ok(None)
}

/// Records the limit for the account live in `profile`, then the next
/// eligible profile, unless the wait was already used for this step.
fn next_profile(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    profile: &ProfileName,
    tried: &[String],
    lim: &Limited,
    waiter: &Waiter,
) -> Option<ProfileName> {
    // Without the ledger the account is unknown: recording the limit under
    // the profile name could mark the wrong account exhausted.
    let account = match state.load_ledger() {
        Ok(l) => l.account_in(profile.as_str()).to_string(),
        Err(e) => {
            eprintln!("auto-switch: no rerun: {e}");
            return None;
        }
    };
    record_limit(state, &account, lim.ledger_reset);
    if waiter.used() {
        return None;
    }
    match state.load_ledger() {
        Ok(ledger) => {
            let readings = state.load_usage_or_warn().accounts;
            nested_candidate(
                state,
                &ledger,
                &profile_names(store),
                &readings,
                cfg,
                &account,
                tried,
            )
        }
        Err(e) => {
            eprintln!("auto-switch: no rerun: {e}");
            None
        }
    }
}
