//! `amplihack-hooks limit-resume` (issues #1658, #2085): a `StopFailure`
//! hook, registered with matcher `rate_limit` and `asyncRewake: true`, that
//! resumes a Claude Code session stopped by a usage limit once the login
//! rotator (`amplihack profile rotate`) has swapped to a login with room.
//!
//! Order of checks (see [`decide::gate`]): auto-switch on, a limit stop
//! with a usable session id, no `stop_hook_active`, an event not resumed
//! before, a rotator that runs (see
//! [`rotator_status::rotator_down`]). Then the hook takes a snapshot of the
//! swap record (`rotate/swap.json`), arms its watches ([`Deps::arm`]),
//! reads the record once, and blocks in [`Deps::wait_event`] until the
//! rotator writes a new record, the user prompts in the session (see
//! [`record_prompt`], called by the `UserPromptSubmit` hook), the session
//! exits, a watch is lost or `MAX_WAIT` passes. It never polls and never
//! sleeps. A record resumes only when
//! [`judge_at`] says so: different from the snapshot, swapped after the
//! stop, onto a login below 100% on both windows.
//!
//! On a resume the hook re-checks the toggle and moved-on, takes a verdict
//! on its tmux pane ([`keys::PaneCheck`]) and, for a proved pane, on what
//! that pane is showing ([`keys::ScreenCheck`]), and then takes exactly one
//! path (D7). With a pane proved to be its own terminal, showing a screen
//! the keys answer, it records the resume with
//! [`guard::GuardStore::commit`] under the lock, sends Escape, C-u,
//! `continue` and Enter ([`keys::RESUME_KEYS`]) and exits 0
//! (`resumed-keys`). Otherwise it records nothing and exits 2 with
//! `RESUME_MSG` and a newline on stderr. A key that fails stops the rest
//! (`resume-keys-failed`), releases the commit with
//! [`guard::GuardStore::release`] so the event stays armed, and never falls
//! back to exit 2. Stdout is never written.
//!
//! Only a resume that reached the session is recorded (issue #3104). The
//! exit-2 wake cannot answer Claude Code's usage-limit menu, which holds
//! the session's input, so it records nothing and the next room event tries
//! again. The three outcomes that take it are told apart in the log instead
//! of falling through silently (issue #2695): `not-resumed` when no pane
//! was named or found, `pane-unverified` when a pane was named or found but
//! not proved to be the session's own terminal (with the reason in
//! `detail`), and `screen-unrecognised` when the terminal is the session's
//! but it was not showing a screen the keys answer. The last also writes
//! [`UNRECOGNISED_MSG`] before `RESUME_MSG`, so a refusal to type is not
//! left to the log alone. Before a wake the hook reads the guard once more,
//! without the lock, and ends as `duplicate` if another hook has resumed
//! the session with keys since this one started, so it never wakes a
//! session into a running turn.
//!
//! The hook reads only the toggle, the profile-root pointer, the rotate
//! ledger, swap record and pid file, the rotator's `/proc/<pid>/cmdline`,
//! `/proc/<pid>/{stat,fd/0}` of its own ancestry, `$TMUX`, `$TMUX_PANE`,
//! the tmux pane ids and ttys of its server when the environment names no
//! pane, and the visible text of its own pane (all matched and dropped;
//! never logged), and its own resume state; it never opens a login file. Of
//! the transcript it reads only the last entry's `timestamp`, through
//! [`swap_record::read_stop_time`] in `amplihack-utils`, to learn when the
//! session stopped; nothing from it is logged, hashed or echoed.

pub mod decide;
pub mod guard;
pub mod keys;
pub mod log;

use amplihack_types::HookInput;
use amplihack_utils::file_event::{PROMPT_MASK, SWAP_MASK, Waiter, Watch, Woke};
use amplihack_utils::organisation_affinity::Affinity;
use amplihack_utils::rotator_status::swap_record::{
    self, Judgement, SWAP_FILE, SwapRecord, judge_at, rotate_dir, stopped_on_outgoing,
};
use amplihack_utils::rotator_status::{self, CycleStatus, MAX_WAIT};
use amplihack_utils::secure_files::{euid, read_bounded};
use decide::{Facts, Gate, event_key, gate, is_limit, session_id_ok, session_key};
use guard::{CommitError, FsGuard, GuardStore};
use keys::{Key, Pane, PaneCheck, RESUME_KEYS, ScreenCheck};
use log::{LogEntry, LogWriteFailure, reduce_error};
use std::cell::Cell;
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the woken model is told on the exit-2 path. Fixed text: nothing
/// from the payload is echoed into the model's context.
pub const RESUME_MSG: &str = "amplihack: the login rotator reports a login with capacity. \
     Continue the task you were working on; if the usage limit is still in place, stop and wait.";

/// What the woken model is told before [`RESUME_MSG`] when the hook proved
/// the pane but did not recognise what it was showing, so it typed nothing.
/// Fixed text: no part of the capture is echoed into the model's context.
pub const UNRECOGNISED_MSG: &str = "amplihack: limit-resume proved this session's terminal but did not recognise what it was \
     showing, so it typed nothing into it. If a usage-limit prompt is still there, press Esc \
     and continue the session by hand.";

/// Larger hook payloads are ignored.
pub const MAX_STDIN_BYTES: u64 = 1024 * 1024;

/// Hash prefix length used in the log.
const LOG_KEY_CHARS: usize = 12;

/// The `detail` logged when the session exits while keys are sent.
const SESSION_GONE: &str = "session-gone";

