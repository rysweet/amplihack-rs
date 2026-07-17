# Bug Fix #898 — NUL-byte prompt sanitization in subprocess delivery

> **Issue:** [#898](https://github.com/rysweet/amplihack-rs/issues/898)

---

## Summary

The recipe runner no longer aborts a workflow when agent or bash-step output
contains a NUL (`0x00`) byte. Previously, a single NUL anywhere in a prompt
caused prompt delivery to fail with:

```
nul byte found in provided data
```

That error propagated up through the orchestration layer and killed every
downstream step in the run.

As of this fix, the prompt-delivery boundary **strips NUL bytes and continues**.
All other bytes are preserved exactly. A single embedded NUL — from a truncated
tool result, a binary blob accidentally echoed by a bash step, or an agent that
emitted control characters — is silently removed from the prompt so the child
process spawns normally and the workflow proceeds.

When stripping occurs, the helper emits a `tracing::warn!` recording **only the
count** of removed bytes. Prompt content is never logged.

## Behavior

### Before

| Input prompt        | Result                                    |
| ------------------- | ----------------------------------------- |
| `"analyze the log"` | Delivered normally                        |
| `"a\0b\0c"`         | `Err(InvalidInput)` → **downstream steps aborted** |

### After

| Input prompt        | Result                                            |
| ------------------- | ------------------------------------------------- |
| `"analyze the log"` | Delivered normally (zero-copy fast path)          |
| `"a\0b\0c"`         | Delivered as `"abc"`; `warn!` logs `removed = 2`  |

The change applies uniformly across all three delivery modes
(`argv`, `tempfile`, `stdin`), so both agent prompts and bash-step prompts are
normalized identically regardless of which mode `auto` selects.

## Scope and non-goals

- **Only** the byte `0x00` is removed. No other characters — including shell
  metacharacters, newlines, or other control bytes — are altered. Delivery uses
  `Command::arg` (no shell), so no additional filtering is required or performed.
- The executable-name NUL check in `agent_binary.rs` remains a **hard reject**.
  A NUL in a binary path is a genuine `execve` injection boundary, not workflow
  data, and is out of scope for this fix.

## API

### `sanitize_prompt_nul`

A single source-of-truth helper in
`crates/amplihack-utils/src/prompt_delivery.rs`:

```rust
use std::borrow::Cow;

/// Strip NUL (`0x00`) bytes from a prompt so it is safe to pass to a child
/// process argv, temp file, or stdin.
///
/// - Fast path: when the input contains no NUL byte, returns
///   [`Cow::Borrowed`] with zero allocation and zero copying — the common case.
/// - Slow path: when one or more NUL bytes are present, returns
///   [`Cow::Owned`] containing every non-NUL character in original order, and
///   emits a `tracing::warn!` recording only the count of removed bytes
///   (never the prompt content).
pub fn sanitize_prompt_nul(prompt: &str) -> Cow<'_, str>;
```

**Guarantees:**

1. Output contains no `0x00` byte.
2. Every non-NUL byte is preserved, in original order (order-preserving strip).
3. No prompt content is logged; the warning is count-only.
4. Idempotent: `sanitize_prompt_nul(sanitize_prompt_nul(x)) == sanitize_prompt_nul(x)`.

### `deliver`

`deliver()` sanitizes the prompt at the top of the function and uses the
sanitized value for both the `Argv` (`cmd.arg`) and `Tempfile` (`write_all`)
branches. The NUL-reject block has been removed — `deliver` no longer returns an
error for NUL-containing prompts.

Mode selection is unchanged: `select_mode` continues to run on the **original**
`prompt.len()`, not the sanitized length. This keeps `deliver`'s choice
consistent with the stdin callers, which also call
`select_mode(requested, prompt.len(), &caps)` on the raw prompt. Because
stripping only removes NUL bytes (which are rare and never near the tempfile
threshold in practice), computing the mode from the original length avoids any
divergence between `deliver` and the caller-built stdin payload. Sanitization
affects only the bytes written to argv / the temp file / stdin — never the mode
decision.

The signature is unchanged:

```rust
pub fn deliver(
    cmd: &mut Command,
    prompt: &str,
    requested: PromptDelivery,
    caps: &DeliveryCaps,
) -> std::io::Result<DeliveryHandle>;
```

### Stdin path

The `Stdin` mode's payload is built by the caller (it bypasses the `argv` /
`tempfile` write in `deliver`) and is an `Option<Vec<u8>>` populated only when
the selected mode is `Stdin`. Both stdin consumers now normalize their payload
through the same helper so no NUL reaches the child, while preserving the
existing mode guard:

