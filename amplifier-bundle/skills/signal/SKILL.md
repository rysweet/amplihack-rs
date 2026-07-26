---
name: signal
version: 1.0.0
description: Drive an agent session from a Signal group chat. Opens an operator-only Signal group for a topic and turns every operator message into a real agent prompt with full session context.
activation_keywords:
  - "/signal"
  - "signal bridge"
  - "drive from signal"
  - "run this in signal"
  - "review over signal"
  - "signal group review"
  - "control from my phone"
---

# Signal Bridge Skill

## Purpose

Hand an agent task off to a **Signal group chat** and drive the whole session
from your phone. The skill starts a **topic-scoped, bidirectional Signal
bridge**: all agent output is posted to a fresh operator-only group, and every
message you send in that group becomes the **next agent prompt** — with full
prior session context preserved across turns.

It is a thin wrapper: the skill simply shells out to
`amplihack signal bridge "<topic>"`. All logic, security, and failure handling
live in that subcommand (see [docs/SIGNAL_BRIDGE.md](../../../docs/SIGNAL_BRIDGE.md)).

## When This Skill Activates

- User wants to run/monitor/steer an agent task **from Signal** ("do this over
  Signal", "let me drive this from my phone").
- User wants an agent **review or investigation** delivered to a Signal group
  they can reply into.
- User types `/signal <topic>`.

## How It Works

The skill runs one command:

```bash
amplihack signal bridge "<topic>"
```

That command:

1. Verifies the local signal-cli daemon is on `127.0.0.1` and the account is
   linked (reusing `amplihack signal setup`; guides you to link via QR if not).
2. Creates a fresh group `amplihack-<host>[-<tmux>]-<slug(topic)>`.
3. Posts a first message announcing the **topic**, the **effective tool
   allowlist** (blast radius), and the **control phrases** (`stop`/`kill`/`status`).
4. Runs the first turn with the topic as the prompt and posts the reply.
5. Loops: each accepted group message → one `copilot --session-id <uuid>` turn
   (serialized, one at a time) → redacted, chunked output posted back.

## Usage

```text
/signal start a crusty-old-engineer review of PR 3967
```

expands to:

```bash
amplihack signal bridge "start a crusty-old-engineer review of PR 3967"
```

### Scoped write access (explicit)

Only investigation is allowed by default. To let the driven agent edit or run a
specific command, add scoped tools:

```bash
amplihack signal bridge "fix the failing lint" \
  --allow-tool view --allow-tool grep --allow-tool glob \
  --allow-tool edit --allow-tool 'shell(cargo fmt)'
```

### Full tools (dangerous, explicit opt-in)

```bash
amplihack signal bridge "do whatever it takes" --dangerous-all-tools
```

## Control Phrases (in the group)

| Phrase   | Effect |
| -------- | ------ |
| `status` | Post session id, current turn, allowlist, queue depth, membership status. |
| `stop`   | Terminate the child agent immediately (even mid-turn), close the group, exit. |
| `kill`   | Synonym for `stop`. |

Control phrases are parsed **before** a message is treated as a prompt and
always pre-empt an in-flight turn.

## Security Notes

- **An accepted message == typing into the agent.** The bridge is
  **least-privilege by default** (read-only tools) and **fails closed**:
  membership is verified before every outbound post, the daemon must be
  loopback-only, and every accepted prompt is audit-logged (redacted). Do not
  pass `--dangerous-all-tools` unless you fully trust the group membership.

## Prerequisites

- amplihack built with `--features signal`.
- A linked signal-cli daemon on this host (`amplihack signal setup`).
- `copilot` (GitHub Copilot CLI) on `PATH`.

## Related

- [docs/SIGNAL_BRIDGE.md](../../../docs/SIGNAL_BRIDGE.md) — full usage, security
  contract, configuration, and failure modes.
- [docs/SIGNAL_ONBOARDING.md](../../../docs/SIGNAL_ONBOARDING.md) — linking and
  the local daemon.