/// The `detail` logged with `not-resumed` when no tmux pane was named by
/// the session's environment or found on its terminal, so there was
/// nothing to send keys to. A fixed word, like the refusal reasons in
/// [`keys::UNVERIFIED_REASONS`] but distinct from them: no environment
/// value, pane id, tty or `/proc` text is ever logged.
const NO_PANE: &str = "no-tmux-pane";

/// How one invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new swap record with room was seen and the session given the
    /// exit-2 wake, but no tmux pane was named or found (`detail` is
    /// `no-tmux-pane`). Nothing was typed and nothing was recorded, so the
    /// event stays armed and the next room event tries again. A session
    /// held on the usage-limit menu stays there until then.
    NotResumed,
    /// As `NotResumed`, for a pane, named or found, that the hook could not
    /// prove is the session's own terminal. `detail` carries one of
    /// [`keys::UNVERIFIED_REASONS`].
    PaneUnverified,
    /// As `NotResumed`, for a session whose pane was proved but whose
    /// screen held none of [`keys::SCREEN_MARKERS`], or could not be read at
    /// all. `detail` carries one of [`keys::UNRECOGNISED_REASONS`]. The
    /// wake says so on stderr as well as in the log.
    ScreenUnrecognised,
    /// A new swap record with room was seen, the resume recorded, and the
    /// resume keys were sent to the session's own pane; the hook exits 0.
    /// The only outcome that records a resume.
    ResumedKeys,
    /// A key could not be sent, or the session exited while they were sent.
    /// The commit is released, so the event stays armed. Never followed by
    /// exit 2.
    ResumeKeysFailed,
    /// A watch could not be set up or was lost: the hook cannot see a
    /// swap, so it stops without a resume.
    WatchLost,
    /// Logged only: a new record from before the stop; the wait goes on.
    SwapStale,
    /// Logged only: a new record onto a login with a full window; the wait
    /// goes on.
    SwapNoRoom,
    /// No swap record with room before `MAX_WAIT` passed.
    NoCapacityTimeout,
    /// The user sent a prompt in the session while the hook waited.
    MovedOn,
    /// This limit event was already resumed with keys, or the session was
    /// resumed with keys after this hook started.
    Duplicate,
    /// Logged only (issue #2912): the session's record of earlier resumes
    /// could not be parsed, so it is read as no earlier resumes and the
    /// commit rewrites it. The resume itself goes on and is logged under
    /// its own outcome.
    SessionRecordReset,
    /// The payload had `stop_hook_active: true`.
    StopHookActive,
    /// The Claude Code session that started the hook has exited.
    SessionGone,
    /// No rotator runs, or its ledger cannot be read, when the hook starts:
    /// no swap can come, so the hook does not wait.
    RotatorDown,
    /// The resume state could not be read or written safely. Usually the
    /// run's own line, but it can also be logged alone before the run's own
    /// line: when the read before a wake fails (the session is still woken)
    /// or when a release fails after failed keys (the event stays used).
    GuardFailed,
    /// Auto-switch is off, at the start or when a swap record would have
    /// resumed the session. Always logged through [`Deps::log_existing`],
    /// so a host that never opted in gets no new folder or file (FS-5).
    Disabled,
    /// The stop's `error` is one of [`decide::NOT_LIMIT_ERRORS`], or the
    /// input was not a `StopFailure`. Logged with the reduced `error` value
    /// (issue #3172).
    NotLimit,
    /// The payload's `session_id` was missing, empty or oversized. Logged
    /// with `session=-` (issue #3172).
    BadSessionId,
    /// Standard input was not a readable hook payload of at most
    /// [`MAX_STDIN_BYTES`]. Logged (issue #3172).
    BadPayload,
    /// The hook panicked; the panic was caught and the hook exits 0. One
    /// attempt is made to log it, after the panic has unwound, so no lock is
    /// held (issue #3172).
    Panicked,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotResumed => "not-resumed",
            Self::PaneUnverified => "pane-unverified",
            Self::ScreenUnrecognised => "screen-unrecognised",
            Self::ResumedKeys => "resumed-keys",
            Self::ResumeKeysFailed => "resume-keys-failed",
            Self::WatchLost => "watch-lost",
            Self::SwapStale => "swap-stale",
            Self::SwapNoRoom => "swap-no-room",
            Self::NoCapacityTimeout => "no-capacity-timeout",
            Self::MovedOn => "moved-on",
            Self::Duplicate => "duplicate",
            Self::SessionRecordReset => "session-record-reset",
            Self::StopHookActive => "stop-hook-active",
            Self::SessionGone => "session-gone",
            Self::RotatorDown => "rotator-down",
            Self::GuardFailed => "guard-failed",
            Self::Disabled => "disabled",
            Self::NotLimit => "not-limit",
            Self::BadSessionId => "bad-session-id",
            Self::BadPayload => "bad-payload",
            Self::Panicked => "panicked",
        }
    }

    /// Whether this outcome wakes the session with exit 2 and the
    /// `RESUME_MSG` line: no keys were sent and nothing was recorded. One
    /// predicate drives both, so a new outcome cannot exit 2 without its
    /// message or print the message without exiting 2.
    pub fn wakes(self) -> bool {
        matches!(
            self,
            Self::NotResumed | Self::PaneUnverified | Self::ScreenUnrecognised
        )
    }

    /// 2 for the outcomes that wake the session (see [`Outcome::wakes`]);
    /// everything else, `ResumedKeys` included, is 0.
    pub fn exit_code(self) -> i32 {
        if self.wakes() { 2 } else { 0 }
    }
}

