# Hook Payload Compatibility (Claude Code, Amplifier, Copilot CLI)

**Status**: Stable
**Applies to**: `amplihack-types` (`HookInput` deserialization), `amplihack-hooks`
(pre-/post-tool-use protections)

## Overview

Amplihack's hook binaries are **host-agnostic**: the same `PreToolUse` /
`PostToolUse` protections run whether the host is Claude Code, Amplifier, or
GitHub Copilot CLI. Each host emits a slightly different JSON shape on the hook's
stdin. `HookInput` normalizes these shapes into a single canonical model so that
downstream guards (XPIA scan, `--no-verify` block, main-branch commit guard,
skill→agent redirect) can read the tool input uniformly via
`tool_input.get("command")`.

This document specifies the accepted payload shapes for tool-use hooks and the
normalization rules that make them interchangeable.

## Canonical model

`HookInput::PreToolUse` and `HookInput::PostToolUse` expose the tool input as a
`serde_json::Value`. For every real host payload it is a JSON **object**;
non-object values (see [Non-object JSON](#non-object-json)) pass through
unchanged rather than being coerced:

```rust
HookInput::PreToolUse {
    tool_name: String,   // e.g. "bash"
    tool_input: Value,   // normally an object, e.g. {"command": "...", "description": "..."}
    session_id: Option<String>,
}
```

Regardless of host, consumers read fields the same way:

```rust
let command = tool_input.get("command").and_then(Value::as_str);
```

## Accepted field aliases

The deserializer accepts host-specific aliases for each logical field. For the
tool input, all of the following keys are recognized (first match wins):

| Logical field | Accepted keys                            |
| ------------- | ---------------------------------------- |
| tool name     | `tool_name`, `toolName`                  |
| **tool input**| `tool_input`, `toolInput`, **`toolArgs`**|
| tool result   | `tool_result`, `toolResult`              |
| session id    | `session_id`, `sessionId`                |

`toolArgs` is the key used by **GitHub Copilot CLI**. It is treated as a full
alias of `tool_input` for both `PreToolUse` and `PostToolUse`, and it also feeds
hook-event inference and `extra` field stripping, so it never leaks into the
`extra` map.

> **Event classification caveat.** When a payload omits `hook_event_name` /
> `hookEventName`, the event is inferred from field shape: a tool-result field
> (`tool_result`/`toolResult`) implies `PostToolUse`, otherwise a tool-name or
> tool-input field implies `PreToolUse`. A Copilot `PostToolUse` payload
> therefore needs either an explicit event name or a result field to be
> classified as post rather than pre. Emitting a `hookEventName` is the reliable
> path; result-field inference for Copilot is out of scope for this change.

## Tool-input value normalization

The tool-input value may arrive in two forms:

1. **Object form** (Claude Code / Amplifier):

   ```json
   { "tool_input": { "command": "echo hi", "description": "greet" } }
   ```

   The object is used as-is.

2. **Stringified-JSON form** (Copilot CLI): the value is a JSON-encoded
   **string**, not an object:

   ```json
   {
     "toolName": "bash",
     "toolArgs": "{\"command\":\"echo hi\",\"description\":\"greet\"}"
   }
   ```

   When the tool-input value is a JSON string, it is parsed once with
   `serde_json::from_str` into a `Value`. After normalization the two forms are
   indistinguishable to downstream consumers — both yield
   `tool_input.get("command") == Some("echo hi")`.

Normalization is applied **only** to the tool-input field (via a dedicated
`required_tool_input_field` helper). Generic field extraction is unchanged, so no
other field is affected by string decoding.

### Non-object JSON

Any *valid* JSON is accepted after decoding. If a `toolArgs` string decodes to a
non-object (e.g. a bare string, number, or array), it is returned as that
`Value`. Downstream `.get("command")` on a non-object simply returns `None`,
matching the framework's existing tolerant behavior — no error is raised.

### Malformed JSON — visible failure, not silent

If the tool-input value is a string that is **not valid JSON**, the
`required_tool_input_field` helper returns an **explicit deserialize error**
referencing the field name (not the raw payload, to avoid leaking secrets). The
helper itself never substitutes an empty `{}`.

What the run-hook harness does with that error depends on the hook's
`FailurePolicy`. `PreToolUse` and `PostToolUse` are `FailurePolicy::Open`, so on
a deserialize error the harness emits an `"error"` telemetry event **and still
fail-opens** — it writes `{}` to stdout and the tool call is allowed. The
improvement over the pre-#912 behavior is *visibility*, not blocking: previously
Copilot's `toolArgs` payload silently failed deserialization and no guard ever
ran; now a valid payload runs every guard, and a malformed one fail-opens **with
a surfaced telemetry error** instead of failing invisibly.

> In short: malformed input is not *silently* swallowed — the parse failure is
> recorded as visible telemetry. But because pre-/post-tool-use are fail-open,
> the tool call itself is not blocked on a parse error. Guards only block when
> the payload deserializes and a guard condition matches.

> Historical note: prior to this normalization, Copilot CLI's `toolArgs` payload
> failed `HookInput` deserialization outright ("missing required field
> `tool_input`/`toolInput`"). The harness fail-opened to `{}`, so **none** of the
> pre-tool-use protections ran under Copilot CLI. They now run correctly.

## Worked examples

### Copilot CLI `bash` pre-tool-use payload

Input on stdin:

```json
{
  "sessionId": "abc-123",
  "timestamp": 1750000000,
  "cwd": "/repo",
  "toolName": "bash",
  "toolArgs": "{\"command\":\"git commit --no-verify -m x\",\"description\":\"commit\"}"
}
```

Normalized `HookInput`:

```rust
HookInput::PreToolUse {
    tool_name: "bash".into(),
    tool_input: json!({ "command": "git commit --no-verify -m x", "description": "commit" }),
    session_id: Some("abc-123".into()),
}
```

Result: the `--no-verify` protection extracts the command and **blocks** the
tool call, exactly as it does for the equivalent Claude Code payload.

### Claude Code `bash` pre-tool-use payload (unchanged)

```json
{
  "hook_event_name": "PreToolUse",
  "session_id": "abc-123",
  "tool_name": "bash",
  "tool_input": { "command": "git commit --no-verify -m x" }
}
```

Behavior is identical to the Copilot example above. Existing Claude Code and
camelCase (`toolInput`) payloads continue to deserialize with no change.

## Behavior matrix

| Tool-input value                         | Result                                             |
| ---------------------------------------- | -------------------------------------------------- |
| Object `{"command": "..."}`              | Used as-is                                          |
| JSON string `"{\"command\":\"...\"}"`    | Parsed to object; command extractable              |
| JSON string decoding to non-object       | Returned as `Value`; `.get("command")` → `None`    |
| String that is not valid JSON            | Helper returns **deserialize error**; harness (fail-open) emits error telemetry and writes `{}`, tool allowed |
| Field absent entirely                    | `missing required field` error (unchanged)         |

## Affected protections

Once the tool input is normalized, all pre-tool-use guards operate on Copilot CLI
payloads:

- **XPIA prompt-injection scan** — inspects the extracted command/args.
- **`--no-verify` block** — denies commits that bypass hooks.
- **Main-branch commit guard** — denies direct commits to `main`/`master`.
- **Skill→agent redirect** — routes skill invocations to the agent path.

## Security notes

- `serde_json::from_str` is the sole, memory-safe validator for stringified
  input; default recursion/size limits guard against decode-amplification DoS.
- Decoding is single-pass; a decoded object is never re-parsed recursively.
- Error messages reference the **field name and parser error only** — the raw
  `toolArgs` payload is never interpolated, avoiding secret leakage.
- Decoded content is read-only (accessed via `.get(...)`) and is never routed to
  a shell or `exec`.

## Testing

Coverage lives with the code it protects:

- `crates/amplihack-types/src/hook_io.rs` unit tests:
  - Copilot `toolArgs` string payload → `tool_input.get("command")` extracts the
    command (pre- and post-tool-use).
  - Invalid-JSON `toolArgs` string → the `required_tool_input_field` helper
    returns a deserialize error (it never substitutes `{}`); the fail-open
    harness behavior is covered separately in `amplihack-hooks`.
  - Existing `tool_input` / `toolInput` (camelCase) tests remain green.
- `crates/amplihack-hooks` pre-tool-use integration tests:
  - Copilot-shape `git commit --no-verify` payload fires the `--no-verify` block.
  - Copilot-shape commit-on-`main` payload is recognized by the main-branch
    guard (command extraction verified).
  - Malformed `toolArgs` string → harness emits an `"error"` telemetry event and
    fail-opens (writes `{}`, tool allowed), confirming the failure is visible
    rather than silent.

Validation gates:

```bash
cargo test  -p amplihack-types
cargo test  -p amplihack-hooks        # add --features signal if the suite requires it
cargo fmt
cargo clippy -p amplihack-types -p amplihack-hooks --all-targets -- -D warnings
```

## See also

- [HOOKS_COMPARISON.md](./HOOKS_COMPARISON.md) — Claude Code vs Copilot CLI hook
  capabilities.
- [HOOK_CONFIGURATION_GUIDE.md](./HOOK_CONFIGURATION_GUIDE.md) — installing and
  configuring hooks.
- [COPILOT_CLI.md](./COPILOT_CLI.md) — overall Copilot CLI integration.
