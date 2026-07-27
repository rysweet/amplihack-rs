# Signal on the Generic Turn Loop (`SignalChannel`)

> **Status:** shipped as PR-3 of issue #910. This document describes the
> finished state: `amplihack signal chat` no longer hand-rolls its own
> `tokio::select!` turn loop. It runs on the crate-generic
> [`amplihack_turn::run_session_loop`](#the-generic-driver-loop), driving a
> [`SignalChannel`](#signalchannel) that implements the reusable
> [`amplihack_turn::Channel`](#the-channel-contract) trait. The change is
> **behavior-preserving**: every externally observable Signal behavior — the
> initial "session started" post, per-turn post-on-completion, the fail-closed
> inbound gate, the bounded operator-configurable queue with evict-oldest, the
> `status` command, and `stop`/`kill` pre-emption — is byte-for-byte identical
> to the previous implementation. The PR-1 characterization tests (INV-3..7)
> pass unchanged.

This feature is compiled only under the `signal` cargo feature (default
**OFF**). With the feature off there is zero runtime cost.

---

## Contents

- [Why this exists](#why-this-exists)
- [Architecture](#architecture)
- [The generic driver loop](#the-generic-driver-loop)
- [The `Channel` contract](#the-channel-contract)
- [`SignalChannel`](#signalchannel)
  - [Construction](#construction)
  - [`next_prompt` — LISTEN](#next_prompt--listen)
  - [`publish_output` — REPLAY](#publish_output--replay)
  - [The background Signal I/O actor](#the-background-signal-io-actor)
  - [Queue capacity policy (`default_capacity`)](#queue-capacity-policy-default_capacity)
- [`AgentSession` for the Copilot turn driver](#agentsession-for-the-copilot-turn-driver)
- [`run_chat_async` wiring](#run_chat_async-wiring)
- [Configuration](#configuration)
- [Behavior-preservation contract](#behavior-preservation-contract)
- [Removed: the dead cross-process `session_channel`](#removed-the-dead-cross-process-session_channel)
- [Testing](#testing)
- [FAQ](#faq)

---

## Why this exists

`amplihack signal chat` and the reusable turn primitives grew up together, so
the live loop in `crates/amplihack-cli/src/commands/signal/chat.rs` was a
bespoke `tokio::select!` that interleaved "receive an inbound Signal frame" with
"a serialized Copilot turn is running." That loop re-implemented, inline, the
exact LISTEN → run turn → REPLAY cadence that PR-2 factored into the
agent-generic [`amplihack-turn`](signal-channel.md#crate-api-reference) crate
(`AgentSession` + `Channel` + `run_session_loop`).

PR-3 finishes the refactor: the Signal specifics (transport, gating, group,
membership allowlist, bounded queue, control phrases) are packaged as a
`SignalChannel` that implements the generic `Channel` trait, and the loop itself
becomes a single call to `run_session_loop`. This deletes duplicated
orchestration, makes the Signal path share the same tested driver as every
future channel, and removes a large block of genuinely dead cross-process code
(`session_channel.rs`) that no live path ever constructed.

Nothing an operator can observe changes. The value is entirely internal:
one driver loop, one place for turn semantics, less dead code.

---

## Architecture

```
                         amplihack signal chat <topic>
                                     │
              connect / validate / create group / announce
                                     │
                                     ▼
   ┌─────────────────────────────────────────────────────────────────┐
   │  amplihack_turn::run_session_loop(&mut session, &mut channel)     │
   │                                                                   │
   │   next_prompt() ──► Ready(p) ──► session.run_turn(p) ──► publish  │
   │        ▲                                                    │      │
   │        └──────────────── Idle (bounded backoff) ◄───────────┘      │
   │                            Closed ──► break                        │
   └─────────────────────────────────────────────────────────────────┘
        │                                            │
        │ AgentSession                               │ Channel
        ▼                                            ▼
  SerialTurnDriver<CopilotTurnRunner>          SignalChannel  (handle only)
  (copilot --session-id <uuid> …)              ├─ req_tx  → actor (acked posts)
                                               ├─ queue_rx ← actor (prompts)
                                               └─ PreemptSlot (stop/kill)
                                                         │  request/ack
                                                         ▼
                                               ┌─ Signal I/O actor (single task) ─┐
                                               │  owns SignalTransport (JSON-RPC)  │
                                               │  owns Gate (fail-closed inbound)  │
                                               │  owns expected-member allowlist   │
                                               │                                   │
                                               │  transport.receive() → evaluate() │
                                               │    → parse_control → enqueue       │
                                               │  post request → verify_and_post    │
                                               │    → record_outbound (echo window) │
                                               └───────────────────────────────────┘
                                 (accept-during-turn preserved)
```

The single behavioral subtlety the generic loop must preserve is
**accept-during-turn**: the old `select!` accepted inbound Signal messages
*while* a turn was running. `run_session_loop` is strictly sequential
(`next_prompt` → `run_turn` → `publish_output`), so `SignalChannel` moves **all
Signal I/O — both receive and send — into a single background actor task** that
exclusively owns the transport and the `Gate`. The actor keeps calling
`transport.receive()` and feeds accepted prompts into a bounded queue whose
receiving end `next_prompt()` drains. `publish_output` does **not** touch the
transport or gate directly; it sends an **acked request** to the actor, which
performs `verify_and_post` and `record_outbound` inline. Concentrating both
directions in one owner is what keeps the echo-suppression / membership window
coherent: a single task serializes `evaluate`, `record_outbound`, and
`verify_and_post`, so no two paths ever contend for `&mut transport` or
`&mut gate`. Inbound messages are still accepted continuously, including
mid-turn.

---

## The generic driver loop

Provided unchanged by `amplihack-turn` (PR-2):

```rust
pub async fn run_session_loop<S, C>(session: &mut S, channel: &mut C) -> ChannelResult<()>
where
    S: AgentSession,
    C: Channel + ?Sized,
```

- `NextPrompt::Ready(p)` → `session.run_turn(&p)` to natural completion, then
  `channel.publish_output(&out)`. The turn fully completes (run + publish)
  before the next prompt is requested.
- `NextPrompt::Idle` → sleep a short bounded `IDLE_BACKOFF` (5 ms), then re-poll.
  **No wall-clock timeout** on the wait; the backoff only prevents a busy-spin.
- `NextPrompt::Closed` → break, return `Ok(())`.

Any `TurnError` or `ChannelError` propagates out unchanged — no swallowed
errors, no hidden retries, no turn cap.

---

## The `Channel` contract

`SignalChannel` implements this trait from `amplihack-turn` verbatim:

```rust
#[async_trait]
pub trait Channel: Send {
    fn id(&self) -> ChannelId;

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        let _ = out;
        Ok(()) // default no-op; SignalChannel overrides it
    }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt>;
}
```

`Channel: Send` (and the `#[async_trait]`-boxed futures) is what lets the loop
own the channel across `.await` points. `SignalChannel` is `Send`; its
background actor is a spawned `tokio` task communicating over channels.

`NextPrompt` is the three-way answer:

```rust
pub enum NextPrompt { Ready(String), Idle, Closed }
```

---

## `SignalChannel`

`crates/amplihack-signal/src/signal_channel.rs` (feature = `signal`).

`SignalChannel` encapsulates exactly what the old `run_chat_async` loop did, but
it is only a **thin handle** to a single background **Signal I/O actor**. The
actor exclusively owns the transport and the `Gate` (design rule R1/D3: one
owner of Signal I/O so `evaluate`, `record_outbound`, and `verify_and_post` are
serialized and the echo-suppression/membership window stays coherent).

The `SignalChannel` handle owns:

| Field | Purpose |
|---|---|
| `req_tx` (post request sender) | Sends acked post/status requests to the actor; the actor performs `verify_and_post` + `record_outbound`. Each request carries a `oneshot` reply channel so `publish_output` awaits the actor's result. |
| `queue_rx` (prompt receiver) | Receiving end of the bounded turn queue the actor feeds. `next_prompt` drains this. |
| `PreemptSlot` | Child-bound `stop`/`kill` trigger, shared with the turn driver. |
| `group_id` | The resolved per-session operator-only group (for `id()` / logging). |

The background **Signal I/O actor** (single spawned task) exclusively owns:

| Owned by actor | Purpose |
|---|---|
| Signal `transport` | signal-cli JSON-RPC client — the **only** holder of `&mut transport` (both `receive()` and outbound `send_group`). |
| `Gate` | Fail-closed inbound decision + outbound echo record (`crates/amplihack-signal/src/gating.rs` — **untouched**). The **only** holder of `&mut gate`. |
| expected-member allowlist | `chat::membership::expected_members(cfg)`, re-checked before every post. |
| bounded turn queue (sender) | A `VecDeque<String>`/mpsc whose capacity comes from `--inbox-capacity` / [`default_capacity`](#queue-capacity-policy-default_capacity). Evict-oldest at capacity. |

```rust
impl amplihack_turn::Channel for SignalChannel {
    fn id(&self) -> ChannelId { /* the group id, for logging/correlation */ }

    async fn next_prompt(&mut self) -> ChannelResult<NextPrompt> { /* drain queue_rx */ }

    async fn publish_output(&mut self, out: &TurnOutput) -> ChannelResult<()> {
        /* send an acked post request to the actor; await its verify_and_post result */
    }
}
```

### Construction

`SignalChannel::new` binds an already-connected transport to a group, an
allowlist, a gate, a pre-empt slot, and a bounded-queue capacity. It moves the
transport and gate **into** the spawned Signal I/O actor and returns a handle
holding only the request sender, the queue receiver, and the pre-empt slot:

```rust
let mut channel = SignalChannel::new(
    transport,          // connected SignalTransport — MOVED into the actor
    &cfg,               // SignalConfig (drives the Gate + expected members)
    group_id,           // resolved operator-only group
    preempt.clone(),    // shared PreemptSlot (stop/kill)
    capacity,           // args.inbox_capacity.unwrap_or_else(SignalChannel::default_capacity)
);
```

The channel does **not** perform the initial connect/validate/create-group
steps or the first "session started" post — those stay in `run_chat_async`
(see [`run_chat_async` wiring](#run_chat_async-wiring)) so the observable
startup sequence is unchanged.

### `next_prompt` — LISTEN

`next_prompt()` drains the shared bounded queue and translates its state into a
`NextPrompt`. Control phrases are classified by the actor on the receive side
(via `chat::control::parse_control`) before a body ever becomes a queued prompt,
so a `status` or `stop` word is never mistaken for a turn prompt:

1. **`status` command** → the **actor** posts the current status line (session
   id, in-flight / idle, queue depth, "membership: verifying before each post")
   through its own `verify_and_post` path; `next_prompt` does **not** return a
   `Ready` prompt. Status never enqueues a turn.
2. **`stop` / `kill` command** → fire the `PreemptSlot` (pre-empting any
   in-flight turn), the actor `quit_group`s, and `next_prompt` returns
   `NextPrompt::Closed`.
3. **A queued prompt** → return `NextPrompt::Ready(prompt)` (FIFO order).
4. **Empty queue** → return `NextPrompt::Idle`. The loop waits (bounded backoff,
   **no wall-clock cap**); the actor keeps accepting inbound frames.
5. **Shutdown / transport end** → return `NextPrompt::Closed` exactly once.

### `publish_output` — REPLAY

`publish_output(out)` does **not** hold the transport or gate itself. It sends
an **acked post request** to the Signal I/O actor over `req_tx` (with a
`oneshot` reply channel) and awaits the result. The actor — the sole owner of
`&mut transport` and `&mut gate` — performs the existing `verify_and_post`
behavior inline, which keeps every outbound step serialized with inbound
`evaluate`/`record_outbound`:

- **fail-closed membership re-check** immediately before *every* post (and every
  chunk), via `chat::verified_send`. Any ambiguity — RPC error, timeout,
  unexpected extra member, missing expected member — **withholds** the post and
  surfaces the reason locally; nothing is sent.
- **redact → chunk**: `chat::outbound::redact_and_chunk` runs on **all**
  outbound bodies (turn output, status line, and the "session started"
  announcement) before any `send_group`.
- **echo-window record**: on a verified send, `gate.record_outbound(&chunk)` so
  the synced-back copy of our own post is not re-ingested as inbound.
- **logging**: the same success / withhold / error log lines as before.

The actor sends the outcome back over the `oneshot`; `publish_output` maps it to
`ChannelResult<()>`. Empty turn output posts `"(turn produced no output)"`; a
`TurnError` is surfaced to the group as `"turn failed: {e}"` and the chat stays
alive (the next turn resumes the **same** session, context preserved) —
identical to today.

### The background Signal I/O actor

A single spawned task exclusively owns `transport` and `gate` and services
**both** directions, so accept-during-turn is preserved and no other path ever
needs `&mut transport`/`&mut gate`. It multiplexes (via `tokio::select!`) the
inbound receive stream and the inbound post-request channel from
`publish_output`/status:

```
loop select:
  # inbound receive
  transport.receive() =>
    Ok(Some(env)) => match gate.evaluate(&env):     # fail-closed
        Some(body) if non-empty =>
            parse_control(body):
                Status  => verify_and_post(status line)   # actor posts directly
                Stop    => signal shutdown (queue Closed)
                Prompt  => enqueue; if len > capacity: pop_front + WARN
        _ => continue                                # rejected / echo-suppressed
    Ok(None) => signal Closed (receive stream closed); stop
    Err(e)   => log "receive error"; continue        # transient, non-fatal

  # outbound post request from publish_output (acked)
  req_rx.recv() => PostRequest{ body, reply } =>
    result = verify_and_post(body)                   # membership re-check → redact/chunk
                                                     #   → send_group → record_outbound
    reply.send(result)                               # oneshot ack back to publish_output
```

- **Single owner of Signal I/O**: because the actor is the only holder of
  `&mut transport` and `&mut gate`, `evaluate` (inbound), `record_outbound`
  (echo window), and `verify_and_post` (outbound) are all serialized on one
  task. The echo-suppression and membership windows stay coherent by
  construction — there is exactly one writer of the outbound record.
- **Fail-closed gate**: `gate.evaluate` rejects anything not from an allowlisted
  sender/device in this group; an **empty allowlist denies all inbound**.
- **Evict-oldest at capacity**: when the queue is full a new accepted prompt
  evicts the oldest, with the existing warning log
  (`"turn queue at capacity (N); dropped oldest pending prompt."`). No new fixed
  cap is introduced — capacity stays operator policy.
- **No silent fallbacks**: a transport receive error is logged and the actor
  continues (matching today); a closed stream is surfaced as `Closed`.

The actor and `next_prompt`/`publish_output` coordinate purely via in-process
channels: an mpsc **prompt queue** (actor → `next_prompt`), an mpsc
**post-request channel** with per-request `oneshot` acks (`publish_output`/status
→ actor), and a shutdown signal. Because the actor is the **single owner** of
both `transport` and `gate`, there is never a second holder of `&mut transport`
or `&mut gate`; the echo-suppression window and the fail-closed membership
checks stay coherent by construction.

### Queue capacity policy (`default_capacity`)

The only surviving piece of the deleted `session_channel` module is its
bounded-queue capacity policy, relocated onto `SignalChannel`:

```rust
impl SignalChannel {
    /// Fallback capacity when AMPLIHACK_SIGNAL_INBOX_CAPACITY is absent/invalid.
    pub const DEFAULT_CAPACITY: usize = 32;

    /// Capacity for a new channel's turn queue.
    ///
    /// Operator-configurable via AMPLIHACK_SIGNAL_INBOX_CAPACITY. Whitespace-only,
    /// non-numeric, or zero values fall back to DEFAULT_CAPACITY — never
    /// unbounded, never disabled.
    #[must_use]
    pub fn default_capacity() -> usize {
        std::env::var("AMPLIHACK_SIGNAL_INBOX_CAPACITY")
            .ok()
            .and_then(|raw| raw.trim().parse::<usize>().ok())
            .filter(|c| *c > 0)
            .unwrap_or(Self::DEFAULT_CAPACITY)
    }
}
```

This preserves the previous `Inbox::default_capacity()` semantics exactly (env
override → trimmed → parsed → must be `> 0` → else `32`). `chat.rs` resolves the
effective capacity as `args.inbox_capacity.unwrap_or_else(SignalChannel::default_capacity)`.

---

## `AgentSession` for the Copilot turn driver

The Copilot side is the existing `SerialTurnDriver<CopilotTurnRunner>` (relocated
into `amplihack-turn` by PR-2), presented to the loop as an `AgentSession`.
`SerialTurnDriver::run_turn` is an inherent `&self` method returning
`io::Result<String>`; the `AgentSession` impl is a **thin `&mut self` adapter**
(the trait requires `&mut self`) that calls it and maps the result into
`TurnResult<TurnOutput>`:

```rust
impl AgentSession for SerialTurnDriver<CopilotTurnRunner> {
    async fn run_turn(&mut self, prompt: &str) -> TurnResult<TurnOutput> {
        // Delegates to the inherent `&self` SerialTurnDriver::run_turn(prompt),
        // then maps io::Result<String> -> TurnResult<TurnOutput>:
        //   Ok(stdout)                         => Ok(TurnOutput::from(stdout))
        //   Err(e) if e.kind() == Interrupted  => Err(TurnError::Preempted)  // stop/kill
        //   Err(e) (spawn/exec failure)        => Err(TurnError::Exec(e.to_string()))
        //   Err(e) (other io)                  => Err(TurnError::Io(e))
    }
    fn session_id(&self) -> &str { /* the pinned v4 UUID */ }
}
```

The `io::ErrorKind::Interrupted → TurnError::Preempted` mapping is how a fired
`PreemptSlot` (`stop`/`kill`) becomes a clean pre-emption rather than an error;
all other io errors are surfaced (never swallowed) as `Exec`/`Io`. The pinned
session UUID and the `--allow-tool` scoped allowlist behavior are unchanged;
every turn resumes the same session so context is preserved across the whole
conversation.

---

## `run_chat_async` wiring

The rewritten entry point keeps every startup effect and delegates the loop:

```rust
async fn run_chat_async(args: SignalChatArgs) -> Result<(), ChatError> {
    let cfg = SignalConfig::load()?;                       // 1. config / linked check
    validate_endpoint(&cfg.endpoint, args.unsafe_remote_endpoint)?; // 2. loopback safety
    probe_copilot_resume()?;                               // 3. --session-id resume probe
    let mut transport = connect_daemon(&cfg.endpoint, retry_budget).await?; // 4. connect

    let group_id = transport.create_group(&group_name).await?; // 5. fresh operator-only group
    let session_id = uuid::Uuid::new_v4().to_string();     // 6. pinned session
    let allowlist = ToolAllowlist::from_flags(&args.allow_tool, args.dangerous_all_tools);

    let preempt: PreemptSlot = Arc::new(Mutex::new(None));
    let mut session = SerialTurnDriver::new(
        CopilotTurnRunner::new(COPILOT_BIN, preempt.clone()),
        &session_id,
        allowlist.clone(),
    );

    let mut channel = SignalChannel::new(transport, &cfg, group_id, preempt, capacity);

    // 7. Initial announce (redacted + chunked + membership-verified), still here:
    channel.announce_session_started(&announcement).await;   // "session started"

    // 8. Drive the generic loop. Topic is the opening prompt (seeded into the queue).
    channel.seed_first_prompt(args.topic.clone());
    amplihack_turn::run_session_loop(&mut session, &mut channel).await?;
    Ok(())
}
```

- Steps 1–6 (config, loopback validation, resume probe, daemon connect, group
  create, session id + allowlist) are unchanged.
- The **initial "session started" post** happens before the loop, through the
  same fail-closed verify-redact-chunk path.
- The **topic** is seeded as the first prompt so the opening turn fires exactly
  as before.
- The loop is now a single `run_session_loop` call.

---

## Configuration

No configuration changes. The bounded queue stays operator policy:

| Setting | Where | Default | Meaning |
|---|---|---|---|
| `--inbox-capacity <N>` | `amplihack signal chat` flag | `32` | Bounded turn-queue capacity. |
| `AMPLIHACK_SIGNAL_INBOX_CAPACITY` | env var | `32` | Same, when the flag is omitted. Whitespace/invalid/`0` → `32`. |

Resolution order is unchanged: the CLI flag wins; otherwise
`SignalChannel::default_capacity()` reads the env var. There is **no new fixed
cap** and **no wall-clock turn timeout** — a full queue evicts the oldest and an
empty queue waits indefinitely (bounded-backoff re-poll only).

---

## Behavior-preservation contract

Every row below is observable to an operator and is **identical** before and
after PR-3:

| Observable behavior | Guarantee |
|---|---|
| Initial "session started" post | Posted once, before the loop, redacted + chunked + membership-verified. |
| Per-turn output | Posted on turn completion, in order; empty output → `"(turn produced no output)"`; failure → `"turn failed: …"` (chat stays alive). |
| Inbound gate | Fail-closed: only allowlisted sender/device in this group; empty allowlist denies all. |
| Bounded queue | Operator-configurable capacity; evict-oldest with the existing warning log. |
| Accept-during-turn | Inbound messages are accepted while a turn is running (background actor). |
| `status` command | Posts the status line; does **not** enqueue a turn. |
| `stop` / `kill` | Pre-empts the in-flight turn, quits the group, ends the chat. |
| Outbound membership re-check | Fail-closed **before every post/chunk**; withholds on any ambiguity. |
| Echo suppression | Our own synced-back posts are recorded and not re-ingested. |
| Secret redaction | Applied to **all** outbound before chunking. |
| No wall-clock cap | Turns run to natural completion; idle waits indefinitely. |
| Exit-code taxonomy | `ChatError` codes 1–5 (+ `0`) unchanged. |

The PR-1 characterization tests (INV-3 fail-closed inbound gate, INV-4
operator-config queue capacity eviction, INV-5 session-id reuse, INV-6 no
per-turn wall-clock cap, INV-7 auto-mode continuation) all pass **unchanged**.

---

## Removed: the dead cross-process `session_channel`

`crates/amplihack-signal/src/session_channel.rs` is **deleted**. It defined
`SignalSession` and a file-backed `Inbox` intended as a cross-process seam (a
detached subscriber pushing into a JSON file, a hook process draining it). That
always-on channel was removed earlier (see
[Signal Channel](signal-channel.md)); `SignalSession` was never constructed in
any live path, and `pump_once` / `announce` / `Inbox::push` / `Inbox::drain`
were exercised only by tests.

PR-3 removes it entirely:

- **Deleted**: `session_channel.rs`, its `pub mod session_channel;` declaration
  in `lib.rs`, and its in-crate unit tests.
- **Relocated (only)**: the `default_capacity()` queue-size policy constant,
  now [`SignalChannel::default_capacity`](#queue-capacity-policy-default_capacity).
  It remains operator-configurable; no new fixed cap.
- **Dependency dropped**: `amplihack-state` (the `AtomicJsonFile` backing store)
  is no longer a dependency of `amplihack-signal` — it was used only by the
  file-backed inbox. `amplihack-types` stays (used elsewhere).
- **Docs updated**: the `session_channel` / `SignalSession` / `Inbox` references
  in `lib.rs` and `fake_endpoint.rs` module docs now describe `SignalChannel`.
- **E2E coverage re-pointed, not lost**: the end-to-end relay tests that drove
  the old `SignalSession` path (`tests/session_relay_it.rs`) and the INV-4
  capacity/evict-oldest assertions (`tests/session_channel_it.rs`) are migrated
  to drive `SignalChannel` / `run_chat_async` over the offline
  `FakeSignalEndpoint`. No coverage is deleted outright.

---

## Testing

All tests remain hermetic and offline against
[`FakeSignalEndpoint`](signal-channel.md#offline-testing-fake-json-rpc-endpoint) —
no test touches the real Signal network.

```bash
# Feature ON — the crates that carry Signal + turn code.
cargo test -p amplihack-turn
cargo test -p amplihack-signal --features signal
cargo test -p amplihack-cli    --features signal

# Whole-workspace gates (must all pass):
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features signal -- -D warnings
cargo build --workspace
cargo test  --workspace
```

New in-crate `#[cfg(all(test, feature = "signal"))]` unit tests for
`SignalChannel` cover, against the fake transport:

- `next_prompt` returns queued accepted prompts in **FIFO order**;
- at capacity the **oldest is dropped** with the warning log;
- `next_prompt` returns **`Idle`** when the queue is empty;
- the gate **fail-closed** rejects unauthorized inbound (and empty allowlist
  denies all) — rejected frames are never enqueued;
- a `status` command **posts status without enqueuing** a turn;
- `publish_output` **re-verifies membership** before posting, withholds on an
  unexpected member, and redacts before chunking;
- an own synced-back post is **echo-suppressed**;
- `stop` **pre-empts** an in-flight turn and closes the channel;
- a body containing the word "stop" inside a longer sentence stays a **prompt**
  (control words match only the whole trimmed body).

Per repo convention, cross-crate integration tests are explicit `[[test]]`
targets that resolve binaries via `env!("CARGO_BIN_EXE_<bin>")`; in-crate unit
tests live under `#[cfg(test)]`.

---

## FAQ

**Does anything an operator sees change?** No. This is an internal refactor;
the observable Signal conversation is identical (see
[Behavior-preservation contract](#behavior-preservation-contract)).

**Is there now a turn timeout, or a hard-coded queue cap?** No and no. Turns
still run to natural completion with no wall-clock cap, and the queue capacity is
still operator policy (`--inbox-capacity` / `AMPLIHACK_SIGNAL_INBOX_CAPACITY`,
default `32`).

**Was `gating.rs` touched?** No. The fail-closed inbound `Gate` is unchanged;
`SignalChannel` composes it exactly as the old loop did.

**What about auto-mode?** Out of scope for this PR (handled separately in PR-4).
`SignalChannel` preserves auto-mode continuation semantics (INV-7) unchanged.

**Where did the cross-process inbox go?** It was already dead code; PR-3 deletes
`session_channel.rs` and keeps only its capacity policy. See
[Removed](#removed-the-dead-cross-process-session_channel).