/// What ended one [`Deps::wait_event`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// `swap.json` was written, or the event queue overflowed: re-read it.
    Swap,
    /// A prompt marker was written: check whether the user moved on.
    Prompted,
    /// The Claude Code session exited.
    SessionGone,
    /// A watch is gone, or the wait failed.
    WatchLost,
    /// The lifetime cap passed.
    Deadline,
}

/// Everything `run_with` needs from the outside world.
pub trait Deps {
    /// Whether auto-switch is on for the profile root.
    fn auto_switch_on(&mut self) -> bool;
    /// Epoch seconds.
    fn now(&mut self) -> i64;
    /// The rotate ledger's last cycle, read at `now` (for the event key).
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus>;
    fn guard(&mut self) -> &mut dyn GuardStore;
    /// Records a decision, creating the resume folder when it is missing.
    /// Returns why no line was written; never prints, panics or changes the
    /// outcome. The caller records the first failure and reports it on
    /// stderr when the hook exits 0 (issue #3172).
    fn log(&mut self, now: i64, entry: &LogEntry) -> Result<(), LogWriteFailure>;
    /// As [`Deps::log`], but appends only to a resume folder that already
    /// exists and creates nothing; a missing folder is
    /// [`LogWriteFailure::DirMissing`]. Used for the early exits, so a host
    /// that never opted in to auto-switch is left as it is (FS-5).
    fn log_existing(&mut self, now: i64, entry: &LogEntry) -> Result<(), LogWriteFailure>;
    /// Whether no rotator can record a cycle (see
    /// [`rotator_status::rotator_down`]).
    fn rotator_down(&mut self, now: i64) -> bool;
    /// Sets up the watches on the swap record, the prompt markers and the
    /// session's process. An error means nothing can be waited on.
    fn arm(&mut self) -> io::Result<()>;
    /// Blocks until an event or `deadline` (epoch seconds). Before a
    /// successful [`Deps::arm`] it returns [`Event::WatchLost`].
    fn wait_event(&mut self, deadline: i64) -> Event;
    /// The swap record, read at `now`; `None` when missing or refused.
    fn read_swap(&mut self, now: i64) -> Option<SwapRecord>;
    /// When the session stopped, epoch seconds, from its transcript; `None`
    /// with no transcript or when it is refused. Never logged.
    fn stop_time(&mut self, transcript: Option<&Path>, start: i64) -> Option<i64>;
    /// The verdict on the session's tmux pane, taken now: the pane the
    /// session's environment names, or else the one pane on the session's
    /// terminal (see [`keys::check_pane_owned`]). Every refusal names
    /// itself, so no branch is silent.
    fn pane_check(&mut self) -> PaneCheck;
    /// Whether the proved `pane` is showing a screen the resume keys
    /// answer. Read once, after the pane is proved and before anything is
    /// typed; no screen text is returned.
    fn screen_check(&mut self, pane: &Pane) -> ScreenCheck;
    /// Sends one key to `pane`; an error when it was not delivered.
    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()>;
    /// Whether the session still runs and `holder`, the process the pane
    /// check proved is reading the terminal ([`PaneCheck::Ready`]), is still
    /// that terminal's foreground process group. Checked without blocking,
    /// before each key (S4).
    fn session_alive_now(&mut self, holder: u32) -> bool;
}

/// Why [`send_resume`] stopped.
#[derive(Debug)]
pub struct SendFailed {
    /// Keys delivered before it stopped.
    pub sent: u8,
    /// The send that failed, or the session-gone reason.
    pub err: io::Error,
    /// The session had exited: nothing more was sent.
    pub session_gone: bool,
}

impl SendFailed {
    /// What the log's `detail` shows: `session-gone` or the error kind.
    pub fn detail(&self) -> String {
        if self.session_gone {
            SESSION_GONE.to_string()
        } else {
            format!("{:?}", self.err.kind())
        }
    }
}

/// Sends [`RESUME_KEYS`] to `pane` in order, checking before each that the
/// session still runs and that `holder`, the process the pane check proved
/// is reading the terminal, is still its foreground group and so not
/// suspended (S4). Stops at the first failure (D8); nothing is retried and
/// there is no other way to wake the session.
pub fn send_resume<D: Deps + ?Sized>(
    deps: &mut D,
    pane: &Pane,
    holder: u32,
) -> Result<(), SendFailed> {
    send_resume_affinity(deps, pane, holder, None)
}

/// [`send_resume`], with `continue` replaced by the warning text for
/// `affinity` when the swap record carries one that warns.
fn send_resume_affinity<D: Deps + ?Sized>(
    deps: &mut D,
    pane: &Pane,
    holder: u32,
    affinity: Option<Affinity>,
) -> Result<(), SendFailed> {
    for (sent, key) in (0u8..).zip(RESUME_KEYS) {
        let key = match (key, affinity) {
            (Key::LiteralContinue, Some(Affinity::Crossing)) => Key::LiteralCrossing,
            (Key::LiteralContinue, Some(Affinity::Unresolved)) => Key::LiteralUnresolved,
            _ => key,
        };
        if !deps.session_alive_now(holder) {
            return Err(SendFailed {
                sent,
                err: io::Error::other(SESSION_GONE),
                session_gone: true,
            });
        }
        if let Err(err) = deps.send_keys(pane, key) {
            return Err(SendFailed {
                sent,
                err,
                session_gone: false,
            });
        }
    }
    Ok(())
}

/// What a log line says beyond the keys, the outcome and the ledger at
/// the stop.
#[derive(Default)]
struct Note<'a> {
    swap: Option<&'a SwapRecord>,
    pane: Option<&'a Pane>,
    keys_sent: Option<u8>,
    detail: Option<String>,
}

