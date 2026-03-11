# Hook Specifications Reference

amplihack registers 7 hooks in `~/.claude/settings.json` when you run `amplihack install`. This page documents each hook: its Claude Code event, the binary or script that runs, and its timeout.

## Canonical Hook Table

| # | Claude Code Event | Command Kind | Command | Timeout |
|---|------------------|--------------|---------|---------|
| 1 | `SessionStart` | Binary subcommand | `amplihack-hooks session-start` | 10 s |
| 2 | `Stop` | Binary subcommand | `amplihack-hooks stop` | 120 s |
| 3 | `PreToolUse` | Binary subcommand | `amplihack-hooks pre-tool-use` | *(none)* |
| 4 | `PostToolUse` | Binary subcommand | `amplihack-hooks post-tool-use` | *(none)* |
| 5 | `UserPromptSubmit` | Python file | `~/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py` **[planned]** | 5 s |
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
            "command": "/home/alice/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py [planned]",
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
- The two `UserPromptSubmit` entries must appear in order: `workflow_classification_reminder.py` **[planned]** first, then `user-prompt-submit`. The installer preserves this order on both fresh install and idempotent update.
- Hook commands are written with the **absolute path** to `amplihack-hooks` resolved at install time. If the binary moves, re-run `amplihack install` to update the paths.

## Hook Descriptions

### 1. SessionStart — `session-start`

Runs when Claude Code starts a new session. Initializes session context by calling the Python `MemoryCoordinator.get_context()` bridge and optionally checks for amplihack version mismatches.

### 2. Stop — `stop`

Runs when Claude Code stops. Triggers reflection, saves session memory, and handles lock-mode signaling. 120-second timeout allows the reflection write to complete.

### 3. PreToolUse — `pre-tool-use`

Runs before every tool call (matcher `*`). Validates Bash commands against the amplihack allow-list and records metrics. No timeout — fail-open so tool calls are never blocked by a slow hook.

### 4. PostToolUse — `post-tool-use`

Runs after every tool call (matcher `*`). Records tool usage metrics. No timeout — fail-open.

### 5. UserPromptSubmit — `workflow_classification_reminder.py` **[planned]**

The **first** of two `UserPromptSubmit` hooks. Injects a workflow classification reminder into the prompt context. Runs as a Python script directly (not via `amplihack-hooks`). 5-second timeout.

> **Note:** `workflow_classification_reminder.py` does not exist yet. This hook is planned for a future release. The installer currently does not register it.

### 6. UserPromptSubmit — `user-prompt-submit`

The **second** of two `UserPromptSubmit` hooks. Injects user preferences and memory context into the prompt. Runs via `amplihack-hooks`. 10-second timeout.

### 7. PreCompact — `pre-compact`

Runs before Claude Code compacts the context window. Exports the current transcript so it can be referenced after compaction.

## Hook File Locations

All Python hook scripts are staged at install time to:

```
~/.amplihack/.claude/tools/amplihack/hooks/
├── session_start.py
├── stop.py
├── pre_tool_use.py
├── post_tool_use.py
├── workflow_classification_reminder.py  [planned — not yet implemented]
├── user_prompt_submit.py
└── pre_compact.py
```

The `amplihack-hooks` binary is deployed to `~/.local/bin/amplihack-hooks` and dispatches to the correct Rust implementation based on the subcommand argument.

## XPIA Hooks

If the XPIA (Cross-Prompt Injection Attack defense) tool is installed alongside amplihack, three additional hooks are registered from `~/.amplihack/.claude/tools/xpia/hooks/`:

| Event | Script |
|-------|--------|
| `SessionStart` | `session_start.py` |
| `PreToolUse` | `pre_tool_use.py` |
| `PostToolUse` | `post_tool_use.py` |

XPIA hooks are preserved by `amplihack uninstall` — they are not removed.

XPIA hook specs use the `PythonFile` command kind. The `hooks_bin: &Path` parameter is accepted by the hook-writing functions for API uniformity but is intentionally unused for XPIA entries — their commands are absolute paths to Python scripts, not binary subcommands.

## Security

### Shell Metacharacter Validation

Hook command strings are validated against a metacharacter blocklist before being written to `settings.json`. The characters rejected are:

```
| & ; $ ` ( ) { } < ! > # ~ * \
```

This validation is performed by `validate_hook_command_string()`, called from `ensure_settings_json()` and `update_hook_paths()` before any write to disk. If a command string contains any of these characters, the install fails immediately with an actionable error identifying the offending string. No partial writes occur.

The check matters because Claude Code executes hook command strings directly. A crafted repo path containing a backtick or semicolon could inject arbitrary commands into Claude Code's hook execution if not caught here.

## Idempotency

Running `amplihack install` a second time updates hook command strings in place. The installer uses type-directed matching:

- **Binary subcommand hooks** — matched by exe filename (`amplihack-hooks`) plus subcommand argument (`session-start`, `stop`, etc.)
- **Python file hooks** — matched by filename (`workflow_classification_reminder.py` **[planned]**) and containment in `tools/amplihack/`

An existing entry is replaced in place (preserving its array position). A missing entry is appended. No duplicates are created.

The `BinarySubcmd` match requires **both** the `amplihack-hooks` filename _and_ the specific subcommand argument (e.g., `session-start`). A user hook command that happens to contain the string `amplihack-hooks` but uses a different subcommand will not be matched and will not be modified.

## See Also

- [Bootstrap Parity](../concepts/bootstrap-parity.md) — why these hooks exist and how they compare to the Python installer
- [amplihack install reference](./install-command.md) — full install command reference
- [Idempotent Installation](../concepts/idempotent-installation.md) — how repeated installs are handled
