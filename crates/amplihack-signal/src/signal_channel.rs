//! [`SignalChannel`] — Signal on the generic turn loop (issue #910, PR-3).
//!
//! `SignalChannel` implements [`amplihack_turn::Channel`] so `amplihack signal
//! chat` can run on the crate-generic
//! [`amplihack_turn::run_session_loop`](amplihack_turn::run_session_loop)
//! instead of a bespoke `tokio::select!`. It is **behaviour-preserving**: every
//! externally observable Signal behaviour — the fail-closed inbound gate, the
//! bounded operator-configurable turn queue with evict-oldest, the `status`
//! command, `stop`/`kill` pre-emption, per-post membership re-verification,
//! outbound redaction, and echo suppression — is identical to the previous
//! hand-rolled loop. See `docs/signal-channel-turn-loop.md`.
//!
//! # Architecture: a thin handle over a single Signal I/O actor
//!
//! The generic driver loop is strictly sequential (`next_prompt` → `run_turn` →
//! `publish_output`), but the old loop accepted inbound Signal messages **while
//! a turn was running**. To preserve that, `SignalChannel` is only a handle: it
//! moves the [`SignalTransport`] and the [`Gate`] into a single background
//! **actor task** that exclusively owns both. The actor continuously calls
//! `transport.receive()` and feeds accepted prompts into a bounded queue that
//! [`SignalChannel::next_prompt`] drains; [`SignalChannel::publish_output`] does
//! not touch the transport directly but sends an **acked post request** to the
//! actor. Concentrating both directions in one owner keeps the
//! echo-suppression / membership window coherent by construction — there is
//! exactly one holder of `&mut transport` and `&mut gate`, so `evaluate`,
//! `record_outbound`, and the outbound `verify_and_post` are always serialized.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};

use amplihack_turn::{Channel, ChannelId, ChannelResult, NextPrompt, TurnOutput};

use crate::chat::control::{Control, parse_control};
use crate::chat::membership::{Membership, expected_members};
use crate::chat::outbound::redact_and_chunk;
use crate::chat::turn::PreemptSlot;
use crate::chat::verified_send;
use crate::config::SignalConfig;
use crate::gating::Gate;
use crate::transport::{Envelope, GroupId, SignalTransport};

/// The operator-configurable env var for the bounded turn-queue capacity.
const CAPACITY_ENV: &str = "AMPLIHACK_SIGNAL_INBOX_CAPACITY";

/// State shared between the background actor (producer / shutdown signaller) and
/// the [`SignalChannel`] handle (consumer via `next_prompt`).
#[derive(Default)]
struct SharedState {
    /// The bounded turn queue the actor feeds and `next_prompt` drains (FIFO).
    queue: VecDeque<String>,
    /// Set once by the actor on `stop`/`kill` or transport end; `next_prompt`
    /// then returns [`NextPrompt::Closed`].
    closed: bool,
    /// Optional session label used in the `status` line (set by the CLI).
    session_label: String,
}

/// An acked outbound post request sent from `publish_output` / the announcement
/// path to the Signal I/O actor (the sole holder of `&mut transport`/`&mut gate`).
struct PostRequest {
    body: String,
    ack: oneshot::Sender<()>,
}

/// A [`amplihack_turn::Channel`] over a per-session operator-only Signal group.
///
/// A thin handle to a single background Signal I/O actor (see the module docs).
/// The handle owns only the request sender, the shared queue state, and the
/// group id; the actor exclusively owns the transport and the [`Gate`].
pub struct SignalChannel {
    /// The resolved operator-only group (for [`Channel::id`] / logging).
    group_id: GroupId,
    /// Sends acked post requests to the actor.
    req_tx: mpsc::UnboundedSender<PostRequest>,
    /// Shared bounded turn queue + shutdown flag + status label.
    state: Arc<Mutex<SharedState>>,
    /// The background actor task; aborted on drop so a channel cannot leak a
    /// task blocked on `transport.receive()`.
    actor: Option<tokio::task::JoinHandle<()>>,
}

impl SignalChannel {
    /// Default bounded turn-queue capacity (flood resistance) used when
    /// `AMPLIHACK_SIGNAL_INBOX_CAPACITY` is absent or invalid.
    pub const DEFAULT_CAPACITY: usize = 32;

