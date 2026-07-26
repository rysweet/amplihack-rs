# Signal Bridge (`amplihack signal bridge`)

Drive a **whole agent session from a Signal group chat**. The bridge opens a
fresh, operator-only Signal group for one *topic*, runs the first agent turn,
posts the agent's output back to the group, and then treats **every operator
message in that group as the next agent prompt** — with full prior session
context preserved across turns.

- **Crate (reusable logic):** `amplihack-signal`
- **Subcommand (CLI glue):** `amplihack signal bridge`
- **Cargo feature:** `signal` (default **OFF**)
- **Model:** turn-based **resume** of one pinned Copilot session UUID — one
  operator message → one `copilot --session-id <uuid> …` invocation → one
  redacted, chunked reply posted to the group. **No PTY, no ANSI parsing, no
  streaming.**
- **Trust model:** an accepted inbound message is **equivalent to typing into
  the agent**. The bridge is therefore **least-privilege by default** and
  **fails closed** on every ambiguity.

> **Status.** The bridge is a **new, opt-in** feature. It **replaces** the old
> auto-per-session mirroring (which produced empty groups). It is compiled out
> entirely unless you build with `--features signal`; with the feature off the
> subcommand still registers and exits with a clean
> "rebuild with `--features signal`" error (never a silent no-op). The bridge
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
│ amplihack signal bridge "<topic>"                            │
│                                                              │
│  signal-cli JSON-RPC (127.0.0.1) ── inbound gating ──┐       │
│                                                       ▼       │
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
        │  membership verified (fail closed) before every post
        ▼
operator (Signal group)
```

1. **One pinned session UUID.** The bridge generates a fresh v4 UUID once. Every
   turn resumes the **same** session with `copilot --session-id <uuid>`, so the
   agent keeps full context across the whole conversation.
2. **One turn at a time.** Turns are **serialized** — exactly one child
   `copilot` process runs per session at any moment. New messages queue.
3. **Copilot invocation.** Each turn runs:

   ```bash
   copilot --session-id <uuid> --no-color -s -p "<message>" \
           --allow-tool <t1> --allow-tool <t2> …
   ```

   - `-s`/`--silent` → the agent **response only** on stdout (clean to capture).
   - `--no-color` → guarantees ANSI-free stdout before redaction/chunking.
   - `--allow-tool` (repeated) → the scoped allowlist (see below).
4. **Output relay.** stdout is **redacted for secrets first**, then **chunked**
   to Signal's per-message limit, then posted to the group — but only after the
   group's membership is **positively verified** as the expected operator-only
   set.

---

## Prerequisites

- **amplihack built with the `signal` feature:**

  ```bash
  cargo build --release --features signal
  ```

- **A linked, running signal-cli JSON-RPC daemon on `127.0.0.1`.** If you have
  not onboarded this host yet, run `amplihack signal setup` first (it installs
  signal-cli, links a device via QR, starts the local daemon, and writes the
  config). See [Signal Onboarding](SIGNAL_ONBOARDING.md). The bridge **reuses**
  that onboarding/link path — it never reimplements linking. If the account is
  not linked, the bridge guides you to link and exits.
- **`copilot` on `PATH`** (GitHub Copilot CLI) — the agent that each turn drives.
- **Your phone with Signal**, on the same account, to send/receive in the group.

---

## Quick start

```bash
# Least-privilege (read-only investigation tools) by default:
amplihack signal bridge "review PR 3967 with a crusty-old-engineer eye"
```

The bridge will:

1. Validate the daemon endpoint is loopback and the account is linked.
2. Create a fresh group named `amplihack-<host>-<slug(topic)>`.
3. Post a first message announcing the **topic**, the **effective tool
   allowlist** (your blast radius), and the **control phrases**.
4. Run the first turn (`-p "<topic>"`) and post the reply.
5. Wait for your next message in the group and treat it as the next prompt.

Grant a scoped write/exec capability explicitly:

```bash
amplihack signal bridge "fix the failing lint in crates/amplihack-signal" \
  --allow-tool view --allow-tool grep --allow-tool glob \
  --allow-tool edit --allow-tool 'shell(cargo fmt)'
```

Full-tools escape hatch (explicit opt-in — widest blast radius):

```bash
amplihack signal bridge "do whatever it takes" --dangerous-all-tools
```

---

## Command reference

```text
amplihack signal bridge <TOPIC> [OPTIONS]
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
> `copilot` flags to `amplihack signal bridge` directly.

> **Note on allowlist form.** The bridge always emits the allowlist as
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
  radius); the bridge deliberately uses the tools-only escape hatch.

---

## Control phrases

Each accepted inbound message is parsed for a **control phrase first**, before it
is ever queued as a prompt. Matching is case-insensitive, trimmed, exact-word.

| Phrase   | Effect |
| -------- | ------ |
| `status` | Post current state to the group: session id, current turn, effective allowlist, queue depth, membership-verification status. Does **not** enqueue a prompt. |
| `stop`   | **Pre-empt**: terminate the tracked child `copilot` PID immediately (even mid-turn), leave/close the group, and exit. |
| `kill`   | Synonym for `stop`. |

Control phrases **always** take precedence over normal prompts and always
pre-empt an in-flight turn. A message like `please stop the review` is treated as
a normal prompt (no exact-word `stop`), whereas `stop` on its own line is a
control command.

---

## Group naming

```
amplihack-<host>-<slug(topic)>                 # no tmux
amplihack-<host>-<tmux>-<slug(topic)>          # inside a tmux session
```

- `<host>` — short system hostname (or `--host` override).
- `<tmux>` — the tmux session name, included when the bridge runs inside tmux
  (from `$TMUX` / `tmux display-message -p '#S'`).
