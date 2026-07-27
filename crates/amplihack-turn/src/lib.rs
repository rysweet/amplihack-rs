//! `amplihack-turn` — the agent-generic turn-driver abstraction (issue #910, PR-2).
//!
//! This leaf crate holds the reusable, Signal-independent core for driving a
//! single agent session one **turn** at a time:
//!
//! * [`AgentSession`] — run ONE turn (resuming the same underlying session) to
//!   natural completion; NEVER a wall-clock cap.
//! * [`Channel`] — LISTEN for the next prompt and (optionally) REPLAY a turn's
//!   output. `publish_output` defaults to a no-op so replay-less channels stay
//!   correct without any override.
//! * [`NextPrompt`] — `Ready` / `Idle` / `Closed`, the three ways a channel can
//!   answer "what should the driver do next?".
//! * [`run_session_loop`] — the single, reusable driver loop that wires the two
//!   together with fail-fast, no-timeout, no-spin semantics.
//!
//! It also hosts the relocated Copilot turn primitives ([`build_turn_argv`],
//! [`TurnRunner`], [`SerialTurnDriver`], [`CopilotTurnRunner`], [`PreemptSlot`])
//! and the scoped [`ToolAllowlist`]. These moved here **behaviour-identical**
//! from `amplihack-signal` — they were already agent-generic. `amplihack-signal`
//! re-exports them at their original paths so existing callers compile unchanged.
//!
//! # Feature footprint (crusty condition #4)
//!
//! This crate is always compiled, so it MUST NOT pull in the tokio **net**
//! stack. The relocated driver needs only process spawning plus async
//! primitives (`process`, `rt`, `macros`, `time`, `sync`, `io-util`). The tokio
//! TCP transport stays gated inside `amplihack-signal`. Verify with
//! `cargo tree -p amplihack-turn -e features -i tokio`.
//!
//! # No silent fallbacks
//!
//! Every failure surfaces as an explicit [`TurnError`] / [`ChannelError`] and
//! propagates out of [`run_session_loop`] unchanged. There is no variant that
//! means "we hit a problem and pretended it was fine".

mod allowlist;
mod turn;

pub use allowlist::ToolAllowlist;
pub use turn::{CopilotTurnRunner, PreemptSlot, SerialTurnDriver, TurnRunner, build_turn_argv};

use async_trait::async_trait;

/// A `Result` whose error is a [`TurnError`] — the outcome of running one turn.
pub type TurnResult<T> = Result<T, TurnError>;

/// A `Result` whose error is a [`ChannelError`] — the outcome of a channel
/// operation (or of the whole [`run_session_loop`]).
pub type ChannelResult<T> = Result<T, ChannelError>;

/// A driven agent session. One turn at a time, resuming the same underlying
/// session on every call.
///
/// Uses native `async fn` in trait (edition 2024). The returned futures are not
/// required to be `Send`; [`run_session_loop`] is monomorphised over the
/// concrete session, so it never needs to box or move them across threads.
#[allow(async_fn_in_trait)]
pub trait AgentSession {
    /// Run ONE turn with `prompt`, resuming the same underlying agent session.
    ///
    /// Runs to natural completion (agent idle / process liveness) — NEVER a
    /// wall-clock cap. Returns the turn's output, or a [`TurnError`] on failure
    /// (propagated, never swallowed).
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput>;

    /// The stable id of the resumed session. Constant for the driver's life.
    fn session_id(&self) -> &str;
}

/// A prompt source (LISTEN) and optional output sink (REPLAY) for a driven
/// session.
///
/// `#[async_trait]` keeps the trait object-safe so [`run_session_loop`] can
/// accept a `&mut dyn Channel` (`C: Channel + ?Sized`). The `Send` supertrait
/// lets the default `publish_output` (which `async_trait` boxes as a `Send`
/// future) be invoked generically — and makes `dyn Channel` itself `Send`, so a
/// `Box<dyn Channel>` drives correctly.
#[async_trait]
pub trait Channel: Send {
    /// A stable identifier for this channel (for logging / correlation).
    fn id(&self) -> ChannelId;

