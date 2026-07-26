# Signal Channel

A **feature-gated, per-session Signal messaging channel** for amplihack that
works for **both GitHub Copilot CLI and Claude Code**. When enabled, each
amplihack session opens a private Signal group **once for the whole session**,
**mirrors the entire conversation** to it (every user prompt and every
assistant turn), and lets an allow-listed operator send messages back into the
running session — injected **as if typed at the CLI input box**.

- **Crate:** `amplihack-signal`
- **Cargo feature:** `signal` (default **OFF**)
- **Wire protocol:** signal-cli JSON-RPC 2.0 over newline-delimited TCP
- **Hosts:** Copilot CLI **and** Claude Code (host-aware injection & teardown)
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
- [In-session onboarding prompt](#in-session-onboarding-prompt)
- [Fleet distribution: `amplihack signal distribute`](#fleet-distribution-amplihack-signal-distribute)
- [Exit codes](#exit-codes)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [Host awareness (Copilot vs Claude Code)](#host-awareness-copilot-vs-claude-code)
- [Group naming and lifecycle](#group-naming-and-lifecycle)
- [Per-session wiring](#per-session-wiring)
- [Full conversation mirroring](#full-conversation-mirroring)
- [Host-aware context injection](#host-aware-context-injection)
- [The inbound path (operator → agent)](#the-inbound-path-operator--agent)
- [The outbound path (agent → operator)](#the-outbound-path-agent--operator)
- [Security model / trust boundary](#security-model--trust-boundary)
- [Crate API reference](#crate-api-reference)
- [Building and testing](#building-and-testing)
- [Offline testing (fake JSON-RPC endpoint)](#offline-testing-fake-json-rpc-endpoint)
- [Troubleshooting](#troubleshooting)
- [FAQ](#faq)

> **External-service integration.** For the injectable seams, `amplihack-remote`
> public re-exports, resumable fleet-distribute state, and how the external
> boundaries (`signal-cli`, `azlin`/`az`, device linking) are tested, see
> [Signal External-Service Integration](signal-external-integration.md).

---

## How it works

```
┌────────────────────────┐    JSON-RPC 2.0 (NDJSON/TCP)     ┌──────────────┐
│   amplihack session     │  ──────────────────────────────► │  signal-cli   │
│   (hooks pipeline)      │   outbound: mirror EVERY user     │  daemon       │
│   host = copilot|claude │   prompt + EVERY assistant turn   │ (your number) │
│                         │  ◄────────────────────────────── └──────┬────────┘
└───────────┬─────────────┘          receive stream                │
            │                                                       │
            │ SessionStart: create group ONCE/session,               │
            │   post "session started", spawn detached subscriber    │
            │                                                       ▼
            │                                          ┌────────────────────────┐
            │  file inbox (AtomicJsonFile) ◄────────── │ signal-subscriber       │
            │        ▲                                 │ (long-lived connection) │
            │        │ drain + host-aware inject       │ allowlist + gate        │
  UserPromptSubmit / │  ───────────────────────────►  │ + groupId + echo-suppr. │
  PostToolUse        │  copilot: top-level             │ (fingerprint file)      │
            │        │  additionalContext (string)     └────────────────────────┘
            │        │  claude:  hookSpecificOutput.additionalContext
            ▼        │
  per-turn Stop:  OUTBOUND relay only (post assistant turn, NO teardown)
  SessionStop:    post summary → quitGroup → stop subscriber  (once/session)
```

The channel is wired through amplihack's existing **hooks** pipeline
(`amplihack-hooks`), not the recipe-runner. It is **host-aware**: the same
channel drives both **GitHub Copilot CLI** and **Claude Code**, detecting the
active host from `AMPLIHACK_AGENT_BINARY` and emitting the output shape each
host understands.

1. **SessionStart** loads config (or, on an un-onboarded host, offers the
   [in-session onboarding prompt](#in-session-onboarding-prompt)), creates a
   Signal group **once per session**, persists its `groupId` in session state,
   posts a "session started" message, and spawns a **detached, long-lived
   subscriber process** whose PID is persisted. The group lives for the
   **entire session**, not per turn.
2. The **subscriber** holds a single JSON-RPC connection, filters messages to
   this session's `groupId`, applies the gate (allowlist + setup-aware
   authorization + echo suppression), and appends accepted operator messages to
   a **per-session file inbox**.
3. **UserPromptSubmit** and **PostToolUse** hooks drain the file inbox and
   inject any queued operator messages into the agent **as if typed at the CLI
   input box**, using the [host-aware injection shape](#host-aware-context-injection).
4. **Full conversation mirroring** relays the whole session both ways: every
   **user prompt** (from `UserPromptSubmit` / `userPromptSubmitted`) and every
   **assistant turn** (from the per-turn `Stop` / `agentStop` hook, read from
   the `transcript_path`) is posted to the group as it happens.
5. The **per-turn Stop hook is OUTBOUND relay only** — it posts the assistant
   turn but **never tears down** the channel.
6. **SessionStop** (Claude `SessionEnd`, Copilot `sessionEnd`) posts a session
   summary, calls `quitGroup`, and stops the subscriber. Teardown fires
   **exactly once** at session end.

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
   with `endpoint`, `account`, and `allowlist = [account]`. It also emits an
   explicit `reuse_rolling_group = false` line (with a short opt-in caveat
   comment) so the **per-session default is discoverable in the file itself**:
   each session gets its own fresh group and the file documents that reusing a
   single shared group is an explicit opt-in. The generator writes **only** that
   `reuse_rolling_group = false` line — it never pre-writes a `rolling_group_id`;
   you add that yourself only when opting in (set `reuse_rolling_group = true`
   together with a `rolling_group_id`). See
   [the single-number rule](#configuration) for why the account's own number is
   allowlisted, and [Group naming and lifecycle](#group-naming-and-lifecycle)
   for the per-session-vs-rolling behavior. Environment variables and an
   explicit `AMPLIHACK_SIGNAL_CONFIG` still override this file; onboarding relies
   on the loader's default-path fallback to `~/.amplihack/signal-config.toml`
   (see [Configuration](#configuration) and
   [Per-session wiring](#per-session-wiring)).

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

## In-session onboarding prompt

You no longer have to remember to run `amplihack signal setup` ahead of time.
When you launch amplihack (Copilot CLI **or** Claude Code) on a host that is
**not yet configured for Signal**, the `SessionStart` hook offers a **fast,
skippable prompt** asking whether you want to add **this host** as a signal-cli
device and mirror your session to Signal:

```text
Signal channel is not configured on this host.
Add this host as a Signal device and mirror your sessions to Signal? [y/N]
```

### When the prompt appears

The prompt is shown **only** when **all** of these hold:

- `SignalConfig::load()` fails (no valid `~/.amplihack/signal-config.toml` and
  no env-var config), **and**
- stdout/stdin is an interactive **TTY**, **and**
- `AMPLIHACK_NONINTERACTIVE` is **unset** (or not `1`), **and**
- no **decline sentinel** exists at
  `~/.amplihack/runtime/signal/.onboarding-declined`.

If any condition fails, the prompt is **silently skipped** and the session
proceeds normally with the channel off. In particular, CI, scripts, and any run
with `AMPLIHACK_NONINTERACTIVE=1` never see the prompt.

### What each answer does

| Answer | Behavior |
|---|---|
| **Yes** (`y`) | Records onboarding intent and **spawns `amplihack signal setup` detached** (see [why it is detached](#why-onboarding-is-non-blocking)), then returns immediately. Signal stays **off for the current session**; mirroring activates automatically on the **next launch** once `~/.amplihack/signal-config.toml` exists. |
| **No** / Enter (`N`) | Writes the `.onboarding-declined` sentinel so **you are not asked again** on this host, and continues with the channel off. |
| **No response / non-TTY** | Skipped; nothing is written; the channel stays off. |

To be asked again after declining, delete the sentinel:

```bash
rm ~/.amplihack/runtime/signal/.onboarding-declined
```

Or onboard explicitly at any time with `amplihack signal setup`.

### Why onboarding is non-blocking

Device linking is **user-paced** (you scan a QR code with your phone) and can
take far longer than a hook is allowed to run — Copilot CLI enforces a ~30s
hook timeout. The prompt therefore only records your choice and **spawns the
real linking flow (`run_setup`) as a detached process**; it never blocks the
`SessionStart` hook on QR scanning. This guarantees the onboarding UX can
**never break, stall, or slow down a session**, whether you accept, decline, or
Signal is unreachable.

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
| Reuse rolling group | `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP` | `reuse_rolling_group` | optional | **Default `false` (per-session groups).** Opt-in only: a truthy value (`1`/`true`/`yes`/`on`, case-insensitive) reuses one long-lived shared group across every session instead of creating a fresh per-session group. Absent, empty, or explicit false values (`0`/`false`/`no`/`off`) resolve to per-session isolation; unknown tokens are rejected |
| Rolling group id | `AMPLIHACK_SIGNAL_ROLLING_GROUP_ID` | `rolling_group_id` | required when rolling reuse is enabled | Existing group id to bind to when — and only when — rolling reuse is opted into. Ignored while the per-session default is in effect |
| Config file path | `AMPLIHACK_SIGNAL_CONFIG` | — | optional | Explicit path to the TOML file below. When unset, the loader falls back to the onboarding default `~/.amplihack/signal-config.toml` |
| Pending inbox capacity | `AMPLIHACK_SIGNAL_INBOX_CAPACITY` | — | optional | Positive integer number of queued inbound operator instructions retained before overflow evicts the oldest pending instruction. Defaults to `32`; tune per host/workflow when operators intentionally send bursts |

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
# Per-session groups are the default. `amplihack signal setup` writes exactly
# this line (and nothing about `rolling_group_id`) so the isolation guarantee is
# visible in the generated file. Flip it to `true` AND uncomment/set
# `rolling_group_id` below to opt into one shared rolling group.
reuse_rolling_group = false
# rolling_group_id = "group.aBcDeF0123456789=="  # only used when reuse_rolling_group = true
```

Any value present in the environment overrides the same key in the file.

---

## Host awareness (Copilot vs Claude Code)

The channel drives **both** GitHub Copilot CLI and Claude Code from one code
path. It resolves the active host from the **`AMPLIHACK_AGENT_BINARY`**
environment variable (via `amplihack_utils::agent_binary::resolve`), which
accepts `copilot`, `claude`, `codex`, or `amplifier` (default: `copilot`).

Host detection controls two things that genuinely differ between the two CLIs:

| Concern | Copilot CLI | Claude Code |
|---|---|---|
| **Inbound injection shape** | **Top-level** `additionalContext` (string) on the `userPromptSubmitted` hook output | Nested `hookSpecificOutput.additionalContext` (string) |
| **Session-end event wiring** | `sessionEnd` → `session-stop-event` subcommand | `SessionEnd` → `session-stop-event` subcommand |

Everything else — group creation, mirroring, the subscriber, gating, echo
suppression, teardown — is host-independent.

> **Why the injection shape matters.** Copilot CLI **ignores** Claude Code's
> nested `hookSpecificOutput.additionalContext`. Operator messages injected in
> the Claude shape would silently never reach the Copilot agent. The channel
> therefore emits the **correct shape per host** so injected operator text
> reaches the agent's context on **both** CLIs. See
> [Host-aware context injection](#host-aware-context-injection).

To force a host explicitly (for testing or in wrappers):

```bash
export AMPLIHACK_AGENT_BINARY=copilot   # or: claude
```

The installers stage host-appropriate hook wrappers automatically (see
[Per-session wiring](#per-session-wiring)); you normally do not set this by hand.

---

## Group naming and lifecycle

### Only the top-level operator session gets a group

The Signal channel is **operator-facing**: it exists so a human can watch and
advise the single session they launched. A real amplihack run can spawn nested
sessions — the orchestrator, recipe steps, and sub-agents each have their own
`session_id`. If every nested session opened Signal, the operator's phone would
fill with empty groups.

SessionStart therefore applies a **nesting gate**: a Signal group is created
**only for the top-level operator session**. Nested sessions are an intended
no-op: they create no group, post no `session started` marker, persist no group
state, and spawn no subscriber.

Nesting is detected from `AMPLIHACK_SESSION_DEPTH`, which amplihack increments
for child sessions:

| `AMPLIHACK_SESSION_DEPTH` | Session kind | Signal group created? |
|---|---|---|
| unset or `0` | Top-level operator session | ✅ Yes |
| `1`, `2`, … | Nested recipe / orchestrator / sub-agent | ❌ No |

A non-numeric value is treated as depth `0`, failing toward preserving the
visible operator session.

### Group name

**Per-session (default).** On the top-level SessionStart a fresh group is
created **once for the entire session**. When running inside tmux the name is:

```
amplihack-<hostname>-<tmux-session>
```

Outside tmux it falls back to:

```
amplihack-<hostname>-<session-id>-<unix-timestamp>
```

Hostname, tmux session name, and session id are sanitized before use in the
display name. The `groupId` returned by signal-cli is persisted in session
state and reused for every mirrored message throughout the session. Group
creation is **idempotent**: if a group already exists for the session it is
reused, never recreated.

**Teardown happens once, at session end — not per turn.** The **per-turn
`Stop` / `agentStop` hook is outbound relay only** and must **never** close the
group. The group is closed with `quitGroup` **only** by the `SessionStop`
handler, which fires on:

- Claude Code **`SessionEnd`**, and
- Copilot CLI **`sessionEnd`** (wired to the `session-stop-event` subcommand —
  see [Per-session wiring](#per-session-wiring)),

both routed to `SessionStopHook`. This guarantees subscribers and groups do not
leak on Copilot session end.

> **The bug this fixes.** In the pre-R5 wiring, teardown lived in the **wrong
> place**: the per-turn `StopHook` (`stop` / `agentStop`) called
> `signal_integration::on_stop`, which posts "session complete" and runs
> `quitGroup`. Because Claude Code fires `Stop` **after every assistant turn**
> (and Copilot's only wired stop event was `agentStop` → `stop`), the group was
> torn down after the **first** turn, while the real `SessionStopHook`
> (`session-stop-event`) did **no** Signal teardown at all. The fix is a
> **migration, not an addition**: move the `on_stop` teardown call **out of**
> `StopHook` and **into** `SessionStopHook`, leave `StopHook` doing outbound
> relay only, and add a Copilot **`sessionEnd` → `session-stop-event`** wrapper
> so Copilot still tears down exactly once. Adding that wrapper is **required**,
> not optional — without it, a relay-only `StopHook` would leave Copilot with no
> teardown path and its groups/subscribers would leak.

**Rolling group (opt-in).** Per-session groups are the default; nothing needs
to be set to get them. To instead reuse a **single** long-lived group across all
sessions, explicitly opt in by setting `reuse_rolling_group = true` (or
`AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP=1`) **and** supplying `rolling_group_id`.
In this mode the group is **not** quit at SessionStop, so you keep one persistent
operator thread. Because this trades per-session isolation for a shared thread,
it must be requested deliberately — any absent, empty, or explicit false reuse
flag keeps the per-session default, unknown tokens are rejected, and a truthy
reuse flag without a group id is rejected.

| Phase | Per-session | Rolling |
|---|---|---|
| SessionStart | create group **once** + post "session started" | reuse group + post "session started" |
| Each user prompt | mirror the prompt to the group | mirror the prompt to the group |
| Each assistant turn (per-turn `Stop`) | mirror the turn to the group — **OUTBOUND relay only, no teardown** | mirror the turn — no teardown |
| SessionStop (`SessionEnd` / `sessionEnd`) | post summary → `quitGroup` + stop subscriber | post summary (group kept), stop subscriber |

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

### Host wrapper generation and `sessionEnd`

The installers generate host-appropriate hook wrappers and settings:

- **Claude Code** (`~/.claude/settings.json`): the `SessionStart`,
  `UserPromptSubmit`, `PostToolUse`, `Stop`, and `SessionEnd` events map to the
  matching `amplihack-hooks` subcommands. `SessionEnd` → `session-stop-event`.
- **Copilot CLI** (`~/.copilot` settings / `.github/hooks/`): the
  `COPILOT_HOOK_WRAPPERS` table maps camelCase events to subcommands, including
  the **`sessionEnd`** wrapper, which is wired to the **`session-stop-event`**
  subcommand (`SessionStopHook`). This is what makes Copilot teardown fire so
  **subscribers and groups do not leak on Copilot session end**.

Dispatch in the hooks binary routes stop-family subcommands to the correct
handler:

| Subcommand / alias | Handler | Role |
|---|---|---|
| `stop`, `agentStop` | `StopHook` | Per-turn **outbound relay only** (mirror assistant turn) — **no teardown** |
| `session-stop-event`, `session-end`, `session-stop` (Copilot `sessionEnd`, Claude `SessionEnd`) | `SessionStopHook` | **Teardown**: post summary → `quitGroup` → stop subscriber (once/session) |

> **Migration hazard — existing `session-end` / `session-stop` aliases.** Today
> `bins/amplihack-hooks/src/main.rs` routes `stop | session-end | session-stop`
> **all** to `StopHook`, and only the distinct `session-stop-event` subcommand
> reaches `SessionStopHook`. Once `StopHook` becomes relay-only, leaving those
> aliases on `StopHook` would silently drop teardown for any host still emitting
> a `session-end` / `session-stop` event name. **R5 resolves this by
> re-pointing the `session-end` and `session-stop` subcommand aliases at
> `SessionStopHook`** (the routing shown above), so every session-end event
> name — `session-stop-event`, `session-end`, `session-stop` — performs
> teardown, while `stop` / `agentStop` stay per-turn relay-only. The design's
> own risk list warns this "could change dispatch for other consumers relying on
> the old StopHook routing," so this is a **required, tested** change: add a
> dispatch test asserting **no teardown runs on per-turn `stop` / `agentStop`**
> and **exactly one** teardown runs for each session-end alias.

Wrapper ownership detection (`is_amplihack_owned`) recognizes the
`session-stop-event` / `sessionEnd` wrappers, so re-running the installer merges
cleanly and never duplicates amplihack-owned entries in user settings.

---

## Full conversation mirroring

The channel mirrors the **whole session conversation** to the Signal group — not
just "session started" / "session complete" markers. Both directions of the
conversation are relayed as they happen:

| Source hook | Copilot event | Claude event | Mirrored to group |
|---|---|---|---|
| User prompt | `userPromptSubmitted` | `UserPromptSubmit` | The **user's prompt text** |
| Assistant turn | `agentStop` | `Stop` | The **assistant's turn output**, read from the run's `transcript_path` |

Key properties:

- **Per-turn Stop is outbound-only.** Relaying the assistant turn happens in the
  per-turn `Stop` / `agentStop` hook, which **never** tears down the channel.
  Teardown **moves to** `SessionStop` — the `on_stop`/`quitGroup` call is
  relocated out of `StopHook` into `SessionStopHook` (see
  [Group lifecycle](#group-naming-and-lifecycle) for the before/after).
- **Message size is bounded.** Every mirrored body — user prompts, assistant
  turns, and injected operator context — is truncated at a UTF-8-safe boundary
  to **4 KiB**, with a `… [truncated N bytes]` suffix. This keeps individual
  Signal messages small and well within the transport's 256 KiB frame cap.
- **No echo loops.** The account's own mirrored messages are **never**
  re-injected as inbound operator instructions. Suppression uses two guards:
  1. **Device/`is_sync` gating** — signal-cli's own synced-back sends (from a
     linked device, `sourceDevice >= 2`) are rejected by the gate.
  2. **Shared outbound fingerprints** — every mirrored outbound body is
     fingerprinted (hashed, with a TTL) into an append-only file under
     `signal_root()` (`~/.amplihack/runtime/signal/<session-id>/`). The detached
     **subscriber process reads these fingerprints before injecting**, so echo
     suppression works **across processes** even though outbound relay runs in
     the hook process and inbound runs in the subscriber process.
- **Non-fatal.** If a mirror send fails (daemon down, truncation edge case,
  transcript unreadable), the failure is recorded in `warnings[]` and the turn
  proceeds normally.

---

## Host-aware context injection

When the inbox is drained (on `UserPromptSubmit` / `userPromptSubmitted` and
`PostToolUse` / `postToolUse`), queued operator messages are injected into the
agent **as if typed at the CLI input box**. The **output shape is host-aware**,
because the two CLIs read `additionalContext` from different places.

**Copilot CLI** — top-level `additionalContext` **string** on the hook output:

```json
{
  "additionalContext": "Operator (via Signal): please also update the changelog"
}
```

**Claude Code** — nested under `hookSpecificOutput` (unchanged, byte-for-byte
compatible with prior releases):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "UserPromptSubmit",
    "additionalContext": "Operator (via Signal): please also update the changelog"
  }
}
```

The host is resolved from `AMPLIHACK_AGENT_BINARY` (see
[Host awareness](#host-awareness-copilot-vs-claude-code)). Both the
`user_prompt` and `post_tool_use` drain sites route through the same host-aware
merge helper, so operator context reaches the agent's context on **both** CLIs.

> **The bug this fixes.** Copilot CLI **ignores** the nested
> `hookSpecificOutput.additionalContext` and reads a **top-level**
> `additionalContext` string (per the
> [Copilot hooks reference](https://docs.github.com/en/copilot/reference/hooks-reference)).
> Previously the inbound path always emitted the Claude-nested shape, so operator
> messages drained from the inbox **never reached the Copilot agent**. Host-aware
> injection makes end-to-end delivery work on Copilot while leaving Claude Code
> output identical.

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
   to prevent path traversal. The inbox is **bounded and tunable**: it holds at
   most `AMPLIHACK_SIGNAL_INBOX_CAPACITY` pending instructions, defaulting to
   `32` when unset or invalid. When full, the **oldest** queued instruction is
   dropped to make room for the newest and the subscriber logs a warning — a
   flood of inbound messages can never grow memory or disk without limit
   (backpressure by bounded queue), and the budget is explicit rather than
   buried in code.
5. On the next **UserPromptSubmit** or **PostToolUse** hook, the inbox is
   **drained** and its queued operator messages are emitted to the agent **as if
   typed at the CLI input box**, using the
   [host-aware injection shape](#host-aware-context-injection) — a **top-level**
   `additionalContext` string for Copilot CLI, or the nested
   `hookSpecificOutput.additionalContext` for Claude Code. Injected bodies are
   truncated to 4 KiB. Draining is one-shot: each message is delivered once.

If the subscriber cannot start, the failure is recorded in `warnings[]` and via
`tracing`; the session continues normally with no inbound channel.

**Reconnect resilience.** Once a connection has been established at least once, a
transient drop (daemon restart, stream close, receive error) does **not** end the
channel: the subscriber reconnects with **bounded exponential backoff** (1s → 2s
→ … capped at 30s), preserving its echo-suppression/de-dup state and file inbox
across reconnects so no instruction is lost or re-delivered. Any inbound message
resets the backoff. To avoid spinning against a permanently-down daemon it gives
keeps retrying at the capped backoff once a connection has succeeded; inbound
mirroring is not silently abandoned after an arbitrary failure count. A
**cold-start** connect failure (no connection ever established) stays fast and
non-fatal — SessionStart spawns the subscriber best-effort and is never stalled
by an absent daemon.

---

## The outbound path (agent → operator)

amplihack mirrors the **whole conversation** to the group (see
[Full conversation mirroring](#full-conversation-mirroring)) — every user prompt
and every assistant turn — plus lifecycle markers:

- **SessionStart** — "session started".
- **Each user prompt** — mirrored from `UserPromptSubmit` / `userPromptSubmitted`.
- **Each assistant turn** — mirrored from the per-turn `Stop` / `agentStop`
  hook, read from the run's `transcript_path` (outbound relay only, no teardown).
- **SessionStop** — a session summary, then `quitGroup` (per-session mode).

Every outbound body is **truncated to 4 KiB** at a UTF-8-safe boundary,
minimized, and redacted before sending. Each posted body is **fingerprinted
(hashed, TTL-bounded) into a shared file under `signal_root()`** so the
subscriber process will not treat the synced-back copy as an operator
instruction — echo suppression that works **across processes**. See
[Full conversation mirroring](#full-conversation-mirroring) for the echo-loop
guards.

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
- **No self-ingestion.** Echo suppression prevents the bot from re-processing
  its own mirrored messages that Signal syncs back to the account. It combines
  **device/`is_sync` gating** (reject the account's own linked-device sends,
  `sourceDevice >= 2`) with a **shared, TTL-bounded outbound-fingerprint file**
  under `signal_root()` that the subscriber consults before injecting — so
  suppression holds **across the hook and subscriber processes**.
- **Bounded egress (mirroring).** [Full conversation mirroring](#full-conversation-mirroring)
  relays user prompts and assistant turns outbound. Every mirrored body is
  **truncated to 4 KiB** (UTF-8-safe), minimized, and redacted before sending;
  egress is gated by the same loaded config and the `SIGNAL_ENABLED` process
  gate, so an un-onboarded or feature-off host sends nothing.
- **Feature default OFF.** No `signal` feature ⇒ no code, no dependencies, no
  network sockets. In-process tests additionally keep the `SIGNAL_ENABLED`
  process gate **off**, so no test performs real Signal I/O.
- **No silent config defaults.** Missing required config is an explicit error,
  never a guessed value.
- **Per-session group isolation by default.** `reuse_rolling_group` defaults to
  `false`, so each session gets its own group that is closed with `quitGroup`
  at SessionStop — no operator thread outlives the session that created it. Sharing
  one long-lived group across sessions is a deliberate opt-in
  (`reuse_rolling_group = true` / `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP=1`) plus
  a `rolling_group_id`; only non-empty truthy values enable it, and a missing
  group id is rejected instead of creating an untracked shared thread.
- **Path safety.** Every per-session file path (inbox, PID, decline sentinel,
  outbound-fingerprint file) is run through `sanitize_session_id`; files are
  written atomically with restrictive permissions.
- **Least privilege on shutdown.** Teardown runs **only** from `SessionStop` and
  kills **only the recorded subscriber PID**, never a name-matched sweep. The
  per-turn `Stop` hook never tears down the channel.
- **Bounded inbox (flood resistance).** The file inbox has a tunable capacity
  (`AMPLIHACK_SIGNAL_INBOX_CAPACITY`, default `32`); under an inbound flood the
  oldest instruction is evicted and the subscriber logs a warning rather than
  allowing unbounded memory/disk growth.
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
| `reuse_rolling_group` | `bool` | Opt-in: reuse one shared rolling group. Defaults to `false` (per-session groups) |
| `rolling_group_id` | `Option<String>` | Existing group id to bind rolling reuse to (required when `reuse_rolling_group` is `true`) |

### `transport`

Newline-delimited JSON-RPC 2.0 client over `tokio` TCP.

| Method | Purpose |
|---|---|
| `connect(addr) -> SignalTransport` | Open a JSON-RPC connection. `addr` is the daemon endpoint in production, or a [`FakeSignalEndpoint`](#offline-testing-fake-json-rpc-endpoint) address in tests |
| `create_group(name) -> GroupId` | Create a group (wraps the `updateGroup` create-by-name RPC) |
| `send_group(group_id, body)` | Post a message (wraps the `send` RPC) |
| `quit_group(group_id)` | Leave/close a group (`quitGroup`) |
| `receive()` stream | Async stream of parsed inbound envelopes |

> **RPC method names** track the signal-cli JSON-RPC surface. `create_group`
> is expected to map to `updateGroup` (creating a group by supplying a name and
> members); if the signal-cli daemon version in use names it differently,
> update this table to match the actual method invoked.

> **`FakeSignalEndpoint`** (test-only) implements the same newline-delimited
> JSON-RPC 2.0 surface on a loopback `127.0.0.1:0` listener, so `connect` can be
> pointed at it for hermetic offline tests. See
> [Offline testing](#offline-testing-fake-json-rpc-endpoint).

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
| `announce()` | Create/reuse the per-session group (idempotent) and post "session started" |
| `post(update)` | Post a throttled outbound update |
| `relay_outbound(kind, body)` | Mirror a user prompt or assistant turn to the group (4 KiB UTF-8-safe truncation + outbound fingerprint) |
| `poll()` / `drain()` | Read (and clear) queued inbound operator messages from the file inbox |

The inbox is an `AtomicJsonFile` (from `amplihack-state`) at a
`sanitize_session_id`-derived path, so writes by the subscriber process and
reads by the hook process are safe across processes.

### Feature modules (host integration)

The `amplihack-hooks` `signal_integration` module ties the crate into the hooks
pipeline. Key host-aware entry points:

| Item | Role |
|---|---|
| `signal_integration::on_session_start` | Create the group once, spawn the subscriber, and offer the [in-session onboarding prompt](#in-session-onboarding-prompt) |
| `signal_integration::drain_into_context` | Drain the inbox and produce the [host-aware injection](#host-aware-context-injection) output for the current host |
| `signal_integration::relay_outbound` | [Full conversation mirroring](#full-conversation-mirroring) for user prompts and assistant turns |
| `signal_integration::on_stop` | `SessionStop` teardown (`quitGroup` + stop subscriber) — **never** called from the per-turn `Stop` hook |
| `signal_integration::set_process_enabled` | Toggle the `SIGNAL_ENABLED` process gate (ON in the real binary, OFF in tests) |
| `FakeSignalEndpoint` | Test-only loopback JSON-RPC fake — see [Offline testing](#offline-testing-fake-json-rpc-endpoint) |

---

## Building and testing

The feature must build and test cleanly **both** ways — this is a hard quality
gate:

```bash
# Default build: feature OFF (Signal fully compiled out).
cargo build
cargo test

# Feature ON — build the hooks binary and run the hooks test-suite.
cargo build --release -p amplihack-hooks-bin --features signal
cargo test  --release -p amplihack-hooks     --features signal
```

Integration tests are registered as explicit `[[test]]` targets and resolve the
hooks binary via `env!("CARGO_BIN_EXE_amplihack-hooks")`, so they exercise the
real `signal-subscriber` subcommand rather than an in-process stub. Pure
wire/gating tests run with no network or filesystem I/O.

**All tests are hermetic and offline.** `cargo test --release -p amplihack-hooks
--features signal` **creates no real Signal groups and touches no real Signal
network** — see [Offline testing](#offline-testing-fake-json-rpc-endpoint). The
existing hook and golden-snapshot suites are **preserved green**, with **two new
Copilot golden cases** added for the top-level `additionalContext` injection
shape (Claude Code golden output is unchanged, byte-for-byte). The regression
bar is "the full pre-existing suite still passes plus the two new Copilot
goldens" — run `cargo test` for the current counts rather than relying on a
number pinned in this doc.

---

## Offline testing (fake JSON-RPC endpoint)

Signal channel tests **never hit the real Signal network**. Two independent
mechanisms guarantee this:

### 1. The `SIGNAL_ENABLED` process gate

In-process hook tests run with Signal **I/O disabled** by default. The hooks
binary opts in explicitly at startup (`signal_integration::set_process_enabled(true)`),
while tests leave the gate **off** so exercising `on_session_start`,
`drain_into_context`, and `on_stop` performs **no real Signal I/O** and creates
**no groups**.

### 2. `FakeSignalEndpoint` — a local JSON-RPC fake

For tests that must exercise the transport and subscriber loop end-to-end,
`amplihack-signal` ships a **fake signal-cli JSON-RPC endpoint** modeled on
signal-cli's own **JSON-RPC daemon (`--tcp`) socket mode**. It is a
`tokio::net::TcpListener` bound to **`127.0.0.1:0`** (an ephemeral loopback
port) that speaks **newline-delimited JSON-RPC 2.0**, the same protocol the real
daemon uses. `SignalTransport::connect` is pointed at the fake's address, so the
full method surface can be driven deterministically:

| RPC method | Exercised behavior |
|---|---|
| `updateGroup` (create) | Returns a synthetic `groupId`; asserts create-group requests |
| `send` | Records outbound bodies for assertions (mirroring, truncation, fingerprints) |
| `receive` | Streams scripted inbound envelopes into the subscriber loop |
| `quitGroup` | Records teardown; asserts `SessionStop` (not per-turn `Stop`) closes the group |

Because the endpoint binds **loopback-only** and returns synthetic ids, tests
can cover **send / create-group / receive / quit-group** and the **inbound
subscriber loop** — including gating, echo suppression, host-aware injection,
reconnect/backoff, and full-conversation mirroring — with **zero** external
dependencies and **no** real Signal account, group, or network traffic.

```rust
// Illustrative test wiring (offline, deterministic, no real Signal).
let fake = FakeSignalEndpoint::start().await;   // binds 127.0.0.1:0
let transport = SignalTransport::connect(fake.addr()).await?;

let group = transport.create_group("amplihack-test").await?;   // synthetic id
transport.send_group(&group, "session started").await?;
fake.push_inbound(/* scripted operator envelope */);           // drive receive()
// ... assert the message was injected in the host-aware shape ...
transport.quit_group(&group).await?;

assert_eq!(fake.groups_created(), 1);   // never a REAL group
```

> **CI guarantee.** The combination of the default-OFF `SIGNAL_ENABLED` gate and
> the loopback-only `FakeSignalEndpoint` means no test path can reach the real
> Signal service. CI runs the full `--features signal` suite with no signal-cli
> installed and no network access.

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
| Some instructions never arrive | Inbox overflowed under a burst | The inbox is bounded and logs overflow warnings; raise `AMPLIHACK_SIGNAL_INBOX_CAPACITY` or send fewer, more deliberate instructions |
| `signal setup` can't install signal-cli | No package/JRE available non-interactively | Follow the printed install guidance, install signal-cli manually, then re-run `amplihack signal setup` |
| `signal setup` fails on port | `127.0.0.1:<port>` held by an unknown process | Free the port or pass `--port <other>` / set `AMPLIHACK_SIGNAL_PORT` |
| A VM shows `failed: link limit reached` | Signal linked-device cap hit | Unlink an unused device in Signal, or use `--identity-mode dedicated-number` for very large fleets |
| `distribute` stopped part-way | Interrupted / a VM failed | Re-run `amplihack signal distribute`; it resumes from `~/.amplihack/signal-distribute-state.json` and retries only pending/failed VMs |
| Operator messages never reach the Copilot agent | Wrong injection shape (nested instead of top-level) | Fixed by [host-aware injection](#host-aware-context-injection). Confirm `AMPLIHACK_AGENT_BINARY=copilot`; Copilot needs the **top-level** `additionalContext` string |
| Conversation not mirrored (only start/stop) | Old behavior / feature off | [Full mirroring](#full-conversation-mirroring) relays every user prompt + assistant turn; confirm `--features signal` and that `transcript_path` is readable |
| Group is torn down after one turn | Teardown wired to per-turn Stop | Per-turn `Stop`/`agentStop` is **outbound-only**; teardown must be on `SessionStop` (`SessionEnd`/`sessionEnd` → `session-stop-event`) |
| Subscriber/group leaks after Copilot exits | Copilot `sessionEnd` not wired | Re-run the installer so the `sessionEnd` → `session-stop-event` wrapper is generated (see [Host wrapper generation](#host-wrapper-generation-and-sessionend)) |
| In-session onboarding prompt never appears | Non-TTY, `AMPLIHACK_NONINTERACTIVE=1`, already configured, or previously declined | Expected. Delete `~/.amplihack/runtime/signal/.onboarding-declined` to be asked again, or run `amplihack signal setup` |
| Bot re-injects its own mirrored messages | Echo suppression gap | Should not happen: device/`is_sync` gating + shared outbound-fingerprint file suppress echoes across processes |

Because every Signal operation is non-fatal, none of the above can break your
amplihack session — worst case the channel is unavailable, with the failure
surfaced through hook warnings and/or `tracing`, and the run proceeds normally.

---

## FAQ

**Does the Signal channel work with GitHub Copilot CLI, or only Claude Code?**
Both. The channel is host-aware (`AMPLIHACK_AGENT_BINARY`): it emits a top-level
`additionalContext` string for Copilot and the nested
`hookSpecificOutput.additionalContext` for Claude Code, and wires teardown to
Copilot's `sessionEnd` as well as Claude's `SessionEnd`. See
[Host awareness](#host-awareness-copilot-vs-claude-code).

**Does enabling Signal add dependencies to the default build?**
No. With the feature off, `amplihack-signal` and its `tokio`-net dependencies
are not compiled or linked.

**Is the whole conversation mirrored, or just start/stop?**
The whole conversation — every user prompt and every assistant turn — bounded to
4 KiB per message. See [Full conversation mirroring](#full-conversation-mirroring).

**Do the tests hit the real Signal network?**
Never. The default-OFF `SIGNAL_ENABLED` gate plus the loopback-only
`FakeSignalEndpoint` guarantee no real groups or network traffic. See
[Offline testing](#offline-testing-fake-json-rpc-endpoint).

**Can an operator make amplihack run a command by texting it?**
No. Inbound text is delivered only as `additionalContext`. The agent decides
whether to act, and all normal safety hooks still apply.

**Per-session vs rolling group — which should I use?**
Per-session is the **default** and needs no configuration: each session creates
its own fresh group and cleans it up at Stop via `quitGroup`, giving clean
isolation and no cross-session message disclosure. Rolling is a deliberate
opt-in that keeps one persistent operator thread across runs; enable it by
setting `reuse_rolling_group = true` with a `rolling_group_id`.
Prefer the per-session default unless you specifically want one shared thread.

**Where do inbound instructions get stored?**
In a per-session, atomically-written JSON inbox whose path is derived through
`sanitize_session_id`. The inbox is bounded (oldest entries evicted under a
flood) and is drained (delivered once) on the next
`PostToolUse` / `UserPromptSubmit`.

---

## See also

- [Signal onboarding how-to](SIGNAL_ONBOARDING.md) — `setup` and `distribute` walkthrough
- [Signal external service integration](signal-external-integration.md) — seam architecture, resumable fleet state, and `amplihack-remote` re-exports
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config
- [Signal onboarding — performance notes](reference/signal-onboarding-performance.md) — hot paths & allocation trims
- [Hook configuration guide](HOOK_CONFIGURATION_GUIDE.md)
- [Security recommendations](SECURITY_RECOMMENDATIONS.md)
