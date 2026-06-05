# How to Troubleshoot Hook Issues

Quick guide to diagnose and fix amplihack hook problems.

## Quick Checks

### 1. Verify Hooks Are Installed

```bash
# Check if hooks exist in settings.json
cat ~/.claude/settings.json | grep -A5 hooks

# Expected output: Should show SessionStart, Stop, and PostToolUse hooks
```

### 2. Verify Hook Runtime Exists

```bash
# Check if the native hook runner is installed
which amplihack-hooks

# Check if the bundle hook directory resolves for metadata/parity checks
amplihack resolve-bundle-asset hooks-dir
```

### 3. Check Hook Execution

Start Claude Code and look for hook execution messages in the output:

```
✓ SessionStart [/home/user/.local/bin/amplihack-hooks session-start] completed
```

## Common Issues and Solutions

### Issue 1: Hooks Disappear After Exit (Pre-v0.9.1)

**Symptom:**

- Hooks work on first launch
- After exiting Claude, hooks are missing
- Must re-run `amplihack install` every time

**Cause:** Bug in versions before 0.9.1 where SettingsManager restored old settings without hooks.

**Solution:**

Upgrade to v0.9.1 or later:

```bash
cargo install amplihack-rs amplihack --version
```

### Issue 2: "Hook not found" Error

**Symptom:**

```
⏺ Stop [amplihack-hooks stop] failed with non-blocking status code 127
```

**Cause:** The `amplihack-hooks` path is incorrect, relative, or missing.

**Solution:**

Check and fix hook paths in ~/.claude/settings.json:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/home/user/.local/bin/amplihack-hooks stop"
          }
        ]
      }
    ]
  }
}
```

**Automatic Fix:** Rerun `amplihack install` to rewrite hook commands with the
current absolute `amplihack-hooks` path.

### Issue 3: Hooks Not Running At All

**Symptom:**

- No hook execution messages
- amplihack features don't work
- No errors shown

**Cause:** Project-level settings.json is overriding global hooks.

**Diagnosis:**

```bash
# Check if project has its own settings
cat .claude/settings.json | grep hooks

# If this shows hooks, they're overriding global ones
```

**Solution:**

Manually merge amplihack hooks into project settings. See [Hook Configuration Guide](configure-hooks.md) for complete instructions.

### Issue 4: Permission Denied Errors

**Symptom:**

```
⏺ SessionStart failed: Permission denied
```

**Cause:** The `amplihack-hooks` binary is not executable.

**Solution:**

```bash
# Reinstall to restore executable permissions
amplihack install
```

### Issue 5: Hook Timeout

**Symptom:**

```
⏺ Stop hook timed out after 120s
```

**Cause:** Stop hook is performing long-running operations.

**Solution:**

This is usually normal for the Stop hook, which captures session artifacts. If it consistently times out:

1. Check disk space: `df -h ~/.claude/runtime/logs/`
2. Check for stuck processes: `ps aux | grep amplihack`
3. Review logs: `tail ~/.claude/runtime/logs/latest/stop_hook.log`

## Diagnostic Commands

### Full Hook Status Check

```bash
# Create diagnostic script
cat > /tmp/check_hooks.sh << 'EOF'
#!/bin/bash

echo "=== Hook Configuration Diagnostic ==="
echo ""

echo "1. Global Settings Hooks:"
grep -A10 hooks ~/.claude/settings.json 2>/dev/null || echo "   No global hooks found"
echo ""

echo "2. Project Settings Hooks:"
grep -A10 hooks .claude/settings.json 2>/dev/null || echo "   No project hooks found"
echo ""

echo "3. Hook Runtime:"
which amplihack-hooks 2>/dev/null || echo "   amplihack-hooks not found"
amplihack resolve-bundle-asset hooks-dir 2>/dev/null || echo "   hooks-dir not found"
echo ""

echo "4. Recent Hook Execution (from logs):"
tail -20 ~/.claude/runtime/logs/latest/session.log 2>/dev/null | grep -i hook || echo "   No recent hook logs"
echo ""

echo "5. Amplihack Version:"
cargo install amplihack-rs amplihack --version 2>/dev/null || echo "   Cannot determine version"

EOF

chmod +x /tmp/check_hooks.sh
/tmp/check_hooks.sh
```

### Watch Hook Execution Live

```bash
# Monitor logs during Claude session
tail -f ~/.claude/runtime/logs/latest/*.log
```

## Getting Help

If hooks still aren't working after trying these solutions:

1. **Collect diagnostic information:**

   ```bash
   /tmp/check_hooks.sh > /tmp/hook_diagnostic.txt
   ```

2. **Check existing issues:**

   [amplihack/issues](https://github.com/rysweet/amplihack-rs/issues?q=is%3Aissue+hook)

3. **File a new issue:**

   Include:
   - Output from diagnostic script
   - amplihack version
   - Operating system and version
   - Steps to reproduce

## Related Documentation

- [Hook Configuration Guide](configure-hooks.md) - Complete configuration reference
- [Changelog v0.9.1](../features/power-steering/changelog-v0.9.1.md) - Hook persistence fix details
