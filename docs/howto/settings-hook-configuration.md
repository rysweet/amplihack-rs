# How-To: Settings Hook Configuration

## Overview

Amplihack automatically configures hooks in `~/.claude/settings.json` during installation and CLI initialization. This guide explains the hook configuration process and how to troubleshoot issues.

## Automatic Configuration (New in v0.5.27)

Starting in v0.5.27, hook configuration is **completely automatic** - no user prompts required.

When `amplihack-hooks` is installed, amplihack registers native hook
subcommands automatically. Hook configuration is binary-first and does not
require helper scripts in the installed framework bundle.

### What Happens Automatically

When you run `amplihack` or install the framework:

1. **Validation**: Hook files are validated before configuration
2. **Backup**: Settings are backed up to `~/.claude/settings.json.backup.<timestamp>`
3. **Configuration**: Hooks are added/updated in settings.json
4. **Verification**: System confirms hooks are properly configured

**No user intervention required!**

## Hook Validation

### What Is Validated

The system validates that all required hook files exist before configuring them:

**Amplihack Hooks (Required):**

- `session_start.py` - Runs when Claude Code session starts
- `stop.py` - Runs when Claude Code session stops
- `post_tool_use.py` - Runs after each tool invocation
- `pre_compact.py` - Runs before context compaction

**XPIA Hooks (Optional):**

- `session_start.py` - Security initialization
- `post_tool_use.py` - Security monitoring
- `pre_tool_use.py` - Input validation

### Error Messages

If required hooks are missing, you'll see:

```
❌ Hook validation failed - missing required hooks:
   • amplihack/session_start.py (expected at /home/user/.amplihack/.claude/tools/amplihack/hooks/session_start.py)
💡 Please reinstall amplihack to restore missing hooks
```

**Resolution:** Run `amplihack install` to restore missing hooks.

## Backups

### Automatic Backups

Every time settings.json is modified, a backup is created:

```
~/.claude/settings.json.backup.<timestamp>
```

Example: `~/.claude/settings.json.backup.1739673234`

### Restoring from Backup

If you need to restore a previous configuration:

```bash
# Find recent backups
ls -lt ~/.claude/settings.json.backup.* | head -5

# Restore from specific backup
cp ~/.claude/settings.json.backup.1739673234 ~/.claude/settings.json
```

## Troubleshooting

### Problem: "Hook validation failed"

**Symptom:** Error message listing missing hooks during installation

**Cause:** Required hook files are missing from `~/.amplihack/.claude/tools/amplihack/hooks/`

**Solution:**

```bash
# Reinstall amplihack to restore hooks
amplihack install
```

### Problem: Hooks not executing

**Symptom:** Session start hooks or tool hooks don't run

**Cause:** Settings.json not properly configured

**Solution:**

```bash
# Reconfigure settings
amplihack install  # This will reconfigure hooks automatically
```

### Problem: XPIA hooks warning

**Symptom:** `❌ session_start.py missing`, `❌ post_tool_use.py missing`, or `❌ pre_tool_use.py missing` during `amplihack install` verification, even though XPIA hook files exist on disk.

**Cause:** XPIA security hooks are optional. The verification step checks for `session_start.py`, `post_tool_use.py`, and `pre_tool_use.py` inside the XPIA hooks directory. If you see `❌` markers but the files exist, confirm you are running amplihack ≥ the version that includes the issue #683 fix (earlier versions checked for `.sh` extensions instead of `.py`, producing false negatives).

**Solution:**

- If XPIA is not installed: this is normal — the `ℹ️` message confirms XPIA is absent and install still succeeds.
- If XPIA is installed but shows `❌`: update amplihack to pick up the corrected `.py` file-extension check.
- To install XPIA: follow the [XPIA installation instructions](../claude/commands/amplihack/xpia.md).

> **Historical note (issue #683):** Before the fix, `XPIA_HOOK_FILES` in `types.rs` listed `.sh` extensions while the actual deployed hooks use `.py`. This caused verification to report all three XPIA hooks as missing even when they were correctly installed. The fix changed the constant to use `.py` extensions.

## Advanced: Path Expansion

Hook paths support environment variable expansion:

- `$HOME` expands to your home directory
- `~` expands to your home directory
- Other environment variables are expanded automatically

Example:

```
$HOME/.amplihack/.claude/tools/amplihack/hooks/session_start.py
→ /home/username/.amplihack/.claude/tools/amplihack/hooks/session_start.py
```

## For Developers

### validate_hook_paths() Function

New in v0.5.27, this function validates hook files exist before configuration:

```rust
// use amplihack_settings:: validate_hook_paths

hooks = [
    {"type": "SessionStart", "file": "session_start.py", "timeout": 10}
]

all_valid, missing = validate_hook_paths(
    "amplihack",
    hooks,
    "~/.amplihack/.claude/tools/amplihack/hooks"
)

if not all_valid:
    print(f"Missing hooks: {missing}")
```

**Returns:**

- `all_valid` (bool): True if all hooks exist
- `missing` (list): List of missing hook descriptions with expected paths

### Running Tests

```bash
# Run hook validation tests
python -m unittest tests.unit.test_validate_hook_paths

# Run all settings tests
python -m unittest discover -s tests -p "test_*settings*.py"
```

## Related Documentation

- [How to Configure the Copilot Parity Control Plane](./configure-copilot-parity-control-plane.md)
- [Copilot Parity Control Plane Reference](../reference/hook-specifications.md)
- Main README: Setup and installation guide
- PHILOSOPHY.md: Zero-BS principle and validation approach
- Settings migration: See `src/amplihack/settings.py` docstrings
