//! TDD contract tests for the new `amplihack-turn` crate (PR-2 of issue #910).
//!
//! These are written **first** and are expected to FAIL to compile until the
//! `amplihack-turn` crate's public API (`AgentSession`, `Channel`, `NextPrompt`,
//! `TurnOutput`, `ChannelId`, `TurnError`/`ChannelError`, and `run_session_loop`)
//! is implemented. They are fully hermetic: no network, no real `copilot`
//! binary — every session and channel is a mock.
//!
//! Run: `cargo test -p amplihack-turn --test driver_loop_it`.
//!
//! What the driver-loop contract requires (see
//! `docs/reference/amplihack-turn-api.md`):
//!   * `NextPrompt::Ready(p)` => `run_turn(&p)` THEN `publish_output(&out)`,
//!     in that order, before the next prompt is requested.
//!   * Prompt ordering is preserved.
//!   * The SAME session (hence `session_id()`) is reused for every turn.
//!   * `NextPrompt::Idle` => wait for liveness/inbound then re-poll (never
//!     treated as Closed, never drops the pending work) — NO wall-clock cap.
//!   * `NextPrompt::Closed` => break cleanly with `Ok(())`.
//!   * The first `TurnError`/`ChannelError` propagates out of the loop unchanged
//!     (no silent fallback, no swallowing).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use amplihack_turn::{
    AgentSession, Channel, ChannelError, ChannelId, ChannelResult, NextPrompt, TurnError,
    TurnOutput, TurnResult, run_session_loop,
};
use async_trait::async_trait;

// A defensive upper bound so a mis-implemented idle-wait (that hangs waiting on
// a liveness signal that never arrives in a hermetic test) fails the test
// instead of hanging the whole suite forever.
const HANG_GUARD: Duration = Duration::from_secs(10);

// =============================================================================
// Shared instrumentation
// =============================================================================

/// A shared, ordered log of the loop's observable actions. Both the mock
/// session and the mock channel append to the SAME log so tests can assert the
/// exact interleaving (e.g. `run_turn` strictly before `publish` within a turn).
type EventLog = Arc<Mutex<Vec<String>>>;

fn log(events: &EventLog, entry: impl Into<String>) {
    events
        .lock()
        .expect("event log not poisoned")
        .push(entry.into());
}

fn snapshot(events: &EventLog) -> Vec<String> {
    events.lock().expect("event log not poisoned").clone()
}

// =============================================================================
// Mock AgentSession (native `async fn` in trait, edition 2024)
// =============================================================================

/// Records every prompt it is asked to run and echoes it back as the output.
/// Pins a single, stable session id for its whole life.
struct MockSession {
    id: String,
    events: EventLog,
    /// Every prompt seen, in call order — proves ordering + session reuse.
    seen: Vec<String>,
}

impl MockSession {
    fn new(id: &str, events: EventLog) -> Self {
        Self {
            id: id.to_string(),
            events,
            seen: Vec::new(),
        }
    }
}

impl AgentSession for MockSession {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        self.seen.push(prompt.to_string());
        log(&self.events, format!("run_turn:{prompt}"));
        Ok(TurnOutput::from_text(format!("echo: {prompt}")))
    }

    fn session_id(&self) -> &str {
        &self.id
    }
}

/// A session that always fails a turn, to prove `TurnError` propagates.
struct FailingSession {
    id: String,
    events: EventLog,
}

impl AgentSession for FailingSession {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        log(&self.events, format!("run_turn:{prompt}"));
        Err(TurnError::Exec("boom".to_string()))
    }

    fn session_id(&self) -> &str {
        &self.id
    }
}

// =============================================================================
// Mock Channels (#[async_trait], object-safe)
// =============================================================================

/// One scripted step the channel yields from `next_prompt`.
#[derive(Clone)]
enum Step {
    Ready(&'static str),
    Idle,
    Closed,
    /// Force a receive-side error to prove `ChannelError` propagates.
    RecvErr,
}

/// A fully scripted channel: yields a fixed sequence of `Step`s and records the
/// outputs it is asked to publish. `publish_output` is OVERRIDDEN here so the
/// test can observe replay ordering.
struct ScriptedChannel {
    id: ChannelId,
    events: EventLog,
    steps: std::collections::VecDeque<Step>,
    published: Vec<String>,
    /// When `true`, `publish_output` returns an error (fail-fast test).
    fail_publish: bool,
}

impl ScriptedChannel {
    fn new(id: &str, events: EventLog, steps: Vec<Step>) -> Self {
        Self {
            id: ChannelId::from(id),
            events,
            steps: steps.into_iter().collect(),
            published: Vec::new(),
            fail_publish: false,
        }
    }

