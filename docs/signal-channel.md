# Signal Channel

A **feature-gated, opt-in Signal integration** for amplihack that works for
**both GitHub Copilot CLI and Claude Code**. It has exactly **two** parts:

1. **Onboarding** — making a host Signal-ready. The `amplihack signal setup`
   command links this host as a signal-cli device, starts a loopback JSON-RPC
   daemon, and writes `~/.amplihack/signal-config.toml`; `amplihack signal
   distribute` rolls that onboarding out across an Azure Linux fleet; and a
   one-time, **purely local** in-session notice offers to run `setup` on an
   un-onboarded interactive host. This is the only Signal-related work the hook
   lifecycle performs.
2. **The operator conversation** — `amplihack signal chat <topic>` opens a
   fresh, operator-only Signal group and drives a two-way agent session from it:
   agent output is posted to the group, and every operator reply becomes the
   next agent turn. This command is **self-contained** (it does not use the hook
   pipeline) and is documented in full in [Signal Chat](SIGNAL_CHAT.md).

> **Session start performs no Signal group I/O.** Starting an amplihack session
> — **at any nesting depth** — never creates or reuses a Signal group, posts no
> "session started" message, persists no group id, and spawns no subscriber.
> The **only** Signal-related thing session start may do is offer the local
> onboarding notice described below. Opening a Signal group and having a
> conversation is an **explicit operator action** (`amplihack signal chat
> <topic>`). This replaces a previous always-on, per-session group-creation
> channel that flooded operators with hundreds of empty, unattributable groups;
> that automatic channel — its background subscriber, its per-turn conversation
> mirroring, and its `UserPromptSubmit`/`PostToolUse` inbox injection — has been
> **removed**.

- **Crates:** `amplihack-signal` (protocol, gating, chat cores),
  `amplihack-cli` (`signal setup` / `distribute` / `chat`), `amplihack-hooks`
  (the local onboarding notice only)
- **Cargo feature:** `signal` (default **OFF**)
- **Wire protocol:** signal-cli JSON-RPC 2.0 over newline-delimited TCP
- **Hosts:** Copilot CLI **and** Claude Code (the onboarding notice fires on
  either host; `signal chat` drives a `copilot` session)
- **Trust model:** operator text is surfaced to the agent as *context / a turn
  prompt only* and is **never auto-executed**

> **Status:** the feature is compiled out entirely unless you build with
> `--features signal`. With the feature off there is zero runtime cost and no
> new dependencies pulled into the default build.

---

## Contents

