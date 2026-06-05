---
title: "Validate Recipe Subprocess and Hook Input Contracts"
description: "Check that recipe execution runs non-interactively, strips Claude-only session state, and accepts supported hook JSON aliases."
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack
doc_type: howto
---

# Validate Recipe Subprocess and Hook Input Contracts

Use this guide when changing `amplihack recipe run`, `EnvBuilder`, hook wrappers,
or `HookInput` deserialization. It validates the implemented subprocess and hook
input contract.

## Prerequisites

- `amplihack` and `amplihack-hooks` are on `PATH`
- You are in an `amplihack-rs` checkout
- Rust toolchain is installed

## 1. Check recipe subprocess behavior with a stub runner

Use `RECIPE_RUNNER_RS_PATH` to replace `recipe-runner-rs` with a temporary stub
that prints the child environment observed by the actual `amplihack recipe run`
subprocess launch:

```bash
tmpdir="$(mktemp -d)"
cat >"$tmpdir/recipe-runner-rs" <<'SH'
#!/usr/bin/env sh
printf '%s\n' "{
  \"recipe_name\":\"env-probe\",
  \"success\":true,
  \"env_probe\":{
    \"AMPLIHACK_NONINTERACTIVE\":\"${AMPLIHACK_NONINTERACTIVE:-}\",
    \"CLAUDECODE_PRESENT\":\"${CLAUDECODE+x}\",
    \"AMPLIHACK_AGENT_BINARY\":\"${AMPLIHACK_AGENT_BINARY:-}\"
  }
}"
SH
chmod +x "$tmpdir/recipe-runner-rs"

env -u AMPLIHACK_NONINTERACTIVE \
  CLAUDECODE=1 \
  RECIPE_RUNNER_RS_PATH="$tmpdir/recipe-runner-rs" \
  amplihack recipe run default-workflow \
  -c task_description="Inspect repository documentation" \
  -c repo_path=. \
  --format json
```

The JSON output includes `env_probe`. The child process contract is:

| Variable | Expected child state |
|----------|----------------------|
| `AMPLIHACK_NONINTERACTIVE` | `1` |
| `CLAUDECODE_PRESENT` | empty string |
| `AMPLIHACK_AGENT_BINARY` | active launcher binary, when set by the launcher |

If a recipe or nested agent prompts for input, treat that as a subprocess
environment bug.

## 2. Check canonical hook input

Send canonical snake_case hook input:

```bash
printf '%s\n' '{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {"command": "pwd"}
}' | amplihack-hooks pre-tool-use
```

The hook accepts the payload. Depending on policy, the output is either `{}` or
a permission decision such as:

```json
{
  "permissionDecision": "allow"
}
```

## 3. Check camelCase hook input

Send the same payload using supported host aliases:

```bash
printf '%s\n' '{
  "hookEventName": "PreToolUse",
  "toolName": "Bash",
  "toolInput": {"command": "pwd"},
  "sessionId": "example-session"
}' | amplihack-hooks pre-tool-use
```

The hook accepts the payload exactly as it accepts the canonical form.

## 4. Check optional-field tolerance

Stop hooks can omit optional fields:

```bash
printf '%s\n' '{
  "hook_event_name": "Stop"
}' | amplihack-hooks stop
```

The hook deserializes the payload with absent optional fields instead of failing
at the JSON boundary.

## 5. Check required-field strictness

Tool events still require `tool_name` and `tool_input`:

```bash
printf '%s\n' '{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash"
}' | amplihack-hooks pre-tool-use
```

This payload is invalid because `tool_input` is missing. The hook runtime must
not convert incomplete known tool events into `Unknown`. Depending on the hook
failure policy, the executable may return an error response or fail-open output,
but the deserialization path must record the payload as invalid rather than
future-compatible.

## Developer validation

Run the focused Rust tests for the two contracts by name:

```bash
cargo test -p amplihack-cli recipe_runner_child_environment
cargo test -p amplihack-types hook_input_accepts_camel_case_aliases
cargo test -p amplihack-types hook_input_rejects_missing_required_tool_fields
cargo test -p amplihack-hooks malformed_known_tool_event_is_not_unknown
```

These tests cover the centralized subprocess environment and the typed
`HookInput` JSON boundary. The CLI stub check above verifies the subprocess
contract end to end.

## Related

- [Recipe Executor Environment](../reference/recipe-executor-environment.md) —
  full subprocess and step environment reference
- [Hook Specifications Reference](../reference/hook-specifications.md) — hook
  events, commands, and JSON input contract
- [Run amplihack in Non-interactive Mode](run-in-noninteractive-mode.md) —
  top-level non-interactive usage
