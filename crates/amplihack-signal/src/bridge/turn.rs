//! Copilot turn driver: build the pinned resume argv and serialize turns.
//!
//! The bridge drives the agent one **turn at a time** by resuming the same
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
//! `copilot` process, tracks its PID (so an out-of-band `stop` can pre-empt an
//! in-flight turn), and captures clean stdout.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use super::allowlist::ToolAllowlist;

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

/// Production [`TurnRunner`]: spawns the real `copilot` binary.
///
/// The currently-running child's PID is published into a shared slot so an
/// out-of-band control message (`stop`/`kill`) can terminate an in-flight turn.
/// On a non-zero exit the combined stderr/stdout is surfaced as an error so the
/// bridge can post the failure to the group and keep going (the next turn
/// resumes the same session, context intact).
pub struct CopilotTurnRunner {
    program: String,
    current_pid: Arc<Mutex<Option<u32>>>,
}

impl CopilotTurnRunner {
    /// Create a runner for `program` (typically `copilot`) sharing `current_pid`
    /// so the bridge can pre-empt the in-flight child.
    #[must_use]
    pub fn new(program: impl Into<String>, current_pid: Arc<Mutex<Option<u32>>>) -> Self {
        Self {
            program: program.into(),
            current_pid,
        }
    }
}

impl TurnRunner for CopilotTurnRunner {
    fn run_argv(
        &self,
        argv: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = io::Result<String>> + Send>> {
        let program = self.program.clone();
        let current_pid = self.current_pid.clone();
        Box::pin(async move {
            use tokio::process::Command;

            let child = Command::new(&program)
                .args(&argv)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            if let Some(pid) = child.id() {
                *current_pid.lock().expect("pid mutex not poisoned") = Some(pid);
            }

            let output = child.wait_with_output().await;
            // Clear the published PID regardless of outcome.
            *current_pid.lock().expect("pid mutex not poisoned") = None;
            let output = output?;

            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                Err(io::Error::other(format!(
                    "copilot turn failed ({}): {}{}",
                    output.status, stdout, stderr
                )))
            }
        })
    }
}