    /// The effective default bounded turn-queue capacity.
    ///
    /// Operator-configurable via `AMPLIHACK_SIGNAL_INBOX_CAPACITY`. Whitespace,
    /// non-numeric, negative, or zero values fall back to [`Self::DEFAULT_CAPACITY`]
    /// — never unbounded, never disabled. This is the sole surviving piece of
    /// the deleted `session_channel` module's capacity policy, preserved verbatim.
    #[must_use]
    pub fn default_capacity() -> usize {
        std::env::var(CAPACITY_ENV)
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .filter(|capacity| *capacity > 0)
            .unwrap_or(Self::DEFAULT_CAPACITY)
    }

    /// Bind an already-connected `transport` to a per-session operator-only
    /// `group_id`, moving the transport and a fresh [`Gate`] into a single
    /// background Signal I/O actor.
    ///
    /// `capacity` is the bounded turn-queue size (the caller resolves it from
    /// `--inbox-capacity` / [`Self::default_capacity`]). `preempt` is the shared
    /// child-bound `stop`/`kill` trigger, also held by the turn driver.
    #[must_use]
    pub fn new(
        transport: SignalTransport,
        cfg: &SignalConfig,
        group_id: GroupId,
        preempt: PreemptSlot,
        capacity: usize,
    ) -> Self {
        let expected = expected_members(cfg);
        let gate = Gate::new(cfg, group_id.as_str());
        let state = Arc::new(Mutex::new(SharedState::default()));
        let (req_tx, req_rx) = mpsc::unbounded_channel();

        let actor = tokio::spawn(actor_loop(ActorContext {
            transport,
            gate,
            expected,
            group_id: group_id.clone(),
            preempt,
            capacity,
            req_rx,
            state: state.clone(),
        }));

        Self {
            group_id,
            req_tx,
            state,
            actor: Some(actor),
        }
    }

    /// Post the initial "session started" announcement before the loop starts,
    /// through the same fail-closed verify → redact → chunk path as every other
    /// outbound post. Keeps the observable startup sequence unchanged.
    pub async fn announce_session_started(&mut self, announcement: &str) {
        self.post(announcement.to_string()).await;
    }

    /// Seed the opening turn prompt (the CLI topic) directly into the queue.
    ///
    /// The topic is a trusted CLI argument, not attacker-influenced inbound
    /// Signal text, so it bypasses the inbound gate — exactly as the old loop
    /// spawned the first turn from `args.topic` directly.
    pub fn seed_first_prompt(&self, prompt: String) {
        self.state
            .lock()
            .expect("signal channel state mutex poisoned")
            .queue
            .push_back(prompt);
    }

    /// Set the session id surfaced in the `status` line (best-effort labelling).
    pub fn set_session_id(&self, session_id: &str) {
        self.state
            .lock()
            .expect("signal channel state mutex poisoned")
            .session_label = session_id.to_string();
    }

    /// Send an acked post request to the actor and await its completion.
    ///
    /// The actor performs the fail-closed `verify_and_post` inline. A withhold
    /// (unverified membership) or send error is surfaced by the actor locally
    /// and is **not** fatal — matching the old `verify_and_post`, which returned
    /// `()` on a withhold. If the actor is gone the post is dropped and surfaced.
    async fn post(&self, body: String) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.req_tx.send(PostRequest { body, ack: ack_tx }).is_err() {
            eprintln!("signal chat: cannot post — the Signal I/O actor has shut down.");
            return;
        }
        // Await the actor's ack so the turn's output is fully published before
        // the loop requests the next prompt. A dropped ack (actor gone) is a
        // surfaced no-op, never a panic.
        let _ = ack_rx.await;
    }
}

