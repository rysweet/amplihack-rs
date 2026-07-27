# Signal Chat (`amplihack signal chat`)

Drive a **whole agent session from a Signal group chat**. The chat opens a
fresh, operator-only Signal group for one *topic*, runs the first agent turn,
posts the agent's output back to the group, and then treats **every operator
message in that group as the next agent prompt** — with full prior session
context preserved across turns.

- **Crate (reusable logic):** `amplihack-signal`
- **Subcommand (CLI glue):** `amplihack signal chat`
- **Cargo feature:** `signal` (default **OFF**)
- **Model:** turn-based **resume** of one pinned Copilot session UUID — one
  operator message → one `copilot --session-id <uuid> …` invocation → one
  redacted, chunked reply posted to the group. **No PTY, no ANSI parsing, no
  streaming.**
- **Trust model:** an accepted inbound message is **equivalent to typing into
  the agent**. The chat is therefore **least-privilege by default** and
  **fails closed** on every ambiguity.

> **Status.** The chat is a **new, opt-in** feature. It **replaces** the old
> auto-per-session mirroring (which produced empty groups). It is compiled out
> entirely unless you build with `--features signal`; with the feature off the
> subcommand still registers and exits with a clean
> "rebuild with `--features signal`" error (never a silent no-op). The chat
> does **not** touch or remove the legacy per-session hooks — that retirement is
> a separate change.

---

## Contents

