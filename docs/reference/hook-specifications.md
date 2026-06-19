# Hook Specifications Reference

amplihack registers 7 hooks in `~/.claude/settings.json` when you run `amplihack install`. This page documents each hook: its Claude Code event, the binary or script that runs, and its timeout.

## Canonical Hook Table

| # | Claude Code Event | Command Kind | Command | Timeout |
|---|------------------|--------------|---------|---------|
| 1 | `SessionStart` | Binary subcommand | `amplihack-hooks session-start` | 10 s |
| 2 | `Stop` | Binary subcommand | `amplihack-hooks stop` | 120 s |
| 3 | `PreToolUse` | Binary subcommand | `amplihack-hooks pre-tool-use` | *(none)* |
| 4 | `PostToolUse` | Binary subcommand | `amplihack-hooks post-tool-use` | *(none)* |
| 5 | `UserPromptSubmit` | Binary subcommand | `amplihack-hooks workflow-classification-reminder` | 5 s |
| 6 | `UserPromptSubmit` | Binary subcommand | `amplihack-hooks user-prompt-submit` | 10 s |
| 7 | `PreCompact` | Binary subcommand | `amplihack-hooks pre-compact` | 30 s |

## Hook Format in settings.json

Each hook is written as a wrapper object under the event key:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks pre-tool-use"
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks workflow-classification-reminder",
            "timeout": 5
          }
        ]
      },
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/alice/.local/bin/amplihack-hooks user-prompt-submit",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

**Key points:**

- `PreToolUse` and `PostToolUse` have no `timeout` field. Absence means fail-open (no timeout enforced by Claude Code).
- Both `PreToolUse` and `PostToolUse` use matcher `"*"` to intercept all tool calls.
- The two `UserPromptSubmit` entries must appear in order: `workflow-classification-reminder` first, then `user-prompt-submit`. The installer preserves this order on both fresh install and idempotent update.
- Hook commands are written with the **absolute path** to `amplihack-hooks` resolved at install time. If the binary moves, re-run `amplihack install` to update the paths.

## Hook Input JSON Contract

Hook binaries read one JSON object from stdin and deserialize it into the shared
`HookInput` type. The compatibility contract is host-agnostic: Claude Code,
Copilot wrappers, Amplifier, and tests can send the same semantic payload.

The contract has two layers:

- **Typed input boundary:** known events deserialize with their required fields;
  malformed known-event payloads are invalid.
- **Hook executable boundary:** hooks keep their existing fail-open policy for
  processing errors and panics, but do not convert malformed known-event JSON
  into `Unknown`. `Unknown` is reserved for valid payloads whose event name is
  not recognized yet.

### Event discriminator

The canonical discriminator is `hook_event_name`. The deserializer also accepts
the known camelCase alias `hookEventName`.

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": {
    "command": "pwd"
  }
}
```

Equivalent camelCase input:

```json
{
  "hookEventName": "PreToolUse",
  "toolName": "Bash",
  "toolInput": {
    "command": "pwd"
  }
}
```

### Field aliases

| Canonical field | Accepted alias | Used by |
|-----------------|----------------|---------|
| `hook_event_name` | `hookEventName` | All hook events |
| `tool_name` | `toolName` | `PreToolUse`, `PostToolUse` |
| `tool_input` | `toolInput` | `PreToolUse`, `PostToolUse` |
| `tool_result` | `toolResult` | `PostToolUse` |
| `session_id` | `sessionId` | All events with session context |
| `stop_hook_active` | `stopHookActive` | `Stop` |
| `transcript_path` | `transcriptPath` | `Stop`, `SessionStop`, `PreCompact` |
| `user_prompt` | `userPrompt` | `UserPromptSubmit` |

Unknown additional fields are ignored unless a variant explicitly captures them
as extra event data.

### Required and optional fields

Missing optional fields deserialize to `None` / absent values. Required semantic
fields remain strict at both the typed boundary and the hook executable entry.

| Event | Required fields | Optional fields |
|-------|-----------------|-----------------|
| `PreToolUse` | `tool_name`, `tool_input` | `session_id` |
| `PostToolUse` | `tool_name`, `tool_input` | `tool_result`, `session_id` |
| `Stop` | none beyond event name | `stop_hook_active`, `transcript_path`, `session_id` |
| `SessionStart` | none beyond event name | `session_id`, `cwd`, extra fields |
| `SessionStop` | none beyond event name | `session_id`, `transcript_path`, extra fields |
| `UserPromptSubmit` | none beyond event name | `user_prompt`, `session_id`, extra fields |
| `PreCompact` | none beyond event name | `session_id`, `transcript_path`, extra fields |

Malformed JSON, a missing event discriminator, or a missing required field for a
tool event is invalid input. The compatibility layer is intentionally narrow: it
accepts known host schema drift without treating incomplete tool payloads as
valid or silently mapping them to `Unknown`.

### Unknown events

Future hook events with a valid event discriminator deserialize to `Unknown`
instead of failing. This makes the hook boundary forward-compatible while
keeping known event payloads typed and strict. Malformed known events are not
future events and must not use this path.

```json
{
  "hook_event_name": "FutureEvent",
  "payload": {
    "anything": true
  }
}
```

## Hook Descriptions

### 1. SessionStart — `session-start`

Runs when Claude Code starts a new session. Initializes session context, checks for version mismatches natively in Rust, and migrates stale global amplihack hook registrations out of `~/.claude/settings.json`.

### 2. Stop — `stop`

Runs when Claude Code stops. Triggers native Rust reflection prompt assembly plus Claude CLI analysis, saves session memory, and handles lock-mode signaling. 120-second timeout allows the reflection write to complete.

### 3. PreToolUse — `pre-tool-use`

Runs before every tool call (matcher `*`). Validates Bash commands against the amplihack allow-list and records metrics. No timeout — fail-open so tool calls are never blocked by a slow hook.

### 4. PostToolUse — `post-tool-use`

Runs after every tool call (matcher `*`). Records tool usage metrics. No timeout — fail-open.

### 5. UserPromptSubmit — `workflow-classification-reminder`

The **first** of two `UserPromptSubmit` hooks. Injects a workflow classification reminder into the prompt context. Runs via `amplihack-hooks`. 5-second timeout.

### 6. UserPromptSubmit — `user-prompt-submit`

The **second** of two `UserPromptSubmit` hooks. Injects user preferences and native Rust memory context for referenced agents into the prompt. Runs via `amplihack-hooks`. 10-second timeout.

### 7. PreCompact — `pre-compact`

Runs before Claude Code compacts the context window. Exports the current transcript so it can be referenced after compaction.

## Hook Runtime Location

Fresh installs register hooks through the native `amplihack-hooks` binary deployed at `~/.local/bin/amplihack-hooks`. The installer writes command strings such as `amplihack-hooks session-start` and `amplihack-hooks user-prompt-submit` into `~/.claude/settings.json`.

## Generated script line endings

Generated Bash-facing hook scripts are written with LF-only line endings. This
includes Copilot `.github/hooks/*` wrappers, generated `_error_handler` content,
and legacy launcher-staged hook scripts. The hook staging boundary normalizes
CRLF and lone CR input before writing executable script bytes, so generated
hooks do not fail with `$'\r': command not found` or bad-interpreter `\r`
errors on Windows-native checkouts.
