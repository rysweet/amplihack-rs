//! Copilot turn driver: build the pinned resume argv and serialize turns.
//!
//! The chat drives the agent one **turn at a time** by resuming the same
//! Copilot session: `copilot --session-id <uuid> --no-color -s -p "<msg>"
//! <allowlist>`. Each turn is a fresh `copilot` process that resumes the SAME
//! session id, so full prior context is preserved without any PTY, ANSI
//! parsing, or streaming.
//!
//! Two seams live here:
//! - [`build_turn_argv`] — the pure argv builder (injection-safe: the prompt is
//!   always exactly one argv element, never concatenated into a shell string).
//! - [`SerialTurnDriver`] — serializes turns so at most one turn per session
//!   runs at a time, over an injectable [`TurnRunner`].
//!
//! [`CopilotTurnRunner`] is the production [`TurnRunner`]: it spawns the real
//! `copilot` process, publishes a child-bound pre-empt trigger into a shared
//! [`PreemptSlot`] (so an out-of-band `stop` can pre-empt an in-flight turn
//! without any PID-reuse race), and captures clean stdout.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::allowlist::ToolAllowlist;

/// Shared pre-emption seam: holds a one-shot trigger bound to the in-flight
/// child, or `None` when no turn is running.
///
/// A control `stop`/`kill` takes the sender out of this slot and `send(())`s it.
/// The turn task selecting on the paired receiver then kills its **owned**
/// [`tokio::process::Child`] via [`tokio::process::Child::start_kill`], which
/// the runtime binds to that exact process — so pre-emption can never target a
/// recycled PID. This replaces the old raw-PID slot and its direct `kill(2)`
/// syscall path and their unavoidable time-of-check/time-of-use (TOCTOU) window.
pub type PreemptSlot = Arc<Mutex<Option<oneshot::Sender<()>>>>;

/// Build the Copilot resume argv for one turn.
///
/// Layout: `--session-id <SID> --no-color -s -p <PROMPT> <allowlist...>`.
///
/// - `--session-id <SID>` pins turn continuity (resume the same session).
/// - `--no-color` guarantees ANSI-free stdout before redaction/chunking.
/// - `-s` (silent) captures the response only.
/// - `-p <PROMPT>` passes the (attacker-influenced) prompt as **exactly one**
///   argv element, verbatim — never a shell string — so metacharacters cannot
///   inject a command.
///
/// The returned vector is the *argument* list (the program, `copilot`, is
/// supplied by the runner).
#[must_use]
pub fn build_turn_argv(session_id: &str, prompt: &str, allowlist: &ToolAllowlist) -> Vec<String> {
    let mut argv = vec![
        "--session-id".to_string(),
        session_id.to_string(),
        "--no-color".to_string(),
        "-s".to_string(),
        "-p".to_string(),
        prompt.to_string(),
    ];
    argv.extend(allowlist.to_copilot_args());
    argv
}

