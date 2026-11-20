//! Usage-limit reruns for nested `-p` launches (recipe agent steps). A
//! supervising amplihack runs the launch as a child (see
//! `profile::attempt`), and when the attempt fails with what the recipe
//! runner's own classifier calls a usage limit, reruns it under the next
//! eligible profile while auto-switch is on (#1569). When none is left, or
//! auto-switch is off or no profile was inherited, the usage-limit wait may
//! run the step once more in the inherited environment (#1618), or on the
//! login that showed capacity: after a capacity wait (#1801), and after a
//! reset wait that ended early because a reading taken during it showed
//! another login had room (#2694). A step pinned to a profile can therefore
//! finish on a different account from the one it started on. Only the final
//! attempt's stdout is printed.

use super::candidate::{self, Pool};
use super::config::{self, Config};
use super::fsx;
use super::overflow_inputs::OverflowInputs;
use super::per_token;
use super::state::{Ledger, State};
use super::takeover::{capacity_candidate, profile_check};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::attempt::{
    Attempt, Limited, Signals, note_key_error, run_attempt_with_recovery,
};
use crate::commands::profile::limit_wait::plan::{self, NoWait, WaitFor};
use crate::commands::profile::limit_wait::scratch::{self, RerunStdin};
use crate::commands::profile::limit_wait::{AfterLimit, Met, Resumed, Waiter};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::recipe::run::limit_patterns::Harness;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(test)]
pub(super) use crate::commands::profile::attempt::limit_reset;

/// Why no rerun can be given this step's prompt when no copy of it was ever
/// made: the pre-#2815 conduct, reported in the same shape as the reasons
/// [`RerunStdin::rerun_refused`] gives (issue #2816).
const NO_STORED_PROMPT: &str = "this step's prompt came on a pipe and could not be stored, so a \
                                rerun cannot read it again";

/// A profile for a nested launch.
#[derive(Debug, PartialEq)]
pub struct Pick {
    pub profile: ProfileName,
    /// The reason line for a move onto a per-token profile, for
    /// [`per_token::record`]; `None` for a plan profile.
    pub per_token: Option<String>,
}

/// An eligible profile for a nested launch: not excluded, not exhausted (an
/// overflow profile: not spend-blocked; its own plan reading does not
/// count), not in `skip`, holding its own login at home, and not hosting
/// another account. Plan profiles come first; an overflow profile only when
/// no plan profile has room (both windows below 100%) and the earliest plan
/// reset is beyond the overflow wait (issues #1606, #1804), and a launch on an
/// overflow profile gets a plan profile as soon as one has capacity (see
/// `candidate::select_with_overflow`). An API-key profile is never a plan
/// profile.
///
/// The caller writes [`Pick::per_token`] when it makes the move, not here: a
/// piped prompt the supervisor could not store can still refuse the rerun.
pub fn nested_candidate(
    state: &State,
    ledger: &Ledger,
    profiles: &[ProfileName],
    readings: &BTreeMap<String, Reading>,
    cfg: &Config,
    current: &str,
    skip: &[String],
) -> Option<Pick> {
    candidate(
        Reads {
            state,
            ledger,
            profiles,
            readings,
            cfg,
        },
        current,
        skip,
        KeyAlert::Write,
    )
}

/// [`nested_candidate`] without the held-key alert: the same choice, made
/// without writing or printing anything (issue #2694). This is for the
/// usage-limit wait's 30-second probe only, which asks the same question for
/// the whole of a wait, so the alert would repeat for hours in the step's
/// stderr and crowd the runner's evidence tail. The step that is waiting has
/// already hit a limit under auto-switch, so the alert for its held-back key
/// was written once by the rerun check before the wait began.
///
/// The retry's own single selection after the wait keeps [`nested_candidate`]
/// and so keeps the alert: it runs once, and it is the call that decides the
/// login the step actually moves to.
pub(super) fn silent_candidate(
    state: &State,
    ledger: &Ledger,
    profiles: &[ProfileName],
    readings: &BTreeMap<String, Reading>,
    cfg: &Config,
    current: &str,
    skip: &[String],
) -> Option<Pick> {
    candidate(
        Reads {
            state,
            ledger,
            profiles,
            readings,
            cfg,
        },
        current,
        skip,
        KeyAlert::Silent,
    )
}

