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
//!
//! Issue #2967: a wait is the last resort. Whenever an attempt reports a
//! usage limit the step first moves to another login the candidate rule
//! allows, whether or not it has waited before. A wait starts only when none
//! can run it, and its deadline is then moved to the earliest moment another
//! login the step could still move to drops under the auto-switch threshold
//! (`limit_choice::cross_login_reset`), with the retry on that login.

use super::candidate::{self, Pool};
use super::config::{self, Config};
use super::fsx;
use super::limit_choice;
use super::overflow_inputs::OverflowInputs;
use super::own_login::{OwnLogins, unreadable_note};
use super::per_token;
use super::state::{Ledger, State};
use super::takeover::{capacity_candidate, profile_check};
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::attempt::{
    Attempt, Limited, Signals, note_key_error, run_attempt_with_recovery,
};
use crate::commands::profile::limit_wait::earliest::earliest;
use crate::commands::profile::limit_wait::plan::{self, NoWait, WaitFor};
use crate::commands::profile::limit_wait::scratch::{self, Replay, RerunStdin};
use crate::commands::profile::limit_wait::{AfterLimit, Met, Resumed, Unconsidered, Waiter};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::rotate;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_cmd;
use crate::commands::recipe::run::limit_patterns::Harness;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[cfg(test)]
pub(super) use crate::commands::profile::attempt::limit_reset;

/// Why a step whose prompt comes on a pipe is not run when the copy of that
/// prompt cannot be set up.
///
/// The copy needs a second description of stdin, the first attempt's stdin
/// pipe and a thread, and only a process out of file descriptors or threads
/// cannot make them. An attempt run without the copy could not be repeated,
/// so a limit on it would end the step; the step is therefore not run at
/// all, before a byte of its prompt is read, and the run is told why. The
/// words avoid every limit marker on purpose: no limit was met, and the
/// recipe runner must not class this failure as one.
const NOT_RUN_WITHOUT_A_COPY: &str = "this step was not run: its prompt comes on a pipe, and the \
                                      copy of it that a retry would read could not be set up";

/// The cause the prompt-loss ending names when this step's prompt could not
/// be read to its end (issues #2816, #3036): the source failed, or the
/// copier panicked part way through it. The attempt in hand was given part
/// of its task, so such a step fails whether or not it meets a limit
/// ([`finish`] refuses to pass its success up), and no rerun can be given
/// the rest. It is the one way a captured prompt is lost.
const PROMPT_CUT_SHORT: &str = "its piped prompt could not be read to its end, so neither this \
                                attempt nor a later one can be given the whole of it";

/// The note the retry after an earliest-reset wait prints (issue #2967),
/// followed by the profile whose reset set the wait's deadline, and only
/// when the step changes login ([`earliest_note`]). It reaches the step's
/// stderr, which the recipe runner's classifier reads, so it carries no
/// usage-limit marker (pinned in `tests_limit_choice`), and it names only
/// the local profile the step moves to.
pub(super) const NOTE_EARLIEST: &str = "amplihack: the earliest reset among this step's logins has arrived; rerunning this step under profile";

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
/// piped prompt that was cut short can still refuse the rerun.
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
/// usage-limit wait's probe only, which asks the same question after every
/// write to the files the wait watches for the whole of a wait, so the alert
/// would repeat for hours in the step's stderr and crowd the runner's
/// evidence tail. The step that is waiting has
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
#[derive(Clone, Copy)]
pub(super) struct Reads<'a> {
    pub(super) state: &'a State,
    pub(super) ledger: &'a Ledger,
    pub(super) profiles: &'a [ProfileName],
    pub(super) readings: &'a BTreeMap<String, Reading>,
    pub(super) cfg: &'a Config,
}

/// The eligibility every choice of a login to move to is made under, set
/// up once by [`with_choice`] (issue #2967). The candidate rule and the
/// reader of other logins' resets (`limit_choice`) both take it from here,
/// so neither can admit a login the other refuses.
pub(super) struct Choice<'c, 'a> {
    pub(super) reads: Reads<'a>,
    /// The directory the step runs in: a profile name.
    pub(super) source_host: &'c str,
    /// The account the auto-switch ledger says is live in that directory.
    pub(super) current: &'c str,
    pub(super) pool: &'c Pool<'a>,
    pub(super) overflow: &'c OverflowInputs,
    own: &'c OwnLogins<'c>,
    /// Free to be moved onto, as far as the caller's skip list and the
    /// ledger say: not skipped, not hosting another account, not leased.
    free: &'c dyn Fn(&ProfileName) -> bool,
    /// `free`, and passing the login checks ([`login_ready`]).
    pub(super) eligible: &'c dyn Fn(&ProfileName) -> bool,
}