/// A note naming only the swap record.
fn swap(r: &SwapRecord) -> Note<'_> {
    Note {
        swap: Some(r),
        ..Note::default()
    }
}

/// How this resume reaches the session.
enum Delivery {
    /// The pane is this session's own terminal and it is showing a screen
    /// the keys answer: type into it, while `holder`, the process the pane
    /// check proved is reading it, stays in the foreground.
    Keys { pane: Pane, holder: u32 },
    /// Type nothing. The session gets the exit-2 wake under this outcome,
    /// with this fixed `detail`.
    Wake(Outcome, Option<&'static str>),
}

/// Chooses the path for one resume, before any commit (D7), so a session
/// never gets both keys and an exit-2 wake. Each refusal is its own
/// outcome with its own reason, so the operator can tell "no tmux" from
/// "ownership unproven" from "that is not a screen the keys answer",
/// without reading the pane.
///
/// Nothing is typed until both questions are answered: the pane is this
/// session's terminal, and it is showing a prompt Escape and `continue`
/// are the right answer to. The holder the pane check proved goes with the
/// pane to the check before each key.
fn delivery<D: Deps + ?Sized>(deps: &mut D) -> Delivery {
    match deps.pane_check() {
        PaneCheck::None => Delivery::Wake(Outcome::NotResumed, Some(NO_PANE)),
        PaneCheck::Unverified(reason) => Delivery::Wake(Outcome::PaneUnverified, Some(reason)),
        PaneCheck::Ready { pane, holder } => match deps.screen_check(&pane) {
            ScreenCheck::Held => Delivery::Keys { pane, holder },
            ScreenCheck::Unrecognised(reason) => {
                Delivery::Wake(Outcome::ScreenUnrecognised, Some(reason))
            }
        },
    }
}

/// Whether another hook has resumed this session with keys since this hook
/// started: `event` has a marker, or the session's last resume is later
/// than `start`. These are the two rules [`GuardStore::commit`] applies,
/// read here without the lock and writing nothing, so a wake path refuses
/// only where a commit would also have refused (issue #3104).
fn claimed(guard: &mut dyn GuardStore, event: &str, session: &str, start: i64) -> io::Result<bool> {
    if guard.seen(event)? {
        return Ok(true);
    }
    Ok(guard.last_resume(session)?.is_some_and(|t| t > start))
}

/// The first log line this call could not write: its outcome and why.
type LogFailed = Cell<Option<(Outcome, LogWriteFailure)>>;

/// Whether `outcome` ends a call before the hook has anything to resume, so
/// its line goes only to an existing resume folder and creates nothing.
fn is_early_exit(outcome: Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Disabled
            | Outcome::NotLimit
            | Outcome::BadSessionId
            | Outcome::BadPayload
            | Outcome::Panicked
    )
}

/// Writes `entry` and keeps the first failure in `failed`. A failure never
/// changes control flow.
///
/// The early exits ([`is_early_exit`]) write through [`Deps::log_existing`]
/// and so create no folder. With no resume folder, `disabled` leaves no
/// record at all: a host that never opted in to auto-switch is left as it is
/// (FS-5). Every other early exit with no folder is recorded as
/// `dir_missing`, which reaches stderr.
fn record<D: Deps + ?Sized>(deps: &mut D, now: i64, entry: &LogEntry, failed: &LogFailed) {
    let written = if is_early_exit(entry.outcome) {
        deps.log_existing(now, entry)
    } else {
        deps.log(now, entry)
    };
    let written = match written {
        Err(LogWriteFailure::DirMissing) if entry.outcome == Outcome::Disabled => Ok(()),
        other => other,
    };
    if let Err(reason) = written
        && failed.get().is_none()
    {
        failed.set(Some((entry.outcome, reason)));
    }
}

/// The line for a call that ends before any event exists: `event=-`, no
/// ledger fields, and `session=-` unless the session id is usable.
fn gate_line(session_id: Option<&str>, outcome: Outcome, error: String) -> LogEntry {
    let session = match session_id.filter(|s| session_id_ok(Some(s))) {
        Some(id) => session_key(id)[..LOG_KEY_CHARS].to_string(),
        None => "-".to_string(),
    };
    LogEntry {
        session,
        event: "-".to_string(),
        outcome,
        live: None,
        cycle: None,
        swap_seq: None,
        swap_live: None,
        pane: None,
        keys_sent: None,
        resumes: None,
        detail: None,
        error,
    }
}

/// Decides one hook invocation.
pub fn run_with<D: Deps + ?Sized>(input: &HookInput, deps: &mut D) -> Outcome {
    run_with_affinity(input, deps, &mut None, &Cell::new(None))
}