- `crates/amplihack-orchestration/src/claude_process.rs`
- `crates/amplihack-launcher/src/prompt_delivery.rs`

```rust
let stdin_payload = (selected_mode == DeliveryMode::Stdin)
    .then(|| sanitize_prompt_nul(prompt).as_bytes().to_vec());
```

## Configuration

No new configuration. Existing delivery configuration is unchanged:

- `AMPLIHACK_PROMPT_DELIVERY` ∈ `{auto, argv, tempfile, stdin}` (case-insensitive)
  still selects the delivery mode. NUL sanitization is applied for all modes.
- `NODE_OPTIONS=--max-old-space-size=32768` and other environment settings are
  unaffected.

## Examples

### Argv mode — embedded NULs are stripped

```rust
use amplihack_utils::prompt_delivery::{deliver, DeliveryCaps, PromptDelivery};
use std::process::Command;

let mut cmd = Command::new("agent-binary");
// DeliveryCaps has no Default derive; all fields are set explicitly.
let caps = DeliveryCaps {
    supports_argv: true,
    supports_tempfile: false,
    supports_stdin: false,
    tempfile_flag: None,
};

// Prompt contains stray NUL bytes from an upstream tool result.
let handle = deliver(&mut cmd, "a\0b\0c", PromptDelivery::Argv, &caps)?;

// The child receives "abc" as a single argv element; delivery succeeds and the
// workflow continues. A warn!("removed = 2") is emitted.
```

### Helper — zero-copy fast path

```rust
use amplihack_utils::prompt_delivery::sanitize_prompt_nul;
use std::borrow::Cow;

// No NUL → borrowed, no allocation.
assert!(matches!(sanitize_prompt_nul("clean prompt"), Cow::Borrowed(_)));

// Contains NUL → owned, stripped, order preserved.
assert_eq!(sanitize_prompt_nul("x\0y\0"), "xy");
```

### Recipe runner — a poisoned tool result no longer kills the run

A bash step that emits a NUL (for example, `printf 'done\0'`) into a result that
becomes the next agent's prompt now behaves as follows:

1. `deliver` (or the stdin payload builder) strips the NUL.
2. `tracing::warn!` logs `removed = 1`.
3. The downstream agent step spawns with the cleaned prompt.
4. The recipe completes instead of aborting.

## Testing

- `crates/amplihack-utils/tests/prompt_delivery_downstream.rs` — the former
  `argv_delivery_rejects_nul_bytes_before_child_spawn` test is now
  `argv_delivery_strips_nul_bytes_before_child_spawn`: it asserts `deliver`
  returns `Ok` and the argv element is NUL-free with all other bytes preserved.
- Regression coverage: `Argv` prompt `"a\0b\0c"` → `"abc"`; `Tempfile` write with
  an embedded NUL succeeds; `Stdin` payload is NUL-free; helper zero-copy
  identity and order-preserving strip; no prompt content appears in logs.

**Validation commands:**

```bash
cargo build -p amplihack-utils -p amplihack-orchestration -p amplihack-launcher
cargo test  -p amplihack-utils prompt_delivery
```

## Security considerations

- Logs record the removed-byte **count only** — never prompt bytes — because
  prompts may carry secrets or PII.
- The executable-name NUL rejection in `agent_binary.rs` stays strict.
- `argv` delivery uses `Command::arg` (no shell), so stripping-only is safe and
  no shell-metacharacter filtering is introduced.
- Temp files retain their `0600` permissions; the sanitized prompt is not
  persisted anywhere new.
- The zero-copy `Cow::Borrowed` fast path avoids extra in-memory copies of
  sensitive prompt data in the common (NUL-free) case.