    /// REPLAY (default no-op): publish a completed turn's output.
    ///
    /// Channels that don't echo output inherit this no-op and stay correct. A
    /// channel that *does* publish surfaces any failure as
    /// `Err(ChannelError::Publish(..))` — never a silent drop.
    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        let _ = out;
        Ok(())
    }

    /// LISTEN: yield the next prompt, or signal idle / closed.
    ///
    /// MUST return [`NextPrompt::Idle`] (not spin, not error) when there is
    /// simply nothing to run yet, and [`NextPrompt::Closed`] exactly once the
    /// channel is permanently done.
    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt>;
}

/// What a [`Channel`] wants the driver to do next.
#[derive(Debug)]
pub enum NextPrompt {
    /// A prompt is ready to run this turn.
    Ready(String),
    /// Nothing to run yet; the loop should wait for liveness/inbound, not spin.
    Idle,
    /// The channel is permanently closed; the loop should break.
    Closed,
}

/// The result of running one turn: the agent's response text captured verbatim
/// (exactly as the agent produced it — not a byte is transformed).
#[derive(Debug, Clone)]
pub struct TurnOutput {
    text: String,
}

impl TurnOutput {
    /// Wrap a response body as a turn output.
    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The agent's response for this turn, captured verbatim.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// An opaque, cheap-to-clone identifier for a [`Channel`], used for logging and
/// correlation. Construct one from any string-like value via `From`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelId(String);

impl From<&str> for ChannelId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ChannelId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A failure while running one turn.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// The agent process failed to spawn or exited non-zero.
    #[error("agent turn failed: {0}")]
    Exec(String),
    /// An I/O error while running or capturing the turn.
    #[error("turn I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The in-flight turn was pre-empted by an out-of-band stop.
    #[error("turn pre-empted")]
    Preempted,
}

/// A failure while receiving from, publishing to, or driving a [`Channel`].
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// Failed to read the next prompt.
    #[error("channel receive error: {0}")]
    Recv(String),
    /// Failed to publish a turn's output.
    #[error("channel publish error: {0}")]
    Publish(String),
    /// A turn run under this channel failed; the turn error surfaces unchanged.
    #[error("turn error: {0}")]
    Turn(#[from] TurnError),
    /// An underlying I/O error.
    #[error("channel I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Backoff applied between `Idle` polls so a channel that repeatedly reports
/// `Idle` cannot pin a CPU core.
///
/// A bare `yield_now()` only re-queues the task at the back of the runtime's
/// run queue; against a channel that keeps returning `Idle` it is
/// indistinguishable from a busy spin and burns ~100% of a core. Sleeping for a
/// short, bounded interval hands the core back to the runtime while keeping
/// re-poll latency low enough that transient idleness is imperceptible. There
/// is still **no** wall-clock timeout on the wait itself — only a floor on how
/// often we re-poll.
const IDLE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(5);

/// Drive `session` from `channel` until the channel closes.
///
/// * [`NextPrompt::Ready`] → run one turn (`session.run_turn(&p)`), then publish
///   its output (`channel.publish_output(&out)`). The turn fully completes
///   (run + publish) before the next prompt is requested.
/// * [`NextPrompt::Idle`] → sleep for a short, bounded [`IDLE_BACKOFF`], then
///   poll again. There is **no** wall-clock timeout on the wait. The backoff
///   hands the core back to the runtime so a channel that keeps reporting
///   `Idle` cannot busy-spin the CPU.
/// * [`NextPrompt::Closed`] → break and return `Ok(())`.
///
/// Any [`TurnError`] or [`ChannelError`] propagates out of the loop unchanged —
/// nothing is swallowed and there is no hidden retry, timer, or turn cap.
pub async fn run_session_loop<S, C>(session: &mut S, channel: &mut C) -> ChannelResult<()>
where
    S: AgentSession,
    C: Channel + ?Sized,
{
    loop {
        match channel.next_prompt().await? {
            NextPrompt::Ready(prompt) => {
                // Run to natural completion; a TurnError converts into a
                // ChannelError via `?` (From) and propagates unchanged.
                let out = session.run_turn(&prompt).await?;
                // REPLAY (default no-op). A publish failure fails fast.
                channel.publish_output(&out).await?;
            }
            // Nothing to run yet: sleep for a short bounded interval and re-poll.
            // No timeout; the backoff hands the core back so a channel stuck on
            // Idle cannot busy-spin. A correct channel returns Idle only
            // transiently, so the added latency is imperceptible in practice.
            NextPrompt::Idle => tokio::time::sleep(IDLE_BACKOFF).await,
            NextPrompt::Closed => break,
        }
    }
    Ok(())
}
