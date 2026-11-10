//! `amplihack-hooks limit-resume` (issues #1658, #2085): a `StopFailure`
//! hook, registered with matcher `rate_limit` and `asyncRewake: true`, that
//! resumes a Claude Code session stopped by a usage limit once the login
//! rotator (`amplihack profile rotate`) has swapped to a login with room.
//!
//! Order of checks (see [`decide::gate`]): auto-switch on, a limit stop
//! with a usable session id, no `stop_hook_active`, an event not resumed
//! before, a session under its cap, a rotator that runs (see
//! [`rotator_status::rotator_down`]). Then the hook takes a snapshot of the
//! swap record (`rotate/swap.json`), arms its watches ([`Deps::arm`]),
//! reads the record once, and blocks in [`Deps::wait_event`] until the
//! rotator writes a new record, the user prompts in the session (see
//! [`record_prompt`], called by the `UserPromptSubmit` hook), the session
//! exits, a watch is lost or `MAX_WAIT` passes. It never polls and never
//! sleeps. A record resumes only when
//! [`judge`] says so: different from the snapshot, swapped after the stop, onto a login below 100% on both windows.
//!
//! On a resume the hook re-checks the toggle and moved-on, looks for a
//! verified tmux pane, records the resume with [`guard::GuardStore::commit`]
//! under the lock, and then takes exactly one path (D7): with a pane it
//! sends Escape, C-u, `continue` and Enter ([`keys::RESUME_KEYS`]) and exits
//! 0 (`resumed-keys`); without one it exits 2 with exactly `RESUME_MSG` and
//! a newline on stderr (`resumed`). A key that fails stops the rest
//! (`resume-keys-failed`) and never falls back to exit 2. Stdout is never
//! written.
//!
//! The hook reads only the toggle, the profile-root pointer, the rotate
//! ledger, swap record and pid file, the rotator's `/proc/<pid>/cmdline`,
//! Claude Code's `/proc/<pid>/{stat,fd/0}`, `$TMUX` and `$TMUX_PANE`, and
//! its own resume state; it never opens a login file or the transcript.

pub mod decide;
pub mod guard;
pub mod keys;
pub mod log;

use amplihack_types::HookInput;
use amplihack_utils::file_event::{PROMPT_MASK, SWAP_MASK, Waiter, Watch, Woke};
use amplihack_utils::rotator_status::swap_record::{
    Judgement, SWAP_FILE, SwapRecord, judge, rotate_dir,
};
use amplihack_utils::rotator_status::{self, CycleStatus, MAX_WAIT};
use amplihack_utils::secure_files::{euid, read_bounded};
use decide::{Facts, Gate, event_key, gate, is_limit, session_id_ok, session_key};
use guard::{CommitError, FsGuard, GuardStore};
use keys::{Key, Pane, RESUME_KEYS};
use log::{Diagnostic, ErrorObservation, LogEntry, Stage};
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What the woken model is told on the exit-2 path. Fixed text: nothing
/// from the payload is echoed into the model's context.
pub const RESUME_MSG: &str = "amplihack: the login rotator reports a login with capacity. \
     Continue the task you were working on; if the usage limit is still in place, stop and wait.";

/// Resumes allowed per session in any 24 hours.
pub const MAX_RESUMES_PER_SESSION: u32 = 3;

/// Larger hook payloads are ignored.
pub const MAX_STDIN_BYTES: u64 = 1024 * 1024;

/// Hash prefix length used in the log.
const LOG_KEY_CHARS: usize = 12;

/// The `detail` logged when the session exits while keys are sent.
const SESSION_GONE: &str = "session-gone";