/// Whether a caller of [`candidate`] wants the held-key ALERT written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyAlert {
    Write,
    Silent,
}

/// Everything a candidate choice is read from, bundled so that [`candidate`]
/// stays inside clippy's argument limit without an exemption. Shared
/// references only: the choice reads this state and never writes it.
struct Reads<'a> {
    state: &'a State,
    ledger: &'a Ledger,
    profiles: &'a [ProfileName],
    readings: &'a BTreeMap<String, Reading>,
    cfg: &'a Config,
}

fn candidate(reads: Reads<'_>, current: &str, skip: &[String], alert: KeyAlert) -> Option<Pick> {
    let Reads {
        state,
        ledger,
        profiles,
        readings,
        cfg,
    } = reads;
    let source_host = current;
    let current = ledger.account_in(source_host);
    let now = fsx::now();
    let overflow = OverflowInputs::load(&state.root, cfg, now);
    let pool = Pool {
        profiles,
        readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now,
    };
    let eligible = |c: &ProfileName| {
        let n = c.as_str();
        !skip.iter().any(|s| s == n)
            && !ledger.hosts.contains_key(n)
            && ledger.leased_to(n).is_none()
            && login_ready(state, &overflow, c)
    };
    let on_overflow = overflow.order.iter().any(|p| p.as_str() == current);
    let need = amplihack_utils::organisation_affinity::Need::of(
        on_overflow,
        !on_overflow && candidate::is_exhausted(readings.get(current), cfg.threshold, now),
    );
    let restriction = super::affinity::restrict(
        state,
        ledger,
        &pool,
        &overflow,
        source_host,
        current,
        need,
        eligible,
    );
    let picked = candidate::select_restricted(
        &pool,
        &overflow.pool(),
        current,
        |p| eligible(p) && restriction.allowed.contains(p),
        |p| restriction.plan_room.contains(p),
    );
    if alert == KeyAlert::Write && picked.is_none_or(|p| overflow.platform.as_ref() != Some(p)) {
        let held = per_token::key_held(&pool, &overflow, current, eligible);
        per_token::record_key_held(state, &held);
    }
    let profile = picked?.clone();
    // Read again for the detail the reason line gives. A key that went away
    // since the check above is refused like any other failed check.
    let check = overflow.check_note(&state.root, &profile)?;
    let per_token = per_token::reason(&pool, &overflow, cfg, current, &profile, check);
    Some(Pick { profile, per_token })
}

/// The login checks a move onto `c` must pass, apart from the readings and
/// the ledger: `c` holds its own login on this platform (macOS keeps logins
/// in the Keychain: no file to look for), and an API-key profile passes its
/// login check (issue #1804). [`nested_candidate`] and the takeover check
/// (`takeover::could_take_over`, issue #1801) both use this one rule, so a
/// capacity wait never starts for a login the selection would refuse.
pub(super) fn login_ready(state: &State, overflow: &OverflowInputs, c: &ProfileName) -> bool {
    (cfg!(target_os = "macos") || fsx::exists(&state.live_creds(c.as_str())))
        && overflow.check_note(&state.root, c).is_some()
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
    /// See [`Pick::per_token`].
    pub per_token: Option<String>,
}