- `slug(topic)` — lowercase; every run of non-alphanumeric characters collapses
  to a single `-`; leading/trailing `-` trimmed; length-capped (≈40 chars) to
  respect signal-cli group-name limits.

Example: topic `"review PR 3967!"` on host `azlin-07` inside tmux session `ops`
→ `amplihack-azlin-07-ops-review-pr-3967`.

Override entirely with `--group-name`.

---

## Security contract

An accepted inbound message == typing into the agent. The bridge enforces:

1. **Least-privilege by default** — scoped `--allow-tool`, never `--allow-all`
   unless `--dangerous-all-tools`. Effective allowlist printed in the first
   group message.
2. **Inbound gating** (existing `Gate`): group-id match, non-empty body, echo
   suppression, allowlist check (an **empty allowlist denies all**), and
   sync-message acceptance only from **your own account on primary device 1**.
3. **Outbound membership verification — FAIL CLOSED.** Before every post (and on
   any membership change), the bridge verifies the group's member set is the
   expected operator-only set. If it cannot be **positively** verified
   (error / timeout / ambiguous / unexpected member), the bridge does **not**
   relay output — it alerts on the local terminal and **pauses relaying** until
   re-verified. It never assumes "probably fine."
4. **Local-only daemon** — the JSON-RPC endpoint must be `127.0.0.1`. A remote
   endpoint requires the explicit `--unsafe-remote-endpoint` opt-in; without it a
   non-loopback endpoint **fails closed** and exits `2`.
5. **Audit log** — every accepted prompt is logged with
   sender / device / timestamp / session id, **redacted**.
6. **Outbound secret redaction** — the existing `redact_for_relay` helper is
   **reused** and runs **before** chunking, so secrets never leak across a chunk
   boundary. Only redaction is reused; multi-message **chunking** to
   `SIGNAL_MAX_BYTES` is new bridge logic (`bridge/chunk.rs`) — replies larger
   than the limit are **chunked, never truncated**.
7. **Adaptive backpressure** — the bounded inbox (operator-configurable) coalesces
   bursts and applies backpressure; drops happen **only** under genuine resource
   pressure and are announced with an explicit in-group notice (**never silent**).
8. **Argv-only spawn** — the operator's message is passed to `copilot` as a
   single `-p <arg>` argument vector (no shell interpolation), closing the
   command-injection sink.

---

## Configuration

The bridge reuses the shared Signal config (`crates/amplihack-signal/config.rs`)
loaded by `amplihack signal setup`. Bridge-relevant knobs:

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
| signal-cli daemon down     | Reconnect with **bounded exponential backoff**, surfacing status to the **local terminal**. If not restored within `--retry-budget`, shut down cleanly: stop accepting turns, print a clear terminal error, tear down the child PID, exit **`4`**. |
| non-loopback endpoint      | Rejected at startup unless `--unsafe-remote-endpoint` is passed; exit **`2`**. |
| account not linked / feature off | Clear terminal guidance (run `amplihack signal setup`, or rebuild with `--features signal`); exit **`1`**. |
| copilot resume unsupported | If the `copilot` session-resume probe fails, refuse to start (turn continuity cannot be guaranteed); exit **`5`**. |
| group create failure       | Abort immediately with a clear error; exit **`3`**. |
| child `copilot` hang       | **Idle/liveness detection** with **no wall-clock cap** on the turn (repo no-agent-timeout policy) + periodic local heartbeat. The operator `stop`/`kill` phrase always pre-empts. |
| child `copilot` non-zero / crash | Post the failure to the group **and** log it; keep the bridge alive. The next turn resumes the **same** session id, so context is preserved. |
| membership unverifiable     | Pause outbound relay, alert locally, retry verification until positive. |
| orphaned bridge/subprocess | The child `copilot` PID is tracked and torn down on stop / session-end. |

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
amplihack signal bridge "start a crusty-old-engineer review of PR 3967"
```

See [`amplifier-bundle/skills/signal/SKILL.md`](../amplifier-bundle/skills/signal/SKILL.md).

---

## Tutorial: a PR review from your phone

1. **Onboard the host once** (if not already linked):

   ```bash
   amplihack signal setup
   ```

   Scan the QR with **Signal → Settings → Linked devices → Link new device**.

2. **Start the bridge** for the review topic (read-only is plenty for a review):

   ```bash
   amplihack signal bridge "review PR 3967 as a crusty old engineer"
   ```

3. **Watch your phone.** A new group `amplihack-<host>-review-pr-3967` appears
   with a first message listing the topic, the read-only allowlist, and the
   control phrases. Moments later the agent's first review turn arrives.

4. **Drive the conversation.** Reply in the group:

   > *"Focus on the error-handling in the turn driver — any silent fallbacks?"*

   The bridge runs one more turn on the **same** session (full context) and posts
   the answer.

5. **Check state any time:** send `status` → you get session id, current turn,
   allowlist, queue depth, and membership status.

6. **Finish:** send `stop` → the child `copilot` is terminated, the group is
   closed, and the bridge exits.

---

## Exit codes

The bridge exposes a **6-code exit contract** so operators can script it
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

- [Signal Channel](signal-channel.md) — the per-session channel this bridge
  complements.
- [Signal Onboarding](SIGNAL_ONBOARDING.md) — `signal setup` / `distribute`,
  linking, and the local daemon the bridge depends on.
- [Copilot CLI](COPILOT_CLI.md) — the agent the bridge drives via
  `--session-id` resume.
- Crate: `crates/amplihack-signal` (reusable bridge logic, feature `signal`).
- Subcommand: `crates/amplihack-cli/src/commands/signal/bridge.rs`.