/// How one invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new swap record with room was seen, the resume recorded, and the
    /// session woken with exit 2 (no verified tmux pane).
    Resumed,
    /// As `Resumed`, but the resume keys were sent to the session's pane
    /// and the hook exits 0.
    ResumedKeys,
    /// The resume was recorded but a key could not be sent, or the session
    /// exited while they were sent. Never followed by exit 2.
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
    /// This limit event was already resumed.
    Duplicate,
    /// The session reached its resume cap.
    Cap,
    /// The payload had `stop_hook_active: true`.
    StopHookActive,
    /// The Claude Code session that started the hook has exited.
    SessionGone,
    /// No rotator runs, or its ledger cannot be read, when the hook starts:
    /// no swap can come, so the hook does not wait.
    RotatorDown,
    /// The resume state could not be read or written safely.
    GuardFailed,
    /// Auto-switch is off. Outcome records are written when turned off while
    /// waiting.
    Disabled,
    /// Not a usage-limit stop, or no usable session id. Receipt/filter records only.
    NotLimit,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resumed => "resumed",
            Self::ResumedKeys => "resumed-keys",
            Self::ResumeKeysFailed => "resume-keys-failed",
            Self::WatchLost => "watch-lost",
            Self::SwapStale => "swap-stale",
            Self::SwapNoRoom => "swap-no-room",
            Self::NoCapacityTimeout => "no-capacity-timeout",
            Self::MovedOn => "moved-on",
            Self::Duplicate => "duplicate",
            Self::Cap => "cap",
            Self::StopHookActive => "stop-hook-active",
            Self::SessionGone => "session-gone",
            Self::RotatorDown => "rotator-down",
            Self::GuardFailed => "guard-failed",
            Self::Disabled => "disabled",
            Self::NotLimit => "not-limit",
        }
    }

    /// 2 wakes the session (`asyncRewake`), only on `Resumed`; everything
    /// else, `ResumedKeys` included, is 0.
    pub fn exit_code(self) -> i32 {
        if self == Self::Resumed { 2 } else { 0 }
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
    /// Records a decision; must never fail or print.
    fn log(&mut self, now: i64, entry: &LogEntry);
    /// Best-effort lifecycle diagnostics; existing test dependencies may ignore them.
    fn diagnostic(&mut self, _now: i64, _entry: &Diagnostic) {}
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
    /// The session's tmux pane, only when it passes the pane check now.
    fn pane(&mut self) -> Option<Pane>;
    /// Sends one key to `pane`; an error when it was not delivered.
    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()>;
    /// Whether the session still runs and is in its terminal's foreground,
    /// checked without blocking.
    fn session_alive_now(&mut self) -> bool;
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
/// session still runs and is not suspended (S4), and that the same pane still
/// belongs to the session. Stops at the first failure
/// (D8); nothing is retried and there is no other way to wake the session.
pub fn send_resume<D: Deps + ?Sized>(deps: &mut D, pane: &Pane) -> Result<(), SendFailed> {
    for (sent, key) in (0u8..).zip(RESUME_KEYS) {
        if !deps.session_alive_now() {
            return Err(SendFailed {
                sent,
                err: io::Error::other(SESSION_GONE),
                session_gone: true,
            });
        }
        if deps.pane().as_ref() != Some(pane) {
            return Err(SendFailed {
                sent,
                err: io::Error::new(io::ErrorKind::PermissionDenied, "pane ownership changed"),
                session_gone: false,
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

/// Decides one hook invocation.
pub fn run_with<D: Deps + ?Sized>(input: &HookInput, deps: &mut D) -> Outcome {
    let start = deps.now();
    run_started(input, deps, start, &invocation_key())
}

// Independent of payload data; the counter distinguishes calls within one process,
// and pid/time distinguish separate hook processes and process-id reuse.
fn invocation_key() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{:x}-{:x}-{:x}",
        std::process::id(),
        nanos,
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn diagnostic<D: Deps + ?Sized>(
    deps: &mut D,
    now: i64,
    invocation: &str,
    session: &Option<String>,
    stage: Stage,
    disposition: &'static str,
) {
    deps.diagnostic(
        now,
        &Diagnostic {
            invocation: invocation.to_string(),
            session: session.clone(),
            event: None,
            stage,
            disposition,
            error: None,
        },
    );
}

fn run_started<D: Deps + ?Sized>(
    input: &HookInput,
    deps: &mut D,
    start: i64,
    invocation: &str,
) -> Outcome {
    let enabled = deps.auto_switch_on();
    let HookInput::StopFailure {
        session_id,
        error,
        error_details,
        last_assistant_message,
        stop_hook_active,
        ..
    } = input
    else {
        return if enabled {
            Outcome::NotLimit
        } else {
            Outcome::Disabled
        };
    };
    let (error, details, message) = (
        error.as_deref(),
        error_details.as_deref(),
        last_assistant_message.as_deref(),
    );
    let mut facts = Facts {
        enabled,
        limit: is_limit(error),
        session_ok: session_id_ok(session_id.as_deref()),
        stop_hook_active: *stop_hook_active == Some(true),
        duplicate: false,
        capped: false,
    };
    match gate(&facts) {
        Gate::Disabled => return Outcome::Disabled,
        Gate::NotLimit => return Outcome::NotLimit,
        _ => {}
    }

    let session_id = session_id.as_deref().unwrap_or_default();
    // The live login at the limit is part of the event: the same login
    // limited again is the same event, a limit after a swap is a new one.
    let first = deps.read_cycle(start);
    let live_at_start = first.as_ref().and_then(|s| s.live.as_deref());
    let event = event_key(session_id, error, details, message, live_at_start);
    let session = session_key(session_id);
    let lifecycle = |deps: &mut D, now: i64, stage: Stage, disposition: &'static str| {
        deps.diagnostic(
            now,
            &Diagnostic {
                invocation: invocation.to_string(),
                session: Some(session[..LOG_KEY_CHARS].to_string()),
                event: Some(event[..LOG_KEY_CHARS].to_string()),
                stage,
                disposition,
                error: None,
            },
        );
    };
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
            detail: n.detail,
        };
        deps.log(now, &entry);
        outcome
    };
    let finish =
        |deps: &mut D, now: i64, outcome: Outcome| note(deps, now, outcome, Note::default());

    if gate(&facts) == Gate::StopHookActive {
        return finish(deps, start, Outcome::StopHookActive);
    }
    // Checked here so a duplicate or capped event exits at once; checked
    // again under the lock by `commit`.
    match deps.guard().seen(&event) {
        Ok(seen) => facts.duplicate = seen,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    }
    match deps.guard().resumes(&session, start) {
        Ok(n) => facts.capped = n >= MAX_RESUMES_PER_SESSION,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    }
    // A swap at or before this session's last resume is never after its
    // stop, however close the times (see `after_stop`).
    let resumed_at = match deps.guard().last_resume(&session) {
        Ok(t) => t,
        Err(_) => return finish(deps, start, Outcome::GuardFailed),
    };
    match gate(&facts) {
        Gate::Duplicate => return finish(deps, start, Outcome::Duplicate),
        Gate::Cap => return finish(deps, start, Outcome::Cap),
        _ => {}
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

    // D4: the record seen at the stop resumes only when it may itself be
    // after the stop (a swap between the stop and this read); otherwise only
    // a different one, newer than the stop, does. See `judge`.
    let snapshot = deps.read_swap(start);
    if let Err(e) = deps.arm() {
        let n = Note {
            detail: Some(format!("{:?}", e.kind())),
            ..Note::default()
        };
        return note(deps, start, Outcome::WatchLost, n);
    }
    lifecycle(deps, start, Stage::WaitingArmed, "armed");
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
                    let judgement = judge(snapshot.as_ref(), &r, start, resumed_at);
                    lifecycle(
                        deps,
                        now,
                        Stage::SwapJudged,
                        match judgement {
                            Judgement::Resume => "eligible",
                            Judgement::Same => "same",
                            Judgement::Stale => "stale",
                            Judgement::NoRoom => "no-room",
                        },
                    );
                    let skipped = match judgement {
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
    // D7: the path is chosen before the commit, so a session never gets
    // both keys and an exit-2 wake.
    let pane = deps.pane();
    match deps.guard().commit(&event, &session, start, now) {
        Ok(()) => {}
        Err(e) => {
            let outcome = match e {
                CommitError::Duplicate => Outcome::Duplicate,
                CommitError::Cap => Outcome::Cap,
                CommitError::Io => Outcome::GuardFailed,
            };
            return note(deps, now, outcome, swap(&record));
        }
    }
    lifecycle(
        deps,
        now,
        Stage::ContinuationAttempt,
        if pane.is_some() { "keys" } else { "exit-two" },
    );
    let Some(pane) = pane else {
        lifecycle(deps, now, Stage::ContinuationResult, "exit-two-selected");
        return note(deps, now, Outcome::Resumed, swap(&record));
    };
    let (outcome, keys_sent, detail) = match send_resume(deps, &pane) {
        Ok(()) => (Outcome::ResumedKeys, RESUME_KEYS.len() as u8, None),
        Err(f) => (Outcome::ResumeKeysFailed, f.sent, Some(f.detail())),
    };
    let n = Note {
        swap: Some(&record),
        pane: Some(&pane),
        keys_sent: Some(keys_sent),
        detail,
    };
    let now = deps.now();
    lifecycle(deps, now, Stage::ContinuationResult, outcome.as_str());
    note(deps, now, outcome, n)
}

/// Reads at most `MAX_STDIN_BYTES` of payload from `stdin`, decides, and
/// writes `RESUME_MSG` to `stderr` only on `Resumed`. Returns the exit code:
/// 2 on `Resumed`, 0 otherwise, including after keys were sent, for an
/// oversized or malformed payload and for a panic.
pub fn run_with_io<D: Deps + ?Sized>(
    stdin: impl Read,
    stderr: &mut dyn Write,
    deps: &mut D,
) -> i32 {
    let decided = catch_unwind(AssertUnwindSafe(|| {
        // Capture before any diagnostics: log latency must not shift the stop.
        let start = deps.now();
        let invocation = invocation_key();
        let mut session = None;
        let raw = match read_bounded(stdin, MAX_STDIN_BYTES) {
            Ok(raw) => raw,
            Err(_) => {
                diagnostic(
                    deps,
                    start,
                    &invocation,
                    &session,
                    Stage::Filtered,
                    "input-read-failed",
                );
                return None;
            }
        };
        if raw.len() as u64 > MAX_STDIN_BYTES {
            diagnostic(
                deps,
                start,
                &invocation,
                &session,
                Stage::Filtered,
                "oversized-input",
            );
            return None;
        }
        let value: serde_json::Value = match serde_json::from_slice(&raw) {
            Ok(value) => value,
            Err(_) => {
                diagnostic(
                    deps,
                    start,
                    &invocation,
                    &session,
                    Stage::Filtered,
                    "malformed-input",
                );
                return None;
            }
        };
        session = value
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| session_id_ok(Some(id)))
            .map(|id| session_key(id)[..LOG_KEY_CHARS].to_string());
        let observation = ErrorObservation::from_value(value.get("error"));
        deps.diagnostic(
            start,
            &Diagnostic {
                invocation: invocation.clone(),
                session: session.clone(),
                event: None,
                stage: Stage::Receipt,
                disposition: "received",
                error: Some(observation),
            },
        );
        let disposition = if !deps.auto_switch_on() {
            Some("disabled-auto-switch")
        } else {
            match value.get("error") {
                None => Some("missing-error"),
                Some(serde_json::Value::String(error)) if error != "rate_limit" => {
                    Some("non-limit-error")
                }
                Some(serde_json::Value::String(_)) => match value.get("session_id") {
                    None | Some(serde_json::Value::Null) => Some("missing-session"),
                    Some(serde_json::Value::String(id)) if session_id_ok(Some(id)) => None,
                    _ => Some("invalid-session"),
                },
                Some(_) => Some("invalid-error-type"),
            }
        };
        if let Some(disposition) = disposition {
            diagnostic(
                deps,
                start,
                &invocation,
                &session,
                Stage::Filtered,
                disposition,
            );
            return None;
        }
        let input: HookInput = match serde_json::from_value(value) {
            Ok(input) => input,
            Err(_) => {
                diagnostic(
                    deps,
                    start,
                    &invocation,
                    &session,
                    Stage::Filtered,
                    "malformed-input",
                );
                return None;
            }
        };
        if !matches!(input, HookInput::StopFailure { .. }) {
            diagnostic(
                deps,
                start,
                &invocation,
                &session,
                Stage::Filtered,
                "not-stop-failure",
            );
            return None;
        }
        Some(run_started(&input, deps, start, &invocation))
    }));
    match decided {
        Ok(Some(Outcome::Resumed)) => {
            // The resume is already recorded; exit 2 alone wakes the
            // session, so a failed write still exits 2.
            let _ = writeln!(stderr, "{RESUME_MSG}");
            let _ = stderr.flush();
            Outcome::Resumed.exit_code()
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

/// The real environment for a resolved profile root.
struct SystemDeps {
    root: PathBuf,
    guard: FsGuard,
    /// The hook's parent when it started: the Claude Code process.
    parent: u32,
    /// `$TMUX_PANE` when the hook started, if it is a pane id under `$TMUX`;
    /// verified by [`Deps::pane`] just before commit.
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

    fn log(&mut self, now: i64, entry: &LogEntry) {
        // An unsafe or uncreatable resume directory means no log line;
        // the outcome is unchanged.
        if let Ok(dir) = self.guard.ensure_resume_dir() {
            log::append(&dir, &log::format_line(now, entry));
        }
    }

    fn diagnostic(&mut self, now: i64, entry: &Diagnostic) {
        if let Ok(dir) = self.guard.ensure_resume_dir() {
            log::append(&dir, &log::format_diagnostic(now, entry));
        }
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
        let until = Instant::now()
            .checked_add(Duration::from_secs(left))
            .unwrap_or_else(|| Instant::now() + MAX_WAIT);
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
        rotator_status::swap_record::read_swap(&self.root, now)
    }

    fn pane(&mut self) -> Option<Pane> {
        self.pane
            .as_ref()
            .filter(|p| keys::verify_pane(self.parent, p))
            .cloned()
    }

    fn send_keys(&mut self, pane: &Pane, key: Key) -> io::Result<()> {
        keys::tmux_send(pane, key)
    }

    fn session_alive_now(&mut self) -> bool {
        match &self.waiter {
            // S4: alive and still in the foreground, so a Ctrl-Z between
            // keys stops the rest from reaching the shell.
            Some(w) => w.process_alive() && keys::foreground_now(self.parent),
            None => self.parent != 1 && std::os::unix::process::parent_id() == self.parent,
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
/// when the profile root cannot be resolved safely; panics are silenced and
/// become exit 0.
pub fn run(stdin: impl Read) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    let Ok(Some(root)) = catch_unwind(rotator_status::resolve_root) else {
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