/// [`run_with`], also setting `affinity` to the organisation outcome of the
/// swap record this resume follows, and `log_failed` to the first line that
/// could not be written. Only a resume reads `affinity`; every other outcome
/// leaves it `None`.
///
/// Every call writes at least one line (issue #3172), the gate exits
/// included. Nothing is logged while this process holds the guard lock:
/// lines are written only after `commit` and `release` have returned, since
/// the log takes the same lock file and would otherwise wait on itself.
fn run_with_affinity<D: Deps + ?Sized>(
    input: &HookInput,
    deps: &mut D,
    affinity: &mut Option<Affinity>,
    log_failed: &LogFailed,
) -> Outcome {
    let enabled = deps.auto_switch_on();
    let HookInput::StopFailure {
        session_id,
        error,
        error_details,
        last_assistant_message,
        stop_hook_active,
        transcript_path,
        ..
    } = input
    else {
        let outcome = if enabled {
            Outcome::NotLimit
        } else {
            Outcome::Disabled
        };
        let now = deps.now();
        record(
            deps,
            now,
            &gate_line(None, outcome, String::new()),
            log_failed,
        );
        return outcome;
    };
    let (error, details, message) = (
        error.as_deref(),
        error_details.as_deref(),
        last_assistant_message.as_deref(),
    );
    // The only form of `error` that reaches a line: `[a-z_]`, never text.
    let error_field = reduce_error(error);
    let mut facts = Facts {
        enabled,
        limit: is_limit(error),
        session_ok: session_id_ok(session_id.as_deref()),
        stop_hook_active: *stop_hook_active == Some(true),
        duplicate: false,
    };
    let gate_exit = match gate(&facts) {
        Gate::Disabled => Some((Outcome::Disabled, session_id.as_deref())),
        Gate::NotLimit => Some((Outcome::NotLimit, session_id.as_deref())),
        Gate::BadSessionId => Some((Outcome::BadSessionId, None)),
        _ => None,
    };
    if let Some((outcome, session)) = gate_exit {
        let now = deps.now();
        let entry = gate_line(session, outcome, error_field);
        record(deps, now, &entry, log_failed);
        return outcome;
    }

    let session_id = session_id.as_deref().unwrap_or_default();
    let start = deps.now();
    let first = deps.read_cycle(start);
    let live_at_start = first.as_ref().and_then(|s| s.live.as_deref());
    let session = session_key(session_id);
    let stop_hook = gate(&facts) == Gate::StopHookActive;
    // D4: the record seen at the stop resumes only when it may itself be
    // after the stop (a swap between the stop and this read); otherwise only
    // a different one, newer than the stop, does. See `noted_after_stop`.
    // With `stop_hook_active` the hook never waits, so it reads neither
    // the transcript nor any guard state.
    let (stop, snapshot, resumed) = if stop_hook {
        (None, None, Ok(None))
    } else {
        let stop = deps.stop_time(transcript_path.as_deref(), start);
        let snapshot = deps.read_swap(start);
        // A swap at or before this session's last resume is never after
        // its stop, however close the times (see `after_stop_at`).
        (stop, snapshot, deps.guard().last_resume(&session))
    };
    // The login the session stopped on is part of the event: the same login
    // limited again is the same event, a limit after a swap is a new one.
    // When a swap noted at the start falls after the stop, the session
    // stopped on its outgoing login, not on the live one (#2235). With a
    // stop time the bound is `stop - 1`, which depends on neither this
    // hook's start nor past resumes, so every delivery of the same stop gets
    // the same key and a repeat is a duplicate (see `stopped_on_outgoing`).
    let key_login = match (&snapshot, &resumed) {
        (Some(r), Ok(resumed_at)) if stopped_on_outgoing(r, start, stop, *resumed_at) => {
            Some(r.from_login.as_str())
        }
        _ => live_at_start,
    };
    let event = event_key(session_id, error, details, message, key_login);
    // Evidence for every line logged after the session record is read
    // (#2912). A `Cell` so the logging closure below reads the latest value
    // without taking the count as an argument at each call.
    let resumes: Cell<Option<u32>> = Cell::new(None);
    let note = |deps: &mut D, now: i64, outcome: Outcome, n: Note<'_>| {
        let entry = LogEntry {
            session: session[..LOG_KEY_CHARS].to_string(),
            event: event[..LOG_KEY_CHARS].to_string(),
            outcome,
            live: first.as_ref().and_then(|s| s.live.clone()),
            cycle: first.as_ref().and_then(|s| s.outcome.clone()),
            swap_seq: n.swap.map(|r| r.seq),
            swap_live: n.swap.map(|r| r.live_login.clone()),
            pane: n.pane.map(Pane::log_tag),
            keys_sent: n.keys_sent,
            resumes: resumes.get(),
            detail: n.detail,
            error: error_field.clone(),
        };
        record(deps, now, &entry, log_failed);
        outcome
    };
    let finish =
        |deps: &mut D, now: i64, outcome: Outcome| note(deps, now, outcome, Note::default());

    if stop_hook {
        return finish(deps, start, Outcome::StopHookActive);
    }
    // Checked here so a duplicate event exits at once; checked again before
    // a wake and under the lock by `commit`.
    match deps.guard().seen(&event) {
        Ok(seen) => facts.duplicate = seen,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    }
    // Read for the log only: no count refuses a resume (#2912).
    let record = match deps.guard().resumes(&session, start) {
        Ok(r) => r,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    };
    resumes.set(Some(record.count));
    if record.malformed {
        // Loud and at once, so an operator sees the damaged record even if
        // the wait that follows ends some other way. The commit rewrites
        // the file; nothing of its contents is logged.
        note(deps, start, Outcome::SessionRecordReset, Note::default());
    }
    let resumed_at = match resumed {
        Ok(t) => t,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    };
    if gate(&facts) == Gate::Duplicate {
        return finish(deps, start, Outcome::Duplicate);
    }
    if deps.rotator_down(start) {
        return finish(deps, start, Outcome::RotatorDown);
    }

    // Lets the prompt hook record a prompt even during the first-ever wait,
    // and makes the `prompted/` directory the hook watches.
    if deps.guard().prepare(start).is_err() {
        return finish(deps, start, Outcome::GuardFailed);
    }
    // A prompt sent after the hook started means the session went on
    // without it. The hook's own wake is not a prompt, so it never counts.
    let moved_on = |deps: &mut D| {
        deps.guard()
            .prompted_at(&session)
            .map(|t| t.is_some_and(|t| t > start))
    };

    if let Err(e) = deps.arm() {
        let n = Note {
            detail: Some(format!("{:?}", e.kind())),
            ..Note::default()
        };
        return note(deps, start, Outcome::WatchLost, n);
    }
    let deadline = start.saturating_add(MAX_WAIT.as_secs() as i64);
    // The last record logged as stale or full, so a rewrite of it is not
    // logged again.
    let mut logged: Option<SwapRecord> = None;
    // The record is read once after arming, before any wait (D10): a swap
    // written before the watch existed is not missed.
    let mut woke = Event::Swap;
    let (now, record) = loop {
        let now = deps.now();
        match woke {
            Event::Swap => {
                if let Some(r) = deps.read_swap(now) {
                    let skipped = match judge_at(snapshot.as_ref(), &r, start, stop, resumed_at) {
                        Judgement::Resume => break (now, r),
                        Judgement::Same => None,
                        Judgement::Stale => Some(Outcome::SwapStale),
                        Judgement::NoRoom => Some(Outcome::SwapNoRoom),
                    };
                    if let Some(outcome) = skipped
                        && logged.as_ref() != Some(&r)
                    {
                        note(deps, now, outcome, swap(&r));
                        logged = Some(r);
                    }
                }
            }
            Event::Prompted => match moved_on(deps) {
                Ok(false) => {}
                Ok(true) => return finish(deps, now, Outcome::MovedOn),
                Err(_) => return finish(deps, now, Outcome::GuardFailed),
            },
            Event::SessionGone => return finish(deps, now, Outcome::SessionGone),
            Event::WatchLost => return finish(deps, now, Outcome::WatchLost),
            Event::Deadline => return finish(deps, now, Outcome::NoCapacityTimeout),
        }
        woke = deps.wait_event(deadline);
    };

    if !deps.auto_switch_on() {
        return note(deps, now, Outcome::Disabled, swap(&record));
    }
    match moved_on(deps) {
        Ok(false) => {}
        Ok(true) => return note(deps, now, Outcome::MovedOn, swap(&record)),
        Err(_) => return note(deps, now, Outcome::GuardFailed, swap(&record)),
    }
    // D7: the path is chosen first, so a session never gets both keys and
    // an exit-2 wake. Only the keys path records a resume (#3104).
    let (pane, holder) = match delivery(deps) {
        Delivery::Keys { pane, holder } => (pane, holder),
        Delivery::Wake(outcome, reason) => {
            // The wake reaches no session held on the menu, so nothing is
            // recorded and the event stays armed for the next room event.
            // A session another hook has just resumed with keys is not
            // woken into its running turn.
            match claimed(deps.guard(), &event, &session, start) {
                Ok(false) => {}
                Ok(true) => return note(deps, now, Outcome::Duplicate, swap(&record)),
                // Logged alone, and the session is still woken: a wake
                // types and records nothing, so it cannot repeat a resume
                // with keys, and leaving the session stopped is worse.
                Err(_) => {
                    note(deps, now, Outcome::GuardFailed, swap(&record));
                }
            }
            // The swap that woke this wait carries its own organisation
            // outcome (#2136). Nothing else is consulted.
            *affinity = record.affinity.filter(|a| a.warning().is_some());
            let n = Note {
                detail: reason.map(str::to_string),
                ..swap(&record)
            };
            return note(deps, now, outcome, n);
        }
    };
    // Claimed before the first key, so only one hook types into a session
    // for an event.
    let committed_at = now;
    match deps.guard().commit(&event, &session, start, committed_at) {
        Ok(()) => {}
        Err(e) => {
            let outcome = match e {
                CommitError::Duplicate => Outcome::Duplicate,
                CommitError::Io => Outcome::GuardFailed,
            };
            return note(deps, now, outcome, swap(&record));
        }
    }
    *affinity = record.affinity.filter(|a| a.warning().is_some());
    let (outcome, keys_sent, detail) = match send_resume_affinity(deps, &pane, holder, *affinity) {
        Ok(()) => (Outcome::ResumedKeys, RESUME_KEYS.len() as u8, None),
        Err(f) => (Outcome::ResumeKeysFailed, f.sent, Some(f.detail())),
    };
    // Keys that did not all arrive are no resume: the claim is taken back
    // before any line is logged, so nothing is logged under the guard lock.
    let released = outcome != Outcome::ResumeKeysFailed
        || deps.guard().release(&event, &session, committed_at).is_ok();
    let now = deps.now();
    if !released {
        // The event stays used, as it would have before #3104.
        note(deps, now, Outcome::GuardFailed, swap(&record));
    }
    let n = Note {
        swap: Some(&record),
        pane: Some(&pane),
        keys_sent: Some(keys_sent),
        detail,
    };
    note(deps, now, outcome, n)
}