    fn failing_publish(mut self) -> Self {
        self.fail_publish = true;
        self
    }
}

#[async_trait]
impl Channel for ScriptedChannel {
    fn id(&self) -> ChannelId {
        self.id.clone()
    }

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        log(&self.events, format!("publish:{}", out.text()));
        if self.fail_publish {
            return Err(ChannelError::Publish("sink closed".to_string()));
        }
        self.published.push(out.text().to_string());
        Ok(())
    }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt> {
        match self.steps.pop_front() {
            Some(Step::Ready(p)) => Ok(NextPrompt::Ready(p.to_string())),
            Some(Step::Idle) => {
                log(&self.events, "idle");
                Ok(NextPrompt::Idle)
            }
            Some(Step::Closed) | None => Ok(NextPrompt::Closed),
            Some(Step::RecvErr) => Err(ChannelError::Recv("transport reset".to_string())),
        }
    }
}

/// A channel that does NOT override `publish_output`, so it inherits the
/// default no-op REPLAY implementation. Used to prove the default keeps the
/// loop correct without any override.
struct DefaultReplayChannel {
    #[allow(dead_code)]
    events: EventLog,
    steps: std::collections::VecDeque<Step>,
}

impl DefaultReplayChannel {
    fn new(events: EventLog, steps: Vec<Step>) -> Self {
        Self {
            events,
            steps: steps.into_iter().collect(),
        }
    }
}

#[async_trait]
impl Channel for DefaultReplayChannel {
    fn id(&self) -> ChannelId {
        ChannelId::from("default-replay")
    }

    // NOTE: publish_output intentionally NOT overridden — inherits default no-op.

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt> {
        match self.steps.pop_front() {
            Some(Step::Ready(p)) => Ok(NextPrompt::Ready(p.to_string())),
            Some(Step::Idle) => Ok(NextPrompt::Idle),
            Some(Step::Closed) | None => Ok(NextPrompt::Closed),
            Some(Step::RecvErr) => Err(ChannelError::Recv("unused".to_string())),
        }
    }
}

// =============================================================================
// Loop behaviour: Ready => run_turn THEN publish, in order
// =============================================================================

#[tokio::test]
async fn ready_runs_turn_then_publishes_in_order() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![Step::Ready("hello"), Step::Closed],
    );

    run_session_loop(&mut session, &mut channel)
        .await
        .expect("loop must terminate cleanly on Closed");

    // Within a single Ready turn, run_turn strictly precedes publish_output.
    assert_eq!(
        snapshot(&events),
        vec![
            "run_turn:hello".to_string(),
            "publish:echo: hello".to_string()
        ],
        "on Ready the loop must call run_turn, THEN publish_output, in that order"
    );
    assert_eq!(session.seen, vec!["hello".to_string()]);
    assert_eq!(channel.published, vec!["echo: hello".to_string()]);
}

// =============================================================================
// Loop behaviour: prompt ordering preserved across multiple turns
// =============================================================================

#[tokio::test]
async fn prompt_ordering_preserved_across_turns() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![
            Step::Ready("first"),
            Step::Ready("second"),
            Step::Ready("third"),
            Step::Closed,
        ],
    );

    run_session_loop(&mut session, &mut channel).await.unwrap();

    assert_eq!(
        session.seen,
        vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string()
        ],
        "prompts must run in the exact order the channel yields them"
    );
    // Each turn fully completes (run + publish) before the next prompt runs.
    assert_eq!(
        snapshot(&events),
        vec![
            "run_turn:first".to_string(),
            "publish:echo: first".to_string(),
            "run_turn:second".to_string(),
            "publish:echo: second".to_string(),
            "run_turn:third".to_string(),
            "publish:echo: third".to_string(),
        ],
        "a turn must fully complete (run then publish) before the next prompt is requested"
    );
}

// =============================================================================
// Loop behaviour: the SAME session id is reused for every turn
// =============================================================================

#[tokio::test]
async fn session_id_reused_across_every_turn() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("pinned-session-42", events.clone());
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![Step::Ready("a"), Step::Ready("b"), Step::Closed],
    );

    assert_eq!(session.session_id(), "pinned-session-42");
    run_session_loop(&mut session, &mut channel).await.unwrap();

    assert_eq!(
        session.session_id(),
        "pinned-session-42",
        "session id must stay stable across turns (one resumed session)"
    );
    assert_eq!(session.seen.len(), 2, "both turns ran against one session");
}

// =============================================================================
// Loop behaviour: Closed breaks cleanly with Ok(())
// =============================================================================

