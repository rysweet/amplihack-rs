# Turn-Failure Error Hygiene

When a Copilot turn's child process exits non-zero, the driver in
[`crates/amplihack-turn`](../crates/amplihack-turn) returns an error that
describes the failure **without** dumping the child's full stdout/stderr into
the surfaced message. By default the error carries only the exit status and a
short, size-bounded tail of the combined output. The full output is still
available to operators who opt into debug-level logging.

This closes the information-disclosure and log-hygiene concern in issue #1092:
raw child output can contain secrets, tokens echoed by tools, absolute paths,
or many megabytes of log text, and this error string is both written to logs
and relayed onward through the Signal chat layer.

Read this document when you need to:

- understand what a failed turn's error message contains and why,
- configure how much trailing output that message includes,
- retrieve the full child output while diagnosing a failure,
- write or read tests that assert the failure-error contract.

---

## Contents

- [Overview](#overview)
- [What the error contains](#what-the-error-contains)
- [Configuration: `AMPLIHACK_TURN_ERROR_TAIL_BYTES`](#configuration-amplihack_turn_error_tail_bytes)
- [Retrieving the full output at debug level](#retrieving-the-full-output-at-debug-level)
- [API reference](#api-reference)
- [Examples](#examples)
- [Design notes](#design-notes)
- [Security invariants](#security-invariants)
- [Testing](#testing)
- [See also](#see-also)

---

## Overview

The production turn runner, `CopilotTurnRunner`, spawns each `copilot` turn as a
child process and captures its stdout and stderr. There are two outcomes:

- **Success (zero exit).** The runner returns the child's captured stdout
  verbatim as the turn output. This path is unchanged — full stdout is the
  turn result, exactly as before.
- **Failure (non-zero exit).** The runner returns an `io::Error` whose message
  is a **summary**: the exact prefix `copilot turn failed ({status})` followed
  by only a bounded tail of the combined stdout+stderr. The complete combined
  output is emitted separately at `tracing::debug!`.

Only the non-zero-exit path changed. The failure error still begins with the
stable prefix `copilot turn failed`, so existing log parsing and the chat
layer's `turn failed: {e}` relay message keep working.

---

## What the error contains

On a non-zero exit the returned error string has the form:

```text
copilot turn failed ({status}); last {n} bytes of output: {tail}
```

- `{status}` — the child's exit status, e.g. `exit status: 3`. The
  `copilot turn failed ({status})` prefix is preserved exactly and is stable
  for downstream parsing.
- `{n}` — the **actual** number of bytes in `{tail}` after the char-boundary
  snap (see below). This is the truthful length of what follows, not the
  configured budget.
- `{tail}` — the last `n` bytes of the combined output, built as the child's
  stdout followed by its stderr (preserving the historical ordering), decoded
  losslessly (`from_utf8_lossy`).

Behavioral details:

- **Bounded.** `{tail}` is at most the configured budget in bytes (see
  [configuration](#configuration-amplihack_turn_error_tail_bytes)). The error
  message length no longer scales with child output size, so a chatty or
  runaway child cannot flood logs or relayed messages.
- **Short output.** If the combined output is already within the budget, the
  whole output is included and `{n}` equals its full length.
- **Multibyte-safe.** The tail start index is snapped **forward** to the
  nearest UTF-8 character boundary, so the tail never splits a multibyte
  character and never panics. Because the snap moves forward, the tail is
  always `<= budget` bytes.
- **Not redacted here.** The tail is bounded, not scrubbed. Redaction of
  relayed message bodies is the responsibility of the relay layer
  (`redact_for_relay` in `amplihack-signal`), which is the correct place to
  apply it. The turn driver deliberately does not add a second redactor.

---

## Configuration: `AMPLIHACK_TURN_ERROR_TAIL_BYTES`

The tail size is an explicit operator policy, not a fixed hardcoded cap.

| | |
| --- | --- |
| **Environment variable** | `AMPLIHACK_TURN_ERROR_TAIL_BYTES` |
| **Meaning** | Maximum number of trailing bytes of a failed turn's combined output to include in the surfaced error string. |
| **Type** | Unsigned integer (bytes). |
| **Default** | `2048` (the `DEFAULT_TURN_ERROR_TAIL_BYTES` constant) when the variable is unset. |
| **Value `0`** | Honored literally — the error carries no tail, only the exit-status summary. |
| **Unparseable value** | Any value that does not parse as an unsigned integer — including negative numbers and values that overflow `usize` — falls back to the default **and** emits a `tracing::warn!` naming the bad value. Misconfiguration is never silent. |

Raise the budget when you want more failure context inline; lower it (or set it
to `0`) to further reduce the amount of child output that can reach logs and
relays. There is no upper limit imposed by the driver — the value you set is
the value used — so choose it against your own log-hygiene policy.

### Examples

```bash
# Default behavior: up to 2048 bytes of tail in the error.
unset AMPLIHACK_TURN_ERROR_TAIL_BYTES

# Tighten to a 256-byte tail.
export AMPLIHACK_TURN_ERROR_TAIL_BYTES=256

# Emit only the exit-status summary, no child output at all.
export AMPLIHACK_TURN_ERROR_TAIL_BYTES=0

# Bad value -> falls back to 2048 and logs a warning naming "banana".
export AMPLIHACK_TURN_ERROR_TAIL_BYTES=banana
```

---

## Retrieving the full output at debug level

The complete combined output is never discarded — it is emitted at
`tracing::debug!` with structured fields, so operators keep full diagnosability
without exposing that output in the default error surface.

The debug event carries:

| Field | Meaning |
| --- | --- |
| `status` | The child's exit status. |
| `stdout_len` | Length of the captured stdout, in bytes. |
| `stderr_len` | Length of the captured stderr, in bytes. |
| `output` | The full combined stdout+stderr text. |

Message: `copilot turn failed; full combined output at debug`.

Enable it by configuring your `tracing` subscriber to allow `DEBUG` for the
`amplihack_turn` target, for example:

```bash
# With tracing-subscriber's EnvFilter:
export RUST_LOG=amplihack_turn=debug
```

> **Operational note.** The full `output` field is intended for opt-in
> diagnosis. Do not run production relays at `DEBUG` for `amplihack_turn` as a
> default, since that reintroduces the full child output into your log sink.

---

## API reference

Public surface (module `amplihack_turn::turn`):

### `DEFAULT_TURN_ERROR_TAIL_BYTES`

```rust
/// Default number of trailing bytes of a failed turn's combined stdout+stderr
/// to include in the surfaced error when `AMPLIHACK_TURN_ERROR_TAIL_BYTES` is
/// unset or cannot be parsed as an unsigned integer.
pub const DEFAULT_TURN_ERROR_TAIL_BYTES: usize = 2048;
```

The documented default budget. Exposed so callers and tests can reference the
same value the runner uses.

### `CopilotTurnRunner` (behavioral contract)

`CopilotTurnRunner` implements the `TurnRunner` trait. Its `run_argv` future
resolves to:

- `Ok(String)` — the child's full captured stdout, on a zero exit.
- `Err(io::Error)` — on a non-zero exit, an `io::Error::other(..)` whose
  message is `copilot turn failed ({status}); last {n} bytes of output: {tail}`
  as described in [What the error contains](#what-the-error-contains). Other
  error kinds (e.g. `Interrupted` for a pre-empted turn) are unaffected by this
  feature.

The tail budget and the char-boundary snapping are internal helpers; only the
default constant and the environment variable are part of the public contract.

---

## Examples

### Reading a failed-turn error in the chat layer

The Signal chat layer surfaces the error unchanged apart from a prefix:

```text
turn failed: copilot turn failed (exit status: 2); last 137 bytes of output: ...trailing output...
```

Because the `copilot turn failed` prefix is preserved, any code that matches on
it (including the existing integration test asserting
`msg.contains("copilot turn failed")`) continues to work.

### Diagnosing a failure with full output

```bash
# Reproduce the failure with full child output visible.
RUST_LOG=amplihack_turn=debug amplihack signal chat ...

# In the logs, find:
#   DEBUG amplihack_turn: copilot turn failed; full combined output at debug
#       status=exit status: 2 stdout_len=41231 stderr_len=88 output=<full text>
```

---

## Design notes

- **Only the failure branch changed.** The success path still returns the full
  captured stdout as the turn output. Nothing about a successful turn's output
  is bounded or truncated.
- **Forward char-boundary snap.** Taking the last `budget` bytes can land in the
  middle of a multibyte UTF-8 sequence. Snapping the start index forward to the
  next boundary guarantees a valid `&str` slice and keeps the result within the
  budget. This mirrors the char-boundary-safe truncation used elsewhere for
  prompt injection handling.
- **No heavy dependencies.** The only new dependency is the lightweight
  `tracing` logging facade (no networking, negligible transitive cost). The
  `amplihack-turn` crate is always compiled and intentionally lean, so no
  regex engine, redactor, or `tokio` `net` feature was added.
- **No silent fallbacks.** An unparseable budget falls back to the default and
  warns; it is never quietly ignored.

---

## Security invariants

- **Bounded by default.** With no configuration, at most
  `DEFAULT_TURN_ERROR_TAIL_BYTES` (2048) bytes of child output can appear in the
  surfaced/relayed error. Full output is never embedded in full by default.
- **Operator-gated full output.** The complete stdout+stderr is available only
  at `tracing::debug!`, which is opt-in.
- **Panic-free on hostile input.** Lossy UTF-8 decode plus a forward
  char-boundary snap mean arbitrary child bytes cannot panic the tail
  computation.
- **Log-flood resistance.** The error-message size is independent of child
  output size, so a runaway child cannot inflate logs or relayed messages
  through this path.
- **Stable prefix.** `copilot turn failed ({status})` is preserved for
  downstream parsing and for the chat relay message.
- **Redaction stays at the relay layer.** The tail is bounded but not scrubbed;
  scrubbing of relayed bodies remains the responsibility of `redact_for_relay`
  in `amplihack-signal`, avoiding a duplicated, misplaced redactor here.

---

## Testing

Validation gate:

```bash
cargo fmt --all
cargo clippy -p amplihack-turn --all-targets -- -D warnings
cargo test  -p amplihack-turn                      # incl. turn_error_it
cargo build -p amplihack-turn
```

Coverage in `crates/amplihack-turn/tests/turn_error_it.rs`:

- **Multibyte safety.** A failing turn whose combined output ends in multibyte
  UTF-8 characters longer than the tail budget produces a bounded tail and does
  not panic.
- **Bound honored.** The returned error's tail portion is `<= budget` bytes
  (allowing for the forward char-boundary snap).
- **Env override respected.** Setting `AMPLIHACK_TURN_ERROR_TAIL_BYTES` to a
  small value shrinks the tail; an unparseable value falls back to
  `DEFAULT_TURN_ERROR_TAIL_BYTES`.
- **Prefix preserved.** The error still contains `copilot turn failed`.
- **Full output at debug only.** A captured `tracing` subscriber observes the
  full combined output at `DEBUG`, and that full output is **not** present in
  the returned error string when it exceeds the tail budget.

> **Test hygiene.** Environment-mutating tests are serialized through a
> process-wide mutex and wrap `std::env::set_var`/`remove_var` in `unsafe`
> (edition 2024) to avoid cross-test races, matching the pattern used elsewhere
> in the workspace.

---

## See also

- [Signal Chat Hardening](signal-chat-hardening.md)
- [Signal Channel Turn Loop](signal-channel-turn-loop.md)
- [`crates/amplihack-turn`](../crates/amplihack-turn)
