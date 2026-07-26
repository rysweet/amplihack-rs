---
name: signal-setup
version: 1.0.0
description: |
  End-to-end, idempotent Signal device-linking for amplihack on a local or
  remote (azlin VM) host. Runs the full loop in one invocation: prerequisite
  check, prompt to open the phone's scan screen, mint a fresh device-link under
  systemd-run, render the QR DIRECTLY in the terminal (zero delivery latency,
  ANSIUTF8i inverted for dark terminals), verify linkage, and optionally start
  the JSON-RPC daemon, ensure the amplihack self-group, and post a test message.
  Use when onboarding a host to the amplihack Signal channel, when a prior link
  attempt showed "invalid response from server", or when you need to re-link /
  verify a fleet host's Signal account.
auto_activates:
  - "Set up Signal for a host"
  - "Link Signal device"
  - "Signal device linking"
  - "Link a fleet host to Signal"
  - "amplihack signal setup"
  - "Signal QR invalid response from server"
priority_score: 36.0
---

# Signal Setup Skill

Link a host (local or remote azlin VM) to Signal for the amplihack Signal
channel — reliably, in one command, without falling into the 60-second trap
that makes slow QR delivery fail.

## When to invoke

- Onboarding a new local or fleet host to the amplihack Signal channel.
- A device-link attempt failed with **"invalid response from server"** on the
  phone (almost always the expired-QR / 60s-window failure — see below).
- You need to verify or re-establish a host's Signal linkage and confirm the
  JSON-RPC daemon + self-group post-test works.

## How to invoke

```bash
# Local host, interactive (prompts you to confirm the scan screen is open):
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh \
  --host local --phone +15551234567

# Remote azlin VM (mint runs on the VM via az run-command; QR renders LOCALLY):
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh \
  --host deva --phone +15551234567 --resource-group rysweet-linux-vm-pool

# Skip the daemon/self-group/post-test step:
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh --host local --no-daemon

# Non-interactive (you have ALREADY opened the scan screen):
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh --host local -y --phone +1555...
```

Run `signal-setup.sh --help` for the full option list.

The skill is **idempotent**: if the host is already linked (detected via
`signal-cli listAccounts`), it does **not** re-mint — it skips straight to the
daemon/self-group/post-test verification and exits successfully.

Environment overrides:

- `SIGNAL_PHONE` — default `--phone`.
- `SIGNAL_SETUP_RG` — default Azure resource group (`rysweet-linux-vm-pool`).
- `SIGNAL_SETUP_AZ_TIMEOUT_SECONDS` — `az vm run-command` timeout (default 90).
- `SIGNAL_SETUP_LOCAL_TIMEOUT_SECONDS` — local `signal-cli` probe timeout
  (default 10).
- `SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS` — half-second daemon/URI poll attempts
  (default 20).
- `SIGNAL_SETUP_RPC_TIMEOUT_SECONDS` — JSON-RPC `nc` timeout (default 15).
- `SIGNAL_SETUP_DAEMON_TCP` — loopback daemon endpoint (default
  `127.0.0.1:7583`; host must stay `127.0.0.1`/`localhost`).

## End-to-end flow

1. **Prereq check** — verifies `signal-cli` (known-good **0.14.5** at
   `~/.local/bin/signal-cli` → `~/.local/opt/signal-cli-0.14.5/bin/signal-cli`),
   `qrencode`, and `systemd-run` on the target host; plus the `az` CLI and a
   local `qrencode` for remote hosts.
2. **Idempotency probe** — `signal-cli listAccounts` showing `Number: +<phone>`
   means already linked → skip minting.
3. **Prompt** — you confirm you are on Signal ▸ Settings ▸ Linked Devices ▸
   **Link New Device** ▸ camera/scan screen **BEFORE** the QR is minted.
4. **Mint** — `signal-cli link -n amplihack-<host>` under a transient
   `systemd-run` unit (`sig-link-<host>`) so it survives the launching shell.