#[tokio::test]
async fn closed_breaks_cleanly_without_running_a_turn() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = ScriptedChannel::new("mock", events.clone(), vec![Step::Closed]);

    let result = run_session_loop(&mut session, &mut channel).await;

    assert!(result.is_ok(), "Closed must terminate the loop with Ok(())");
    assert!(
        session.seen.is_empty(),
        "no turn should run when the channel is immediately Closed"
    );
    assert!(snapshot(&events).is_empty(), "nothing observable happened");
}

// =============================================================================
// Loop behaviour: Idle waits (does not close, does not drop work) then resumes
// =============================================================================

#[tokio::test]
async fn idle_waits_then_processes_pending_prompt() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    // Idle appears BEFORE and BETWEEN real prompts. The loop must not treat
    // Idle as Closed and must not drop the subsequent Ready work.
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![
            Step::Idle,
            Step::Ready("after-idle-1"),
            Step::Idle,
            Step::Ready("after-idle-2"),
            Step::Closed,
        ],
    );

    // Guard against a hang if idle-wait is mis-implemented as an unsatisfiable
    // await; a correct idle-wait re-polls and the loop finishes promptly.
    tokio::time::timeout(HANG_GUARD, run_session_loop(&mut session, &mut channel))
        .await
        .expect("idle-wait must re-poll and let the loop finish (no unbounded hang)")
        .expect("loop terminates Ok on Closed");

    assert_eq!(
        session.seen,
        vec!["after-idle-1".to_string(), "after-idle-2".to_string()],
        "Idle must be a wait, not a Close or a drop — pending prompts still run in order"
    );
    // The loop observed both idles and still produced both turns' outputs.
    assert_eq!(
        snapshot(&events),
        vec![
            "idle".to_string(),
            "run_turn:after-idle-1".to_string(),
            "publish:echo: after-idle-1".to_string(),
            "idle".to_string(),
            "run_turn:after-idle-2".to_string(),
            "publish:echo: after-idle-2".to_string(),
        ]
    );
}

// =============================================================================
// Default REPLAY: a channel that does not override publish_output stays correct
// =============================================================================

#[tokio::test]
async fn default_publish_output_is_noop_but_loop_still_drives_turns() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = DefaultReplayChannel::new(
        events.clone(),
        vec![Step::Ready("x"), Step::Ready("y"), Step::Closed],
    );

    run_session_loop(&mut session, &mut channel)
        .await
        .expect("default no-op publish must not error the loop");

    // Turns still ran (default publish is a no-op, not a failure).
    assert_eq!(session.seen, vec!["x".to_string(), "y".to_string()]);
    // Only run_turn events were recorded (the default publish is a silent no-op).
    assert_eq!(
        snapshot(&events),
        vec!["run_turn:x".to_string(), "run_turn:y".to_string()],
        "the default publish_output must be a genuine no-op"
    );
}

// =============================================================================
// Fail-fast: a TurnError propagates out of the loop, unchanged, and halts it
// =============================================================================

#[tokio::test]
async fn turn_error_propagates_and_stops_the_loop() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = FailingSession {
        id: "s-1".to_string(),
        events: events.clone(),
    };
    // A second Ready follows; it must NEVER be requested once the first turn errs.
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![
            Step::Ready("will-fail"),
            Step::Ready("never-reached"),
            Step::Closed,
        ],
    );

    let err = run_session_loop(&mut session, &mut channel)
        .await
        .expect_err("a TurnError must propagate out of the loop");

    match err {
        ChannelError::Turn(TurnError::Exec(msg)) => assert_eq!(msg, "boom"),
        other => panic!("expected the turn's Exec error to surface unchanged, got {other:?}"),
    }
    // The failing turn ran; NOTHING after it did (no publish, no second prompt).
    assert_eq!(
        snapshot(&events),
        vec!["run_turn:will-fail".to_string()],
        "the loop must halt on the first TurnError without publishing or advancing"
    );
}

// =============================================================================
// Fail-fast: a publish error propagates and halts before the next prompt
// =============================================================================

#[tokio::test]
async fn publish_error_propagates_and_stops_the_loop() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![Step::Ready("p"), Step::Ready("never-reached"), Step::Closed],
    )
    .failing_publish();

    let err = run_session_loop(&mut session, &mut channel)
        .await
        .expect_err("a ChannelError from publish must propagate out of the loop");

    match err {
        ChannelError::Publish(msg) => assert_eq!(msg, "sink closed"),
        other => panic!("expected a Publish error, got {other:?}"),
    }
    // The turn ran and publish was attempted; the loop then halted — the second
    // prompt is never requested.
    assert_eq!(
        snapshot(&events),
        vec!["run_turn:p".to_string(), "publish:echo: p".to_string()],
        "a publish failure must fail-fast, not silently drop output or advance"
    );
    assert_eq!(session.seen, vec!["p".to_string()]);
}

