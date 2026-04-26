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

### Step 2: Locate amplihack Hook Scripts

```bash
# The install command places hooks relative to the bundle:
amplihack resolve-bundle-asset hooks/session_start.py
amplihack resolve-bundle-asset hooks/stop.py
amplihack resolve-bundle-asset hooks/post_tool_use.py
```

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
            "command": "/home/you/.claude/tools/amplihack/hooks/session_start.py",
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
            "command": "/home/you/.claude/tools/amplihack/hooks/stop.py",
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
            "command": "/home/you/.claude/tools/amplihack/hooks/post_tool_use.py"
          }
        ]
      }
    ]
  }
}
```

!!! warning "Use Absolute Paths"
    Always use absolute paths for hook commands. Relative paths cause
    "file not found" errors (exit code 127).

### Step 4: Verify

Start a new Claude Code session. Check the output panel for:

```
SessionStart [/home/you/.claude/tools/amplihack/hooks/session_start.py] completed
```

## Troubleshooting

### "Hook not found" (exit code 127)

The hook path is wrong or relative. Use absolute paths starting with `/home/`
(Linux) or `/Users/` (macOS).

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
