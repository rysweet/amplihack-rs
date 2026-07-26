# Signal Setup Skill

Link a host — local machine or a remote **azlin** Azure VM — to **Signal** as a
linked device, END-TO-END, and wire it into amplihack's Signal channel. The
whole manual device-linking loop becomes one idempotent command.

See [`SKILL.md`](./SKILL.md) for the full flow and the critical facts, and
[`USAGE.md`](./USAGE.md) for the full CLI reference, configuration, tutorials,
and troubleshooting. The executable lives at
[`scripts/signal-setup.sh`](./scripts/signal-setup.sh).

## Quick start

```bash
# Link the local host
scripts/signal-setup.sh --host "$(hostname -s)"

# Link a remote azlin VM (auto-detected; links via `az vm run-command`)
scripts/signal-setup.sh --host deva2

# Skip the interactive prompt (scan screen already open)
scripts/signal-setup.sh --host dev -y

# Link only, skip the daemon + self-group + post-test phase
scripts/signal-setup.sh --host dev --no-daemon
```

## What it does

1. **Prereqs** — `signal-cli` (0.14.5, `~/.local/bin/signal-cli`), `qrencode`,
   and `az` (remote hosts only).
2. **Prompt** to open Signal → Linked Devices → Link New Device.
3. **Mint** the link under `systemd-run` (local, or remote via
   `az vm run-command` + `systemd-run --uid=azureuser`).
4. **Render** the QR in-terminal with `qrencode -t ANSIUTF8i` (zero latency,
   dark-terminal-safe).
5. **Verify** linkage via `signal-cli listAccounts`.
6. **Channel** (optional) — start the JSON-RPC daemon on `127.0.0.1:7583`,
   ensure a self-only group, and post-test.

## The three facts that make it work

- **60-second window**: Signal's provisioning socket closes (code 1001) 60s
  after the QR is minted. Be on the scan screen first; scan immediately.
- **`ANSIUTF8i`** (inverted): required so the QR is visible/scannable on dark
  terminals. Never deliver the QR through Signal itself.
- **`systemd-run`**: keeps the link process alive the full window; remote runs
  need `--uid=azureuser` because `az vm run-command` runs as root.

Idempotent: re-running an already-linked host skips linking and just (re)ensures
the daemon/group.