/// The profile a nested launch inheriting `inherited` should use, when the
/// account live there is exhausted and another profile has quota left, or
/// when `inherited` is an overflow profile and a plan profile has capacity
/// (issue #1606: overflow is left as soon as a plan profile can work).
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
    let reading = readings.get(&account);
    let exhausted = candidate::is_exhausted(reading, cfg.threshold, now);
    let (reason, inherited_reset) = match reading {
        Some(r) if exhausted => (limit_reason(r, now), exhausted_reset(r, now)),
        _ if cfg.is_overflow(inherited) => (
            "an overflow profile and a plan profile has capacity".to_string(),
            None,
        ),
        _ => return None,
    };
    let profiles = profile_names(store);
    let skip = vec![inherited.to_string()];
    nested_candidate(
        state,
        &ledger,
        &profiles,
        &readings,
        cfg,
        inherited.as_str(),
        &skip,
    )
    .map(|c| Substitute {
        profile: c.profile,
        reason,
        inherited_reset,
        per_token: c.per_token,
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
///
/// Every `Ok(None)` goes through [`unsupervised`], which checks that fd 0
/// is still whole for the plain launch that follows, and every result of an
/// attempt goes through [`finish`], which refuses to report a success as
/// the step's when the prompt that reached that attempt stopped short of
/// the one that was sent (issue #2816).
pub(crate) fn run(switch: Option<Switch<'_>>, mut waiter: Waiter) -> Result<Option<i32>> {
    let on = switch.is_some();
    // Looks at stdin without reading it, so nothing here can block. It is
    // taken before the scratch directory so that every path out of this
    // function, including the ones that stand down below, can say what has
    // been taken off stdin and hand it back (issue #2816).
    let mut stdin = RerunStdin::capture();
    let setup = scratch_dir(switch.as_ref().map(|s| &s.state)).and_then(|dir| {
        Ok((
            dir,
            std::env::current_exe().context("cannot find the amplihack binary")?,
        ))
    });
    let (rerun_dir, exe) = match setup {
        Ok(s) => s,
        Err(e) => return unsupervised(&waiter, on, &stdin, &e),
    };
    super::sweep_rerun(&rerun_dir);
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // Issue #2815: what this step reads is on the pipe, which is how a
    // recipe agent step is always given its prompt. A prompt is copied
    // aside as it arrives, so a rerun can read the same bytes. The copier
    // reads nothing until the first attempt is running and this thread
    // never reads stdin at all, so a pipe nobody closes cannot hold the
    // supervisor up (issue #2816).
    //
    // A `--input-format` channel is the one piped stdin that is not a
    // prompt: it has no end to copy, its caller holds it open on purpose,
    // and replaying a session's history to a retry would be wrong. It is
    // read here as input that cannot be kept, which is the conduct every
    // piped prompt had before #2815: no wait and no rerun (issue #2816).
    let input_on_stdin = matches!(stdin, RerunStdin::Pipe) && !waiter.prompt_in_args();
    if input_on_stdin
        && !waiter.stdin_is_a_channel()
        && let Some(captured) = scratch::capture_own_prompt(&rerun_dir)
    {
        stdin = captured;
    }
    // No copy was made: as before #2815, neither an auto-switch rerun nor
    // the wait could give what this step reads to another attempt. Set
    // again below if the copy is later lost (#2816).
    let mut no_capture = input_on_stdin && !stdin.is_captured();
    if no_capture {
        waiter.block(NoWait::PipedPrompt);
    }
    let signals = match Signals::register() {
        Ok(s) => s,
        Err(e) => {
            return unsupervised(
                &waiter,
                on,
                &stdin,
                &format!("cannot catch stop signals: {e}"),
            );
        }
    };

    // The profile this attempt runs under; `None` keeps the inherited
    // environment.
    let mut profile: Option<ProfileName> = None;
    // The inherited profile's own limit. Only its reset times the wait: the
    // limits of the profiles auto-switch tried matter only to its choice of
    // candidate, which `next_profile` records (#1618). The retry runs under
    // that profile too, unless the wait ended on another login's capacity,
    // in which case it moves to that login (#1801, #2694).
    let mut inherited_wait: Option<WaitFor> = None;
    // Set when the step was moved off the inherited profile only for being
    // over the auto-switch threshold (no window at 100%). That profile still
    // has capacity, so when the candidate is limited the step goes back to
    // it as an auto-switch rerun, not a wait: the step's single wait stays
    // free for the inherited profile's own limit, which is the likelier one
    // on a nearly-empty window (#1618).
    let mut inherited_has_capacity = false;
    // Whether another login could take over, so a capacity wait is worth
    // starting: the one read-only check, made when a limit is seen (#1801).
    // Both kinds of step need a running rotator, so a capacity wait never
    // starts when nothing could end it.
    let takeover = |waiter: &Waiter| match &switch {
        Some(sw) => profile_check(sw.store, sw.inherited.as_str()),
        None => waiter.takeover_home(),
    };
    // Silent: this runs every 30 s for the whole of a wait, and it only
    // answers whether a move is possible; the retry's own selection below
    // is the call that moves the step and writes the alert (#2694).
    let probe = |start, now| {
        switch.as_ref().is_some_and(|sw| {
            capacity_candidate(
                sw.store,
                &sw.state,
                &sw.inherited,
                start,
                now,
                KeyAlert::Silent,
            )
            .is_some()
        })
    };
    if let Some(sw) = &switch {
        let eff = config::effective(sw.store.root());
        if let Some(sub) = substitute(sw.store, &sw.state, &eff.config, &sw.inherited) {
            eprintln!(
                "{}",
                substitution_note(&sw.inherited, &sub.reason, &sub.profile)
            );
            per_token::record(&sw.state.root, sub.per_token.as_deref());
            // The inherited account never runs before the retry, so its
            // reset comes from the readings, moved onto the wait's clock.
            // Without one (no window at 100%) no wait is built at all: see
            // `inherited_has_capacity`. A candidate's limit is another
            // account's and never times this wait.
            match sub.inherited_reset {
                Some(r) => {
                    let wait_now = waiter.now();
                    let reset = wait_now.saturating_add(r.saturating_sub(fsx::now()));
                    let wait = plan::from_reading(Harness::Claude, reset, wait_now);
                    let seen = Some((Harness::Claude, Some(reset)));
                    inherited_wait = Some(plan::upgrade(wait, seen, || takeover(&waiter)));
                }
                None => inherited_has_capacity = true,
            }
            profile = Some(sub.profile);
        }
    }
    let mut tried: Vec<String> = Vec::new();
    let mut n = 0u32;
    let mut recovery = recipe_runner_rs::recovery::ipc::Client::from_environment()?;
    loop {
        n += 1;
        if n > 1
            && let Some(client) = &recovery
        {
            recovery = Some(client.renew()?);
        }
        let switch_dir = switch
            .as_ref()
            .zip(profile.as_ref())
            .map(|(sw, p)| (p, sw.state.profile_dir(p.as_str())));
        let env = switch_dir.as_ref().map(|(p, d)| (*p, d.as_path()));
        // Issue #2731: which login this attempt runs on, read before it
        // starts. The rotator reacts to the same exhausted reading that
        // fails it, so the swap often lands while the attempt is failing.
        waiter.note_attempt_login();
        let attempt = match run_attempt_with_recovery(
            &exe,
            &args,
            &rerun_dir,
            env,
            &mut stdin,
            n,
            &signals,
            recovery.as_ref(),
        ) {
            Ok(a) => a,
            // The first attempt never started, so a plain launch can
            // follow. `unsupervised` gives it back the prompt a copier has
            // taken off stdin by now, or refuses to let it run (#2816).
            Err(e) if n == 1 && profile.is_none() => {
                return unsupervised(&waiter, on, &stdin, &e);
            }
            Err(e) => return Err(e),
        };
        // Issue #1700: a key error on an API-key profile blocks the API key.
        note_key_error(
            &attempt,
            profile.as_ref(),
            switch.as_ref().map(|sw| sw.store.root()),
        );
        // Issue #2857: how this attempt came to run, read once and consumed,
        // so an auto-switch rerun under another profile cannot inherit it.
        // Asked before the output is judged, because what it says decides
        // whether a 429 is a limit worth waiting for or the transient fault
        // it is everywhere else.
        let resumed_on_a_signal = waiter.resumed_on_a_signal();
        let output = attempt.output();
        let limited = output.usage_limited(waiter.now());
        // Issue #2857, part 1: the retry a wait earned was rejected at once,
        // so the signal that ended that wait was wrong about the login. A
        // usage-limit message says what is wrong and when it clears, so it
        // decides; a bare 429 states nothing and asks for a capacity wait.
        let met = match (&limited, resumed_on_a_signal) {
            (_, false) => None,
            (Some(_), true) => Some(Met::UsageLimit),
            (None, true) => output.rate_limited().then_some(Met::RateLimit),
        };
        if limited.is_none() && met.is_none() {
            return finish(attempt, None, &stdin);
        }
        // Issue #2816: the copier knows at once when the capture file was
        // lost, but `for_attempt` only asks when a rerun is already due,
        // which is after the wait. Ask here instead, without joining the
        // copier: a capture already known to be lost then refuses the
        // auto-switch rerun and the wait on the spot, with the reasons
        // either already gives, and this attempt's result — its held-back
        // stdout included — is passed up. Waiting hours and then failing
        // adds an outcome no caller asked for.
        if !no_capture && stdin.capture_lost() {
            no_capture = true;
            waiter.block(NoWait::PipedPrompt);
        }
        if let Some(client) = &recovery {
            client.outcome(recipe_runner_rs::recovery::model::Outcome::UsageLimit)?;
        }
        // Issue #2857: a bare 429 on the retry a signalled wait earned, which
        // is the only way past the return above with no limit message. It
        // states no reset, so what it asks for is a capacity wait, and only
        // where one could start at all: the same check issue #1801 makes for
        // a limit that states no readable reset. With nothing that could end
        // such a wait, the 429 goes back to the recipe runner as the
        // transient fault it is everywhere else, and no line is written.
        //
        // Nothing is recorded against any account here. A 429 is not a
        // reading of a login's windows, so no profile is marked limited and
        // no auto-switch candidate is chosen: the step waits on the login it
        // is on.
        if limited.is_none() {
            let planned = plan::upgrade(
                WaitFor::Skip(NoWait::NoReset),
                Some((Harness::Claude, None)),
                || takeover(&waiter),
            );
            if !matches!(planned, WaitFor::Capacity { .. }) {
                return finish(attempt, None, &stdin);
            }
            inherited_wait = Some(planned);
        }
        // Issue #2857: a usage limit decides the wait, as it always has. The
        // 429 arm above ran only when there is no limit message at all, so a
        // retry that printed both is planned by the message here and the 429
        // beside it adds nothing.
        if let Some(lim) = &limited {
            if profile.is_none() {
                // This attempt ran under the inherited profile: its limit is the
                // freshest word on that profile.
                let wait = plan::upgrade(lim.wait, lim.seen, || takeover(&waiter));
                // Issue #2731: an organisation spend limit with no reset on a
                // `$HOME/.claude` step waits for the login to change instead.
                inherited_wait = Some(plan::upgrade_spend(
                    wait,
                    lim.spend.filter(|_| waiter.is_home()),
                    || waiter.rotator_running(),
                ));
            }
            if let Some(sw) = &switch {
                let current = profile.as_ref().unwrap_or(&sw.inherited).clone();
                tried.push(current.to_string());
                let eff = config::effective(&sw.state.root);
                // The profile an unstored prompt kept this step from rerunning
                // under.
                let mut refused: Option<ProfileName> = None;
                if eff.on
                    && let Some(next) = next_profile(
                        sw.store,
                        &sw.state,
                        &eff.config,
                        &current,
                        &tried,
                        lim,
                        &waiter,
                    )
                {
                    // The limit is recorded; a rerun that has no whole prompt to
                    // read is refused rather than given part of one. When the
                    // capture was never made the wait below refuses too, with
                    // `piped_prompt`; when the prompt is merely still arriving
                    // the wait is still worth making, and is asked again when
                    // it ends (issue #2816).
                    let unstored = no_capture.then(|| NO_STORED_PROMPT.to_string());
                    if let Some(why) = unstored.or_else(|| stdin.rerun_refused()) {
                        eprintln!(
                            "auto-switch: no rerun under profile {:?}: {why}",
                            next.profile.as_str()
                        );
                        refused = Some(next.profile);
                    } else {
                        if let Some(sig) = signals.received() {
                            eprintln!("auto-switch: no rerun: this step was told to stop");
                            return finish(attempt, Some(128 + sig), &stdin);
                        }
                        eprintln!(
                            "amplihack: profile {:?} hit its usage limit; rerunning this step under profile {:?} (auto-switch)",
                            current.as_str(),
                            next.profile.as_str()
                        );
                        per_token::record(&sw.state.root, next.per_token.as_deref());
                        attempt.discard();
                        profile = Some(next.profile).filter(|p| *p != sw.inherited);
                        continue;
                    }
                }
                // No other candidate: back to the inherited profile, which was
                // passed over only for the threshold and still has capacity.
                // This is an auto-switch rerun; it does not call `after_limit`,
                // so the wait is still there if that profile is limited next.
                if inherited_has_capacity && profile.is_some() {
                    // As for a candidate above: a rerun needs the whole prompt.
                    // The `skipped` event is written below.
                    let unstored = no_capture.then(|| NO_STORED_PROMPT.to_string());
                    if let Some(why) = unstored.or_else(|| stdin.rerun_refused()) {
                        inherited_has_capacity = false;
                        if refused.as_ref() != Some(&sw.inherited) {
                            eprintln!(
                                "auto-switch: no rerun under profile {:?}: {why}",
                                sw.inherited.as_str()
                            );
                        }
                    }
                }
                if inherited_has_capacity && profile.is_some() {
                    inherited_has_capacity = false;
                    if let Some(sig) = signals.received() {
                        eprintln!("auto-switch: no rerun: this step was told to stop");
                        return finish(attempt, Some(128 + sig), &stdin);
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
        }
        // Set by now unless the inherited profile never ran and has no
        // known reset: a candidate's limit is another account's and never
        // times the wait. That happens when the move back to the inherited
        // profile was refused (a prompt on a pipe that could not be
        // stored); the wait is refused for the same reason, so
        // `after_limit` still writes the `skipped` event (`piped_prompt`,
        // or `disabled`/`ci`) and never starts a wait.
        // With nothing blocking the wait there is no reset to wait for, and
        // the result is passed up.
        let Some(wait) = inherited_wait.or_else(|| waiter.blocked_skip()) else {
            return finish(attempt, None, &stdin);
        };
        // Issue #2816: the retry the wait earns is announced only once the
        // prompt it would read is known to be there. `rerun_refused` is
        // asked at that moment and not before, because a prompt still
        // arriving when the limit was seen has normally arrived by the time
        // the wait ends; when it has not, the step ends here with this
        // attempt's result rather than on a rerun with nothing to read.
        match waiter.after_limit(
            wait,
            n,
            &signals,
            &probe,
            &mut || stdin.rerun_refused(),
            match met {
                Some(met) => Resumed::AfterASignal(met),
                None => Resumed::Ordinarily,
            },
        ) {
            AfterLimit::Finish(code) => return finish(attempt, code, &stdin),
            AfterLimit::Retry => {
                attempt.discard();
                profile = retry_profile(switch.as_ref(), &waiter);
            }
        }
    }
}

/// Ends the step with this attempt's result, unless the prompt that reached
/// the attempt stopped short of the one that was sent (issue #2816).
///
/// The copier sits between the caller's pipe and the attempt, and closing a
/// pipe is the only signal it can give its reader. A read of the source
/// that fails therefore arrives at the attempt as a clean end of file: the
/// attempt acts on whatever came before it, cannot tell that from a whole
/// prompt, and may well exit 0. Reporting that as the step's success would
/// put a result for part of a task where the result of the task belongs.
///
/// So the attempt's own output is still printed — nothing it produced is
/// hidden — its exit code is kept when it already failed, and only a
/// success is replaced, because only a success would be believed. The
/// copier has already named the cause on stderr; this line names the
/// consequence.
///
/// The flag is read without joining the copier, and for this case it cannot
/// be missed: the copier publishes it before it drops the attempt's pipe,
/// the attempt reaches end of file only once that pipe is dropped, and this
/// runs after the attempt has exited. The one case it cannot see is a
/// source that fails after an attempt which exited without reading to the
/// end of its prompt, and there the copier's own line is the report.
fn finish(attempt: Attempt, code: Option<i32>, stdin: &RerunStdin) -> Result<Option<i32>> {
    let out = attempt.finish(code)?;
    if !stdin.prompt_cut_short() {
        return Ok(out);
    }
    eprintln!(
        "amplihack: this step's prompt was cut short on its way to the attempt, so the \
         attempt was not given the whole task and its result is not this step's result"
    );
    Ok(Some(out.filter(|c| *c != 0).unwrap_or(1)))
}

/// The profile the retry after a wait runs under; `None` keeps the inherited
/// environment. After a capacity wait, and after a wait of any kind that
/// ended because another login has room (#2694), that is the login which
/// showed capacity, never the exhausted inherited one; after a reset wait
/// that ran to its deadline, `None`. The candidate is chosen again here
/// rather than carried over from the wait's own probe, because the live
/// login may have changed since: an in-session swap on this host leases the
/// login it moves to and can remove the candidate, and a newer reading can
/// disqualify it. A waiting step takes no lease, so waiting steps do not
/// remove each other's candidates; several of them land on the same login
/// together, which is issue #2733. So the pick may now be `None`, and the
/// one retry then runs on the inherited profile.
///
/// What that costs is [`room_was_seen`]'s decision, not this
/// function's: only a wait that a reading ended early gave up something, so
/// only that case prints. A capacity wait that reached its own reset, or saw
/// its login change, never had room to lose and retries on a login that has
/// just recovered, which is the designed outcome and stays as silent as it
/// was before #2694. Resuming the original wait instead of spending the
/// retry is issue #2719.
///
/// [`room_was_seen`]: crate::commands::profile::limit_wait::RetryMove::room_was_seen
fn retry_profile(switch: Option<&Switch<'_>>, waiter: &Waiter) -> Option<ProfileName> {
    let sw = switch?;
    let mv = waiter.retry_move()?;
    let Some(pick) = capacity_candidate(
        sw.store,
        &sw.state,
        &sw.inherited,
        mv.start(),
        waiter.now(),
        KeyAlert::Write,
    ) else {
        if mv.room_was_seen() {
            // A reading ended this wait early, but the login no longer
            // qualifies at the moment the retry moves: an in-session swap on
            // this host took it, or a newer reading disqualified it (#2694).
            // Not another waiting step, which takes no lease and would land
            // on the same login rather than remove it (#2733). Say so. Every
            // other transition in this loop prints, and without a line here
            // the log reads
            // `end reason=login capacity` and then fails on the login that
            // had none, with nothing in between to name the cause.
            eprintln!(
                "amplihack: no login has room now; this step retries under the limited profile (auto-switch)"
            );
        }
        return None;
    };
    // The run log names the login the retry moves to, as every other move
    // in this loop does (#1801, item 3).
    eprintln!(
        "amplihack: a login has capacity again; rerunning this step under profile {:?} (auto-switch)",
        pick.profile.as_str()
    );
    // A move onto a per-token profile gives its reason line, as any rerun
    // onto one does (#1804).
    per_token::record(&sw.state.root, pick.per_token.as_deref());
    Some(pick.profile)
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

/// Stands down: the supervisor cannot go on and no attempt has run, so the
/// caller runs the step once in this process (`Ok(None)`).
///
/// That launch inherits this process's own fd 0, which is why `stdin` is
/// taken. A prompt copier started for issue #2815 would have been reading
/// fd 0, and handing the launch what was left of that pipe would give it a
/// truncated prompt. It never has: the copier starts only once the first
/// attempt is running, and every path here is reached before that (issue
/// #2816). `stand_down` is the check that says so, for a capture as for
/// every other kind of stdin, and it returns `Err` — leaving the step
/// unrun — rather than let a launch read a prompt that is short by whatever
/// a copier took.
fn unsupervised(
    waiter: &Waiter,
    on: bool,
    stdin: &RerunStdin,
    why: &dyn std::fmt::Display,
) -> Result<Option<i32>> {
    let who = if on {
        "auto-switch"
    } else {
        "the usage-limit wait"
    };
    stdin.stand_down().map_err(|e| {
        anyhow::anyhow!(
            "{who} cannot supervise this step ({why}), and this step's prompt \
             cannot be given to a plain launch either, so the step was not run: {e}"
        )
    })?;
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
) -> Option<Pick> {
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
                profile.as_str(),
                tried,
            )
        }
        Err(e) => {
            eprintln!("auto-switch: no rerun: {e}");
            None
        }
    }
}