/// Reads at most `MAX_STDIN_BYTES` of payload from `stdin`, decides, and
/// writes `RESUME_MSG` to `stderr` for every outcome [`Outcome::wakes`]
/// holds for, preceded by the swap record's organisation warning when it
/// has one (#2136), and, for `ScreenUnrecognised` alone, by
/// [`UNRECOGNISED_MSG`], so a refusal to type reaches the model and not
/// only the log. These wakes record nothing (#3104). On `ResumedKeys` the
/// warning went with the keys instead. Returns the exit code: 2 for those
/// outcomes, 0 otherwise, including after keys were sent, for an oversized
/// or malformed payload (`bad-payload`) and for a panic (`panicked`).
///
/// When a log line could not be written and the exit code is 0, one fixed
/// line naming the outcome and the [`LogWriteFailure`] token goes to stderr
/// (issue #3172). On exit 2 stderr goes to the model, so it carries the wake
/// text alone.
pub fn run_with_io<D: Deps + ?Sized>(
    stdin: impl Read,
    stderr: &mut dyn Write,
    deps: &mut D,
) -> i32 {
    let mut affinity = None;
    let log_failed: LogFailed = Cell::new(None);
    let decided = catch_unwind(AssertUnwindSafe(|| {
        let raw = read_bounded(stdin, MAX_STDIN_BYTES).ok()?;
        if raw.len() as u64 > MAX_STDIN_BYTES {
            return None;
        }
        let input: HookInput = serde_json::from_slice(&raw).ok()?;
        Some(run_with_affinity(&input, deps, &mut affinity, &log_failed))
    }));
    // Logged here, after `catch_unwind` has returned, so an unwound panic
    // holds no lock. One attempt; a second panic is caught and ends here.
    let late = match decided {
        Ok(None) => Some(Outcome::BadPayload),
        Err(_) => Some(Outcome::Panicked),
        Ok(Some(_)) => None,
    };
    if let Some(outcome) = late {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let now = deps.now();
            record(
                deps,
                now,
                &gate_line(None, outcome, String::new()),
                &log_failed,
            );
        }));
    }
    let code = exit_and_wake(decided, affinity, stderr);
    if code == 0
        && let Some((outcome, reason)) = log_failed.get()
    {
        let _ = writeln!(
            stderr,
            "amplihack limit-resume: the resume.log line for outcome={} was not written \
             (reason={}); see resume/log-write-failed",
            outcome.as_str(),
            reason.as_str()
        );
        let _ = stderr.flush();
    }
    code
}