- [How it works](#how-it-works)
- [Prerequisites](#prerequisites)
- [Onboarding: `amplihack signal setup`](#onboarding-amplihack-signal-setup)
- [In-session onboarding notice](#in-session-onboarding-notice)
- [Fleet distribution: `amplihack signal distribute`](#fleet-distribution-amplihack-signal-distribute)
- [Exit codes](#exit-codes)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [The operator conversation: `amplihack signal chat`](#the-operator-conversation-amplihack-signal-chat)
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

> **Generic turn loop.** The `amplihack signal chat` loop is built on the
> agent-generic `amplihack_turn::run_session_loop` via a `SignalChannel` that
> implements `amplihack_turn::Channel`. See
> [Signal on the Generic Turn Loop](signal-channel-turn-loop.md).

---

## How it works

```
                          ┌──────────────────────────────────────────┐
  amplihack session       │  Onboarding (setup / distribute / notice) │
  (any host, any depth)   │  → links a signal-cli device on this host │
        │                 │  → starts a loopback JSON-RPC daemon      │
        │  SessionStart:   │  → writes ~/.amplihack/signal-config.toml │
        │  purely-local    └──────────────────────────────────────────┘
        │  onboarding                            │  makes the host
        │  notice only                           │  Signal-ready
        ▼  (no group I/O)                         ▼
  ┌───────────────────┐                ┌────────────────────────────────┐
  │ session proceeds   │                │  amplihack signal chat <topic>  │
  │ normally; Signal   │  operator      │  (explicit operator command)    │
  │ stays off unless   │  opens a  ───► │  → creates a fresh group        │
  │ chat is opened     │  chat          │  → drives a two-way `copilot`    │
  └───────────────────┘                │    session from the group       │
                                        └────────────────────────────────┘
```

- **SessionStart performs no Signal group I/O.** It never creates a group,
  posts a message, persists a group id, or spawns a subscriber. On an
  un-onboarded interactive host it may offer the one-time, purely-local
  [onboarding notice](#in-session-onboarding-notice); that is its only
  Signal-related action. It does **no** network I/O.
- **Groups and conversations exist only on demand.** When you want to observe
  or steer a running host, you run [`amplihack signal chat <topic>`](#the-operator-conversation-amplihack-signal-chat).
  That command owns the entire conversation lifecycle — group creation, posting
  agent output, accepting operator replies as turns, and teardown — inside its
  own process. See [Signal Chat](SIGNAL_CHAT.md) for the complete contract.
- **Non-fatal by design.** Onboarding and the session-start notice are
  non-fatal: a missing, malformed, or unreachable Signal configuration is logged
  via `tracing` (and, for the notice, appended to the hook `warnings[]`) and the
  session proceeds normally. A broken Signal daemon can never crash or block
  your session.

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

> **New — one-command onboarding.** You no longer have to install signal-cli,
> link a device, start a daemon, and hand-write a config file yourself. The
> `amplihack signal setup` command does all of it interactively (QR-based device
> linking), and `amplihack signal distribute` rolls the same onboarding out
> across an entire fleet of Azure Linux VMs so a host is **ready out of the box**.
> The manual [Quick start](#quick-start) below still works and documents exactly
> what `setup` automates. See the companion how-to:
> [Signal onboarding](SIGNAL_ONBOARDING.md).

---

## Onboarding: `amplihack signal setup`

`amplihack signal setup` is a **first-class onboarding command** that turns a
bare host into a Signal-ready host with a single interactive run. It performs
every step the feature needs and is **idempotent** — re-running it repairs only
what is missing and never re-links an already-linked device or clobbers a valid
config.

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
   over a tunnel does not work for the low-latency JSON-RPC this feature
   requires.
4. **Writes the config.** It writes `~/.amplihack/signal-config.toml` (mode
   `0600`) using the **existing [`SignalConfig`](#configuration) schema**, with
   `endpoint`, `account`, and `allowlist = [account]`. Environment variables and
   an explicit `AMPLIHACK_SIGNAL_CONFIG` still override this file; onboarding
   relies on the loader's default-path fallback to
   `~/.amplihack/signal-config.toml` (see [Configuration](#configuration)).

That is the whole onboarding. The host is now Signal-ready: run
[`amplihack signal chat <topic>`](#the-operator-conversation-amplihack-signal-chat)
whenever you want to open a conversation. **No group is opened by onboarding
itself.**

### Flags

| Flag | Purpose |
|---|---|
| `--port <PORT>` | Daemon bind port on `127.0.0.1` (default `7583`, or `AMPLIHACK_SIGNAL_PORT`). If the port is held by an amplihack-managed daemon it is reused; if held by an unknown process, `setup` fails cleanly with guidance. |
| `--device-name <NAME>` | Device name registered with Signal for this host (default `amplihack-<hostname>`). |
| `--force` | Repair/overwrite even when probes report an existing setup. Use with care — this can re-link the device. |
| `--all-vms` | Also run [`amplihack signal distribute`](#fleet-distribution-amplihack-signal-distribute) after onboarding this host. |
| `--resource-group <rg>` | Resource group to use with `--all-vms`. |

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

## In-session onboarding notice

You do not have to remember to run `amplihack signal setup` ahead of time. When
you launch amplihack (Copilot CLI **or** Claude Code) on a host that is **not
yet configured for Signal**, the `SessionStart` hook offers a **fast, skippable
notice** asking whether you want to add **this host** as a signal-cli device:

```text
Signal is not configured on this host.
Add this host as a Signal device now? [y/N]
```

This notice is **purely local**: it performs **no** network I/O, creates **no**
Signal group, and posts **no** message. Its only job is to offer to launch
`amplihack signal setup`.

### When the notice appears

The notice is shown **only** when **all** of these hold:

- `SignalConfig::load()` fails (no valid `~/.amplihack/signal-config.toml` and
  no env-var config), **and**
- stdin/stdout is an interactive **TTY**, **and**
- `AMPLIHACK_NONINTERACTIVE` is **unset** (or not `1`), **and**
- no **decline sentinel** exists at
  `~/.amplihack/runtime/signal/.onboarding-declined`.

If any condition fails, the notice is **silently skipped** and the session
proceeds normally with Signal off. In particular, CI, scripts, and any run with
`AMPLIHACK_NONINTERACTIVE=1` never see the notice.

### What each answer does

| Answer | Behavior |
|---|---|
| **Yes** (`y`) | Records onboarding intent and **spawns `amplihack signal setup` detached** (see [why it is detached](#why-onboarding-is-non-blocking)), then returns immediately. The current session proceeds unchanged; the host becomes Signal-ready once `~/.amplihack/signal-config.toml` exists. |
| **No** / Enter (`N`) | Writes the `.onboarding-declined` sentinel so **you are not asked again** on this host, and continues. |
| **No response / non-TTY** | Skipped; nothing is written. |

To be asked again after declining, delete the sentinel:

```bash
rm ~/.amplihack/runtime/signal/.onboarding-declined
```

Or onboard explicitly at any time with `amplihack signal setup`.

### Why onboarding is non-blocking

Device linking is **user-paced** (you scan a QR code with your phone) and can
take far longer than a hook is allowed to run — Copilot CLI enforces a ~30s
hook timeout. The notice therefore only records your choice and **spawns the
real linking flow (`amplihack signal setup`) as a detached process**; it never
blocks the `SessionStart` hook on QR scanning. This guarantees the onboarding UX
can **never break, stall, or slow down a session**, whether you accept, decline,
or Signal is unreachable.

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
  sequentially.
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
**single source-of-truth taxonomy** so results are scriptable. Codes are
distinct — a caller can branch on *why* onboarding stopped:

| Code | Name | Meaning |
|---|---|---|
| `0` | `SUCCESS` | Fully onboarded (or nothing to do — idempotent re-run). |
| `2` | `USAGE` | Invalid arguments / flag combination (clap-level). |
| `3` | `UNSUPPORTED` | Built without the `signal` feature. A clean error, **not** a hidden no-op. |
| `4` | `PRECONDITION` | signal-cli missing/uninstallable, or an invalid/unwritable config — a setup precondition failed. |
| `5` | `PARTIAL` | Fleet run finished but **one or more VMs failed** (`distribute` only). Re-run to retry pending/failed VMs. |
| `6` | `DAEMON` | Local daemon could not start — e.g. `127.0.0.1:<port>` held by an unknown process. Never silently rebinds. |
| `7` | `LINK` | Device linking failed — e.g. approval error or Signal's **linked-device cap** reached. |

The link URI is **never** included in any structured output (see the security
model). For the `amplihack signal chat` exit-code taxonomy, see
[Signal Chat](SIGNAL_CHAT.md#exit-codes).

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

# 4. Run amplihack as usual. Session start does NOT open a Signal group.
#    When you want to observe/steer a host, open a conversation explicitly:
#        amplihack signal chat "review the failing CI"
#    That creates a fresh Signal group, adds you to it, and drives a two-way
#    agent session. There is no automatic "session started" message.
```

Reply in that Signal group (from an allow-listed number, from your primary
device) with a short instruction such as *"focus on the failing test first"*.
It becomes the agent's next turn — **not executed automatically**. See
[Signal Chat](SIGNAL_CHAT.md) for the full conversation contract.

---

## Configuration

Configuration is resolved **env-first**, then from an optional TOML file, then
**explicit error** — there are **no silent defaults**. If a required value is
missing from both sources, the loader fails and the feature stays off.

Resolution order for each setting:

```
environment variable  >  TOML at AMPLIHACK_SIGNAL_CONFIG  >  ~/.amplihack/signal-config.toml  >  error
```

The final `~/.amplihack/signal-config.toml` step is the **onboarding default
path**: `amplihack signal setup` writes the config there, and the loader
consults it when neither environment variables nor an explicit
`AMPLIHACK_SIGNAL_CONFIG` supply a setting. Absent onboarding (and without env
vars or `AMPLIHACK_SIGNAL_CONFIG`) the loader still errors and the feature stays
off; there are **no other silent defaults**.

### Settings

| Setting | Env var | TOML key | Required | Format / notes |
|---|---|---|---|---|
| Endpoint | `AMPLIHACK_SIGNAL_ENDPOINT` | `endpoint` | ✅ | `host:port` of the signal-cli JSON-RPC daemon |
| Account | `AMPLIHACK_SIGNAL_ACCOUNT` | `account` | ✅ | E.164 (`+` then digits) — the number amplihack sends **as** |
| Allowlist | `AMPLIHACK_SIGNAL_ALLOWLIST` | `allowlist` | ✅ | Operator numbers allowed to send inbound. Env = comma-separated E.164. **Empty ⇒ fail-closed (deny all inbound).** |
| Own device id | `AMPLIHACK_SIGNAL_OWN_DEVICE_ID` | `own_device_id` | optional | signal-cli's **own** linked-device id (must be `>= 2`). Only used to reject the bot's own synced-back echoes explicitly; the primary-phone (device `1`) gate is the main loop guard and needs no configuration. Leave unset unless you know your signal-cli device id |
| Config file path | `AMPLIHACK_SIGNAL_CONFIG` | — | optional | Explicit path to the TOML file below. When unset, the loader falls back to the onboarding default `~/.amplihack/signal-config.toml` |

> **Fail-closed allowlist.** An **empty** allowlist is a valid, deliberate
> configuration meaning "accept no inbound instructions." It is *not* treated
> as "allow everyone."

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
```

Any value present in the environment overrides the same key in the file.

---

## The operator conversation: `amplihack signal chat`

Opening a Signal group and having a two-way conversation is an **explicit
operator command**:

```bash
amplihack signal chat "review the failing CI job"
```

`amplihack signal chat <topic>`:

- **Creates a fresh, operator-only Signal group** named
  `amplihack-<host>[-<tmux>]-<slug(topic)>` and auto-accepts your linked device
  into it (fail-closed membership verification before every outbound post).
- **Drives a two-way agent session.** The topic becomes the first turn's prompt;
  the agent's output is posted to the group, and every operator reply you send
  becomes the next `copilot --session-id` turn with full session context.
- **Is least-privilege by default.** With no `--allow-tool` the driven agent
  gets only read-only investigation tools (`view`/`grep`/`glob`); you widen the
  blast radius explicitly per invocation.
- **Runs loopback-only** against the local signal-cli daemon unless you pass an
  explicit `--unsafe-remote-endpoint` opt-in.
- **Tears itself down** when the conversation ends: it closes the group and
  terminates the driven child in its own process. There is **no** hook-lifecycle
  teardown wiring involved.

This command is **self-contained**: it does **not** flow through the
`amplihack-hooks` pipeline, spawn a background hook subscriber, or rely on
`UserPromptSubmit`/`PostToolUse` injection. The complete command reference,
control phrases, tool-allowlist model, group-naming rules, security contract,
failure modes, and a step-by-step tutorial live in the dedicated document:

> **See [Signal Chat (`amplihack signal chat`)](SIGNAL_CHAT.md).**

---

## Security model / trust boundary

The feature is designed around one hard rule:

> **Inbound Signal text is data, never commands.**

Concretely, across onboarding and `amplihack signal chat`:

- **Never auto-executed.** Operator text is surfaced only as a turn prompt /
  context to the agent. amplihack never turns inbound text into a shell command,
  file write, or any other mutating action on its own. The agent may choose to
  act on the advice, subject to all normal amplihack safety hooks and the
  session's tool allowlist.
- **Fail-closed gate.** Inbound requires *all* of: sender on the allowlist,
  matching session group, and setup-appropriate authorization — an account
  `syncMessage` is accepted only from the **primary phone** (`sourceDevice == 1`)
  and never from signal-cli's own linked device, while a separate allowlisted
  number is accepted via a normal `dataMessage`. An **empty allowlist denies
  everything**.
- **No always-on group creation.** Session start performs **no** Signal group
  I/O at any nesting depth, so an idle, automated, or nested session can never
  create a group or leak session/host metadata to Signal. Groups exist only
  after an explicit `amplihack signal chat <topic>`.
- **Feature default OFF.** No `signal` feature ⇒ no code, no dependencies, no
  network sockets.
- **No silent config defaults.** Missing required config is an explicit error,
  never a guessed value.
- **Path safety.** Every per-session/per-host file path (config, decline
  sentinel, distribute state) is derived through `sanitize_session_id` /
  validated inputs; files are written atomically with restrictive permissions.
- **Non-fatal contract.** Every onboarding / session-start Signal operation that
  fails is logged to `warnings[]` + `tracing` and the hook still exits `0`.

For the `amplihack signal chat` outbound redaction, membership verification, and
audit-log guarantees, see [Signal Chat — Security contract](SIGNAL_CHAT.md#security-contract).

### Onboarding-specific boundaries

`signal setup` / `signal distribute` add their own hardening:

- **Loopback-only daemon.** The JSON-RPC daemon binds `127.0.0.1:<port>` only;
  non-loopback / wildcard binds are refused and the port is never forwarded.
- **Link URI is High-sensitivity.** The device-link URI (`sgnl://linkdevice?...`
  or legacy `tsdevice:/?...`, whichever signal-cli emits) is written to
  **stderr only** — never logged, persisted, or emitted in structured output.
- **Injection-safe fan-out.** VM / resource-group names are **validated and
  rejected** at the boundary (E.164 account, `1..=65535` port, charset-checked
  names) *before* shell-escaping; secrets travel via base64-over-stdin, never on
  `argv`. Validation is fail-closed, not silent stripping.
- **`0600` on disk.** `signal-config.toml` and `signal-distribute-state.json`
  are written atomically (temp-then-rename) with `0600` permissions.
- **Allowlist integrity preserved.** The writer emits **only**
  `allowlist = [account]` — never empty or wildcard — keeping the fail-closed
  gate intact.

---

## Crate API reference

`amplihack-signal` is organized as a small "brick" with a **pure core**
(`config` / wire helpers / `gating` / `chat` logic — no network or filesystem
I/O, unit-testable in isolation) plus a **gated I/O shell** (`transport`, which
requires the async `tokio` net stack). The crate is pulled into
`amplihack-hooks` and `amplihack-cli` only under `--features signal`; with the
feature off it is neither compiled nor linked.

### `config`

Env-first loader with explicit errors and no silent defaults. Used by both
onboarding and `amplihack signal chat`.

```rust
use amplihack_signal::config::SignalConfig;

// Resolves env > TOML(AMPLIHACK_SIGNAL_CONFIG) > ~/.amplihack/signal-config.toml > error.
let cfg = SignalConfig::load()?;
assert!(cfg.allowlist.iter().all(|n| n.starts_with('+')));
```

| Field | Type | Meaning |
|---|---|---|
| `endpoint` | `String` | `host:port` of the daemon |
| `account` | `String` | E.164 sending account |
| `allowlist` | `Vec<String>` | Permitted E.164 senders (empty ⇒ deny all inbound) |
| `own_device_id` | `Option<u32>` | signal-cli's own linked-device id (`>= 2`) for explicit echo rejection |

### `transport`, `gating`, and the `chat` cores

The JSON-RPC transport (`connect` / `create_group` / `send_group` /
`quit_group` / `receive`), the fail-closed `gating::Gate`, and the reusable
`chat` cores (turn driver, membership verification, outbound redaction, control
phrases) are all owned by `amplihack signal chat`. They are documented in
[Signal Chat — Reference](SIGNAL_CHAT.md#reference), including the pure wire
helpers (`build_send_request` / `parse_incoming`) and their fixture-driven unit
tests.

### `amplihack-hooks` integration (onboarding notice only)

The `amplihack-hooks` `signal_integration` module now exposes a **single**
entry point:

| Item | Role |
|---|---|
| `signal_integration::on_session_start` | **No Signal group I/O.** Never creates a group, posts a message, persists a group id, or spawns a subscriber. Its only action is to offer the purely-local [in-session onboarding notice](#in-session-onboarding-notice) on an un-onboarded interactive host. Feature-off it is a zero-cost no-op shim. |

The previous automatic-channel entry points (`drain_into_context`,
`relay_outbound`, `on_stop`, `run_subscriber`, `is_channel_configured`,
`set_process_enabled`, the `signal-subscriber` subcommand, and the
`SIGNAL_ENABLED` process gate) have been **removed** along with the always-on
channel.

---

## Building and testing

The feature must build and test cleanly **both** ways — this is a hard quality
gate:

```bash
# Default build: feature OFF (Signal fully compiled out).
cargo build
cargo test

# Feature ON — build and test the crates that carry Signal code.
cargo build --release -p amplihack-hooks-bin --features signal
cargo test  --release -p amplihack-hooks     --features signal
cargo test  --release -p amplihack-signal    --features signal
```

`cargo clippy -p amplihack-hooks --features signal -- -D warnings` must pass
with **zero** warnings (in particular no `dead_code`) in **both** the feature-on
and feature-off configurations.

**All tests are hermetic and offline.** No test creates a real Signal group or
touches the real Signal network — see
[Offline testing](#offline-testing-fake-json-rpc-endpoint). Run `cargo test` for
the current counts rather than relying on a number pinned in this doc.

---

## Offline testing (fake JSON-RPC endpoint)

Signal tests **never hit the real Signal network**. For tests that must exercise
the transport and the `amplihack signal chat` loop end-to-end, `amplihack-signal`
ships a **fake signal-cli JSON-RPC endpoint** modeled on signal-cli's own
**JSON-RPC daemon (`--tcp`) socket mode**. It is a `tokio::net::TcpListener`
bound to **`127.0.0.1:0`** (an ephemeral loopback port) that speaks
**newline-delimited JSON-RPC 2.0**, the same protocol the real daemon uses.
`SignalTransport::connect` is pointed at the fake's address, so the full method
surface can be driven deterministically:

| RPC method | Exercised behavior |
|---|---|
| `updateGroup` (create) | Returns a synthetic `groupId`; asserts create-group requests |
| `send` | Records outbound bodies for assertions (redaction, chunking) |
| `receive` | Streams scripted inbound envelopes into the chat loop |
| `quitGroup` | Records teardown; asserts the chat closes the group |

Because the endpoint binds **loopback-only** and returns synthetic ids, tests
can cover **send / create-group / receive / quit-group** and the **inbound
gating loop** with **zero** external dependencies and **no** real Signal
account, group, or network traffic.

```rust
// Illustrative test wiring (offline, deterministic, no real Signal).
let fake = FakeSignalEndpoint::start().await;   // binds 127.0.0.1:0
let transport = SignalTransport::connect(fake.addr()).await?;

let group = transport.create_group("amplihack-test").await?;   // synthetic id
transport.send_group(&group, "hello").await?;
fake.push_inbound(/* scripted operator envelope */);           // drive receive()
// ... assert gating accepted/rejected the message ...
transport.quit_group(&group).await?;

assert_eq!(fake.groups_created(), 1);   // never a REAL group
```

> **CI guarantee.** The loopback-only `FakeSignalEndpoint` means no test path can
> reach the real Signal service. CI runs the full `--features signal` suite with
> no signal-cli installed and no network access.

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| No group appears when a session starts | **By design** — session start never opens a group | Run `amplihack signal chat <topic>` to open a conversation |
| `amplihack signal chat` says Signal is not configured | Host not onboarded | Run `amplihack signal setup` (or answer **Yes** to the in-session notice) |
| `amplihack signal chat` can't reach the daemon | Feature not built / daemon down | Build with `--features signal`; confirm `signal-cli ... daemon --tcp`; see [Signal Chat — failure modes](SIGNAL_CHAT.md#failure-modes) |
| Your replies are ignored in a chat | Not allow-listed, or sent from a linked (non-primary) device | Add your number to `AMPLIHACK_SIGNAL_ALLOWLIST`; reply from your **primary** device (device 1) |
| Nothing ever accepted | Allowlist is empty (fail-closed) | Populate the allowlist |
| `signal setup` can't install signal-cli | No package/JRE available non-interactively | Follow the printed install guidance, install signal-cli manually, then re-run `amplihack signal setup` |
| `signal setup` fails on port | `127.0.0.1:<port>` held by an unknown process | Free the port or pass `--port <other>` / set `AMPLIHACK_SIGNAL_PORT` |
| A VM shows `failed: link limit reached` | Signal linked-device cap hit | Unlink an unused device in Signal, or use `--identity-mode dedicated-number` for very large fleets |
| `distribute` stopped part-way | Interrupted / a VM failed | Re-run `amplihack signal distribute`; it resumes from `~/.amplihack/signal-distribute-state.json` and retries only pending/failed VMs |
| In-session onboarding notice never appears | Non-TTY, `AMPLIHACK_NONINTERACTIVE=1`, already configured, or previously declined | Expected. Delete `~/.amplihack/runtime/signal/.onboarding-declined` to be asked again, or run `amplihack signal setup` |

Because every onboarding / session-start Signal operation is non-fatal, none of
the above can break your amplihack session — worst case the feature is
unavailable, with the failure surfaced through hook warnings and/or `tracing`,
and the run proceeds normally.

---

## FAQ

**Does starting a session open a Signal group?**
No. Session start performs no Signal group I/O at any nesting depth. Its only
Signal-related action is the one-time, purely-local
[onboarding notice](#in-session-onboarding-notice) on an un-onboarded
interactive host. To open a conversation, run `amplihack signal chat <topic>`.

**Why doesn't starting a session open a Signal group anymore?**
Because it flooded operators. A previous always-on channel created (or reused) a
group and posted "session started" on every `SessionStart`. With a
`signal-config.toml` present on a host, every session — including nested
recipe/orchestrator/sub-agent sessions — spawned a brand-new empty group,
producing 1000+ unattributable groups. That automatic channel, its background
subscriber, and its per-turn conversation mirroring have been **removed**. Group
creation and conversation are now **explicit and opt-in per topic** via
`amplihack signal chat <topic>`.

**Does the Signal integration work with GitHub Copilot CLI, or only Claude
Code?**
Both. The in-session onboarding notice fires on either host. `amplihack signal
chat` drives a `copilot` session from the group.

**Does enabling Signal add dependencies to the default build?**
No. With the feature off, `amplihack-signal` and its `tokio`-net dependencies
are not compiled or linked.

**Can an operator make amplihack run a command by texting it?**
No. Operator text is delivered only as an agent turn prompt / context. The agent
decides whether to act, subject to all normal safety hooks and the session's
tool allowlist. See [Signal Chat — Security contract](SIGNAL_CHAT.md#security-contract).

**Do the tests hit the real Signal network?**
Never. The loopback-only `FakeSignalEndpoint` guarantees no real groups or
network traffic. See [Offline testing](#offline-testing-fake-json-rpc-endpoint).

---

## See also

- [Signal Chat (`amplihack signal chat`)](SIGNAL_CHAT.md) — the operator
  conversation: command reference, control phrases, security contract, tutorial
- [Signal onboarding how-to](SIGNAL_ONBOARDING.md) — `setup` and `distribute` walkthrough
- [Signal external service integration](signal-external-integration.md) — seam architecture, resumable fleet state, and `amplihack-remote` re-exports
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config
- [Hook configuration guide](HOOK_CONFIGURATION_GUIDE.md)
- [Security recommendations](SECURITY_RECOMMENDATIONS.md)