5. **Render QR in-terminal** — `qrencode -t ANSIUTF8i` immediately. Scan it now.
6. **Verify linkage** — polls `listAccounts` for `Number: +<phone>`; the
   transient unit goes `inactive` on success.
7. **Daemon + self-group + post-test** (optional, on by default) — starts the
   JSON-RPC daemon on `127.0.0.1:7583`, ensures a self-only group via
   `updateGroup{name}`, and confirms `send{groupId,message}` returns
   `{"results":[],"timestamp":...}` (empty `results` is normal success for a
   self-group).

---

## ⚠️ Critical Signal-linking invariants

### 1. The 60-second window (the real root cause)

Signal's device-link **provisioning websocket** to `chat.signal.org` closes
with **server code 1001 EXACTLY ~60 seconds** after it opens — verified in a
`signal-cli -vv` trace: `onOpen` → `onClosing(1001)` at **+60s**. A minted link
QR is therefore valid for only **~60 seconds** from mint. Any delivery path
slower than a few seconds expires the QR and the phone shows
**"invalid response from server"**.

Routing the QR through a remote Signal daemon (azlin/bastion round-trip =
40–60s) leaves no useful margin and causes expiry. The fix is
**zero-latency, in-terminal delivery** so the phone scans a fresh QR within a
couple of seconds.

### 2. ANSIUTF8i — inverted, for dark terminals

Render with **`qrencode -t ANSIUTF8i`**. The trailing **`i` = inverted**, which
is **required** so the QR is visible/scannable on **dark** terminal
backgrounds. Plain `ANSIUTF8` is dark-on-dark and effectively **invisible** on
dark terminals.

### 3. systemd-run persistence

The `signal-cli link` process runs under a **transient systemd unit**
(`sig-link-<host>`) via `systemd-run`, so it **survives the launching shell**
and stays connected for the full window; it exits cleanly on a successful scan.

For **remote** hosts the same unit is launched via
`az vm run-command invoke ... RunShellScript` calling:

```bash
systemd-run --unit=sig-link-<host> --uid=azureuser --gid=azureuser \
  --setenv=HOME=/home/azureuser ...
```

`run-command` runs as **root**, so `--uid=azureuser --gid=azureuser` is
**required** so the linked account lands under the `azureuser` home, not root's.
The script captures the `sgnl://` URI from the link stdout on the VM, returns it,
and **renders the QR LOCALLY** — the URI (a few hundred bytes of text) travels
fast; the QR image never leaves your terminal.

### 4. NEVER route the QR through Signal

Do **NOT** deliver the QR as a Signal message/attachment during linking. That is
the **DEPRECATED slow path** (relay / daemon delivery) that reliably blew the
60s window. The QR goes to the **terminal**, nowhere else. You must be on the
phone's scan screen **before** the QR prints.

---

## Verifying linkage manually

- `signal-cli listAccounts` shows `Number: +<phone>`.
- The `sig-link-<host>` systemd unit becomes `inactive` on success.
- Trace log `/tmp/scli-<host>-<run-token>.log` (the exact path is printed on a
  verification failure) shows `Associated with: +<phone>` then
  `Finishing new device registration`.

## Daemon + self-group + post-test details

- Daemon: `signal-cli -a +<phone> daemon --tcp 127.0.0.1:7583` (loopback, the
  established amplihack convention — matches `crates/amplihack-signal` and the
  `amplihack signal setup` command).
- Self-group: JSON-RPC `updateGroup{account,name}` → returns a `groupId`.
- Post-test: JSON-RPC `send{account,groupId,message}` → `{"results":[],...}`.
  An **empty `results` array is expected success** for a self-only group.

This aligns with the existing Rust integration: see
`crates/amplihack-cli/src/commands/signal/` (`setup.rs` idempotency probes,
`render.rs`, daemon on `127.0.0.1:7583`) and `docs/SIGNAL_ONBOARDING.md`. This
skill provides the **zero-latency in-terminal linking loop** that those tools
assume you have already completed.

---

## ⚠️ Operational gotchas (remote / azlin)

