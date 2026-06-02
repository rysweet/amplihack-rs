# How-To: Settings Hook Configuration

## Overview

Amplihack automatically configures hooks in `~/.claude/settings.json` during installation and CLI initialization. This guide explains the hook configuration process and how to troubleshoot issues.

## Automatic Configuration

Hook configuration is **completely automatic** — no user prompts required.

The `amplihack-hooks` binary provides all hook functionality as native subcommands. There are no Python or shell script hook files to install, validate, or maintain. Hook registration writes `amplihack-hooks <subcommand>` entries directly into `settings.json`.

### What Happens Automatically

When you run `amplihack` or install the framework:

1. **Binary resolution**: The installer locates the `amplihack-hooks` binary (see [Binary Resolution](../reference/binary-resolution.md))
2. **Backup**: Settings are backed up to `~/.claude/settings.json.backup.<timestamp>`
3. **Registration**: Hook subcommand entries are added/updated in settings.json
4. **Verification**: System confirms hooks are properly configured

**No user intervention required!**

## Hook Architecture

### Rust-Native Hooks

All hooks are implemented as subcommands of the compiled `amplihack-hooks` binary. The installer registers entries like:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "type": "command",
        "command": "amplihack-hooks session-start"
      }
    ],
    "PostToolUse": [
      {
        "type": "command",
        "command": "amplihack-hooks post-tool-use"
      }
    ]
  }
}
```

**Hook Subcommands:**

| Subcommand | Event | Purpose |
|---|---|---|
| `session-start` | SessionStart | Session initialization and context setup |
| `pre-tool-use` | PreToolUse | Pre-tool analysis (registered when XPIA assets present) |
| `post-tool-use` | PostToolUse | Post-tool analysis and workflow enforcement |
| `stop` | Stop | Session cleanup and metrics flush |
| `pre-compact` | PreCompact | Context preservation before compaction |

**XPIA security behavior:** When the `tools/xpia` directory is present in the staged framework assets, the `session-start`, `pre-tool-use`, and `post-tool-use` subcommands additionally activate XPIA defense logic (implemented in `amplihack-security::XpiaDefender`). No extra hook entries are added — the same `amplihack-hooks <subcommand>` invocation handles both framework and security duties. The `pre-tool-use` subcommand is only registered when XPIA assets are present.

### What Is Validated

The installer validates:

- The `amplihack-hooks` binary exists and is executable
- Hook command strings contain no shell metacharacters (injection prevention)
- Each hook entry is syntactically correct before writing to settings.json

**No per-file hook script validation occurs** — all hook logic lives in the compiled binary.

### Error Messages

If the hooks binary is not found:

```
❌ amplihack-hooks binary not found
💡 Please reinstall amplihack to restore the hooks binary
```

**Resolution:** Run `amplihack install` to redeploy the binary.

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

### Problem: Hooks binary not found

**Symptom:** Error message about missing `amplihack-hooks` during installation

**Cause:** The `amplihack-hooks` binary is not in any of the 5 resolution locations

**Solution:**

```bash
# Reinstall amplihack to redeploy the binary
amplihack install
```

### Problem: Hooks not executing

**Symptom:** Session start hooks or tool hooks don't run

**Cause:** Settings.json not properly configured, or `amplihack-hooks` not on `$PATH`

**Solution:**

```bash
# Verify the binary is accessible
which amplihack-hooks

# Reconfigure settings
amplihack install  # This will reconfigure hooks automatically
```

### Problem: XPIA hooks directory info message

**Symptom:** Informational message that XPIA security hooks directory is not installed

**Cause:** The `tools/xpia` directory was not present in the staged framework assets

**Solution:**

- This is normal if your framework bundle does not include XPIA assets
- XPIA security is an optional feature — the info message is not an error
- When XPIA assets are present, the installer automatically registers the XPIA hook subcommands via the `amplihack-hooks` binary

## Advanced: Path Expansion

Hook command paths support environment variable expansion:

- `$HOME` expands to your home directory
- `~` expands to your home directory
- Other environment variables are expanded automatically

The `amplihack-hooks` binary is typically deployed to `~/.local/bin/amplihack-hooks` and found via `$PATH`.

## For Developers

### Hook Registration Internals

Hook specifications are defined in `crates/amplihack-cli/src/commands/install/types.rs`:

- `AMPLIHACK_HOOK_SPECS` — required amplihack hook subcommands
- `XPIA_HOOK_SPECS` — optional XPIA security hook subcommands

Both use `HookCommandKind::BinarySubcmd` to generate `"amplihack-hooks <subcmd>"` command strings. The installer calls `validate_hook_command_string()` on each generated command before writing to settings.json.

### Running Tests

```bash
# Run settings-related tests
cargo test -p amplihack-cli -- settings

# Run all install tests
cargo test -p amplihack-cli -- install
```

## Related Documentation

- [How to Configure the Copilot Parity Control Plane](./configure-copilot-parity-control-plane.md)
- [Copilot Parity Control Plane Reference](../reference/hook-specifications.md)
- [Binary Resolution](../reference/binary-resolution.md)
- Main README: Setup and installation guide
- PHILOSOPHY.md: Zero-BS principle and validation approach