/// The exit code for `decided`, writing the wake text to `stderr` for the
/// outcomes that wake the session.
fn exit_and_wake(
    decided: std::thread::Result<Option<Outcome>>,
    affinity: Option<Affinity>,
    stderr: &mut dyn Write,
) -> i32 {
    match decided {
        Ok(Some(outcome)) if outcome.wakes() => {
            // Exit 2 alone wakes the session, so a failed write still
            // exits 2. Nothing was recorded for this wake.
            if outcome == Outcome::ScreenUnrecognised {
                let _ = writeln!(stderr, "{UNRECOGNISED_MSG}");
            }
            if let Some(text) = affinity.and_then(Affinity::warning) {
                let _ = writeln!(stderr, "{text}");
            }
            let _ = writeln!(stderr, "{RESUME_MSG}");
            let _ = stderr.flush();
            outcome.exit_code()
        }
        Ok(Some(outcome)) => outcome.exit_code(),
        Ok(None) | Err(_) => 0,
    }
}

/// Whether `dir` may be watched (S2): a real directory, not a symlink,
/// owned by the current user and not writable by the group or others.
/// Never changes anything.
pub fn check_watch_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(dir)?;
    if m.file_type().is_symlink() || !m.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "is not a real directory",
        ));
    }
    if m.uid() != euid() || m.mode() & 0o022 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "is not private to the current user",
        ));
    }
    Ok(())
}

/// The prompt markers' directory, `<root>/.auto-switch.d/resume/prompted`.
fn prompted_dir(root: &Path) -> PathBuf {
    root.join(".auto-switch.d").join("resume").join("prompted")
}

impl SystemDeps {
    /// Leaves the fixed-name `log-write-failed` marker for a line that was
    /// not written; [`log::mark_failure`] writes only into a private resume
    /// folder. Returns `written` unchanged.
    fn mark(
        &self,
        now: i64,
        outcome: Outcome,
        written: Result<(), LogWriteFailure>,
    ) -> Result<(), LogWriteFailure> {
        if let Err(reason) = written {
            log::mark_failure(&self.guard.resume_dir(), now, outcome, reason);
        }
        written
    }
}

/// The real environment for a resolved profile root.
struct SystemDeps {
    root: PathBuf,
    guard: FsGuard,
    /// The hook's parent when it started: the process through which Claude
    /// Code ran the hook command, not Claude Code itself (#3156). Its pidfd
    /// ends the wait when it exits. The pane check does not consult it.
    parent: u32,
    /// `$TMUX_PANE` when the hook started, if it is a pane id under `$TMUX`.
    /// When it is `None`, [`Deps::pane_check`] looks the pane up by the
    /// session's terminal instead; either pane is proved to be this
    /// session's terminal before anything is typed.
    pane: Option<Pane>,
    /// Set by [`Deps::arm`].
    waiter: Option<Waiter>,
}

impl Deps for SystemDeps {
    fn auto_switch_on(&mut self) -> bool {
        rotator_status::auto_switch_on(&self.root)
    }

    fn now(&mut self) -> i64 {
        epoch_now()
    }

    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus> {
        rotator_status::read_cycle(&self.root, now)
    }

    fn guard(&mut self) -> &mut dyn GuardStore {
        &mut self.guard
    }

