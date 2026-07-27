//! TDD (RED) contract tests for the `AgentSession` adapter over the Copilot
//! turn driver — PR-3 of issue #910.
//!
//! Written **first**: these FAIL to compile until PR-3 adds
//! `impl AgentSession for SerialTurnDriver<CopilotTurnRunner>` (the thin
//! `&self` -> `&mut self` adapter that maps `io::Result<String>` into
//! `TurnResult<TurnOutput>`). See `docs/signal-channel-turn-loop.md`
//! ("`AgentSession` for the Copilot turn driver").
//!
//! Hermetic: no real `copilot` process. The driver is built over a mock
//! [`TurnRunner`] so we can drive every branch of the result mapping:
//!   * `Ok(stdout)`                        -> `Ok(TurnOutput{ text == stdout })`
//!   * `Err(Interrupted)` (stop/kill)      -> `Err(TurnError::Preempted)`
//!   * `Err(other io / non-zero exit)`     -> `Err(TurnError::Exec(..))`
//!   * `session_id()`                      -> the pinned id, unchanged
//!
//! Run: `cargo test -p amplihack-turn --test agent_session_it`.

use std::future::Future;
use std::io;
use std::pin::Pin;

use amplihack_turn::{
    AgentSession, CopilotTurnRunner, PreemptSlot, SerialTurnDriver, ToolAllowlist, TurnError,
    TurnRunner,
};

const HANG_GUARD: std::time::Duration = std::time::Duration::from_secs(10);

/// A mock `TurnRunner` that yields a scripted `io::Result<String>`, so tests can
/// exercise the adapter's mapping without spawning a real process.
struct MockRunner {
    /// The scripted outcome the adapter must map. `Ok` is a captured-stdout
    /// success; `Err` models a pre-empt (`Interrupted`) or an exec failure.
    outcome: fn() -> io::Result<String>,
}

impl TurnRunner for MockRunner {
    fn run_argv(
        &self,
        _argv: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = io::Result<String>> + Send>> {
        let outcome = self.outcome;
        Box::pin(async move { outcome() })
    }
}

fn driver(outcome: fn() -> io::Result<String>) -> SerialTurnDriver<MockRunner> {
    SerialTurnDriver::new(
        MockRunner { outcome },
        "sid-11111111-2222-3333-4444-555555555555",
        ToolAllowlist::read_only_default(),
    )
}

#[tokio::test]
async fn ok_stdout_maps_to_turn_output_verbatim() {
    let mut session = driver(|| Ok("the agent said this".to_string()));
    // Fully-qualified: `SerialTurnDriver` also has an inherent `run_turn(&self)`
    // returning `io::Result<String>`; we specifically exercise the trait method.
    let out = tokio::time::timeout(HANG_GUARD, AgentSession::run_turn(&mut session, "hello"))
        .await
        .expect("run_turn did not hang")
        .expect("a successful turn maps to Ok(TurnOutput)");
    assert_eq!(
        out.text(),
        "the agent said this",
        "the captured stdout must be surfaced verbatim as the turn output"
    );
}

#[tokio::test]
async fn interrupted_maps_to_preempted() {
    // A fired PreemptSlot surfaces from CopilotTurnRunner as io::ErrorKind::Interrupted.
    // The adapter must translate that into a clean TurnError::Preempted (not Io/Exec),
    // so a `stop`/`kill` reads as a pre-emption rather than a failure.
    let mut session = driver(|| {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "turn pre-empted by stop",
        ))
    });
    let err = tokio::time::timeout(HANG_GUARD, AgentSession::run_turn(&mut session, "work"))
        .await
        .expect("run_turn did not hang")
        .expect_err("an Interrupted io error must map to a TurnError");
    assert!(
        matches!(err, TurnError::Preempted),
        "io::ErrorKind::Interrupted must map to TurnError::Preempted, got {err:?}"
    );
}

#[tokio::test]
async fn non_zero_exit_maps_to_exec_error() {
    // A non-zero copilot exit surfaces as a generic io error (io::Error::other);
    // the adapter surfaces it (never swallows it) as TurnError::Exec carrying the
    // message so the chat can post "turn failed: ..." and keep going.
    let mut session = driver(|| Err(io::Error::other("copilot turn failed (exit 1): boom")));
    let err = tokio::time::timeout(HANG_GUARD, AgentSession::run_turn(&mut session, "work"))
        .await
        .expect("run_turn did not hang")
        .expect_err("a non-zero exit must map to a TurnError");
    match err {
        TurnError::Exec(msg) => assert!(
            msg.contains("copilot turn failed"),
            "the underlying failure text must be preserved, got {msg:?}"
        ),
        other => panic!("a non-zero exit must map to TurnError::Exec, got {other:?}"),
    }
}

#[tokio::test]
async fn session_id_is_the_pinned_id() {
    let session = driver(|| Ok(String::new()));
    assert_eq!(
        AgentSession::session_id(&session),
        "sid-11111111-2222-3333-4444-555555555555",
        "the AgentSession must expose the driver's pinned session id unchanged"
    );
}

#[tokio::test]
async fn production_runner_type_satisfies_agent_session() {
    // Compile-time proof that the *production* driver (over CopilotTurnRunner) is
    // the type the loop drives — this is the exact type `run_chat_async` builds.
    // We never call run_turn here (no real copilot), only bind the trait.
    fn assert_agent_session<S: AgentSession>() {}
    assert_agent_session::<SerialTurnDriver<CopilotTurnRunner>>();
    let _preempt: PreemptSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
}
