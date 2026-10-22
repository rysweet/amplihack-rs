//! `amplihack-hooks limit-resume` (issue #1658): a `StopFailure` hook,
//! registered with matcher `rate_limit` and `asyncRewake: true`, that wakes
//! a Claude Code session stopped by a usage limit once the login rotator
//! (`amplihack profile rotate`) has recorded a fresh cycle with capacity.
//!
//! Protocol: exit 2 with exactly `RESUME_MSG` and a newline on stderr to
//! wake the session; otherwise exit 0 with no output. Stdout is never
//! written.
//!
//! Order of checks (see [`decide::gate`]): auto-switch on, a limit stop
//! with a usable session id, no `stop_hook_active`, an event not resumed
//! before, a session under its cap. Then the hook reads the rotate ledger
//! every `POLL_INTERVAL` for up to `MAX_WAIT`, plus once at the deadline,
//! and resumes only after [`guard::GuardStore::commit`] recorded the
//! resume under the lock. Each poll also re-checks the toggle and whether
//! the user sent a prompt in the session since the hook started (see
//! [`record_prompt`], called by the `UserPromptSubmit` hook).
//!
//! The hook reads only the toggle, the profile-root pointer, the rotate
//! ledger and its own resume state; it never opens a login file or the
//! transcript.

pub mod decide;
pub mod guard;
pub mod log;

use amplihack_types::HookInput;
use amplihack_utils::rotator_status::{self, CycleStatus, MAX_WAIT, POLL_INTERVAL};
use amplihack_utils::secure_files::read_bounded;
use decide::{Facts, Gate, event_key, fresh_capacity, gate, is_limit, session_id_ok, session_key};
use guard::{CommitError, FsGuard, GuardStore};
use log::LogEntry;
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What the woken model is told. Fixed text: nothing from the payload is
/// echoed into the model's context.
pub const RESUME_MSG: &str = "amplihack: the login rotator reports a login with capacity. \
     Continue the task you were working on; if the usage limit is still in place, stop and wait.";

/// Resumes allowed per session in any 24 hours.
pub const MAX_RESUMES_PER_SESSION: u32 = 3;

/// Larger hook payloads are ignored.
pub const MAX_STDIN_BYTES: u64 = 1024 * 1024;

/// Hash prefix length used in the log.
const LOG_KEY_CHARS: usize = 12;

/// How one invocation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A fresh cycle with capacity was seen and the resume recorded.
    Resumed,
    /// No fresh cycle with capacity before the deadline.
    NoCapacityTimeout,
    /// The user sent a prompt in the session while the hook waited.
    MovedOn,
    /// This limit event was already resumed.
    Duplicate,
    /// The session reached its resume cap.
    Cap,
    /// The payload had `stop_hook_active: true`.
    StopHookActive,
    /// The resume state could not be read or written safely.
    GuardFailed,
    /// Auto-switch is off. Logged only when it was turned off while
    /// waiting.
    Disabled,
    /// Not a usage-limit stop, or no usable session id. Not logged.
    NotLimit,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resumed => "resumed",
            Self::NoCapacityTimeout => "no-capacity-timeout",
            Self::MovedOn => "moved-on",
            Self::Duplicate => "duplicate",
            Self::Cap => "cap",
            Self::StopHookActive => "stop-hook-active",
            Self::GuardFailed => "guard-failed",
            Self::Disabled => "disabled",
            Self::NotLimit => "not-limit",
        }
    }

    /// 2 wakes the session (`asyncRewake`); everything else is 0.
    pub fn exit_code(self) -> i32 {
        if self == Self::Resumed { 2 } else { 0 }
    }
}

/// Everything `run_with` needs from the outside world.
pub trait Deps {
    /// Whether auto-switch is on for the profile root.
    fn auto_switch_on(&mut self) -> bool;
    /// Epoch seconds.
    fn now(&mut self) -> i64;
    fn sleep(&mut self, d: Duration);
    /// The rotate ledger's last cycle, read at `now`.
    fn read_cycle(&mut self, now: i64) -> Option<CycleStatus>;
    fn guard(&mut self) -> &mut dyn GuardStore;
    /// Records a decision; must never fail or print.
    fn log(&mut self, now: i64, entry: &LogEntry);
}

