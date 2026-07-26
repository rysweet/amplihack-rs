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

## Group naming and lifecycle

### Only the top-level operator session gets a group

The Signal channel is **operator-facing**: it exists so a human can watch and
advise the single session they launched. A real amplihack run, however, spawns
**many** nested sessions — the orchestrator, each recipe step, and every
sub-agent all start their own session with their own `session_id`. If every one
of those opened a Signal group, the operator's phone would fill with dozens of
**empty** groups (each containing only a lone `session started` message) and it
would be impossible to tell which group belonged to the run they care about.

To prevent this, SessionStart integration applies a **nesting gate**: a Signal
group is created **only for the top-level operator session**. Nested sessions
are a **silent no-op** — they create no group, post no `session started`
message, persist no group state, and spawn no subscriber. This is normal,
expected behavior, not a warning or an error.

Nesting is detected from the `AMPLIHACK_SESSION_DEPTH` environment variable,
which amplihack increments for every child session it spawns:

| `AMPLIHACK_SESSION_DEPTH` | Session kind | Signal group created? |
|---|---|---|
| unset or `0` | Top-level operator session | ✅ Yes |
| `1`, `2`, … (any value > 0) | Nested recipe / orchestrator / sub-agent | ❌ No (silent no-op) |

A non-numeric or malformed value is treated as depth `0` (fail toward creating
the group for the visible operator session).

> **Why this matters.** Before the nesting gate, a single run could create tens
> of empty groups. With the gate, one run produces exactly **one** group — the
> operator's — and all meaningful output flows there.

### Group name

**Per-session (default).** On the top-level SessionStart a fresh group is
created. When the session is running under **tmux** the group name embeds the
tmux session name so the operator can immediately tell which group maps to which
terminal/session:

```
amplihack-<tmux-session-name>-<session-id>-<unix-timestamp>
```

When **not** running under tmux (or the tmux lookup fails or times out), the
name gracefully falls back to the previous format:

```
amplihack-<session-id>-<unix-timestamp>
```

Both the tmux session name and the session id are **sanitized** to the
allowlist `[A-Za-z0-9_-]` (the same allowlist used by `sanitize_session_id`);
the tmux portion is truncated to **32 characters**
to keep names bounded. If the tmux name is empty after sanitization it is
omitted (fallback form is used).

The tmux name is discovered by running `tmux display-message -p
'#{session_name}'` **only when the `TMUX` environment variable is set**, with a
**~2-second timeout** and a graceful fallback: any failure, timeout, or
absence of tmux simply omits the tmux part. The subprocess is invoked with an
explicit argument vector (no shell), so the tmux name — which is untrusted
input — can never trigger shell or argument injection, and it is used **only**
in the display group name (never in any filesystem path).

The `groupId` returned by signal-cli is persisted in session state. On Stop the
group is closed with `quitGroup`.

> **Nested / no-group sessions on Stop.** Because nested sessions never create a
> group or persist a `group_id`, their Stop hook is likewise a clean no-op: no
> summary is posted and no `quitGroup` is attempted when there is no persisted
> `group_id` for the session.

**Rolling group (opt-in).** Per-session groups are the default; nothing needs
to be set to get them. To instead reuse a **single** long-lived group across all
sessions, explicitly opt in by setting `reuse_rolling_group = true` (or
`AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP=1`) **and** supplying `rolling_group_id`.
In this mode the group is **not** quit at Stop, so you keep one persistent
operator thread. Because this trades per-session isolation for a shared thread,
it must be requested deliberately — any absent, empty, or explicit false reuse
flag keeps the per-session default, unknown tokens are rejected, and a truthy
reuse flag without a group id is rejected.

| Phase | Per-session | Rolling |
|---|---|---|
| SessionStart (top-level) | create group + post "session started" | reuse group + post "session started" |
| SessionStart (nested) | **silent no-op** (no group, no post) | **silent no-op** (no group, no post) |
| During run | post at meaningful transitions | post at meaningful transitions |
| Stop (with group) | post summary → `quitGroup` | post summary (group kept) |
| Stop (no group) | **silent no-op** | **silent no-op** |

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
- **Per-session group isolation by default.** `reuse_rolling_group` defaults to
  `false`, so each session gets its own group that is closed with `quitGroup`
  at Stop — no operator thread outlives the session that created it. Sharing one
  long-lived group across sessions is a deliberate opt-in
  (`reuse_rolling_group = true` / `AMPLIHACK_SIGNAL_REUSE_ROLLING_GROUP=1`) plus
  a `rolling_group_id`; only non-empty truthy values enable it, and a missing
  group id is rejected instead of creating an untracked shared thread.
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
| `reuse_rolling_group` | `bool` | Opt-in: reuse one shared rolling group. Defaults to `false` (per-session groups) |
| `rolling_group_id` | `Option<String>` | Existing group id to bind rolling reuse to (required when `reuse_rolling_group` is `true`) |

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
| Many **empty** groups piling up (each with only "session started") | Older behavior created a group for every nested session | Fixed: only the **top-level** session (`AMPLIHACK_SESSION_DEPTH` unset/`0`) creates a group; nested sessions are a silent no-op. Delete the stale empty groups; new runs produce exactly one group |
| Can't tell which group belongs to which session | Group name lacked session context | Group names now embed the tmux session name when running under tmux: `amplihack-<tmux-session>-<session-id>-<ts>` |

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

**Why don't nested sessions (recipes, orchestrator, sub-agents) get their own
Signal group?**
By design. A single run spawns many nested sessions; if each opened a group the
operator would be buried in empty groups with no way to tell them apart. Only
the top-level operator session (`AMPLIHACK_SESSION_DEPTH` unset or `0`) creates
a group and posts output. Every nested session (`AMPLIHACK_SESSION_DEPTH > 0`)
is a silent no-op — no group, no message, no state, no subscriber. All
meaningful output still flows to the single operator group.

**Why is the tmux session name in the group name? What if I'm not using tmux?**
It makes each group instantly identifiable — you can match a Signal group to the
terminal/session it came from. The lookup runs `tmux display-message -p
'#{session_name}'` only when `TMUX` is set, with a ~2-second timeout. If you are
not under tmux (or the lookup fails/times out) the name gracefully falls back to
`amplihack-<session-id>-<ts>`. The tmux name is sanitized to `[A-Za-z0-9_-]`,
truncated to 32 chars, and never used in any filesystem path.

---

## See also

- [Signal onboarding how-to](SIGNAL_ONBOARDING.md) — `setup` and `distribute` walkthrough
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config
- [Signal onboarding — performance notes](reference/signal-onboarding-performance.md) — hot paths & allocation trims
- [Hook configuration guide](HOOK_CONFIGURATION_GUIDE.md)
- [Security recommendations](SECURITY_RECOMMENDATIONS.md)
