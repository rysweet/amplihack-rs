# Signal Channel

A **feature-gated, per-session Signal messaging channel** for amplihack. When
enabled, each amplihack session opens a private Signal group, posts meaningful
progress updates to it, and lets an allow-listed operator send **advisory**
instructions back into the running session.

- **Crate:** `amplihack-signal`
- **Cargo feature:** `signal` (default **OFF**)
- **Wire protocol:** signal-cli JSON-RPC 2.0 over newline-delimited TCP
- **Trust model:** inbound text is surfaced to the agent as *context only* and
  is **never auto-executed**

> **Status:** the channel is compiled out entirely unless you build with
> `--features signal`. With the feature off there is zero runtime cost and no
> new dependencies pulled into the default build.

---

## Contents

- [How it works](#how-it-works)
- [Prerequisites](#prerequisites)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [Group naming and lifecycle](#group-naming-and-lifecycle)
- [The inbound path (operator → agent)](#the-inbound-path-operator--agent)
- [The outbound path (agent → operator)](#the-outbound-path-agent--operator)
- [Security model / trust boundary](#security-model--trust-boundary)
- [Crate API reference](#crate-api-reference)
- [Building and testing](#building-and-testing)
- [Troubleshooting](#troubleshooting)
- [FAQ](#faq)

---

## How it works

```
┌────────────────────┐        JSON-RPC 2.0 (NDJSON/TCP)        ┌──────────────┐
│  amplihack session │  ───────────────────────────────────►  │  signal-cli   │
│  (hooks pipeline)  │                                         │  daemon       │
│                    │  ◄───────────────────────────────────  │ (your number) │
└─────────┬──────────┘         receive stream                 └──────┬────────┘
          │                                                          │
          │ SessionStart: create group, post "session started",       │
          │               spawn detached subscriber (persist PID)      │
          │                                                          ▼
          │                                             ┌────────────────────────┐
          │  file inbox (AtomicJsonFile) ◄───────────── │ signal-subscriber       │
          │        ▲                                    │ (long-lived connection) │
          │        │ drain                              │ allowlist + gate        │
   PostToolUse /   │                                    │ + groupId + echo-suppr. │
   UserPromptSubmit│  additionalContext                 └────────────────────────┘
          │        │
          ▼        │
   Stop: post summary → quitGroup → stop subscriber
```

The channel is wired through amplihack's existing **hooks** pipeline
(`amplihack-hooks`), not the recipe-runner:

1. **SessionStart** creates a Signal group, persists its `groupId` in session
   state, posts a "session started" message, and spawns a **detached,
   long-lived subscriber process** whose PID is persisted.
2. The **subscriber** holds a single JSON-RPC connection, filters messages to
   this session's `groupId`, applies the gate (allowlist + setup-aware
   authorization + echo suppression), and appends accepted instructions to a
   **per-session file inbox**.
3. **PostToolUse** and **UserPromptSubmit** hooks drain the file inbox and
   inject any queued operator instructions into the agent as
   `hookSpecificOutput.additionalContext`.
4. **Stop** posts a session summary, calls `quitGroup`, and stops the
   subscriber.

Every Signal operation is **non-fatal**: failures are appended to the hook's
`warnings[]` and emitted via `tracing`, and the hook still exits `0`. A broken
or unreachable Signal daemon can never crash or block your session.

---

## Prerequisites

- A working **[signal-cli](https://github.com/AsamK/signal-cli)** installation
  with a **registered account** (a dedicated phone number for the bot is
  strongly recommended — this account will send and receive messages).
- signal-cli running in **JSON-RPC daemon** mode over TCP:

  ```bash
  signal-cli -a "+15551230000" daemon --tcp 127.0.0.1:7583
  ```

- amplihack built with the `signal` feature (see [Building](#building-and-testing)).

---

## Quick start

```bash
# 1. Start signal-cli in JSON-RPC daemon mode (in its own terminal).
signal-cli -a "+15551230000" daemon --tcp 127.0.0.1:7583

# 2. Configure the channel via environment variables.
export AMPLIHACK_SIGNAL_ENDPOINT="127.0.0.1:7583"
export AMPLIHACK_SIGNAL_ACCOUNT="+15551230000"
export AMPLIHACK_SIGNAL_ALLOWLIST="+15551230001"      # your personal number

# 3. Build amplihack with the feature enabled.
cargo build --release --features signal

# 4. Run amplihack as usual. On SessionStart you'll be added to a new
#    Signal group named "amplihack-<session-id>-<timestamp>" and receive a
#    "session started" message.
```

Reply in that Signal group (from an allow-listed number, from your primary
device) with a short instruction such as *"focus on the failing test first"*.
It will be delivered to the agent at the next `UserPromptSubmit` /
`PostToolUse` boundary as additional context — **not executed automatically**.

---

## Configuration

Configuration is resolved **env-first**, then from an optional TOML file, then
**explicit error** — there are **no silent defaults**. If a required value is
missing from both sources, the loader fails and the channel stays off.

Resolution order for each setting:

```
environment variable  >  TOML file (AMPLIHACK_SIGNAL_CONFIG)  >  error
```

### Settings

| Setting | Env var | TOML key | Required | Format / notes |
|---|---|---|---|---|
| Endpoint | `AMPLIHACK_SIGNAL_ENDPOINT` | `endpoint` | ✅ | `host:port` of the signal-cli JSON-RPC daemon |
| Account | `AMPLIHACK_SIGNAL_ACCOUNT` | `account` | ✅ | E.164 (`+` then digits) — the number amplihack sends **as** |
| Allowlist | `AMPLIHACK_SIGNAL_ALLOWLIST` | `allowlist` | ✅ | Operator numbers allowed to send inbound. Env = comma-separated E.164. **Empty ⇒ fail-closed (deny all inbound).** |
| Own device id | `AMPLIHACK_SIGNAL_OWN_DEVICE_ID` | `own_device_id` | optional | signal-cli's **own** linked-device id (must be `>= 2`). Only used to reject the bot's own synced-back echoes explicitly; the primary-phone (device `1`) gate is the main loop guard and needs no configuration. Leave unset unless you know your signal-cli device id |
| Reuse rolling group | `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP` | `reuse_rolling_group` | optional | `true`/`1` reuses one long-lived group instead of per-session groups |
| Rolling group id | `AMPLIHACK_SIGNAL_ROLLING_GROUP_ID` | `rolling_group_id` | optional | Existing group id to reuse when rolling mode is on |
| Config file path | `AMPLIHACK_SIGNAL_CONFIG` | — | optional | Path to the TOML file below |

> **Fail-closed allowlist.** An **empty** allowlist is a valid, deliberate
> configuration meaning "accept no inbound instructions." It is *not* treated
> as "allow everyone." Outbound posting still works; only the inbound path is
> closed.

### Example TOML file

See [`examples/signal-config.toml`](../examples/signal-config.toml) for a fully
annotated example. Point the loader at it with:

```bash
export AMPLIHACK_SIGNAL_CONFIG=/path/to/signal-config.toml
```

```toml
endpoint = "127.0.0.1:7583"
account  = "+15551230000"
allowlist = ["+15551230001", "+15551230002"]
# own_device_id = 2
# reuse_rolling_group = false
# rolling_group_id = "group.aBcDeF0123456789=="
```

Any value present in the environment overrides the same key in the file.

---

## Group naming and lifecycle

**Per-session (default).** On SessionStart a fresh group is created named:

```
amplihack-<session-id>-<unix-timestamp>
```

The `groupId` returned by signal-cli is persisted in session state. On Stop the
group is closed with `quitGroup`.

**Rolling group (opt-in).** Set `reuse_rolling_group = true` (or
`AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP=1`) to reuse a **single** long-lived
group across all sessions. In this mode the group is **not** quit at Stop, so
you keep one persistent operator thread. Supply `rolling_group_id` to bind to
an existing group instead of creating a new one on first use.

| Phase | Per-session | Rolling |
|---|---|---|
| SessionStart | create group + post "session started" | reuse group + post "session started" |
| During run | post at meaningful transitions | post at meaningful transitions |
| Stop | post summary → `quitGroup` | post summary (group kept) |

---

## The inbound path (operator → agent)

1. The **subscriber** (`amplihack-hooks signal-subscriber`, spawned detached at
   SessionStart) holds one long-lived JSON-RPC connection to signal-cli.
2. For each incoming envelope it validates the **group envelope shape** —
   handling both `dataMessage.groupInfo.groupId` and
   `syncMessage.sentMessage.message.groupInfo` — and keeps only messages for
   this session's `groupId`.
3. It applies the gate: **allowlist** membership, **setup-aware
   authorization**, `groupId` match, and **echo suppression** (recently-sent
   outbound bodies are ignored within a bounded TTL window so the bot never
   re-ingests its own synced-back messages). Setup-aware authorization supports
   both deployment shapes: on a **single-number linked-device** setup the
   operator types on their **primary phone**, so the message arrives as the
   account's own `syncMessage` from `sourceDevice == 1` and is accepted; on a
   **dedicated-number** setup the operator commands from a separate allowlisted
   number via a normal `dataMessage`. signal-cli's own sends sync back from a
   linked device (`>= 2`) and are rejected.
4. Accepted instruction text is appended to a **per-session file inbox**, a
   JSON document managed by `AtomicJsonFile` (crash-safe, lock-guarded). The
   inbox path is derived through `amplihack_types::paths::sanitize_session_id`
   to prevent path traversal. The inbox is **bounded**: it holds at most a
   fixed number of pending instructions (a small cap, e.g. 32). When full, the
   **oldest** queued instruction is dropped to make room for the newest and the
   drop is recorded in `warnings[]` — a flood of inbound messages can never grow
   memory or disk without limit (backpressure by bounded queue).
5. On the next **PostToolUse** or **UserPromptSubmit** hook, the inbox is
   **drained** and its queued instructions are emitted to the agent via
   `hookSpecificOutput.additionalContext`. Draining is one-shot: each
   instruction is delivered once.

If the subscriber cannot start, the failure is recorded in `warnings[]` and via
`tracing`; the session continues normally with no inbound channel.

**Reconnect resilience.** Once a connection has been established at least once, a
transient drop (daemon restart, stream close, receive error) does **not** end the
channel: the subscriber reconnects with **bounded exponential backoff** (1s → 2s
→ … capped at 30s), preserving its echo-suppression/de-dup state and file inbox
across reconnects so no instruction is lost or re-delivered. Any inbound message
resets the backoff. To avoid spinning against a permanently-down daemon it gives
up after a small number of consecutive failures. A **cold-start** connect failure
(no connection ever established) stays fast and non-fatal — SessionStart spawns
the subscriber best-effort and is never stalled by an absent daemon.

---

## The outbound path (agent → operator)

amplihack posts to the group **only at meaningful transitions** — not on every
tool call — and posting is **throttled/batched**:

- **SessionStart** — "session started".
- **Checkpoints / key results** — significant milestones.
- **Stop** — a session summary.

Outbound bodies are minimized and redacted before sending. Each posted body is
recorded in the echo-suppression window so the subscriber will not treat the
synced-back copy as an operator instruction.

---

## Security model / trust boundary

The channel is designed around one hard rule:

> **Inbound Signal text is data, never commands.**

Concretely:

- **Never auto-executed.** Accepted instructions are surfaced *only* as
  `additionalContext`. amplihack never turns inbound text into a shell command,
  file write, or any other mutating action on its own. The agent may choose to
  act on the advice, subject to all normal amplihack safety hooks.
- **Fail-closed gate.** Inbound requires *all* of: sender on the allowlist,
  matching session `groupId`, and setup-appropriate authorization — an account
  `syncMessage` is accepted only from the **primary phone** (`sourceDevice == 1`)
  and never from signal-cli's own linked device, while a separate allowlisted
  number is accepted via a normal `dataMessage`. An **empty allowlist denies
  everything**.
- **No self-ingestion.** Echo suppression (bounded TTL window over recent
  outbound bodies) prevents the bot from re-processing its own messages that
  Signal syncs back to the account.
- **Feature default OFF.** No `signal` feature ⇒ no code, no dependencies, no
  network sockets.
- **No silent config defaults.** Missing required config is an explicit error,
  never a guessed value.
- **Path safety.** Every per-session file path is run through
  `sanitize_session_id`; inbox/PID files are written atomically with
  restrictive permissions.
- **Least privilege on shutdown.** Stop kills **only the recorded subscriber
  PID**, never a name-matched sweep.
- **Bounded inbox (flood resistance).** The file inbox has a fixed capacity;
  under an inbound flood the oldest instruction is evicted (logged to
  `warnings[]`) rather than allowing unbounded memory/disk growth.
- **Non-fatal contract.** Every Signal operation that fails is logged to
  `warnings[]` + `tracing` and the hook still exits `0`.

---

## Crate API reference

`amplihack-signal` is organized as a small "brick" with a **pure core**
(`config` / wire helpers / `gating` / `session_channel` logic — no network or
filesystem I/O, unit-testable in isolation) plus a **gated I/O shell**
(`transport` and the `SignalSession` I/O owner, which require the async
`tokio` net stack). The crate is pulled into `amplihack-hooks` only under
`--features signal`; with the feature off it is neither compiled nor linked.

### `config`

Env-first loader with explicit errors and no silent defaults.

```rust
use amplihack_signal::config::SignalConfig;

// Resolves env > TOML(AMPLIHACK_SIGNAL_CONFIG) > error.
let cfg = SignalConfig::load()?;
assert!(cfg.allowlist.iter().all(|n| n.starts_with('+')));
```

| Field | Type | Meaning |
|---|---|---|
| `endpoint` | `String` | `host:port` of the daemon |
| `account` | `String` | E.164 sending account |
| `allowlist` | `Vec<String>` | Permitted E.164 senders (empty ⇒ deny all inbound) |
| `own_device_id` | `Option<u32>` | signal-cli's own linked-device id (`>= 2`) for explicit echo rejection |
| `reuse_rolling_group` | `bool` | Use one rolling group |
| `rolling_group_id` | `Option<String>` | Bind rolling mode to an existing group |

### `transport`

Newline-delimited JSON-RPC 2.0 client over `tokio` TCP.

| Method | Purpose |
|---|---|
| `create_group(name) -> GroupId` | Create a group (wraps the `updateGroup` create-by-name RPC) |
| `send_group(group_id, body)` | Post a message (wraps the `send` RPC) |
| `quit_group(group_id)` | Leave/close a group (`quitGroup`) |
| `receive()` stream | Async stream of parsed inbound envelopes |

> **RPC method names** track the signal-cli JSON-RPC surface. `create_group`
> is expected to map to `updateGroup` (creating a group by supplying a name and
> members); if the signal-cli daemon version in use names it differently,
> update this table to match the actual method invoked.

**Pure wire helpers** (no I/O, unit-tested in isolation):

```rust
use amplihack_signal::transport::{build_send_request, parse_incoming};

// Build a JSON-RPC request frame for an outbound message.
let frame = build_send_request(&group_id, "hello");

// Parse one inbound NDJSON line into a typed envelope (tolerant / fail-safe).
let envelope = parse_incoming(line)?;
```

`parse_incoming` validates both group envelope shapes
(`dataMessage.groupInfo.groupId` and
`syncMessage.sentMessage.message.groupInfo`) and is covered by fixture-driven
unit tests over realistic JSON.

### `gating`

Fail-closed decision function combining allowlist + `groupId` match +
setup-aware authorization (accept the account's own `syncMessage` only from the
primary phone `sourceDevice == 1`; accept a separate allowlisted number via
`dataMessage`) + bounded-TTL echo suppression.

```rust
use amplihack_signal::gating::Gate;

let mut gate = Gate::new(&cfg, session_group_id);
gate.record_outbound("session started");     // seed echo-suppression window

match gate.evaluate(&envelope) {
    Some(instruction) => { /* append to inbox */ }
    None => { /* dropped: not allow-listed / wrong device / echo / other group */ }
}
```

### `session_channel`

`SignalSession` owns one per-session group and a file-backed inbox.

| Method | Purpose |
|---|---|
| `announce()` | Create/reuse group and post "session started" |
| `post(update)` | Post a throttled outbound update |
| `poll()` / `drain()` | Read (and clear) queued inbound instructions from the file inbox |

The inbox is an `AtomicJsonFile` (from `amplihack-state`) at a
`sanitize_session_id`-derived path, so writes by the subscriber process and
reads by the hook process are safe across processes.

---

## Building and testing

The feature must build and test cleanly **both** ways — this is a hard quality
gate:

```bash
# Default build: feature OFF (Signal fully compiled out).
cargo build
cargo test

# Feature ON.
cargo build --features signal
cargo test  --features signal
```

Integration tests are registered as explicit `[[test]]` targets and resolve the
hooks binary via `env!("CARGO_BIN_EXE_amplihack-hooks")`, so they exercise the
real `signal-subscriber` subcommand rather than an in-process stub. Pure
wire/gating tests run with no network or filesystem I/O.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| No "session started" message | Feature not built / daemon down | Build with `--features signal`; confirm `signal-cli ... daemon --tcp`; check `warnings[]` |
| Warning: config error at SessionStart | Missing required setting | Set `AMPLIHACK_SIGNAL_ENDPOINT` / `_ACCOUNT` / `_ALLOWLIST` (or the TOML file) |
| Your replies are ignored | Not allow-listed, or sent from a linked (non-primary) device | Add your number to `AMPLIHACK_SIGNAL_ALLOWLIST`; reply from your **primary** device (device 1) |
| Nothing ever accepted | Allowlist is empty (fail-closed) | Populate the allowlist |
| Bot seems to "hear itself" | (Should not happen) echo window too short | Instructions equal to a recent outbound body are suppressed by design |
| Subscriber not running | Spawn failed | Check `warnings[]`/`tracing`; the persisted PID file records the detached process |
| Some instructions never arrive | Inbox overflowed under a burst | The inbox is bounded; oldest entries are evicted (see `warnings[]`). Send fewer, more deliberate instructions |

Because every Signal operation is non-fatal, none of the above can break your
amplihack session — worst case the channel is silently unavailable and the run
proceeds normally.

---

## FAQ

**Does enabling Signal add dependencies to the default build?**
No. With the feature off, `amplihack-signal` and its `tokio`-net dependencies
are not compiled or linked.

**Can an operator make amplihack run a command by texting it?**
No. Inbound text is delivered only as `additionalContext`. The agent decides
whether to act, and all normal safety hooks still apply.

**Per-session vs rolling group — which should I use?**
Per-session (default) gives clean isolation and auto-cleanup via `quitGroup`.
Rolling keeps one persistent operator thread across runs; set
`reuse_rolling_group = true`.

**Where do inbound instructions get stored?**
In a per-session, atomically-written JSON inbox whose path is derived through
`sanitize_session_id`. The inbox is bounded (oldest entries evicted under a
flood) and is drained (delivered once) on the next
`PostToolUse` / `UserPromptSubmit`.

---

## See also

- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config
- [Hook configuration guide](HOOK_CONFIGURATION_GUIDE.md)
- [Security recommendations](SECURITY_RECOMMENDATIONS.md)