/// Runs `f` with the eligibility for a move from directory `current`, never
/// onto a name in `skip`. The one place it is set up.
pub(super) fn with_choice<T>(
    reads: Reads<'_>,
    current: &str,
    skip: &[String],
    f: impl FnOnce(&Choice<'_, '_>) -> T,
) -> T {
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
    // One reader for the whole choice, so each profile is asked about once.
    let store = ProfileStore::new(state.root.clone());
    let home = usage_cmd::home_dir();
    let own = OwnLogins::new(&store, home.as_deref());
    let pool = Pool {
        profiles,
        readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now,
    };
    let free = |c: &ProfileName| {
        let n = c.as_str();
        !skip.iter().any(|s| s == n)
            && !ledger.hosts.contains_key(n)
            && ledger.leased_to(n).is_none()
    };
    let eligible = |c: &ProfileName| free(c) && login_ready(state, &overflow, &own, c);
    f(&Choice {
        reads,
        source_host,
        current,
        pool: &pool,
        overflow: &overflow,
        own: &own,
        free: &free,
        eligible: &eligible,
    })
}

fn candidate(reads: Reads<'_>, current: &str, skip: &[String], alert: KeyAlert) -> Option<Pick> {
    with_choice(reads, current, skip, |choice| candidate_in(choice, alert))
}

/// The candidate rule over an eligibility already set up by [`with_choice`].
fn candidate_in(choice: &Choice<'_, '_>, alert: KeyAlert) -> Option<Pick> {
    let Choice {
        reads,
        source_host,
        current,
        pool,
        overflow,
        own,
        free,
        eligible,
    } = choice;
    let Reads {
        state,
        ledger,
        profiles,
        readings,
        cfg,
    } = *reads;
    let (pool, overflow, current) = (*pool, *overflow, *current);
    let on_overflow = overflow.order.iter().any(|p| p.as_str() == current);
    let need = amplihack_utils::organisation_affinity::Need::of(
        on_overflow,
        !on_overflow && candidate::is_exhausted(readings.get(current), cfg.threshold, pool.now),
    );
    let restriction = super::affinity::restrict(
        state,
        ledger,
        pool,
        overflow,
        source_host,
        current,
        need,
        eligible,
    );
    let picked = candidate::select_restricted(
        pool,
        &overflow.pool(),
        current,
        |p| eligible(p) && restriction.allowed.contains(p),
        |p| restriction.plan_room.contains(p),
    );
    if alert == KeyAlert::Write && picked.is_none_or(|p| overflow.platform.as_ref() != Some(p)) {
        let held = per_token::key_held(pool, overflow, current, eligible);
        per_token::record_key_held(state, &held);
    }
    if alert == KeyAlert::Write && own.macos() {
        // Issue #2885: a profile that is out of this choice only because its
        // Keychain could not be read is named, one line each, so the failure
        // is not a silent skip. The wait's probe prints nothing, as it
        // prints nothing about the held key. Off macOS there is no
        // Keychain to fail, and nothing more is read.
        for name in profiles
            .iter()
            .filter(|p| !cfg.exclude.contains(p) && free(p))
        {
            if let Some(cause) = own.unreadable(name) {
                eprintln!("{}", unreadable_note(name, cause));
            }
        }
    }
    let profile = picked?.clone();
    // Read again for the detail the reason line gives. A key that went away
    // since the check above is refused like any other failed check.
    let check = overflow.check_note(&state.root, &profile)?;
    let per_token = per_token::reason(pool, overflow, cfg, current, &profile, check);
    Some(Pick { profile, per_token })
}

