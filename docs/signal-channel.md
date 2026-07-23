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
- [Onboarding: `amplihack signal setup`](#onboarding-amplihack-signal-setup)
- [Fleet distribution: `amplihack signal distribute`](#fleet-distribution-amplihack-signal-distribute)
- [Exit codes](#exit-codes)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [Group naming and lifecycle](#group-naming-and-lifecycle)
- [Per-session wiring](#per-session-wiring)
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

> **New in this release — one-command onboarding.** You no longer have to
> install signal-cli, link a device, start a daemon, and hand-write a config
> file yourself. The `amplihack signal setup` command does all of it
> interactively (QR-based device linking), and `amplihack signal distribute`
> rolls the same onboarding out across an entire fleet of Azure Linux VMs so
> the per-session channel works **out of the box on any host**. The manual
> [Quick start](#quick-start) below still works and documents exactly what
> `setup` automates. See the companion how-to:
> [Signal onboarding](SIGNAL_ONBOARDING.md).

---

## Onboarding: `amplihack signal setup`

`amplihack signal setup` is a **first-class onboarding command** that turns a
bare host into a fully working Signal channel host with a single interactive
run. It performs every step the channel needs and is **idempotent** — re-running
it repairs only what is missing and never re-links an already-linked device or
clobbers a valid config.

```bash
amplihack signal setup
```

### What it does

1. **Detects signal-cli.** If it is already installed it is reused. If it is
   missing, `setup` installs it where it safely can, otherwise it prints
   **clear, actionable install guidance** and exits non-zero. There is **no
   silent fallback** — an unusable signal-cli is always surfaced as an error.
2. **Links this host as a Signal device.** It runs `signal-cli link`, captures
   the device-link URI it emits, and renders it as a **scannable QR code
   directly in your terminal**, with the raw URI printed underneath as a
   copy/paste fallback. Open **Signal on your phone → Settings → Linked
   devices → Link new device** and scan it.

   > **Link-URI scheme.** `setup` encodes **whatever URI signal-cli emits** — it
   > does not assume a scheme. Recent signal-cli (libsignal-based) emits
   > `sgnl://linkdevice?uuid=...&pub_key=...`; older builds emit the legacy
   > `tsdevice:/?uuid=...&pub_key=...`. Both are handled transparently; the QR
   > renderer is scheme-agnostic.

   ```text
   Scan this QR code with Signal (Settings → Linked devices → Link new device):

     █▀▀▀▀▀█ ▄▀ ▄▀█ █▀▀▀▀▀█
     █ ███ █ ▀█▄▀▄  █ ███ █
     █ ▀▀▀ █ █ ▄▀▀█ █ ▀▀▀ █
     ▀▀▀▀▀▀▀ █▄▀▄█▀ ▀▀▀▀▀▀▀
     ... (truncated) ...

   Or paste this link into Signal manually:
     sgnl://linkdevice?uuid=...&pub_key=...

   Waiting for you to approve the link on your phone…
   ```

   The wait for approval uses **liveness / idle detection**, not a fixed
   wall-clock timeout — take as long as you need to reach your phone. `setup`
   knows linking finished when signal-cli reports the new device is registered.
3. **Starts a local JSON-RPC daemon.** After linking succeeds it starts
   signal-cli in daemon mode bound to **loopback only** (`127.0.0.1:<port>`,
   default `7583`) as a **managed background service** — a **systemd `--user`
   unit** when systemd is available, otherwise a detached `nohup` process. The
   daemon **must be local to the session host**; a shared remote daemon reached
   over a tunnel does not work for the low-latency JSON-RPC this channel
   requires.
4. **Writes the config.** It writes `~/.amplihack/signal-config.toml` (mode
   `0600`) using the **exact existing [`SignalConfig`](#configuration) schema**,
   with `endpoint`, `account`, and `allowlist = [account]`. See
   [the single-number rule](#configuration) for why the account's own number is
   allowlisted. Environment variables and an explicit `AMPLIHACK_SIGNAL_CONFIG`
   still override this file; onboarding relies on the loader's default-path
   fallback to `~/.amplihack/signal-config.toml` (see
   [Configuration](#configuration) and [Per-session wiring](#per-session-wiring)).

That is the whole onboarding. The next amplihack session on this host will pick
up the config automatically — no further steps (see
[Per-session wiring](#per-session-wiring)).

### Flags

| Flag | Purpose |
|---|---|
| `--port <PORT>` | Daemon bind port on `127.0.0.1` (default `7583`, or `AMPLIHACK_SIGNAL_PORT`). If the port is held by an amplihack-managed daemon it is reused; if held by an unknown process, `setup` fails cleanly with guidance. |
| `--force` | Repair/overwrite even when probes report an existing setup. Use with care — this can re-link the device. |
| `--json` | Machine-readable status output. The link URI is **never** emitted in `--json` (it is a secret, stderr-only). |
| `--all-vms` | Alias for [`amplihack signal distribute`](#fleet-distribution-amplihack-signal-distribute). |

### Idempotency and repair

`setup` reports three independent probes and repairs only the missing pieces:

| Probe | Meaning | If already satisfied |
|---|---|---|
| **linked** | signal-cli account data / `listDevices` present | Never re-links |
| **daemon-running** | JSON-RPC ping to the endpoint succeeds | Reuses the running daemon |
| **config-written** | `~/.amplihack/signal-config.toml` parses under the schema | Left untouched (unless `--force`) |

Running `amplihack signal setup` a second time on an already-onboarded host is
safe and fast — it verifies all three probes and exits `0`.

---

## Fleet distribution: `amplihack signal distribute`

`amplihack signal distribute` runs the same onboarding across **every VM in your
Azure Linux (azlin) fleet**, so each host ends up with its **own local
signal-cli daemon** and its own `~/.amplihack/signal-config.toml`.

```bash
# Roll onboarding out to every discovered VM in a resource group.
amplihack signal distribute --resource-group <rg>

# Or target an explicit VM list.
amplihack signal distribute --vms vm-a,vm-b,vm-c --resource-group <rg>

# Equivalent alias.
amplihack signal setup --all-vms --resource-group <rg>
```

### Identity model — one number, many linked devices

Each VM becomes **its own linked device on your single Signal number**. This is
Signal-native and preserves **one chat identity** across the whole fleet. The
consequences are important:

- **Every VM needs its own device-link approval.** This is an unavoidable Signal
  requirement — you scan one QR code per VM, one at a time. `distribute`
  orchestrates this: it generates a per-host link URI/QR, presents it, waits
  (idle detection, no wall-clock cap), and moves on once that VM is linked.
- **signal-cli account data is never cloned between hosts.** Cloning one host's
  account store to multiple concurrently-running hosts causes device-identity /
  ratchet conflicts and is unsafe. Each VM links **independently**.
- **Signal enforces a linked-device count limit** (a small, fixed number of
  devices per account). For fleets larger than that limit, use the
  **dedicated-number mode extension point** (see below).

### How the rollout runs

- **Discovery is generic.** VMs are enumerated via the existing azlin CLI
  (`azlin list` / `az vm list` within the operator's resource group) or an
  explicit `--vms` list. **No host is hardcoded.**
- **Remote execution** uses the existing azlin transport:
  `azlin connect <vm> --resource-group <rg> --no-tmux -y -- '<cmd>'`.
- **Onboarding runs one VM at a time.** Interactive linking is inherently
  sequential (you have one phone, and interleaved QR codes on a single terminal
  are unscannable), so the fleet rollout onboards VMs one at a time.
  `--concurrency` is accepted for forward-compatibility with future
  non-interactive rollout phases but is **not** applied to the interactive
  device-link step; passing a value `> 1` prints a notice and proceeds
  sequentially. (There is **no arbitrary hard cap** — the constraint is the
  human scan step, not a fixed resource limit.)
- **Resumable.** State is persisted to `~/.amplihack/signal-distribute-state.json`
  keyed by VM name. Re-running `distribute` **skips VMs that already succeeded**
  and retries only `pending` / `failed` ones.
- **Failures are isolated and explicit.** A failure on one VM (e.g. signal-cli
  install failed, device-limit reached, port conflict) **never aborts the
  run**. It is recorded with a reason and surfaced in the summary — there is
  **no silent degradation**.

### Per-VM status

Each VM moves through these states, all reported at the end of the run:

| Status | Meaning |
|---|---|
| `pending` | Not yet started (or queued for retry) |
| `linking` | Waiting for you to approve the device link on your phone |
| `linked` | Device linked, daemon not yet up |
| `daemon-running` | Local JSON-RPC daemon is up on `127.0.0.1:<port>` |
| `config-written` | `~/.amplihack/signal-config.toml` written — **terminal success** |
| `failed` | Onboarding could not complete; a human-readable `reason` is recorded |

Example summary:

```text
Signal fleet distribution — 5 VMs
  vm-build-01   config-written
  vm-build-02   config-written
  vm-gpu-03     failed          reason: signal-cli install (no JRE; install guidance printed)
  vm-gpu-04     config-written
  vm-edge-05    failed          reason: link limit reached (Signal linked-device cap)

3 succeeded, 2 failed. Re-run `amplihack signal distribute` to retry the failed VMs.
```

### Flags

| Flag | Purpose |
|---|---|
| `--resource-group <rg>` | Azure resource group to discover / connect VMs in |
| `--vms <a,b,c>` | Explicit VM list instead of auto-discovery |
| `--concurrency <N>` | Reserved for future non-interactive phases; interactive linking always runs sequentially (values `> 1` print a notice) |
| `--identity-mode <mode>` | `linked-device` (default) or `dedicated-number` (see below) |
| `--json` | Machine-readable per-VM status output |
| `--force` | Re-run onboarding on VMs already marked successful |

### Dedicated-number mode (extension point)

For fleets larger than Signal's linked-device limit, `distribute` reserves a
config-selectable `identity_mode = "dedicated-number"` in which each VM would
register its **own** Signal number instead of linking to a shared one. This mode
is a **clean, documented extension point**: selecting it today returns an
explicit "not yet implemented" error rather than a partial or silent behavior.
The default `linked-device` mode is fully implemented.

---

## Exit codes

Both `signal setup` and `signal distribute` map every failure through a
**single source-of-truth taxonomy** so results are scriptable (and stable under
`--json`). Codes are distinct — a caller can branch on *why* onboarding stopped:

| Code | Name | Meaning |
|---|---|---|
| `0` | `SUCCESS` | Fully onboarded (or nothing to do — idempotent re-run). |
| `2` | `USAGE` | Invalid arguments / flag combination (clap-level). |
| `3` | `UNSUPPORTED` | Built without the `signal` feature. A clean error, **not** a hidden no-op (#921). |
| `4` | `PRECONDITION` | signal-cli missing/uninstallable, or an invalid/unwritable config — a setup precondition failed. |
| `5` | `PARTIAL` | Fleet run finished but **one or more VMs failed** (`distribute` only). Re-run to retry pending/failed VMs. |
| `6` | `DAEMON` | Local daemon could not start — e.g. `127.0.0.1:<port>` held by an unknown process. Never silently rebinds. |
| `7` | `LINK` | Device linking failed — e.g. approval error or Signal's **linked-device cap** reached. |

`--json` emits the same outcome as a structured object (per-VM for
`distribute`); the link URI is **never** included (see the security model).

---

## Quick start

> The fastest path is `amplihack signal setup` (see
> [Onboarding](#onboarding-amplihack-signal-setup)), which automates every step
> below. The manual steps here document exactly what `setup` does for you.

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
environment variable  >  TOML at AMPLIHACK_SIGNAL_CONFIG  >  ~/.amplihack/signal-config.toml  >  error
```

The final `~/.amplihack/signal-config.toml` step is the **onboarding default
path**: `amplihack signal setup` writes the config there, and the loader
consults it when neither environment variables nor an explicit
`AMPLIHACK_SIGNAL_CONFIG` supply a setting. This default-path fallback is added
by the onboarding feature (a loader change in `SignalConfig::load` /
`load_config_or_disabled`) so a freshly-onboarded host needs **no** exported
variables — see [Per-session wiring](#per-session-wiring). Absent onboarding
(and without env vars or `AMPLIHACK_SIGNAL_CONFIG`) the loader still errors and
the channel stays off; there are **no other silent defaults**.

### Settings

| Setting | Env var | TOML key | Required | Format / notes |
|---|---|---|---|---|
| Endpoint | `AMPLIHACK_SIGNAL_ENDPOINT` | `endpoint` | ✅ | `host:port` of the signal-cli JSON-RPC daemon |
| Account | `AMPLIHACK_SIGNAL_ACCOUNT` | `account` | ✅ | E.164 (`+` then digits) — the number amplihack sends **as** |
| Allowlist | `AMPLIHACK_SIGNAL_ALLOWLIST` | `allowlist` | ✅ | Operator numbers allowed to send inbound. Env = comma-separated E.164. **Empty ⇒ fail-closed (deny all inbound).** |
| Own device id | `AMPLIHACK_SIGNAL_OWN_DEVICE_ID` | `own_device_id` | optional | signal-cli's **own** linked-device id (must be `>= 2`). Only used to reject the bot's own synced-back echoes explicitly; the primary-phone (device `1`) gate is the main loop guard and needs no configuration. Leave unset unless you know your signal-cli device id |
| Reuse rolling group | `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP` | `reuse_rolling_group` | optional | `true`/`1` reuses one long-lived group instead of per-session groups |
| Rolling group id | `AMPLIHACK_SIGNAL_ROLLING_GROUP_ID` | `rolling_group_id` | optional | Existing group id to reuse when rolling mode is on |
| Config file path | `AMPLIHACK_SIGNAL_CONFIG` | — | optional | Explicit path to the TOML file below. When unset, the loader falls back to the onboarding default `~/.amplihack/signal-config.toml` |

> **Fail-closed allowlist.** An **empty** allowlist is a valid, deliberate
> configuration meaning "accept no inbound instructions." It is *not* treated
> as "allow everyone." Outbound posting still works; only the inbound path is
> closed.

> **Single-number setups must allowlist their own number.** If signal-cli is a
> linked device on your *own* number, your phone replies arrive as the account's
> own synced messages, so the **`account` number itself must be on the
> allowlist**. For a dedicated-number setup, allowlist the operator's *separate*
> number instead. An account number missing from the allowlist means every
> reply is silently denied (fail-closed).

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

## Per-session wiring

Onboarding output feeds the existing SessionStart integration with **zero
further steps**. The channel loads its configuration with the standard
precedence, extended by the onboarding feature with a **default-path fallback**:

```
environment variables  >  AMPLIHACK_SIGNAL_CONFIG (TOML)  >  ~/.amplihack/signal-config.toml  >  error
```

> **Implementation note.** Today `SignalConfig::load` reads a TOML file only via
> `AMPLIHACK_SIGNAL_CONFIG`. The onboarding feature adds the final
> `~/.amplihack/signal-config.toml` step (in `SignalConfig::load` or the hook's
> `load_config_or_disabled`). This default-path fallback is the mechanism that
> makes the "zero further steps" promise hold — it **must ship with onboarding**.

So after `amplihack signal setup` (or `distribute`) has written
`~/.amplihack/signal-config.toml` on a host, **every new amplihack session on
that host automatically opens its own dedicated Signal group** — you do not need
to export any environment variables or set `AMPLIHACK_SIGNAL_CONFIG`. Env vars
still override the file when present, so nothing about the existing precedence
changes; the default path is only consulted when neither env vars nor an
explicit config path supply the settings.

Every Signal operation remains **non-fatal**: any failure is appended to the
hook's `warnings[]` and logged via `tracing`, and the session proceeds
normally. Onboarding does not change this contract — a missing, malformed, or
unreachable configuration can never crash or block a session.

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

### Onboarding-specific boundaries

`signal setup` / `signal distribute` add their own hardening on top of the
runtime channel:

- **Loopback-only daemon.** The JSON-RPC daemon binds `127.0.0.1:<port>` only;
  non-loopback / wildcard binds are refused and the port is never forwarded.
- **Link URI is High-sensitivity.** The device-link URI (`sgnl://linkdevice?...`
  or legacy `tsdevice:/?...`, whichever signal-cli emits) is written to
  **stderr only** — never logged, persisted, or emitted under `--json`.
- **Injection-safe fan-out.** VM / resource-group names are **validated and
  rejected** at the boundary (E.164 account, `1..=65535` port, charset-checked
  names) *before* shell-escaping; secrets travel via base64-over-stdin, never on
  `argv`. Validation is fail-closed, not silent stripping.
- **`0600` on disk.** `signal-config.toml` and `signal-distribute-state.json`
  are written atomically (temp-then-rename) with `0600` permissions.
- **Allowlist integrity preserved.** The writer emits **only**
  `allowlist = [account]` — never empty or wildcard — keeping the fail-closed
  gate intact. `gating.rs` is untouched.

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
| `signal setup` can't install signal-cli | No package/JRE available non-interactively | Follow the printed install guidance, install signal-cli manually, then re-run `amplihack signal setup` |
| `signal setup` fails on port | `127.0.0.1:<port>` held by an unknown process | Free the port or pass `--port <other>` / set `AMPLIHACK_SIGNAL_PORT` |
| A VM shows `failed: link limit reached` | Signal linked-device cap hit | Unlink an unused device in Signal, or use `--identity-mode dedicated-number` for very large fleets |
| `distribute` stopped part-way | Interrupted / a VM failed | Re-run `amplihack signal distribute`; it resumes from `~/.amplihack/signal-distribute-state.json` and retries only pending/failed VMs |

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

- [Signal onboarding how-to](SIGNAL_ONBOARDING.md) — `setup` and `distribute` walkthrough
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config
- [Signal onboarding — performance notes](reference/signal-onboarding-performance.md) — hot paths & allocation trims
- [Hook configuration guide](HOOK_CONFIGURATION_GUIDE.md)
- [Security recommendations](SECURITY_RECOMMENDATIONS.md)