impl Drop for SignalChannel {
    fn drop(&mut self) {
        if let Some(handle) = self.actor.take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl Channel for SignalChannel {
    fn id(&self) -> ChannelId {
        ChannelId::from(self.group_id.as_str())
    }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt> {
        let mut state = self
            .state
            .lock()
            .expect("signal channel state mutex poisoned");
        // `stop`/`kill` and transport end take precedence: the old loop broke
        // immediately on `stop`, dropping any still-pending prompts.
        if state.closed {
            Ok(NextPrompt::Closed)
        } else if let Some(prompt) = state.queue.pop_front() {
            Ok(NextPrompt::Ready(prompt))
        } else {
            // Nothing to run yet: the driver waits (bounded backoff, NO
            // wall-clock cap) while the actor keeps accepting inbound frames.
            Ok(NextPrompt::Idle)
        }
    }

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        // If the channel is already closed (e.g. `stop` quit the group and shut
        // the actor down mid-turn, so the in-flight turn returned preempted),
        // there is nothing left to post to — skip silently instead of surfacing
        // a spurious `cannot post — actor shut down` diagnostic.
        if self
            .state
            .lock()
            .expect("signal channel state mutex poisoned")
            .closed
        {
            return Ok(());
        }
        let text = out.text();
        let body = if text.trim().is_empty() {
            "(turn produced no output)".to_string()
        } else {
            text.to_string()
        };
        self.post(body).await;
        Ok(())
    }
}

/// Everything the background Signal I/O actor exclusively owns.
struct ActorContext {
    transport: SignalTransport,
    gate: Gate,
    expected: Vec<String>,
    group_id: GroupId,
    preempt: PreemptSlot,
    capacity: usize,
    req_rx: mpsc::UnboundedReceiver<PostRequest>,
    state: Arc<Mutex<SharedState>>,
}

/// The single background task that owns the transport and gate and services
/// **both** directions, so inbound is still accepted while a turn runs.
async fn actor_loop(mut ctx: ActorContext) {
    loop {
        tokio::select! {
            // Prefer acking in-flight outbound posts promptly, then drain inbound.
            biased;

            maybe_req = ctx.req_rx.recv() => {
                let Some(req) = maybe_req else {
                    // The handle was dropped; no more posts can arrive. Shut down.
                    break;
                };
                post_body(
                    &mut ctx.transport,
                    &ctx.group_id,
                    &ctx.expected,
                    &mut ctx.gate,
                    &req.body,
                )
                .await;
                let _ = req.ack.send(());
            }

            recv = ctx.transport.receive() => {
                match recv {
                    Ok(Some(env)) => {
                        // Fail-closed inbound gate (empty allowlist denies all);
                        // echo-suppressed and rejected frames yield `None`.
                        let Some(body) = ctx.gate.evaluate(&env) else { continue };
                        match parse_control(&body) {
                            Control::Status => {
                                let status = {
                                    let state = ctx
                                        .state
                                        .lock()
                                        .expect("signal channel state mutex poisoned");
                                    let label = if state.session_label.is_empty() {
                                        ctx.group_id.as_str()
                                    } else {
                                        state.session_label.as_str()
                                    };
                                    format!(
                                        "status: session {} | queue depth {} | membership: verifying before each post",
                                        label,
                                        state.queue.len(),
                                    )
                                };
                                post_body(
                                    &mut ctx.transport,
                                    &ctx.group_id,
                                    &ctx.expected,
                                    &mut ctx.gate,
                                    &status,
                                )
                                .await;
                            }
                            Control::Stop => {
                                eprintln!(
                                    "signal chat: stop received; terminating child and closing group."
                                );
                                preempt_child(&ctx.preempt);
                                let _ = ctx.transport.quit_group(&ctx.group_id).await;
                                ctx.state
                                    .lock()
                                    .expect("signal channel state mutex poisoned")
                                    .closed = true;
                                break;
                            }
                            Control::Prompt(prompt) => {
                                let mut state = ctx
                                    .state
                                    .lock()
                                    .expect("signal channel state mutex poisoned");
                                audit_accepted(&state.session_label, &env, &prompt);
                                state.queue.push_back(prompt);
                                // Bounded: evict oldest at capacity (operator policy).
                                while state.queue.len() > ctx.capacity {
                                    state.queue.pop_front();
                                    eprintln!(
                                        "signal chat: turn queue at capacity ({}); dropped oldest pending prompt.",
                                        ctx.capacity
                                    );
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        eprintln!("signal chat: receive stream closed; shutting down.");
                        ctx.state
                            .lock()
                            .expect("signal channel state mutex poisoned")
                            .closed = true;
                        break;
                    }
                    Err(e) => {
                        // Transient receive error: surface it and keep going,
                        // exactly as the old loop did (no silent disable).
                        eprintln!("signal chat: receive error: {e}");
                        continue;
                    }
                }
            }
        }
    }
}

/// Re-verify group membership **fail closed**, then relay `body` (redacted +
/// chunked), re-checking membership immediately before *every* chunk. On any
/// verification failure or send error the remaining chunks are withheld and the
/// reason is surfaced locally — nothing is silently dropped. Identical to the
/// old CLI `verify_and_post`.
async fn post_body(
    transport: &mut SignalTransport,
    group_id: &GroupId,
    expected: &[String],
    gate: &mut Gate,
    body: &str,
) {
    for chunk in redact_and_chunk(body) {
        match verified_send(transport, group_id, expected, &chunk).await {
            Ok(Membership::Verified) => gate.record_outbound(&chunk),
            Ok(Membership::Unverified(reason)) => {
                eprintln!(
                    "signal chat: WITHHOLDING outbound relay — group membership unverified before post: {reason}"
                );
                return;
            }
            Err(e) => {
                eprintln!("signal chat: failed to post to group: {e}");
                return;
            }
        }
    }
}

/// Pre-empt the in-flight turn, if any, by firing its child-bound trigger.
///
/// Takes the one-shot sender out of the shared [`PreemptSlot`] and sends `()`.
/// The turn task then kills its **owned** child, immune to PID reuse. A no-op if
/// no turn is in flight.
fn preempt_child(preempt: &PreemptSlot) {
    if let Some(tx) = preempt.lock().expect("preempt mutex not poisoned").take() {
        let _ = tx.send(());
    }
}

/// Audit-log an accepted prompt (redacted) to the local terminal, mirroring the
/// old `run_chat_async` audit trail: every accepted operator prompt is recorded
/// (sender, device, redacted preview) before it is queued as a turn.
fn audit_accepted(session_label: &str, env: &Envelope, prompt: &str) {
    let sender = env.source.as_deref().unwrap_or_default();
    let device = env.source_device;
    let redacted = crate::chat::outbound::redact_for_relay(prompt);
    let preview: String = redacted.chars().take(120).collect();
    tracing::info!(
        session_id = session_label,
        sender,
        device = device.unwrap_or(0),
        "signal chat accepted prompt: {preview}"
    );
    eprintln!("signal chat: accepted prompt from {sender} (device {device:?}): {preview}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the capacity tests: cargo runs tests as parallel threads in
    /// one process and env vars are process-global, so without this lock the
    /// `default_capacity_*` tests race on `CAPACITY_ENV` and flake.
    static CAPACITY_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_capacity_env() -> std::sync::MutexGuard<'static, ()> {
        CAPACITY_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded within this test; restored on drop.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded within this test; restored on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                // SAFETY: restoring the pre-test value on the same thread.
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn default_capacity_honours_valid_operator_value() {
        let _serial = lock_capacity_env();
        let _guard = EnvGuard::set(CAPACITY_ENV, "7");
        assert_eq!(SignalChannel::default_capacity(), 7);
    }

    #[test]
    fn default_capacity_falls_back_on_invalid_values() {
        let _serial = lock_capacity_env();
        for bad in ["0", "-1", "   ", "not-a-number", "3.5", ""] {
            let _guard = EnvGuard::set(CAPACITY_ENV, bad);
            assert_eq!(
                SignalChannel::default_capacity(),
                SignalChannel::DEFAULT_CAPACITY,
                "invalid env value {bad:?} must fall back to DEFAULT_CAPACITY"
            );
        }
        assert_eq!(SignalChannel::DEFAULT_CAPACITY, 32);
    }

    #[test]
    fn default_capacity_falls_back_when_absent() {
        let _serial = lock_capacity_env();
        let _guard = EnvGuard::unset(CAPACITY_ENV);
        assert_eq!(
            SignalChannel::default_capacity(),
            SignalChannel::DEFAULT_CAPACITY
        );
    }
}