/// The login checks a move onto `c` must pass, apart from the readings and
/// the ledger: `c` holds its own login (a login file; on macOS also an item
/// in the Keychain, by the launch guard's rule, issue #2885; an API-key
/// profile there counts too), and an API-key profile passes its login check
/// (issue #1804). A macOS profile whose Keychain cannot be read holds none.
/// [`nested_candidate`] and the takeover check (`takeover::could_take_over`,
/// issue #1801) both use this one rule, so the takeover check never counts a
/// login the selection would refuse. Since issue #2911 a limit starts its
/// wait without asking that check; the bare 429 is the one reading left that
/// asks it.
pub(super) fn login_ready(
    state: &State,
    overflow: &OverflowInputs,
    own: &OwnLogins<'_>,
    c: &ProfileName,
) -> bool {
    own.of(c).holds() && overflow.check_note(&state.root, c).is_some()
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
    // Issue #3036: an `--input-format` stream channel is copied the same
    // way, so a retry replays it from its first byte. Under supervision
    // every attempt's stdout is buffered to a file and printed only when
    // the step finishes (`profile::attempt`), and the output of an attempt
    // that is run again is discarded, so the caller of a supervised step
    // has seen nothing of an attempt that met a limit. A retry given the
    // same input from its first byte is therefore the same step from the
    // caller's side, as it is for a plain prompt. A channel its caller still
    // holds open when a retry is due is neither refused nor waited for: the
    // retry is fed what has arrived and then the rest as it arrives, as the
    // first attempt was.
    let input_on_stdin = matches!(stdin, RerunStdin::Pipe) && !waiter.prompt_in_args();
    if input_on_stdin {
        // A copy that cannot be set up is not a reason to run the step
        // without one. Such an attempt could not be repeated, so a limit on
        // it would end the step. The step is not run at all, and nothing has
        // been read from the pipe.
        stdin = scratch::capture_own_prompt()
            .map_err(|e| anyhow::anyhow!("{NOT_RUN_WITHOUT_A_COPY} ({e})"))?;
    }
    // Set when the prompt is known to be cut short, the one way a captured
    // prompt is lost (#2816).
    let mut cut_short = false;
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
    // Silent: this runs after each write to the files the wait watches, for
    // the whole of a wait, and it only answers whether a move is possible;
    // the retry's own selection below is the call that moves the step and
    // writes the alert (#2694).
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
                    // `from_reading` decides the wait on its own (#2911): a
                    // reset too far away, or already behind the clock, is a
                    // capacity wait whether or not another login has room
                    // this instant.
                    inherited_wait = Some(plan::from_reading(Harness::Claude, reset, wait_now));
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
        // Issue #2967: the login's name is resolved here too, so the wait
        // is marked with it and its recipe-log lines say `account=<name>`.
        waiter.note_attempt_login(profile.as_ref());
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
        // The wait's clock, read once. The message's reset is read against it
        // below, and so is every judgement this block makes about that reset,
        // so the two cannot disagree about where the clock stands.
        let now = waiter.now();
        let limited = output.usage_limited(now);
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
            return finish(attempt, None, &mut stdin);
        }
        // Issue #2816: the copier knows at once when the prompt could not be
        // read to its end, but `for_attempt` only asks when a rerun is
        // already due, which is after the wait. Ask here instead, without
        // joining the copier: a prompt already known to be cut short then
        // refuses the auto-switch rerun and the wait on the spot, and this
        // attempt's result, its held-back stdout included, is passed up with
        // the prompt-loss ending. Such a step fails whether or not it met a
        // limit (`finish`). It is the one way a captured prompt is lost:
        // every byte the copier read is kept.
        if !cut_short && stdin.prompt_cut_short() {
            cut_short = true;
            waiter.block(NoWait::PromptCutShort, PROMPT_CUT_SHORT);
        }
        // Issue #2334: the limit is reported to the
        // recipe runner as data, for this step's scope, whatever happens
        // next. When the step goes on to end as a prompt cut short, this is
        // what the run's classification reads (`failure_cause`) and what
        // makes the run resumable at this step, so the runner sends the
        // whole prompt again. The sentence `limit_wait::ending` writes
        // beside that ending is for a person and decides nothing.
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
            let Some(planned) = plan::rate_limit_capacity(Harness::Claude, || takeover(&waiter))
            else {
                return finish(attempt, None, &mut stdin);
            };
            inherited_wait = Some(planned);
        }
        // Issue #2857: a usage limit decides the wait, as it always has. The
        // 429 arm above ran only when there is no limit message at all, so a
        // retry that printed both is planned by the message here and the 429
        // beside it adds nothing.
        if let Some(lim) = &limited {
            // Issues #2890, #2911, #2909: every limit of every kind blocks
            // the login this attempt ran under, until the reset the message
            // stated, held to the kind's own window (five hours for a
            // session limit, seven days for a weekly one). The rotator's
            // `/usage` probe cannot see a spend limit at all and learns of a
            // window limit only on its own cycle, so without this record it
            // keeps the login live while every step on it fails. It is data
            // the rotator reads: it ends no run, and the wait below is
            // planned whatever happens to it.
            //
            // Recorded *before* the wait is planned, so the wake conditions
            // and the rotator's own target choice already leave the blocked
            // login out, with no second mechanism.
            if let Some((kind, reset)) = lim.block
                && let Some(login) = waiter.attempt_profile(profile.as_ref())
                && let Some(root) = waiter.store_root()
            {
                // The reset was read against the wait's clock, which a test
                // may set; the ledger is expired against the real one. Move
                // it over as the distance it is, saturating. A message that
                // stated no reset passes `None` through, and the emitter
                // decides what, if anything, that means for the kind.
                let at = fsx::now();
                let until = reset.map(|r| at.saturating_add(r.saturating_sub(now)));
                rotate::step_block::note_limit_block(root, &login, at, until, kind);
            }
            if profile.is_none() {
                // This attempt ran under the inherited profile: its limit is
                // the freshest word on that profile, and `plan` has already
                // decided which wait it earns (issue #2911). None is gated on
                // another login being free now: a login can gain room during
                // a wait, and the wait's own wake conditions see that.
                inherited_wait = Some(lim.wait);
            }
            if let Some(sw) = &switch {
                let current = profile.as_ref().unwrap_or(&sw.inherited).clone();
                tried.push(current.to_string());
                let eff = config::effective(&sw.state.root);
                // The profile a prompt cut short kept this step from rerunning
                // under.
                let mut refused: Option<ProfileName> = None;
                if eff.on
                    && let Some(next) =
                        next_profile(sw.store, &sw.state, &eff.config, &current, &tried, lim)
                {
                    // The limit is recorded; a rerun that can never have a
                    // whole prompt to read is refused rather than given part of
                    // one, and the wait below writes the prompt-loss ending.
                    // A prompt merely still arriving is fed to the rerun as it
                    // arrives, and the rerun runs at once (issues #2816,
                    // #3036).
                    if let Some(why) = rerun_refusal(cut_short, &mut stdin) {
                        eprintln!(
                            "auto-switch: no rerun under profile {:?}: {why}",
                            next.profile.as_str()
                        );
                        refused = Some(next.profile);
                    } else {
                        if let Some(sig) = signals.received() {
                            eprintln!("auto-switch: no rerun: this step was told to stop");
                            return finish(attempt, Some(128 + sig), &mut stdin);
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
                    // The ending is written below, by `after_limit`.
                    if let Some(why) = rerun_refusal(cut_short, &mut stdin) {
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
                        return finish(attempt, Some(128 + sig), &mut stdin);
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
        // profile was refused because the prompt was cut short, and the
        // wait is refused for the same reason, so `after_limit` still writes
        // the ending (`failed reason=prompt_cut_short`, or `skipped
        // reason=disabled`) and never starts a wait.
        // A stop signal is honoured here as `after_limit` would honour it.
        // With nothing blocking the wait there is no reset to wait for, and
        // the result is passed up.
        if inherited_wait.is_none()
            && let Some(sig) = signals.received()
        {
            return finish(attempt, Some(128 + sig), &mut stdin);
        }
        let Some(wait) = inherited_wait.or_else(|| waiter.blocked_skip()) else {
            return finish(attempt, None, &mut stdin);
        };
        // Issue #2967: no login can run the step now, so it waits. The wait
        // is offered the earliest moment another login it could still move
        // to drops under the threshold, and ends there when that is sooner
        // than its own deadline (`plan::earliest`). The other logins are
        // read again for each wait, so a wait never carries an instant an
        // earlier one computed. `bound_login` is the login whose reset that
        // is, which is where the retry runs when the wait reaches it, with
        // the real time the deadline was set: only what is recorded about
        // that login after it can keep the retry off it
        // (`limit_choice::held_since`).
        let planned_at = fsx::now();
        let earliest = earliest_other(switch.as_ref(), &waiter, profile.as_ref(), &tried);
        waiter.offer_earliest_reset(earliest.as_ref().map(|(_, at)| *at));
        let bound_login = earliest.map(|(login, _)| (login, planned_at));
        // Issue #2816: the retry the wait earns is announced only once its
        // prompt is known not to be cut short. `replay` is asked at that
        // moment and not before, so a source that failed during the wait is
        // seen. It never waits: a prompt still arriving is fed to the retry
        // as it arrives.
        match waiter.after_limit(
            wait,
            n,
            &signals,
            &probe,
            &mut || stdin.replay(),
            match met {
                Some(met) => Resumed::AfterASignal(met),
                None => Resumed::Ordinarily,
            },
        ) {
            AfterLimit::Finish(code) => return finish(attempt, code, &mut stdin),
            AfterLimit::Retry => {
                attempt.discard();
                profile = retry_profile(
                    switch.as_ref(),
                    &waiter,
                    bound_login.as_ref(),
                    profile.as_ref(),
                );
            }
        }
    }
}

/// Why an auto-switch rerun cannot be given this step's prompt; `None` when
/// it can (issues #2816, #3036).
///
/// Only a prompt cut short is refused. A prompt still arriving is fed to the
/// rerun as it arrives, so the rerun runs at once and nothing waits on the
/// caller's pipe. It used to be refused, which on a
/// step whose inherited profile never ran ended the step on a limit, and
/// then waited for, which held the step for ever on a caller that never
/// closed its pipe.
fn rerun_refusal(cut_short: bool, stdin: &mut RerunStdin) -> Option<String> {
    if cut_short {
        return Some(PROMPT_CUT_SHORT.to_string());
    }
    match stdin.replay() {
        Replay::Ready => None,
        Replay::Never(why) => Some(why),
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
/// For this case the flag cannot be missed: the copier, or a rerun's
/// feeder, publishes it before it drops the attempt's pipe, the attempt
/// reaches end of file only once that pipe is dropped, and this runs after
/// the attempt has exited. A step that ran a rerun first lets the copier
/// finish when the caller has closed the pipe ([`RerunStdin::settle`]), so
/// a source that failed after the last attempt stopped reading is seen
/// too, and every line the copier writes comes before the result.
/// Elsewhere the copier's own line is the report of that case.
fn finish(attempt: Attempt, code: Option<i32>, stdin: &mut RerunStdin) -> Result<Option<i32>> {
    stdin.settle();
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
///
/// Issue #2967: after a wait that reached a deadline another login's reset
/// set, the retry runs on `bound`, that login, unless something recorded
/// about it since the deadline was set still holds it out
/// (`limit_choice::held_since`). The login must also still pass the checks
/// that need no reading (`limit_choice::may_run_on`). When either fails, or
/// the logins cannot be read, the retry runs on the step's own login and
/// says why ([`bound_profile`]). `bound` carries the real time the deadline
/// was set. `from` is the profile the limited attempt ran under, `None` for
/// the inherited one.
fn retry_profile(
    switch: Option<&Switch<'_>>,
    waiter: &Waiter,
    bound: Option<&(ProfileName, i64)>,
    from: Option<&ProfileName>,
) -> Option<ProfileName> {
    let sw = switch?;
    if waiter.reached_earliest_reset() {
        let (bound, since) = bound?;
        return bound_profile(sw, waiter, from.unwrap_or(&sw.inherited), bound, *since);
    }
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

/// Why a step's wait, or the retry after an earliest-reset wait, could not
/// consider the step's other stored logins (issue #2967).
///
/// The step goes on as it would have without them, waiting for or retrying
/// on its own login. There is no retry and no gate; the cause is reported.
/// Each case writes one stderr line that names the cause, and the error for
/// a file that could not be read ([`NotConsidered::wait_line`],
/// [`NotConsidered::retry_line`]). It also writes one wait line from a
/// closed set, which also reaches the run's recipe log
/// (`Waiter::other_logins_unread`, `Waiter::retry_not_moved`). Without
/// them, "no other login had a reset ahead", which writes nothing, and "the
/// logins could not be read" look the same, and the second is the quiet wait
/// on the step's own reset that #2967 is about. `next_profile` and
/// `State::load_usage_or_warn` already report the same failures on the
/// paths they serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum NotConsidered {
    /// Auto-switch was turned off while the step ran. A step that started
    /// with it off has no `Switch`, never had another login to consider, and
    /// writes nothing here: that is the operator's standing instruction.
    AutoSwitchOff,
    /// The auto-switch ledger could not be read, with the error.
    LedgerUnreadable(String),
    /// The usage readings could not be read, with the error.
    ReadingsUnreadable(String),
    /// The login whose reset ended the wait may no longer be run on
    /// (`limit_choice::may_run_on`). Only the retry after a wait meets it.
    LoginNotEligible,
    /// A limit block on the login whose reset ended the wait, or a reading
    /// of it at or above the threshold, recorded after the deadline was set
    /// still holds it out (`limit_choice::held_since`, issue #3105). Only
    /// the retry after a wait meets it.
    LoginStillLimited,
}

impl NotConsidered {
    /// The closed-set reason the wait lines write.
    pub(super) fn why(&self) -> Unconsidered {
        match self {
            NotConsidered::AutoSwitchOff => Unconsidered::AutoSwitchOff,
            NotConsidered::LedgerUnreadable(_) => Unconsidered::LedgerUnreadable,
            NotConsidered::ReadingsUnreadable(_) => Unconsidered::ReadingsUnreadable,
            NotConsidered::LoginNotEligible => Unconsidered::LoginNotEligible,
            NotConsidered::LoginStillLimited => Unconsidered::LoginStillLimited,
        }
    }

    /// The cause in words, with the error for a file that could not be read.
    fn cause(&self) -> String {
        match self {
            NotConsidered::AutoSwitchOff => {
                "auto-switch was turned off while this step ran".to_string()
            }
            NotConsidered::LedgerUnreadable(e) => {
                format!("the auto-switch ledger cannot be read: {e}")
            }
            NotConsidered::ReadingsUnreadable(e) => {
                format!("the auto-switch usage readings cannot be read: {e}")
            }
            NotConsidered::LoginNotEligible => "that profile was excluded, leased, given \
                                                another account, logged out or removed \
                                                while this step waited"
                .to_string(),
            NotConsidered::LoginStillLimited => "a limit block on that profile, or a reading \
                                                 of it at or above the auto-switch threshold, \
                                                 came in while this step waited and still \
                                                 holds it out"
                .to_string(),
        }
    }

    /// The stderr line for a wait whose other logins could not be read.
    pub(super) fn wait_line(&self) -> String {
        format!(
            "amplihack: this step's other logins were not read for its wait (reason={}): {}; \
             no other login's reset can end this wait early (auto-switch)",
            self.why().as_str(),
            self.cause()
        )
    }

    /// The stderr line for a retry after an earliest-reset wait that runs
    /// under `own`, the step's inherited profile, and not under `bound`, the
    /// profile whose reset ended the wait.
    pub(super) fn retry_line(&self, bound: &ProfileName, own: &ProfileName) -> String {
        format!(
            "amplihack: the wait ended at the reset of profile {:?}, but this step reruns under \
             its own profile {:?} (reason={}): {} (auto-switch)",
            bound.as_str(),
            own.as_str(),
            self.why().as_str(),
            self.cause()
        )
    }
}

/// Loads the readings for one choice among the stored logins and runs `f` on
/// them. `Err` says why they could not be loaded: auto-switch has been
/// turned off since the step started, or the ledger or the readings cannot
/// be read. The caller then keeps the step where it is and reports the
/// cause, so this never fails quietly.
pub(super) fn with_reads<T>(
    store: &ProfileStore,
    state: &State,
    f: impl FnOnce(Reads<'_>) -> T,
) -> Result<T, NotConsidered> {
    let eff = config::effective(&state.root);
    if !eff.on {
        return Err(NotConsidered::AutoSwitchOff);
    }
    let ledger = state
        .load_ledger()
        .map_err(|e| NotConsidered::LedgerUnreadable(e.to_string()))?;
    let readings = state
        .try_load_usage()
        .map_err(|e| NotConsidered::ReadingsUnreadable(e.to_string()))?
        .accounts;
    let profiles = profile_names(store);
    Ok(f(Reads {
        state,
        ledger: &ledger,
        profiles: &profiles,
        readings: &readings,
        cfg: &eff.config,
    }))
}

/// The earliest moment another login the step could still move to drops
/// under the auto-switch threshold, and the login it is (issue #2967).
/// Whether it is sooner than the step's own deadline is the wait's to
/// decide (`plan::earliest`).
///
/// `None` without auto-switch, when the step's wait is off (no wait, so
/// nothing to bound and nothing is read), and when no such login has a
/// reset ahead, which writes nothing. It is also `None` when the logins
/// cannot be read, and then the step says why, on stderr and in the run's
/// recipe log ([`NotConsidered`]), before the wait's `start` line.
///
/// `profile` is the profile the limited attempt ran under, `None` for the
/// inherited one. The readings are moved onto the wait's clock by
/// `limit_choice::cross_login_reset`.
fn earliest_other(
    switch: Option<&Switch<'_>>,
    waiter: &Waiter,
    profile: Option<&ProfileName>,
    tried: &[String],
) -> Option<(ProfileName, i64)> {
    let sw = switch?;
    if !waiter.enabled() {
        return None;
    }
    let current = profile.unwrap_or(&sw.inherited);
    let now = waiter.now();
    match with_reads(sw.store, &sw.state, |reads| {
        limit_choice::cross_login_reset(reads, current.as_str(), tried, now)
    }) {
        Ok(others) => earliest(&others),
        Err(why) => {
            eprintln!("{}", why.wait_line());
            waiter.other_logins_unread(why.why());
            None
        }
    }
}

/// Whether the retry after an earliest-reset wait may run on `bound`, the
/// login whose reset ended it. It must pass the checks that need no reading
/// (`limit_choice::may_run_on`), asked with `from`, the login the limited
/// attempt ran on, as the step's login, as `cross_login_reset` was asked
/// when it chose `bound`. Nothing recorded about it after `since`, the real
/// time the deadline was set, may still hold it out
/// (`limit_choice::held_since`). `Err` says why not.
pub(super) fn may_retry_on(
    store: &ProfileStore,
    state: &State,
    from: &ProfileName,
    bound: &ProfileName,
    since: i64,
) -> Result<(), NotConsidered> {
    with_reads(store, state, |reads| {
        if !limit_choice::may_run_on(reads, from.as_str(), bound) {
            Err(NotConsidered::LoginNotEligible)
        } else if limit_choice::held_since(reads, bound, since) {
            Err(NotConsidered::LoginStillLimited)
        } else {
            Ok(())
        }
    })?
}

/// The note for a retry that moves from `from` to `bound` after an
/// earliest-reset wait; `None` when the two are one login, so the note
/// never claims a move that does not happen. `cross_login_reset` never
/// chooses the step's own login and `may_run_on` refuses it, so `None` is a
/// check here and not a path the supervisor takes.
pub(super) fn earliest_note(from: &ProfileName, bound: &ProfileName) -> Option<String> {
    (from != bound).then(|| format!("{NOTE_EARLIEST} {:?} (auto-switch)", bound.as_str()))
}

/// The retry after an earliest-reset wait: `bound`, when the step may still
/// be run on it ([`may_retry_on`]), else `None` for the step's own login,
/// with the cause on stderr and in the run's recipe log ([`NotConsidered`]).
/// `from` is the login the limited attempt ran on, and `since` the real time
/// the wait's deadline was set.
///
/// `None` is the inherited environment, which is the profile
/// `sw.inherited` itself. So the filter at the end maps that profile onto
/// `None`. It does not mean the login is unchanged: when `bound` is the
/// inherited profile, `from` is another profile the step was moved to, and
/// the step moves back. The note is printed for exactly the moves that
/// happen ([`earliest_note`]).
fn bound_profile(
    sw: &Switch<'_>,
    waiter: &Waiter,
    from: &ProfileName,
    bound: &ProfileName,
    since: i64,
) -> Option<ProfileName> {
    if let Err(why) = may_retry_on(sw.store, &sw.state, from, bound, since) {
        eprintln!("{}", why.retry_line(bound, &sw.inherited));
        waiter.retry_not_moved(why.why());
        return None;
    }
    if let Some(note) = earliest_note(from, bound) {
        eprintln!("{note}");
    }
    Some(bound.clone()).filter(|p| *p != sw.inherited)
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
/// eligible profile.
///
/// Issue #2967: this is asked for every limit, whether or not the step has
/// waited. Before, a step that had waited was never moved again, so a limit
/// on the retry that wait earned made it wait again with another login
/// under the threshold. A move is not a wait and records no mark. The moves
/// still end, because `tried` holds every login the step has run on and
/// grows with each one; then the step waits.
fn next_profile(
    store: &ProfileStore,
    state: &State,
    cfg: &Config,
    profile: &ProfileName,
    tried: &[String],
    lim: &Limited,
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

#[cfg(test)]
#[path = "tests_rerun_unread.rs"]
mod tests_unread;
