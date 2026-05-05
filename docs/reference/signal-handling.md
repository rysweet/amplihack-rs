# Signal Handling and Exit Codes — Reference

How `amplihack` handles SIGINT, SIGTERM, and SIGHUP when a child process
(claude, copilot, codex, amplifier) is running, and what exit code is returned
in each case.

## Contents

- [Signal handler registration](#signal-handler-registration)
- [Exit code contract](#exit-code-contract)
  - [SIGINT — Ctrl-C](#sigint-ctrl-c)
  - [Normal child exit](#normal-child-exit)
  - [Signal-killed child (no exit code)](#signal-killed-child-no-exit-code)
- [Python launcher parity](#python-launcher-parity)
- [Parity test coverage](#parity-test-coverage)
- [Related](#related)

---

## Signal handler registration

When `amplihack` starts a tool subprocess it immediately registers handlers for
three signals on the parent process:

| Signal  | Number | Trigger                          |
|---------|--------|----------------------------------|
| SIGINT  | 2      | Ctrl-C in an interactive terminal |
| SIGTERM | 15     | `kill <pid>` or system shutdown  |
| SIGHUP  | 1      | Terminal closed / session end    |

All three handlers set a shared `AtomicBool` flag (`shutdown`). The launcher
main loop polls this flag every 50 ms. When the flag becomes `true`, the loop
exits with code 0 and `ManagedChild::drop` handles graceful child shutdown.

```rust
// crates/amplihack-cli/src/signals.rs
pub fn register_handlers() -> Result<Arc<AtomicBool>> {
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register(sig, Arc::clone(&shutdown))?;
    }
    Ok(shutdown)
}
```

---

## Exit code contract

### SIGINT — Ctrl-C

When the user presses Ctrl-C (or any other event delivers SIGINT to
`amplihack`), `amplihack` exits **0**.

```sh
# User presses Ctrl-C mid-session
amplihack claude
^C
echo $?
# 0
```

This mirrors the Python launcher's `signal_handler`, which catches SIGINT and
calls `sys.exit(0)` unconditionally.

### Normal child exit

When the child process exits on its own, `amplihack` propagates the child's
exit code:

| Child exit code | `amplihack` exit code |
|-----------------|----------------------|
| 0               | 0                    |
| 1               | 1                    |
| N (any integer) | N                    |

```sh
amplihack claude --print "exit successfully"
echo $?
# 0

amplihack claude --print "trigger an error"
echo $?
# 1  (if claude exits 1)
```

### Signal-killed child (no exit code)

If the child is killed by a signal and the OS reports no integer exit code
(POSIX: `WIFEXITED` is false), `amplihack` exits **0**.

```
Child killed by signal → status.code() returns None → unwrap_or(0) → exit 0
```

> **Note:** The `unwrap_or(0)` behavior described here is introduced by
> PR `fix/sigint-exit-code-parity`. It is not present in versions prior to
> that fix (which used `unwrap_or(1)` and would exit 1 in this case).

This case arises when the stub claude binary in parity tests runs `kill -INT $$`
— the child exits via signal delivery rather than a `sys.exit()` call, so no
numeric exit code is available.

---

## Python launcher parity

The Rust and Python launchers agree on SIGINT exit code behavior:

| Scenario                          | Python exit code | Rust exit code |
|-----------------------------------|-----------------|----------------|
| User presses Ctrl-C               | 0               | 0              |
| Child exits normally (code 0)     | 0               | 0              |
| Child exits normally (code N)     | N               | N              |
| Child killed by SIGINT (no code)  | 0               | 0              |

**Python mechanism:** `signal_handler` in `src/amplihack/launcher/core.py` is
registered for SIGINT via `signal.signal(signal.SIGINT, signal_handler)`. The
handler calls `sys.exit(0)`.

**Rust mechanism:** `signals::register_handlers()` sets an `AtomicBool` on
SIGINT. `wait_for_child_or_signal()` detects the flag and returns `Ok(0)`.
When the child exits without a numeric code, `status.code().unwrap_or(0)`
returns 0 (introduced by PR `fix/sigint-exit-code-parity`).

---

## Parity test coverage

Two parity scenarios verify SIGINT exit code behavior. Both use a stub
`claude` binary that runs `kill -INT $$`:

| Scenario file             | Test name                     | Tier |
|---------------------------|-------------------------------|------|
| `tier5-gap-tests.yaml`    | `gap-launch-sigint-exit-code` | 5    |
| `tier7-launcher-parity.yaml` | `gap-sigint-exit-code`     | 7    |

Run the native integration tests to verify behavior:

```sh
cargo test -p amplihack --test cli_launch --locked
cargo test -p amplihack --test no_python_probe --locked
```

Expected result: the Rust launcher preserves the documented SIGINT behavior and
does not require Python for the tested paths.

---

## Related

- [Launch Flag Injection](./launch-flag-injection.md) — How the child command line is assembled before spawn
- [Environment Variables](./environment-variables.md) — Variables propagated into the child process
- [Parity Test Scenarios](./parity-test-scenarios.md) — Full list of tier5 and tier7 cases
- [Run amplihack in Non-interactive Mode](../howto/run-in-noninteractive-mode.md) — CI usage where SIGINT behavior matters