/// An injectable executor of one Copilot turn given its argv.
///
/// Returns the captured stdout on success. Implemented for real by
/// [`CopilotTurnRunner`] and by mocks in tests.
pub trait TurnRunner: Send + Sync {
    /// Run `copilot` with `argv` and resolve to its captured stdout.
    fn run_argv(
        &self,
        argv: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = io::Result<String>> + Send>>;
}

/// Serializes turns for one pinned session so at most one turn runs at a time.
///
/// Turn continuity requires that two `copilot --session-id <same>` processes are
/// never in flight concurrently (they would race the same session state). An
/// async mutex enforces one-at-a-time execution even when `run_turn` is called
/// from multiple tasks.
pub struct SerialTurnDriver<R: TurnRunner> {
    runner: R,
    session_id: String,
    allowlist: ToolAllowlist,
    lock: tokio::sync::Mutex<()>,
}

impl<R: TurnRunner> SerialTurnDriver<R> {
    /// Create a driver bound to `session_id` and `allowlist`.
    #[must_use]
    pub fn new(runner: R, session_id: &str, allowlist: ToolAllowlist) -> Self {
        Self {
            runner,
            session_id: session_id.to_string(),
            allowlist,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// The pinned session id this driver resumes.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Run one turn with `prompt`, serialized against any other turn.
    pub async fn run_turn(&self, prompt: &str) -> io::Result<String> {
        let argv = build_turn_argv(&self.session_id, prompt, &self.allowlist);
        // Hold the lock across the whole child lifetime so turns never overlap.
        let _guard = self.lock.lock().await;
        self.runner.run_argv(argv).await
    }
}

/// Present the serialized Copilot turn driver to the generic
/// [`run_session_loop`](crate::run_session_loop) as an [`AgentSession`](crate::AgentSession).
///
/// The inherent [`SerialTurnDriver::run_turn`] is a `&self` method returning
/// `io::Result<String>`; the trait requires `&mut self` and
/// [`TurnResult`](crate::TurnResult)`<`[`TurnOutput`](crate::TurnOutput)`>`. This
/// is the thin adapter that bridges the two, surfacing every failure (never
/// swallowing it):
///
/// * `Ok(stdout)` → `Ok(TurnOutput{ text == stdout })` (captured verbatim);
/// * `Err(Interrupted)` (a fired [`PreemptSlot`] from `stop`/`kill`) →
///   `Err(TurnError::Preempted)`, so a pre-emption reads as a clean stop rather
///   than a failure;
/// * any other `io::Error` (spawn failure, non-zero exit, I/O) →
///   `Err(TurnError::Exec(..))` carrying the underlying message so the caller can
///   report it and keep the same session.
impl<R: TurnRunner> crate::AgentSession for SerialTurnDriver<R> {
    async fn run_turn(&mut self, prompt: &str) -> crate::TurnResult<crate::TurnOutput> {
        // Call the inherent `&self` method explicitly (path form) so this never
        // resolves back to the trait method and self-recurses.
        match SerialTurnDriver::run_turn(self, prompt).await {
            Ok(stdout) => Ok(crate::TurnOutput::from_text(stdout)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Err(crate::TurnError::Preempted),
            Err(e) => Err(crate::TurnError::Exec(e.to_string())),
        }
    }

    fn session_id(&self) -> &str {
        SerialTurnDriver::session_id(self)
    }
}

/// Production [`TurnRunner`]: spawns the real `copilot` binary.
///
/// A child-bound pre-empt trigger is published into a shared [`PreemptSlot`] so
/// an out-of-band control message (`stop`/`kill`) can terminate an in-flight
/// turn. Pre-emption fires the trigger; this runner reacts by killing its
/// **owned** [`tokio::process::Child`] handle (via `start_kill`), so the kill is
/// bound to the exact process and is immune to PID reuse. A pre-empted turn
/// surfaces as [`io::ErrorKind::Interrupted`]. On a non-zero exit the combined
/// stderr/stdout is surfaced as an error so the chat can post the failure to
/// the group and keep going (the next turn resumes the same session, context
/// intact).
pub struct CopilotTurnRunner {
    program: String,
    preempt: PreemptSlot,
}

impl CopilotTurnRunner {
    /// Create a runner for `program` (typically `copilot`) sharing `preempt` so
    /// the chat can pre-empt the in-flight child by firing its trigger.
    #[must_use]
    pub fn new(program: impl Into<String>, preempt: PreemptSlot) -> Self {
        Self {
            program: program.into(),
            preempt,
        }
    }
}

impl TurnRunner for CopilotTurnRunner {
    fn run_argv(
        &self,
        argv: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = io::Result<String>> + Send>> {
        let program = self.program.clone();
        let preempt = self.preempt.clone();
        Box::pin(async move {
            use tokio::io::AsyncReadExt;
            use tokio::process::Command;

            let mut child = Command::new(&program)
                .args(&argv)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                // Defense-in-depth: if this future is dropped on any non-`stop`
                // shutdown path (task cancellation, error unwinding) before we
                // reach the explicit reap below, ensure the child is killed
                // rather than orphaned. Mirrors claude_process.rs.
                .kill_on_drop(true)
                .spawn()?;

            // Publish a pre-empt trigger bound to THIS child. A control `stop`
            // takes the sender out of the slot and fires it; we react below by
            // killing this exact owned handle — no raw PID is ever exposed, so
            // there is no PID-reuse window.
            let (pre_tx, mut pre_rx) = oneshot::channel::<()>();
            *preempt.lock().expect("preempt mutex not poisoned") = Some(pre_tx);

            // Take the pipes and drain them concurrently so a full pipe can
            // never deadlock `wait()` (or the post-kill reap).
            let stdout_pipe = child.stdout.take();
            let stderr_pipe = child.stderr.take();
            let stdout_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut p) = stdout_pipe {
                    let _ = p.read_to_end(&mut buf).await;
                }
                buf
            });
            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                if let Some(mut p) = stderr_pipe {
                    let _ = p.read_to_end(&mut buf).await;
                }
                buf
            });

            // Race the child's natural exit against a pre-empt request. On
            // pre-empt, kill the owned handle and reap it (immune to PID reuse).
            let mut was_preempted = false;
            let wait_result: io::Result<std::process::ExitStatus> = tokio::select! {
                res = child.wait() => res,
                _ = &mut pre_rx => {
                    was_preempted = true;
                    let _ = child.start_kill();
                    child.wait().await
                }
            };

            // Clear our slot so a later pre-empt is a harmless no-op.
            *preempt.lock().expect("preempt mutex not poisoned") = None;

            // Join the drains; the pipes are closed once the child is gone.
            let stdout_buf = stdout_task.await.unwrap_or_default();
            let stderr_buf = stderr_task.await.unwrap_or_default();

            if was_preempted {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "turn pre-empted by stop",
                ));
            }

            let status = wait_result?;
            if status.success() {
                // Zero-copy on the common valid-UTF-8 path: `from_utf8` reuses
                // the captured buffer; only invalid bytes fall back to a lossy
                // copy. Avoids allocating+memcpy of the whole output each turn.
                Ok(match String::from_utf8(stdout_buf) {
                    Ok(s) => s,
                    Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
                })
            } else {
                let stderr = String::from_utf8_lossy(&stderr_buf);
                let stdout = String::from_utf8_lossy(&stdout_buf);
                Err(io::Error::other(format!(
                    "copilot turn failed ({status}): {stdout}{stderr}"
                )))
            }
        })
    }
}
