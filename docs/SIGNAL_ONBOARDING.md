# Signal Onboarding (setup & fleet distribution)

A task-oriented how-to for getting the amplihack [Signal channel](signal-channel.md)
working **out of the box** — on one host with `amplihack signal setup`, or across
an entire Azure Linux (azlin) fleet with `amplihack signal distribute`.

> **Why onboarding exists.** The per-session Signal channel already ships in
> amplihack (`amplihack-signal`, wired via `amplihack-hooks` SessionStart). But
> it needs a **local**, low-latency signal-cli JSON-RPC daemon and a valid
> config on each host. Doing that by hand — install signal-cli, link a device,
> start a daemon, hand-write TOML — is the friction these commands remove.
> Resolves issues **#921–#924**.

- [Before you start](#before-you-start)
- [Onboard one host: `signal setup`](#onboard-one-host-signal-setup)
- [Onboard a fleet: `signal distribute`](#onboard-a-fleet-signal-distribute)
- [Key constraints](#key-constraints)
- [Verifying it worked](#verifying-it-worked)
- [Reference](#reference)

---

## Before you start

You need:

- **amplihack built with the `signal` feature.** The `signal` subcommand is
  always registered, but with the feature compiled out it exits with a clean
  "rebuild with `--features signal`" error (never a silent no-op).

  ```bash
  cargo build --release --features signal
  ```

- **Your phone with Signal installed** and access to
  **Settings → Linked devices → Link new device** (you scan a QR code per host).
- For fleet distribution: the **azlin CLI** configured for your Azure
  subscription and an operator resource group (or an explicit VM list).

You do **not** need to pre-install signal-cli — `setup` installs it, or prints
actionable guidance if it cannot.

---

## Onboard one host: `signal setup`

Run:

```bash
amplihack signal setup
```

Walkthrough:

1. **signal-cli check/install.** If signal-cli is missing, `setup` installs it
   where it can, or prints exact install instructions and exits non-zero.
2. **Device linking.** A **QR code is drawn in your terminal**, with the raw
   device-link URI printed underneath as a fallback (`sgnl://linkdevice?...` on
   recent signal-cli, or the legacy `tsdevice:/?...` on older builds — `setup`
   encodes whichever signal-cli emits). On your phone: **Signal → Settings →
   Linked devices → Link new device → scan**. `setup` waits using
   **idle/liveness detection** — there is **no wall-clock timeout**, so take as
   long as you need.
3. **Local daemon.** Once linked, `setup` starts signal-cli in JSON-RPC daemon
   mode on **`127.0.0.1:7583`** (loopback only) as a managed background service
   — a **systemd `--user` unit** if available, otherwise a detached `nohup`
   process.
4. **Config.** It writes `~/.amplihack/signal-config.toml` (`0600`) with:

   ```toml
   endpoint  = "127.0.0.1:7583"
   account   = "+15551230000"        # your linked Signal number
   allowlist = ["+15551230000"]      # the account's OWN number — see below
   ```

That's it. The next amplihack session on this host automatically opens its own
Signal group — no environment variables required, because the channel's config
loader falls back to `~/.amplihack/signal-config.toml` (the default path the
onboarding feature adds; see
[Per-session wiring](signal-channel.md#per-session-wiring)).

### Re-running is safe

`setup` is **idempotent**. It probes three things — **linked**,
**daemon-running**, **config-written** — and repairs only what's missing. It
never re-links an already-linked device and never clobbers a valid config
(unless you pass `--force`).

Common flags:

```bash
amplihack signal setup --port 7600     # custom loopback port
amplihack signal setup --json          # machine-readable status (URI never printed)
amplihack signal setup --force         # repair/overwrite existing setup
```

---

## Onboard a fleet: `signal distribute`

Roll the same onboarding out to every VM so each host runs its **own local
daemon**:

```bash
amplihack signal distribute --resource-group <rg>
# target specific hosts instead of auto-discovery:
amplihack signal distribute --vms vm-a,vm-b --resource-group <rg>
# alias:
amplihack signal setup --all-vms --resource-group <rg>
```

What happens:

- **VMs are discovered generically** via azlin (`azlin list` / `az vm list`) or
  your explicit `--vms` list. No host is hardcoded.
- Each VM is reached with
  `azlin connect <vm> --resource-group <rg> --no-tmux -y -- '<cmd>'`.
- **You scan one QR code per VM.** Each VM becomes its **own linked device on
  your single Signal number** (one chat identity across the fleet). Linking is
  necessarily **sequential** (one phone); the non-interactive phases run with
  **bounded concurrency** (`--concurrency`, default 4).
- The rollout is **resumable** via
  `~/.amplihack/signal-distribute-state.json`. Re-run to retry only
  `pending`/`failed` VMs.
- **One VM failing never aborts the run.** Failures are recorded with a reason
  and shown in the final per-VM summary — nothing is silently degraded.

Per-VM statuses: `pending → linking → linked → daemon-running →
config-written`, or `failed` (with a reason). Example:

```text
3 succeeded, 2 failed. Re-run `amplihack signal distribute` to retry the failed VMs.
```

### Very large fleets

Signal enforces a **linked-device count limit** per account. If your fleet
exceeds it, use the config-selectable extension point:

```bash
amplihack signal distribute --identity-mode dedicated-number --resource-group <rg>
```

`dedicated-number` gives each VM its **own** Signal number. It is a documented
extension point; today it returns an explicit "not yet implemented" error rather
than any partial behavior. The default `linked-device` mode is fully supported.

---

## Key constraints

- **The daemon must be local.** Each session host needs its own signal-cli
  daemon on `127.0.0.1`. A shared remote daemon reached over a bastion/tunnel
  does **not** work — JSON-RPC responses never return and calls time out.
  `distribute` therefore installs a daemon **on every VM**, never a shared one.
- **Single-number setups must allowlist their own number.** When signal-cli is a
  **linked device on your own number**, your phone replies arrive as the
  account's own synced messages — so the **`account` number itself is on the
  allowlist** (`allowlist = [account]`). This is exactly what `setup` writes.
- **Linked-device limit.** Signal caps linked devices per account. Fleets larger
  than that cap should plan for `--identity-mode dedicated-number`.
- **No wall-clock caps on the interactive step.** Device linking waits on idle
  detection, so slow phone approvals never fail spuriously.
- **Fail-closed, no silent fallback.** Missing signal-cli, port conflicts, and
  link failures are always surfaced as explicit errors — never worked around
  silently. Each maps to a distinct
  [exit code](signal-channel.md#exit-codes) (`3` unsupported, `4` precondition,
  `5` partial fleet, `6` daemon, `7` link) so scripts can branch on the cause.

---

## Verifying it worked

```bash
# 1. Config is present and valid.
cat ~/.amplihack/signal-config.toml

# 2. The local daemon answers on loopback.
ss -ltnp | grep 127.0.0.1:7583        # or your --port

# 3. Start any amplihack session; you should receive a "session started"
#    message in a fresh Signal group named amplihack-<session-id>-<timestamp>.
```

If nothing arrives, check the session's `warnings[]` — every Signal operation is
non-fatal, so problems are reported there rather than crashing the session. See
[Troubleshooting](signal-channel.md#troubleshooting).

---

## Reference

- [Signal Channel](signal-channel.md) — full channel documentation, config
  schema, security model, and per-session wiring.
- [`examples/signal-config.toml`](../examples/signal-config.toml) — annotated config.
- Related issues: **#921** (onboarding command), **#922** (device linking),
  **#923** (fleet distribution), **#924** (idempotent local daemon + config).