- **azlin SIGPIPE core-dump** — `azlin` is a Rust binary that **aborts
  (SIGABRT / core-dump) on a broken pipe (SIGPIPE)**. **Never** pipe `azlin`
  (or `az` output you treat like it) into `grep -q` or any early-closing
  consumer. **Capture full output first**, then filter. The script follows this
  rule (`remote_run` captures the whole `az` message before `sed`/`grep`).
- **One bastion session per host** — only **ONE** bastion/azlin session to a
  given VM at a time. Concurrent sessions to the same VM core-dump. Do not run
  this skill against the same host from two places at once.
- **azlin invocation form**:
  ```bash
  azlin connect <host> --resource-group rysweet-linux-vm-pool --no-tmux -y -- "<cmd>"
  ```

## Security model

The script mints Signal device-link secrets and (for remote hosts) runs
payloads on Azure VMs, so its threat surface is **secret handling, privilege
use, and injection** — not web auth. Hardening built into the script:

- **Input validation (fail closed).** `--host`, `--resource-group`, `--group`,
  and `--phone` are validated against strict allowlists right after arg parsing.
  These values flow into shell command lines, `az vm run-command` payloads
  (executed as root remotely), and JSON-RPC strings, so malformed input like
  `--host 'x;reboot'` is rejected before use.
- **Daemon endpoint override (`SIGNAL_SETUP_DAEMON_TCP`).** The local JSON-RPC
  daemon endpoint defaults to `127.0.0.1:7583` and may be overridden with the
  `SIGNAL_SETUP_DAEMON_TCP` env var (e.g. to change the loopback port). It is
  **loopback-allowlist validated (fail-closed)**: only `127.0.0.1`/`localhost`
  with a numeric `1-65535` port is accepted; routable hosts (`0.0.0.0`, LAN IPs)
  and IPv6/multi-colon forms are rejected before any daemon is spawned. This
  enforces the SECURITY.md §6 loopback-only invariant in code, not just by
  convention.
- **JSON-RPC escaping.** `--phone`/`--group`/`groupId` are `json_escape`d before
  being embedded in daemon requests, preventing JSON/argument injection.
- **Secret temp files.** The minted `sgnl://` URI (a ~60s provisioning secret)
  and the `-vv` trace log are written with `umask 077` to **unguessable,
  per-run** paths at `0600`, cleaned up via an `EXIT`/`INT`/`TERM` trap, and the
  URI copy is **deleted immediately after the QR renders** rather than left for
  the whole window. This defeats predictable-`/tmp` symlink and disclosure
  attacks by other local users.
- **Trust anchors.** For remote hosts the operator's **`az` login identity** and
  **passwordless `sudo`** are the trust anchor; the linked account is dropped to
  `--uid/--gid=azureuser`. The local JSON-RPC daemon is **unauthenticated but
  bound to `127.0.0.1` only** — never expose it on `0.0.0.0`.
- **PII.** The phone number lives only in argv/env; note argv is visible via
  `ps` to other local users.

See **[`SECURITY.md`](./SECURITY.md)** for the full threat model, per-input
allowlist tables, secret temp-file regime, JSON-RPC escaping rules, trust
anchors, and the security invariants that must not regress.

---

## Prerequisites (install if missing)

- **signal-cli 0.14.5** (known-good) at `~/.local/bin/signal-cli`
  (symlink → `~/.local/opt/signal-cli-0.14.5/bin/signal-cli`).
- **qrencode** — `apt-get install -y qrencode` (on the machine that renders the
  QR: local host, or your local machine for remote linking).
- **systemd-run / systemctl** — present on the target host.
- **az CLI** (remote hosts only) with `az vm run-command` permission for the
  target resource group.

## See also

- [`USAGE.md`](./USAGE.md) — full CLI reference, configuration, tutorials, and
  troubleshooting for this skill
- `docs/SIGNAL_ONBOARDING.md`, `docs/signal-channel.md`
- [`SECURITY.md`](./SECURITY.md) — security hardening reference for this skill
- `crates/amplihack-signal/`, `crates/amplihack-cli/src/commands/signal/`
