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

Older installs may still contain historical `tools/amplihack/hooks/*.py` command paths in `settings.json`. Reinstalling with the Rust installer upgrades those entries in place to the native binary format, and uninstall continues to recognize and remove the legacy paths.

## XPIA Hooks

If the XPIA (Cross-Prompt Injection Attack defense) tool is installed alongside amplihack, the Rust installer checks for staged XPIA hook assets at `~/.amplihack/.claude/tools/xpia/hooks/`. Current native installs do **not** add a second set of distinct hook wrappers for XPIA; they keep the unified `amplihack-hooks` entries already used by core amplihack hooks and use XPIA asset presence only as the optional-feature and legacy-upgrade signal.

| Event | Native command |
|-------|----------------|
| `SessionStart` | `amplihack-hooks session-start` |
| `PreToolUse` | `amplihack-hooks pre-tool-use` |
| `PostToolUse` | `amplihack-hooks post-tool-use` |

Older installs may still contain historical `tools/xpia/hooks/*.py` command paths. Reinstalling with the Rust installer upgrades those XPIA entries in place to the native binary-subcommand format instead of leaving duplicate legacy and native wrappers side by side.

XPIA hook entries are preserved by `amplihack uninstall` — they are not removed.

XPIA remains an optional auxiliary hook surface. amplihack uninstall preserves any XPIA-managed entries rather than treating them as part of the core native amplihack hook registration set.

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
- **Python file hooks** — matched by filename and containment in `tools/amplihack/` (kept for stale-entry cleanup and XPIA compatibility)

An existing entry is replaced in place (preserving its array position). A missing entry is appended. No duplicates are created.

The `BinarySubcmd` match requires **both** the `amplihack-hooks` filename _and_ the specific subcommand argument (e.g., `session-start`). A user hook command that happens to contain the string `amplihack-hooks` but uses a different subcommand will not be matched and will not be modified.

## See Also

- [Bootstrap Parity](../concepts/bootstrap-parity.md) — why these hooks exist and how they compare to the Python installer
- [amplihack install reference](./install-command.md) — full install command reference
- [Idempotent Installation](../concepts/idempotent-installation.md) — how repeated installs are handled