    fn log(&mut self, now: i64, entry: &LogEntry) -> Result<(), LogWriteFailure> {
        let written = match self.guard.ensure_resume_dir() {
            Ok(dir) => log::append(&dir, &log::format_line(now, entry)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(LogWriteFailure::DirMissing),
            Err(_) => Err(LogWriteFailure::DirUnprivate),
        };
        self.mark(now, entry.outcome, written)
    }

    fn log_existing(&mut self, now: i64, entry: &LogEntry) -> Result<(), LogWriteFailure> {
        let written = match self.guard.existing_resume_dir() {
            // Never created here (FS-5); `record` decides what a missing
            // folder means for the outcome.
            Ok(None) => Err(LogWriteFailure::DirMissing),
            Ok(Some(dir)) => log::append(&dir, &log::format_line(now, entry)),
            Err(_) => Err(LogWriteFailure::DirUnprivate),
        };
        self.mark(now, entry.outcome, written)
    }

    fn rotator_down(&mut self, now: i64) -> bool {
        rotator_status::rotator_down(&self.root, now)
    }

    fn arm(&mut self) -> io::Result<()> {
        let rotate = rotate_dir(&self.root);
        let prompted = prompted_dir(&self.root);
        check_watch_dir(&rotate)?;
        check_watch_dir(&prompted)?;
        // S5: init adopts orphans, so a parent of 1 is no session.
        if self.parent == 1 {
            return Err(io::Error::other("the session has already exited"));
        }
        let watches = [
            Watch {
                dir: rotate,
                name: Some(SWAP_FILE),
                mask: SWAP_MASK,
            },
            Watch {
                dir: prompted,
                name: None,
                mask: PROMPT_MASK,
            },
        ];
        let waiter = Waiter::new(&watches, self.parent)?;
        // S5: the pidfd names the session only while it is still the
        // parent; checked after it is open, so a reused pid cannot pass.
        if std::os::unix::process::parent_id() != self.parent {
            return Err(io::Error::other("the session has already exited"));
        }
        self.waiter = Some(waiter);
        Ok(())
    }

    fn wait_event(&mut self, deadline: i64) -> Event {
        let Some(waiter) = self.waiter.as_mut() else {
            return Event::WatchLost;
        };
        // Capped at MAX_WAIT: a clock set back must not stretch the wait
        // past the hook's own timeout.
        let left = u64::try_from(deadline.saturating_sub(epoch_now()))
            .unwrap_or(0)
            .min(MAX_WAIT.as_secs());
        // A deadline the clock cannot represent cannot be waited on.
        let Some(until) = Instant::now().checked_add(Duration::from_secs(left)) else {
            return Event::WatchLost;
        };
        match waiter.wait(until) {
            Ok(Woke::File(1)) => Event::Prompted,
            // An overflow may have dropped a write of the record.
            Ok(Woke::File(_) | Woke::Overflow) => Event::Swap,
            Ok(Woke::ProcessGone) => Event::SessionGone,
            Ok(Woke::Deadline) => Event::Deadline,
            Ok(Woke::Lost(_)) | Err(_) => Event::WatchLost,
        }
    }

    fn read_swap(&mut self, now: i64) -> Option<SwapRecord> {
        swap_record::read_swap(&self.root, now)
    }

    fn stop_time(&mut self, transcript: Option<&Path>, start: i64) -> Option<i64> {
        transcript.and_then(|p| swap_record::read_stop_time(p, start))
    }

    fn pane_check(&mut self) -> PaneCheck {
        keys::check_pane_owned(self.pane.as_ref())
    }

    fn screen_check(&mut self, pane: &Pane) -> ScreenCheck {
        keys::screen_of(pane)
    }

    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()> {
        keys::tmux_send(pane, key)
    }

    fn session_alive_now(&mut self, holder: u32) -> bool {
        match &self.waiter {
            // S4: alive, and the holder the pane check proved is still in
            // the foreground, so a Ctrl-Z between keys stops the rest from
            // reaching the shell. The hook's parent is never the terminal's
            // foreground group (#3156), so it is not the process asked.
            Some(w) => w.process_alive() && keys::foreground_now(holder),
            // Without the pidfd the session's liveness is unproven (a pid
            // can be reused), so no key is sent.
            None => false,
        }
    }
}

/// Notes a user prompt in `session_id` for a waiting `limit-resume` hook.
/// Called by the `UserPromptSubmit` hook with the resolved profile root;
/// silent, never fails, takes no lock, and writes nothing unless
/// `<root>/.auto-switch.d/resume/` already exists and is private.
pub fn record_prompt(root: &Path, session_id: Option<&str>) {
    record_prompt_at(root, session_id, epoch_now());
}

/// [`record_prompt`] for a given profile root and time.
pub fn record_prompt_at(root: &Path, session_id: Option<&str>, now: i64) {
    if let Some(id) = session_id.filter(|id| session_id_ok(Some(id))) {
        let _ = FsGuard::new(root).record_prompt(&session_key(id), now);
    }
}

fn epoch_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The `limit-resume` subcommand. Exits 0 at once, without reading stdin,
/// when the profile root cannot be resolved safely, with one fixed line on
/// stderr, the only record such a call can leave (issue #3172). Panics are
/// silenced and become exit 0.
pub fn run(stdin: impl Read) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    let Ok(Some(root)) = catch_unwind(rotator_status::resolve_root) else {
        let _ = writeln!(
            std::io::stderr().lock(),
            "amplihack limit-resume: the profile root could not be resolved \
             (reason=no_profile_root); no resume.log line was written"
        );
        return 0;
    };
    let var = |k: &str| std::env::var(k).ok();
    let mut deps = SystemDeps {
        guard: FsGuard::new(&root),
        root,
        parent: std::os::unix::process::parent_id(),
        pane: Pane::parse(var("TMUX").as_deref(), var("TMUX_PANE").as_deref()),
        waiter: None,
    };
    run_with_io(stdin, &mut std::io::stderr().lock(), &mut deps)
}