/// Decides one hook invocation.
pub fn run_with<D: Deps + ?Sized>(input: &HookInput, deps: &mut D) -> Outcome {
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
    let start = deps.now();
    // The live login at the limit is part of the event: the same login
    // limited again is the same event, a limit after a swap is a new one.
    let first = deps.read_cycle(start);
    let live_at_start = first.as_ref().and_then(|s| s.live.as_deref());
    let event = event_key(session_id, error, details, message, live_at_start);
    let session = session_key(session_id);
    let finish = |deps: &mut D, now: i64, outcome: Outcome, status: Option<&CycleStatus>| {
        let entry = LogEntry {
            session: session[..LOG_KEY_CHARS].to_string(),
            event: event[..LOG_KEY_CHARS].to_string(),
            outcome,
            live: status.and_then(|s| s.live.clone()),
            cycle: status.and_then(|s| s.outcome.clone()),
        };
        deps.log(now, &entry);
        outcome
    };

    if gate(&facts) == Gate::StopHookActive {
        return finish(deps, start, Outcome::StopHookActive, None);
    }
    // Checked here so a duplicate or capped event exits at once; checked
    // again under the lock by `commit`.
    match deps.guard().seen(&event) {
        Ok(seen) => facts.duplicate = seen,
        Err(_) => return finish(deps, start, Outcome::GuardFailed, None),
    }
    match deps.guard().resumes(&session, start) {
        Ok(n) => facts.capped = n >= MAX_RESUMES_PER_SESSION,
        Err(_) => return finish(deps, start, Outcome::GuardFailed, None),
    }
    match gate(&facts) {
        Gate::Duplicate => return finish(deps, start, Outcome::Duplicate, None),
        Gate::Cap => return finish(deps, start, Outcome::Cap, None),
        _ => {}
    }

    // A prompt sent after the hook started means the session went on
    // without it. The hook's own wake is not a prompt, so it never counts.
    let moved_on = |deps: &mut D| {
        deps.guard()
            .prompted_at(&session)
            .map(|t| t.is_some_and(|t| t > start))
    };
    let deadline = start.saturating_add(MAX_WAIT.as_secs() as i64);
    let (now, status) = loop {
        let now = deps.now();
        if !deps.auto_switch_on() {
            return finish(deps, now, Outcome::Disabled, None);
        }
        match moved_on(deps) {
            Ok(false) => {}
            Ok(true) => return finish(deps, now, Outcome::MovedOn, None),
            Err(_) => return finish(deps, now, Outcome::GuardFailed, None),
        }
        let status = deps.read_cycle(now);
        if fresh_capacity(status.as_ref(), start) || now >= deadline {
            break (now, status);
        }
        let left = Duration::from_secs((deadline - now) as u64);
        deps.sleep(POLL_INTERVAL.min(left));
    };
    if !fresh_capacity(status.as_ref(), start) {
        return finish(deps, now, Outcome::NoCapacityTimeout, status.as_ref());
    }
    match moved_on(deps) {
        Ok(false) => {}
        Ok(true) => return finish(deps, now, Outcome::MovedOn, status.as_ref()),
        Err(_) => return finish(deps, now, Outcome::GuardFailed, status.as_ref()),
    }
    let outcome = match deps.guard().commit(&event, &session, start, now) {
        Ok(()) => Outcome::Resumed,
        Err(CommitError::Duplicate) => Outcome::Duplicate,
        Err(CommitError::Cap) => Outcome::Cap,
        Err(CommitError::Io) => Outcome::GuardFailed,
    };
    finish(deps, now, outcome, status.as_ref())
}

/// Reads at most `MAX_STDIN_BYTES` of payload from `stdin`, decides, and
/// writes `RESUME_MSG` to `stderr` only on a resume. Returns the exit code:
/// 2 on a resume, 0 otherwise, including for an oversized or malformed
/// payload and for a panic.
pub fn run_with_io<D: Deps + ?Sized>(
    stdin: impl Read,
    stderr: &mut dyn Write,
    deps: &mut D,
) -> i32 {
    let decided = catch_unwind(AssertUnwindSafe(|| {
        let raw = read_bounded(stdin, MAX_STDIN_BYTES).ok()?;
        if raw.len() as u64 > MAX_STDIN_BYTES {
            return None;
        }
        let input: HookInput = serde_json::from_slice(&raw).ok()?;
        Some(run_with(&input, deps))
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

/// The real environment for a resolved profile root.
struct SystemDeps {
    root: PathBuf,
    guard: FsGuard,
}

impl Deps for SystemDeps {
    fn auto_switch_on(&mut self) -> bool {
        rotator_status::auto_switch_on(&self.root)
    }

    fn now(&mut self) -> i64 {
        epoch_now()
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
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
}

/// Notes a user prompt in `session_id` for a waiting `limit-resume` hook.
/// Called by the `UserPromptSubmit` hook; silent, never fails, takes no
/// lock, and writes nothing unless `<root>/.auto-switch.d/resume/` already
/// exists and is private.
pub fn record_prompt(session_id: Option<&str>) {
    // Unit tests of other hooks must never touch the real profile root.
    if cfg!(test) && std::env::var_os(rotator_status::ROOT_ENV).is_none() {
        return;
    }
    if let Some(root) = rotator_status::resolve_root() {
        record_prompt_at(&root, session_id, epoch_now());
    }
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
    let mut deps = SystemDeps {
        guard: FsGuard::new(&root),
        root,
    };
    run_with_io(stdin, &mut std::io::stderr().lock(), &mut deps)
}
