# Configure Hooks

**Type**: How-To (Task-Oriented)

How to configure Claude Code and Copilot CLI hooks when using amplihack,
including merging with existing project hooks.

## Prerequisites

- amplihack-rs installed (`amplihack install` completed)
- Claude Code or GitHub Copilot CLI installed

## Scenario 1: Fresh Installation

If you have no existing hooks, `amplihack install` handles everything:

```bash
amplihack install
# Hooks are written to ~/.claude/settings.json automatically
```

No further configuration needed.

## Scenario 2: Merging with Existing Hooks

Claude Code uses a settings merge strategy where **project settings override
global settings**. If your project defines hooks in `.claude/settings.json`,
they replace ALL global hooks — including amplihack's.

### Step 1: Check for Existing Project Hooks

```bash
cat .claude/settings.json 2>/dev/null | grep -A5 hooks
```

If you see hook definitions, you need to merge manually.

### Step 2: Locate amplihack Hook Runtime

```bash
# Hook behavior is served by the native amplihack-hooks binary.
which amplihack-hooks

# The bundle hook directory remains available for hook metadata and parity checks.
HOOKS_DIR="$(amplihack resolve-bundle-asset hooks-dir)"
printf 'hooks-dir -> %s\n' "$HOOKS_DIR"
test -f "$HOOKS_DIR/README.md"
```

Do not pass individual hook script names such as `hooks/session_start.py` to
`resolve-bundle-asset`. The resolver accepts the named `hooks-dir` asset; hook
commands themselves run through `amplihack-hooks <subcommand>`.

### Step 3: Add amplihack Hooks to Project Settings

Edit your project's `.claude/settings.json` and add amplihack hooks alongside
your existing ones:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./your-existing-hook.sh"
          },
          {
            "type": "command",
            "command": "/home/you/.local/bin/amplihack-hooks session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/you/.local/bin/amplihack-hooks stop",
            "timeout": 120
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "/home/you/.local/bin/amplihack-hooks post-tool-use"
          }
        ]
      }
    ]
  }
}
```

!!! warning "Use Absolute Paths"
    Always use the absolute path to `amplihack-hooks` when hand-editing
    settings. Relative paths cause "file not found" errors (exit code 127).

### Step 4: Verify

Start a new Claude Code session. Check the output panel for:

```
SessionStart [/home/you/.local/bin/amplihack-hooks session-start] completed
```

## Troubleshooting

### "Hook not found" (exit code 127)

The `amplihack-hooks` path is wrong or relative. Use an absolute path starting
with `/home/` (Linux) or `/Users/` (macOS), or rerun `amplihack install` to
rewrite hook commands automatically.

### Hooks Not Running

1. Check if project-level settings override globals:
   ```bash
   cat .claude/settings.json | grep hooks
   ```
2. If yes, follow the merge steps above.
3. If no, check global config: `cat ~/.claude/settings.json | grep -A10 hooks`

### Back Up Before Editing

```bash
cp .claude/settings.json .claude/settings.json.backup
```

## Why Manual Merging?

Claude Code replaces entire hook arrays rather than merging them. This is a
platform limitation, not an amplihack issue. We keep the solution simple rather
than building complex merge workarounds.

## Related

- [Hooks Comparison](../concepts/hooks-comparison.md) — understand differences between Claude Code and Copilot CLI hooks
- [Hook Specifications](../reference/hook-specifications.md) — full reference for hook configuration schema
- [Shell Command Hook](../reference/shell-command-hook.md) — the `!command` prompt hook