// =============================================================================
// Fail-fast: a receive error propagates and halts the loop
// =============================================================================

#[tokio::test]
async fn recv_error_propagates_and_stops_the_loop() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let mut channel = ScriptedChannel::new(
        "mock",
        events.clone(),
        vec![Step::RecvErr, Step::Ready("never-reached"), Step::Closed],
    );

    let err = run_session_loop(&mut session, &mut channel)
        .await
        .expect_err("a ChannelError from next_prompt must propagate out of the loop");

    match err {
        ChannelError::Recv(msg) => assert_eq!(msg, "transport reset"),
        other => panic!("expected a Recv error, got {other:?}"),
    }
    assert!(
        session.seen.is_empty(),
        "no turn runs when the very first next_prompt errors"
    );
}

// =============================================================================
// Supporting types: TurnOutput / ChannelId / error taxonomy
// =============================================================================

#[test]
fn turn_output_from_text_roundtrips_verbatim() {
    // Behaviour-preserving: TurnOutput wraps the response string without
    // transforming a single byte.
    let raw = "line one\nline two\twith tab and \x1b-ish text";
    let out = TurnOutput::from_text(raw);
    assert_eq!(
        out.text(),
        raw,
        "TurnOutput must carry the response verbatim"
    );

    // Debug + Clone are part of the contract.
    let cloned = out.clone();
    assert_eq!(cloned.text(), raw);
    let _ = format!("{out:?}");
}

#[test]
fn channel_id_is_cloneable_comparable_and_displayable() {
    let a = ChannelId::from("mock");
    let b = ChannelId::from("mock".to_string());
    let c = ChannelId::from("other");

    assert_eq!(a, b, "equal names produce equal ids");
    assert_ne!(a, c, "different names produce different ids");
    assert_eq!(a.clone(), a, "ChannelId is Clone");
    assert_eq!(
        a.to_string(),
        "mock",
        "Display renders the stable string form"
    );
    let _ = format!("{a:?}"); // Debug is part of the contract.
}

#[test]
fn turn_error_io_converts_via_from() {
    // `TurnError::Io` is a `#[from]` of std::io::Error (explicit, no silent
    // fallback). This confirms the `?`-friendly conversion the driver relies on.
    let io_err = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe gone");
    let turn_err: TurnError = io_err.into();
    assert!(matches!(turn_err, TurnError::Io(_)));

    // `TurnError::Preempted` renders a stable, secret-free message.
    assert_eq!(TurnError::Preempted.to_string(), "turn pre-empted");
}

#[test]
fn channel_error_wraps_turn_error_via_from() {
    // The loop returns `ChannelResult<()>`, so a `TurnError` from a turn must be
    // convertible into a `ChannelError` (the `?` path used by run_session_loop).
    let turn_err = TurnError::Exec("failed".to_string());
    let chan_err: ChannelError = turn_err.into();
    assert!(
        matches!(chan_err, ChannelError::Turn(TurnError::Exec(_))),
        "a TurnError must surface through ChannelError unchanged (no swallowing)"
    );

    // `ChannelError::Io` is a `#[from]` of std::io::Error too.
    let io_err = std::io::Error::other("disk");
    let chan_io: ChannelError = io_err.into();
    assert!(matches!(chan_io, ChannelError::Io(_)));
}

// =============================================================================
// Object safety: `Channel` must be usable behind `dyn` (run_session_loop takes
// `C: Channel + ?Sized`). This is a compile-time contract check.
// =============================================================================

#[tokio::test]
async fn channel_is_object_safe_and_drivable_via_dyn() {
    let events: EventLog = Arc::new(Mutex::new(Vec::new()));
    let mut session = MockSession::new("s-1", events.clone());
    let concrete = ScriptedChannel::new(
        "dyn-mock",
        events.clone(),
        vec![Step::Ready("via-dyn"), Step::Closed],
    );
    let mut boxed: Box<dyn Channel> = Box::new(concrete);

    run_session_loop(&mut session, boxed.as_mut())
        .await
        .expect("run_session_loop must accept a `&mut dyn Channel` (C: Channel + ?Sized)");

    assert_eq!(session.seen, vec!["via-dyn".to_string()]);
    assert_eq!(boxed.id().to_string(), "dyn-mock");
}
