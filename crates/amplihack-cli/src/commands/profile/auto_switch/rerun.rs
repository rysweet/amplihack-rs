//! Usage-limit reruns for nested `-p` launches (recipe agent steps). A
//! supervising amplihack runs the launch as a child (see
//! `profile::attempt`), and when the attempt fails with what the recipe
//! runner's own classifier calls a usage limit, reruns it under the next
//! eligible profile while auto-switch is on (#1569). When none is left, or
//! auto-switch is off or no profile was inherited, the usage-limit wait may
//! run the step once more in the inherited environment (#1618). Only the
//! final attempt's stdout is printed.

use super::candidate::{self, Pool};
use super::config::{self, Config};
use super::fsx;
use super::state::{Ledger, State};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::attempt::{Limited, Signals, run_attempt, usage_limited};
use crate::commands::profile::limit_wait::plan::{self, NoWait, WaitFor};
use crate::commands::profile::limit_wait::scratch::{self, RerunStdin};
use crate::commands::profile::limit_wait::{AfterLimit, Waiter};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::recipe::run::limit_patterns::Harness;
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
    // Overflow profiles are never auto-switch candidates (issue #1606).
    let exclude = cfg.selection_exclude();
    let pool = Pool {
        profiles,
        readings,
        threshold: cfg.threshold,
        exclude: &exclude,
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

/// When the windows of `r` that are used up reset: the latest of them, as a
/// wait would need (#1618). Only a window at 100% counts. A window over the
/// auto-switch threshold but under 100% is a reason to prefer another
/// profile, not proof that this one refuses work, so it neither times nor
/// cancels a wait. `None` when no window is used up.
pub(super) fn exhausted_reset(r: &Reading, now: i64) -> Option<i64> {
    [r.five_hour.as_ref(), r.seven_day.as_ref()]
        .into_iter()
        .flatten()
        .filter(|w| candidate::adjusted(Some(w), now) >= 100.0)
        .map(|w| w.resets_at)
        .max()
}

/// The note printed when a nested launch is moved to another profile.
pub fn substitution_note(from: &ProfileName, reason: &str, to: &ProfileName) -> String {
    format!(
        "amplihack: profile {:?} is {reason}; auto-switch is using profile {:?} for this launch",
        from.as_str(),
        to.as_str()
    )
}

/// A profile chosen before the first attempt because the inherited one is
/// exhausted.
pub struct Substitute {
    pub profile: ProfileName,
    /// For the note: why the inherited profile was passed over.
    pub reason: String,
    /// When the inherited account's used-up windows reset (ledger clock);
    /// `None` when it was passed over only for being over the threshold.
    pub inherited_reset: Option<i64>,
}

/// The profile a nested launch inheriting `inherited` should use, when the
/// account live there is exhausted and another profile has quota left.
pub fn substitute(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    inherited: &ProfileName,
) -> Option<Substitute> {
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
    let inherited_reset = exhausted_reset(reading, now);
    let profiles = profile_names(store);
    let skip = vec![inherited.to_string()];
    nested_candidate(state, &ledger, &profiles, &readings, cfg, &account, &skip).map(|c| {
        Substitute {
            profile: c,
            reason,
            inherited_reset,
        }
    })
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

/// Auto-switch for one supervised step: the store, its state and the
/// profile the step inherited.
pub(crate) struct Switch<'a> {
    store: &'a ProfileStore,
    state: State,
    inherited: ProfileName,
}

/// Runs a launch that inherited a profile under supervision. `None` when
/// neither auto-switch nor the wait applies; otherwise as [`run`].
pub fn supervise(
    store: &ProfileStore,
    inherited: ProfileName,
    waiter: Waiter,
) -> Result<Option<i32>> {
    let eff = config::effective(store.root());
    if !eff.on && !waiter.enabled() {
        return Ok(None);
    }
    // Auto-switch off creates nothing under the profile root (#1618).
    let switch = if eff.on {
        Some(Switch {
            store,
            state: State::new(store.root())?,
            inherited,
        })
    } else {
        None
    };
    run(switch, waiter)
}

/// Runs the launch under supervision and returns its final exit code. With
/// `switch` a limited attempt is rerun under the next eligible profile;
/// without it (auto-switch off, or no inherited profile) only the wait can
/// rerun it, in the inherited environment. `Ok(None)` when the supervisor
/// could not start, so the caller launches unsupervised; once an attempt
/// has run, errors are returned instead, so a step is never run twice by
/// accident.
pub(crate) fn run(switch: Option<Switch<'_>>, mut waiter: Waiter) -> Result<Option<i32>> {
    let on = switch.is_some();
    let setup = scratch_dir(switch.as_ref().map(|s| &s.state)).and_then(|dir| {
        Ok((
            dir,
            std::env::current_exe().context("cannot find the amplihack binary")?,
        ))
    });
    let (rerun_dir, exe) = match setup {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, on, &e),
    };
    super::sweep_rerun(&rerun_dir);
    // Never reads stdin, so nothing here can block before signals are caught.
    let stdin = RerunStdin::capture();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // The prompt is on the pipe: neither an auto-switch rerun nor the wait
    // could give it to another attempt.
    let piped_prompt = matches!(stdin, RerunStdin::Pipe) && !waiter.prompt_in_args();
    if piped_prompt {
        waiter.block(NoWait::PipedPrompt);
    }
    let signals = match Signals::register() {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, on, &format!("cannot catch stop signals: {e}")),
    };

    // The profile this attempt runs under; `None` keeps the inherited
    // environment.
    let mut profile: Option<ProfileName> = None;
    // The inherited profile's own limit. The retry after a wait always runs
    // under that profile, so only its reset times the wait; the limits of
    // the profiles auto-switch tried matter only to its choice of candidate,
    // which `next_profile` records (#1618).
    let mut inherited_wait: Option<WaitFor> = None;
    // Set when the step was moved off the inherited profile only for being
    // over the auto-switch threshold (no window at 100%). That profile still
    // has capacity, so when the candidate is limited the step goes back to
    // it as an auto-switch rerun, not a wait: the step's single wait stays
    // free for the inherited profile's own limit, which is the likelier one
    // on a nearly-empty window (#1618).
    let mut inherited_has_capacity = false;
    if let Some(sw) = &switch {
        let eff = config::effective(sw.store.root());
        if let Some(sub) = substitute(sw.store, &sw.state, &eff.config, &sw.inherited) {
            eprintln!(
                "{}",
                substitution_note(&sw.inherited, &sub.reason, &sub.profile)
            );
            // The inherited account never runs before the retry, so its
            // reset comes from the readings, moved onto the wait's clock.
            // Without one (no window at 100%) no wait is built at all: see
            // `inherited_has_capacity`. A candidate's limit is another
            // account's and never times this wait.
            match sub.inherited_reset {
                Some(r) => {
                    let wait_now = waiter.now();
                    let reset = wait_now.saturating_add(r.saturating_sub(fsx::now()));
                    inherited_wait = Some(plan::from_reading(Harness::Claude, reset, wait_now));
                }
                None => inherited_has_capacity = true,
            }
            profile = Some(sub.profile);
        }
    }
    let mut tried: Vec<String> = Vec::new();
    let mut n = 0u32;
    loop {
        n += 1;
        let switch_dir = switch
            .as_ref()
            .zip(profile.as_ref())
            .map(|(sw, p)| (p, sw.state.profile_dir(p.as_str())));
        let env = switch_dir.as_ref().map(|(p, d)| (*p, d.as_path()));
        let input = stdin.for_attempt(n);
        let attempt = match run_attempt(&exe, &args, &rerun_dir, env, input, n, &signals) {
            Ok(a) => a,
            // The first attempt never started, so a plain launch can follow.
            Err(e) if n == 1 && profile.is_none() => {
                return unsupervised(&waiter, on, &e);
            }
            Err(e) => return Err(e),
        };
        let Some(lim) = usage_limited(&attempt, waiter.now()) else {
            return attempt.finish(None);
        };
        if profile.is_none() {
            // This attempt ran under the inherited profile: its limit is the
            // freshest word on that profile.
            inherited_wait = Some(lim.wait);
        }
        if let Some(sw) = &switch {
            let current = profile.as_ref().unwrap_or(&sw.inherited).clone();
            tried.push(current.to_string());
            let eff = config::effective(&sw.state.root);
            // The profile a piped prompt kept this step from rerunning under.
            let mut refused: Option<ProfileName> = None;
            if eff.on
                && let Some(next) = next_profile(
                    sw.store,
                    &sw.state,
                    &eff.config,
                    &current,
                    &tried,
                    &lim,
                    &waiter,
                )
            {
                if piped_prompt {
                    // The limit is recorded; a rerun would get no prompt. The
                    // wait below refuses too, with `piped_prompt`.
                    eprintln!(
                        "auto-switch: no rerun under profile {:?}: this step's prompt came on a pipe, which a rerun cannot read again",
                        next.as_str()
                    );
                    refused = Some(next);
                } else {
                    if let Some(sig) = signals.received() {
                        eprintln!("auto-switch: no rerun: this step was told to stop");
                        return attempt.finish(Some(128 + sig));
                    }
                    eprintln!(
                        "amplihack: profile {:?} hit its usage limit; rerunning this step under profile {:?} (auto-switch)",
                        current.as_str(),
                        next.as_str()
                    );
                    attempt.discard();
                    profile = Some(next).filter(|p| *p != sw.inherited);
                    continue;
                }
            }
            // No other candidate: back to the inherited profile, which was
            // passed over only for the threshold and still has capacity.
            // This is an auto-switch rerun; it does not call `after_limit`,
            // so the wait is still there if that profile is limited next.
            if inherited_has_capacity && profile.is_some() && piped_prompt {
                // As for a candidate above: a rerun would get no prompt. The
                // `skipped` event is written below.
                inherited_has_capacity = false;
                if refused.as_ref() != Some(&sw.inherited) {
                    eprintln!(
                        "auto-switch: no rerun under profile {:?}: this step's prompt came on a pipe, which a rerun cannot read again",
                        sw.inherited.as_str()
                    );
                }
            }
            if inherited_has_capacity && profile.is_some() {
                inherited_has_capacity = false;
                if let Some(sig) = signals.received() {
                    eprintln!("auto-switch: no rerun: this step was told to stop");
                    return attempt.finish(Some(128 + sig));
                }
                eprintln!(
                    "amplihack: profile {:?} hit its usage limit; rerunning this step under profile {:?}, which is over the auto-switch threshold but not limited (auto-switch)",
                    current.as_str(),
                    sw.inherited.as_str()
                );
                attempt.discard();
                profile = None;
                continue;
            }
        }
        // Set by now unless the inherited profile never ran and has no
        // known reset: a candidate's limit is another account's and never
        // times the wait. That happens when the move back to the inherited
        // profile was refused (a piped prompt); the wait is refused for the
        // same reason, so `after_limit` still writes the `skipped` event
        // (`piped_prompt`, or `disabled`/`ci`) and never starts a wait.
        // With nothing blocking the wait there is no reset to wait for, and
        // the result is passed up.
        let Some(wait) = inherited_wait.or_else(|| waiter.blocked_skip()) else {
            return attempt.finish(None);
        };
        match waiter.after_limit(wait, n, &signals) {
            AfterLimit::Finish(code) => return attempt.finish(code),
            AfterLimit::Retry => {
                attempt.discard();
                profile = None;
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
