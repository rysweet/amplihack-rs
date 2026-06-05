# Hook Configuration Guide for Amplihack

## Understanding Claude Code Hook Configuration

Claude Code uses a settings merge strategy where **project settings override global settings**. This means if your project has any hooks defined in `~/.amplihack/.claude/settings.json`, they will completely replace ALL global hooks, including amplihack's hooks.

## Installation Scenarios

### Scenario 1: Fresh Installation (Works Perfectly ✅)

If you don't have any existing hooks, amplihack installation works seamlessly:

```bash
# Install amplihack
uvx --from git+https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding amplihack install

# Hooks are installed to ~/.claude/settings.json
# Everything works!
```

### Scenario 2: Project with Existing Hooks (Manual Configuration Required ⚠️)

If your project has a `~/.amplihack/.claude/settings.json` with hooks, you'll need to manually add amplihack hooks.

## Manual Hook Configuration

### Step 1: Check if Your Project Has Hooks

```bash
# Check for project settings
cat .claude/settings.json | grep -A5 hooks
```

If you see hook definitions, you need to manually merge amplihack hooks.

### Step 2: Find Your Amplihack Installation

```bash
# Find the native hook runner.
which amplihack-hooks

# Resolve the bundle hook metadata directory used by parity checks.
HOOKS_DIR="$(amplihack resolve-bundle-asset hooks-dir)"
printf 'hooks-dir -> %s\n' "$HOOKS_DIR"
```

### Step 3: Add Amplihack Hooks to Your Project

Edit your project's `~/.amplihack/.claude/settings.json` and add the amplihack hooks alongside your existing hooks:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          // YOUR EXISTING HOOKS HERE (don't remove them!)
          {
            "type": "command",
            "command": "/path/to/your/existing/hook.py"
          },
          // ADD AMPLIHACK HOOK
          {
            "type": "command",
            "command": "/Users/YOUR_USERNAME/.local/bin/amplihack-hooks session-start",
            "timeout": 10
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          // YOUR EXISTING HOOKS HERE
          // ADD AMPLIHACK HOOK
          {
            "type": "command",
            "command": "/Users/YOUR_USERNAME/.local/bin/amplihack-hooks stop",
            "timeout": 120
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          // YOUR EXISTING HOOKS HERE
          // ADD AMPLIHACK HOOK
          {
            "type": "command",
            "command": "/Users/YOUR_USERNAME/.local/bin/amplihack-hooks post-tool-use"
          }
        ]
      }
    ]
  }
}
```

**Important:** Replace `/Users/YOUR_USERNAME/.local/bin/amplihack-hooks` with
the absolute path printed by `which amplihack-hooks`.

### Step 4: Verify Your Configuration

```bash
# Check that the native hook runner exists and the bundle hook directory resolves.
test -x /Users/YOUR_USERNAME/.local/bin/amplihack-hooks
test -d "$(amplihack resolve-bundle-asset hooks-dir)"
```

## Troubleshooting

### "Hook not found" Errors

If you see errors like:

```
⏺ Stop [amplihack-hooks stop] failed with non-blocking status code 127
```

This means:

1. The `amplihack-hooks` path is wrong (check for typos)
2. The path is relative instead of absolute
3. The `amplihack-hooks` binary is not installed

**Solution:** Use the absolute path printed by `which amplihack-hooks`, or
rerun `amplihack install` to rewrite hook commands automatically.

### Hooks Not Running At All

If amplihack features aren't working:

1. **Check if project has hooks:**

   ```bash
   cat .claude/settings.json | grep hooks
   ```

2. **If yes:** Your project hooks are overriding global hooks. Follow the manual configuration steps above.

3. **If no:** Check global configuration:
   ```bash
   cat ~/.claude/settings.json | grep -A10 hooks
   ```

### Determining Which Hooks Are Running

To see which hooks Claude Code is actually using:

1. Check the Claude Code output panel for hook execution messages
2. Look for lines like:
   ```
   ⚡ SessionStart [/Users/...] completed
   ```

## Best Practices

1. **Always use absolute paths** for hooks to avoid "file not found" errors
2. **Keep a backup** of your settings before modifying:
   ```bash
   cp .claude/settings.json .claude/settings.json.backup
   ```
3. **Test after changes** by starting a new Claude Code session

## Why This Manual Process?

Claude Code's current merge strategy replaces entire hook arrays rather than merging them. This is a limitation of Claude Code, not amplihack. We've kept our solution simple rather than building complex workarounds.

## Future Improvements

We're tracking this issue and may:

1. Request Claude Code to implement additive hook merging
2. Create a simple merge tool if many users need it
3. Continue documenting workarounds as we discover them

## Getting Help

If you're still having issues:

1. Check that amplihack is properly installed:

   ```bash
   which amplihack
   which amplihack-hooks
   amplihack resolve-bundle-asset hooks-dir
   ```

2. Verify your paths are absolute and correct

3. File an issue at: https://github.com/rysweet/MicrosoftHackathon2025-AgenticCoding/issues

## Example: Complete Settings File

Here's a complete example of a project's `~/.amplihack/.claude/settings.json` with both project hooks and amplihack hooks:

```json
{
  "permissions": {
    "allow": ["Bash", "TodoWrite", "WebSearch", "WebFetch"],
    "deny": [],
    "defaultMode": "bypassPermissions"
  },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./my-project/startup.sh",
            "timeout": 5
          },
          {
            "type": "command",
            "command": "/Users/johndoe/.local/bin/amplihack-hooks session-start",
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
            "command": "./my-project/cleanup.sh",
            "timeout": 5
          },
          {
            "type": "command",
            "command": "/Users/johndoe/.local/bin/amplihack-hooks stop",
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
            "command": "/Users/johndoe/.local/bin/amplihack-hooks post-tool-use"
          }
        ]
      }
    ]
  }
}
```

This configuration runs both your project's hooks AND amplihack's hooks.