- [How it works](#how-it-works)
- [Prerequisites](#prerequisites)
- [Quick start](#quick-start)
- [Command reference](#command-reference)
- [Tool allowlist (blast radius)](#tool-allowlist-blast-radius)
- [Control phrases](#control-phrases)
- [Group naming](#group-naming)
- [Group creation & auto-accept](#group-creation--auto-accept)
- [Security contract](#security-contract)
- [Configuration](#configuration)
- [Failure modes](#failure-modes)
- [The `/signal` skill](#the-signal-skill)
- [Tutorial: a PR review from your phone](#tutorial-a-pr-review-from-your-phone)
- [Exit codes](#exit-codes)
- [Reference](#reference)

---

## How it works

```
operator (Signal app on phone)
        │  message in group
        ▼
┌──────────────────────────────────────────────────────────────┐
│ amplihack signal chat "<topic>"                            │
│                                                              │
│  signal-cli JSON-RPC (127.0.0.1) ── inbound gating ──┐       │
│      (cancel-safe framing; no dropped inbound frame)  ▼       │
│                                          control-phrase parse │
│                                    (stop / kill / status)     │
│                                                       │       │
│                                         normal prompt │       │
│                                                       ▼       │
│                                        bounded turn queue     │
│                                                       │       │
│                                    serialized turn driver     │
│                                                       ▼       │
│   copilot --session-id <uuid> --no-color -s \                │
│           -p "<msg>" --allow-tool <t1> --allow-tool <t2> …    │
│                                                       │stdout │
│                                     redact → chunk → post ────┤
└──────────────────────────────────────────────────────────────┘
        │  membership re-verified (fail closed) before EVERY chunk
        ▼
operator (Signal group)
```

0. **Group creation is auto-accepted.** Before the loop starts, the chat creates
   the operator-only group. A group that *code* originates via signal-cli lands
   on the operator's linked device as a **pending message request** — until it is
   explicitly accepted, messages the chat posts to it are **not reliably
   delivered** to the phone. The chat therefore **accepts the message request
   automatically**, immediately after the group id is known, via signal-cli's
   `sendMessageRequestResponse` (`type: "accept"`) — so the very first
   announcement message is delivered without any manual "Accept" tap. See
   [Group creation & auto-accept](#group-creation--auto-accept).
1. **Cancel-safe inbound framing.** Inbound frames from the signal-cli JSON-RPC
   socket are read on a **cancel-safe** path. The subscriber loop races the
   receive future against the turn queue in a `biased` `select!`; if a competing
   event wins mid-frame, the partially-read frame is **retained** (partial-frame
   state persists across polls) and resumed on the next read — a fragmented
   operator message split across TCP segments is **never silently dropped**. A
   notification that arrives while a JSON-RPC request is in flight is **queued**
   and delivered on the next receive, never discarded. The 256 KiB frame bound
   stays enforced on the retained buffer.
2. **One pinned session UUID.** The chat generates a fresh v4 UUID once. Every
   turn resumes the **same** session with `copilot --session-id <uuid>`, so the
   agent keeps full context across the whole conversation.
3. **One turn at a time.** Turns are **serialized** — exactly one child
   `copilot` process runs per session at any moment. New messages queue.
4. **Copilot invocation.** Each turn runs:

   ```bash
   copilot --session-id <uuid> --no-color -s -p "<message>" \
           --allow-tool <t1> --allow-tool <t2> …
   ```

   - `-s`/`--silent` → the agent **response only** on stdout (clean to capture).
   - `--no-color` → guarantees ANSI-free stdout before redaction/chunking.
   - `--allow-tool` (repeated) → the scoped allowlist (see below).
5. **Output relay.** stdout is **redacted for secrets first**, then **chunked**
   to Signal's per-message limit, then posted to the group — but membership is
   **re-verified fail-closed before EACH chunk**, so a member added mid-body
   cannot receive later chunks; on any mid-body verification failure the
   remaining chunks are **withheld and logged** (never silently dropped).

---

## Prerequisites

- **amplihack built with the `signal` feature:**

  ```bash
  cargo build --release --features signal
  ```

- **A linked, running signal-cli JSON-RPC daemon on `127.0.0.1`.** If you have
  not onboarded this host yet, run `amplihack signal setup` first (it installs
  signal-cli, links a device via QR, starts the local daemon, and writes the
  config). See [Signal Onboarding](SIGNAL_ONBOARDING.md). The chat **reuses**
  that onboarding/link path — it never reimplements linking. If the account is
  not linked, the chat guides you to link and exits.
- **`copilot` on `PATH`** (GitHub Copilot CLI) — the agent that each turn drives.
- **Your phone with Signal**, on the same account, to send/receive in the group.

---

## Quick start

```bash
# Least-privilege (read-only investigation tools) by default:
amplihack signal chat "review PR 3967 with a crusty-old-engineer eye"
```

The chat will:

1. Validate the daemon endpoint is loopback and the account is linked.
2. Create a fresh group named `amplihack-<host>-<slug(topic)>`.
3. Post a first message announcing the **topic**, the **effective tool
   allowlist** (your blast radius), and the **control phrases**.
4. Run the first turn (`-p "<topic>"`) and post the reply.
5. Wait for your next message in the group and treat it as the next prompt.

Grant a scoped write/exec capability explicitly:

```bash
amplihack signal chat "fix the failing lint in crates/amplihack-signal" \
  --allow-tool view --allow-tool grep --allow-tool glob \
  --allow-tool edit --allow-tool 'shell(cargo fmt)'
```

Full-tools escape hatch (explicit opt-in — widest blast radius):

```bash
amplihack signal chat "do whatever it takes" --dangerous-all-tools
```

---

## Command reference

```text
amplihack signal chat <TOPIC> [OPTIONS]
```

| Argument / Option        | Kind                  | Default              | Description |
| ------------------------ | --------------------- | -------------------- | ----------- |
| `<TOPIC>`                | positional (required) | —                    | Free-text topic. Becomes the **first prompt** and seeds the group name. |
| `--allow-tool <TOOL>`    | repeatable            | read-only set        | Add one tool to the scoped Copilot allowlist. Repeat for multiple. Maps to `copilot --allow-tool`. |
| `--dangerous-all-tools`  | flag                  | off                  | Opt in to `copilot --allow-all-tools` (all **tools**, not paths/URLs). Overrides `--allow-tool`. |
| `--group-name <NAME>`    | override              | derived (see naming) | Override the generated group name. |
| `--host <NAME>`          | override              | system hostname      | Override the `<host>` token used in the group name. |
| `--retry-budget <N>`     | override              | `10`                 | Max reconnect attempts (bounded exponential backoff) before clean shutdown when the daemon is down. |
| `--inbox-capacity <N>`   | override              | `32`                 | Bounded turn-queue capacity (also settable via `AMPLIHACK_SIGNAL_INBOX_CAPACITY`). |
| `--unsafe-remote-endpoint` | flag                | off                  | Explicit, documented opt-in to allow a **non-loopback** signal-cli daemon endpoint. Without it, a non-`127.0.0.1` endpoint **fails closed** (exit `2`). Never use across an untrusted network. |

> **Note on `-s`.** The `-s` you see in the underlying `copilot` invocation is
> Copilot's `--silent` flag (response-only stdout), **not** an allowlist. The
> allowlist is always expressed with repeated `--allow-tool`. You never pass
> `copilot` flags to `amplihack signal chat` directly.

> **Note on allowlist form.** The chat always emits the allowlist as
> **repeated** `--allow-tool <TOOL>` arguments (one per tool), e.g.
> `--allow-tool view --allow-tool grep`. Scoped shell commands are quoted using
> Copilot's convention, e.g. `--allow-tool 'shell(cargo fmt)'`.

---

## Tool allowlist (blast radius)

The driven agent runs with a **scoped allowlist**, never `--allow-all`, unless
you explicitly opt in. The effective list is **printed verbatim in the group's
first message** so the blast radius is always visible.

| Mode                      | How to select                         | Copilot flags emitted                           |
| ------------------------- | ------------------------------------- | ----------------------------------------------- |
| **Read-only (default)**   | pass no `--allow-tool`                | `--allow-tool view --allow-tool grep --allow-tool glob` plus read-only shell (`git status`, `git log`, `cat`, …) |
| **Scoped**                | one or more `--allow-tool <TOOL>`     | exactly the tools you listed                    |
| **Dangerous (all tools)** | `--dangerous-all-tools`               | `--allow-all-tools`                             |

- **Least-privilege by default.** With no `--allow-tool`, the agent gets only
  read-only investigation tools. Anything not on the allowlist (writes, exec,
  network) is **denied** in non-interactive mode rather than auto-approved.
- **`--dangerous-all-tools` maps to `--allow-all-tools`**, not `--allow-all`.
  `--allow-all` would additionally open arbitrary paths and URLs (a wider blast
  radius); the chat deliberately uses the tools-only escape hatch.

---

## Control phrases

Each accepted inbound message is parsed for a **control phrase first**, before it
is ever queued as a prompt. Matching is case-insensitive, trimmed, exact-word.

| Phrase   | Effect |
| -------- | ------ |
| `status` | Post current state to the group: session id, current turn, effective allowlist, queue depth, membership-verification status. Does **not** enqueue a prompt. |
| `stop`   | **Pre-empt**: kill the in-flight child `copilot` turn immediately (even mid-turn), leave/close the group, and exit. |
| `kill`   | Synonym for `stop`. |

Control phrases **always** take precedence over normal prompts and always
pre-empt an in-flight turn. A message like `please stop the review` is treated as
a normal prompt (no exact-word `stop`), whereas `stop` on its own line is a
control command.

### How pre-emption works (race-free)

Pre-emption is bound to the **specific** child process, not to a raw PID, so it
can never mis-fire against an unrelated process that the OS happened to recycle
the PID for:

- When a turn starts, the runner spawns the `copilot` child and publishes a
  one-shot **pre-empt trigger** (a `oneshot::Sender<()>`) bound to *that* child
  into a shared slot. It holds the owned [`tokio::process::Child`] handle and
  drains stdout/stderr concurrently so a full pipe can never deadlock the wait.
- The runner races the child's natural exit against the trigger. On `stop`/`kill`
  the chat takes the sender out of the slot and fires it; the runner reacts by
  calling `Child::start_kill()` on its **owned** handle — the runtime binds the
  signal to that exact process — then reaps it with `wait()`.
- A pre-empted turn surfaces as an `Interrupted` error ("turn pre-empted by
  stop"); the operator sees the group close and the chat exit `0`.
- When any turn completes, the slot is cleared, so a later `stop` is a harmless
  no-op. No raw PID is ever passed to `kill(2)`, eliminating the PID-reuse
  (TOCTOU) window entirely.

See [`docs/signal-chat-hardening.md`](signal-chat-hardening.md#f2--child-pre-emption-pid-reuse-toctou-fixed)
for the hardening rationale and tests.

---

## Group naming

```
amplihack-<host>-<slug(topic)>                 # no tmux
amplihack-<host>-<tmux>-<slug(topic)>          # inside a tmux session
```

- `<host>` — short system hostname (or `--host` override).
- `<tmux>` — the tmux session name, included when the chat runs inside tmux
  (from `$TMUX` / `tmux display-message -p '#S'`).
- `slug(topic)` — lowercase; every run of non-alphanumeric characters collapses
  to a single `-`; leading/trailing `-` trimmed; length-capped (≈40 chars) to
  respect signal-cli group-name limits.

Example: topic `"review PR 3967!"` on host `azlin-07` inside tmux session `ops`
→ `amplihack-azlin-07-ops-review-pr-3967`.

Override entirely with `--group-name`.

---

## Group creation & auto-accept

Every Signal group the chat creates is **automatically accepted on the
operator's linked device** — no manual "Accept message request" tap is ever
required before the chat can talk to you.

### Why this exists

When code creates a group through signal-cli, that group is delivered to the
operator's *linked* device as a **pending message request**, not an open
conversation. While the request is pending, Signal may **withhold or delay**
messages the group posts to the operator — so without acceptance the chat's
first announcement (topic, allowlist, control phrases) and even early agent
replies can silently fail to reach your phone. Requiring the operator to hunt
for a hidden request and tap **Accept** before every session is exactly the kind
of silent, manual, easy-to-miss step this feature removes.

### What happens

Immediately after the group is created and its id is known — and **before** the
first message is posted — the chat issues a message-request acceptance for that
group. This is baked into group creation itself, so **every** code-originated
group is auto-accepted with no extra flag, config, or operator action:

```
create group  ──►  obtain <groupId>  ──►  accept message request  ──►  first post
                                           (sendMessageRequestResponse:accept)
```

The acceptance uses signal-cli's JSON-RPC `sendMessageRequestResponse` method:

```json
{
  "method": "sendMessageRequestResponse",
  "params": { "groupId": "<groupId>", "type": "accept" }
}
```

A successful call returns `{"result":{}}`. The chat treats acceptance as a
**mandatory** part of bringing a group online.

### Fail closed

Acceptance is **not** best-effort. If the accept call fails (daemon error,
timeout, RPC error), group creation **fails with it** — the chat aborts with a
clear terminal error and exit code **`3`** (group create/setup failure) rather
than proceeding with a group whose messages might never reach the operator.
This follows the repo-wide **no-silent-degradation** policy: the chat never
leaves a group in a pending, silently-undelivered state and never "hopes" the
request was accepted.

### Scope

Auto-accept applies to **every group the chat creates**, including groups made
under `--group-name`. It is transparent — there is no flag to enable it and no
flag to disable it, because a group the chat cannot reliably deliver to is not
usable.

> API note: auto-accept is implemented at the transport layer inside
> `create_group`, so **any** caller that creates a group through the shared
> `amplihack-signal` transport gets it for free. See
> [Reference](#reference) for the transport methods (`create_group`,
> `accept_group`).

---

## Security contract

An accepted inbound message == typing into the agent. The chat enforces:

1. **Least-privilege by default** — scoped `--allow-tool`, never `--allow-all`
   unless `--dangerous-all-tools`. Effective allowlist printed in the first
   group message.
2. **Inbound gating** (existing `Gate`): group-id match, non-empty body, echo
   suppression, allowlist check (an **empty allowlist denies all**), and
   sync-message acceptance only from **your own account on primary device 1**.
   Inbound frames are read on a **cancel-safe** path: a fragmented frame
   interrupted by a competing `select!` event is retained and resumed, and a
   notification interleaved with a JSON-RPC request is queued — **no inbound
   operator message is ever silently dropped** (256 KiB frame bound preserved).
3. **Outbound membership verification — FAIL CLOSED, before EVERY post.** Before
   **each** outbound chunk (not once per body) — and on any membership change —
   the chat verifies the group's member set is the expected operator-only set,
   with every member number validated as a well-formed **E.164** value (a member
   whose number is missing, empty, or malformed fails the whole check). If
   membership cannot be **positively** verified (error / timeout / ambiguous /
   unexpected member / invalid number), the chat does **not** relay the chunk —
   it **withholds the remaining chunks**, alerts on the local terminal, and
   **pauses relaying** until re-verified. It never assumes "probably fine" and a
   member added mid-body cannot receive later chunks.
4. **Local-only daemon** — the JSON-RPC endpoint must be `127.0.0.1`. A remote
   endpoint requires the explicit `--unsafe-remote-endpoint` opt-in; without it a
   non-loopback endpoint **fails closed** and exits `2`.
5. **Audit log** — every accepted prompt is logged with
   sender / device / timestamp / session id, **redacted**.
6. **Outbound secret redaction** — the existing `redact_for_relay` helper is
   **reused** and runs **before** chunking, so secrets never leak across a chunk
   boundary. Only redaction is reused; multi-message **chunking** to
   `SIGNAL_MAX_BYTES` is new chat logic (`chat/chunk.rs`) — replies larger
   than the limit are **chunked, never truncated**.
7. **Adaptive backpressure** — the bounded inbox (operator-configurable) coalesces
   bursts and applies backpressure; drops happen **only** under genuine resource
   pressure and are announced with an explicit in-group notice (**never silent**).
8. **Argv-only spawn** — the operator's message is passed to `copilot` as a
   single `-p <arg>` argument vector (no shell interpolation), closing the
   command-injection sink.

---

## Configuration

The chat reuses the shared Signal config (`crates/amplihack-signal/config.rs`)
loaded by `amplihack signal setup`. Chat-relevant knobs:

| Setting                          | Source                                   | Default     | Meaning |
| -------------------------------- | ---------------------------------------- | ----------- | ------- |
| daemon endpoint                  | signal config TOML                       | `127.0.0.1` | JSON-RPC host:port; must be loopback unless `--unsafe-remote-endpoint` is passed (else fail closed, exit `2`). |
| account / linked device          | signal config TOML                       | —           | The linked signal-cli account used to create the group and gate sync messages. |
| inbound allowlist                | signal config TOML                       | —           | Which senders are accepted; **empty = deny all**. |
| `AMPLIHACK_SIGNAL_INBOX_CAPACITY`| env / `--inbox-capacity`                 | `32`        | Bounded turn-queue capacity. |
| retry budget                     | `--retry-budget`                         | `10`        | Reconnect attempts before clean shutdown. |
| outbound chunk size              | `SIGNAL_MAX_BYTES` constant              | `2000` B    | UTF-8-boundary-safe per-message size for the group (distinct from the JSON-RPC frame bound). |

---

## Failure modes

Errors are **surfaced, never silently swallowed**.

| Failure                    | Behavior |
| -------------------------- | -------- |
| signal-cli daemon down     | Reconnect with **bounded exponential backoff**, surfacing status to the **local terminal**. If not restored within `--retry-budget`, shut down cleanly: stop accepting turns, print a clear terminal error, pre-empt and reap the in-flight child, exit **`4`**. |
| non-loopback endpoint      | Rejected at startup unless `--unsafe-remote-endpoint` is passed; exit **`2`**. |
| account not linked / feature off | Clear terminal guidance (run `amplihack signal setup`, or rebuild with `--features signal`); exit **`1`**. |
| copilot resume unsupported | If the `copilot` session-resume probe fails, refuse to start (turn continuity cannot be guaranteed); exit **`5`**. |
| group create failure       | Abort immediately with a clear error; exit **`3`**. Group creation **includes** auto-accepting the new group's message request (`sendMessageRequestResponse:accept`); if that accept fails, group creation fails with it (fail closed — never a pending, silently-undelivered group). |
| child `copilot` hang       | **Idle/liveness detection** with **no wall-clock cap** on the turn (repo no-agent-timeout policy) + periodic local heartbeat. The operator `stop`/`kill` phrase always pre-empts. |
| child `copilot` non-zero / crash | Post the failure to the group **and** log it; keep the chat alive. The next turn resumes the **same** session id, so context is preserved. |
| membership unverifiable / invalid E.164 member | Withhold outbound relay (remaining chunks too), alert locally, retry verification until positive. Re-checked before **every** chunk. |
| fragmented inbound frame     | Retained across the cancel-safe `select!` and resumed on the next read — **never dropped**; a notification interleaved with a JSON-RPC request is queued and delivered next. Frames over 256 KiB are still bounded/drained. |
| orphaned chat/subprocess | The in-flight child `copilot` is owned by the turn runner and pre-empted (owned-`Child` `start_kill` + reap) on stop / session-end — no orphan, no raw-PID signalling. |

---

## The `/signal` skill

The `/signal` skill (`amplifier-bundle/skills/signal/`) is a thin wrapper that
shells out to the subcommand. Invoke it when you want to hand an agent task off
to a Signal group and drive it from your phone.

```text
/signal start a crusty-old-engineer review of PR 3967
```

expands to:

```bash
amplihack signal chat "start a crusty-old-engineer review of PR 3967"
```

See [`amplifier-bundle/skills/signal/SKILL.md`](../amplifier-bundle/skills/signal/SKILL.md).

---

## Tutorial: a PR review from your phone

1. **Onboard the host once** (if not already linked):

   ```bash
   amplihack signal setup
   ```

   Scan the QR with **Signal → Settings → Linked devices → Link new device**.

2. **Start the chat** for the review topic (read-only is plenty for a review):

   ```bash
   amplihack signal chat "review PR 3967 as a crusty old engineer"
   ```

3. **Watch your phone.** A new group `amplihack-<host>-review-pr-3967` appears
   with a first message listing the topic, the read-only allowlist, and the
   control phrases. Moments later the agent's first review turn arrives.

4. **Drive the conversation.** Reply in the group:

   > *"Focus on the error-handling in the turn driver — any silent fallbacks?"*

   The chat runs one more turn on the **same** session (full context) and posts
   the answer.

5. **Check state any time:** send `status` → you get session id, current turn,
   allowlist, queue depth, and membership status.

6. **Finish:** send `stop` → the in-flight `copilot` turn is pre-empted (its
   owned child is killed and reaped, immune to PID reuse), the group is
   closed, and the chat exits.

---

## Exit codes

The chat exposes a **6-code exit contract** so operators can script it
reliably. Every non-zero exit is also accompanied by a clear terminal message.

| Code | Meaning |
| ---- | ------- |
| `0`  | Clean shutdown via `stop`/`kill` or normal session end. |
| `1`  | Generic fatal error — feature not built (`--features signal` off), account not linked, or unexpected internal error during setup. |
| `2`  | Non-loopback daemon endpoint rejected (loopback safety) without `--unsafe-remote-endpoint`. |
| `3`  | Group create/setup failure (could not create or join the operator-only group). |
| `4`  | signal-cli daemon not restored within `--retry-budget`; clean shutdown after exhausting reconnect attempts. |
| `5`  | Copilot session-resume probe failed — the installed `copilot` did not accept `--session-id` resume, so turn continuity cannot be guaranteed. |

---

## Reference

- [Signal Channel](signal-channel.md) — the per-session channel this chat
  complements.
- [Signal on the Generic Turn Loop](signal-channel-turn-loop.md) — how this
  chat's loop is built on `amplihack_turn::run_session_loop` via `SignalChannel`.
- [Signal Onboarding](SIGNAL_ONBOARDING.md) — `signal setup` / `distribute`,
  linking, and the local daemon the chat depends on.
- [Copilot CLI](COPILOT_CLI.md) — the agent the chat drives via
  `--session-id` resume.
- Crate: `crates/amplihack-signal` (reusable chat logic, feature `signal`).
- Subcommand: `crates/amplihack-cli/src/commands/signal/chat.rs`.

### Transport API (group lifecycle)

The shared `amplihack-signal` transport (`crates/amplihack-signal/src/transport.rs`)
owns the signal-cli JSON-RPC client used by the chat:

| Method | RPC | Behavior |
| ------ | --- | -------- |
| `create_group(name) -> io::Result<GroupId>` | `updateGroup` (create-by-name), then `sendMessageRequestResponse` | Creates the named group, extracts the new `GroupId`, then **auto-accepts** its message request before returning. If the accept fails, `create_group` returns the error (fail closed). Every caller gets auto-accept for free. |
| `accept_group(&group_id) -> io::Result<()>` | `sendMessageRequestResponse` (`type: "accept"`) | Accepts a pending group message request so the operator's linked device reliably receives messages posted to the group. Called internally by `create_group`; also usable directly to (re)accept an existing group. |
