# Bootstrap Parity

## What Is Bootstrap Parity?

Bootstrap parity means the Rust CLI's `amplihack install` command performs the same first-install setup that the Python installer previously required: validating the Python environment, deploying native binaries, and registering all Claude Code hooks — without any separate manual steps.

Before bootstrap parity, a user installing amplihack-rs had to:

1. Run `amplihack install` (staged framework assets, wired hooks using Python script paths)
2. Separately figure out that `amplihack-hooks` also needed to be on `PATH`
3. Separately understand that hook commands needed to reference the Rust binary, not the Python scripts

After bootstrap parity, step 1 is the only step. The Rust installer mirrors what the Python `amplihack install` command always did: hand you a working system.

## Why This Matters

Claude Code executes hook commands as subprocesses. If the hook command string in `settings.json` points to a binary that does not exist, the hook fails silently (fail-open behavior for `PreToolUse`/`PostToolUse`) or fails with an error (other events). Either way, amplihack features are silently degraded.

The Python installer avoided this problem by deploying everything it needed before writing `settings.json`. The Rust installer must do the same.

## What Changed

### Before: Hook Commands Used Python Paths

Before bootstrap parity, the Rust installer inherited Python-style hook command strings:

```json
{
  "hooks": {
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "/home/alice/.amplihack/.claude/tools/amplihack/hooks/session_start.py",
        "timeout": 10
      }]
    }]
  }
}
```

This worked if Python was installed and the amplihack package was importable. It broke silently if either was missing.

### After: Hook Commands Use the Native Binary

With bootstrap parity, hook commands use the `amplihack-hooks` binary for all hooks that have Rust implementations:

```json
{
  "hooks": {
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "/home/alice/.local/bin/amplihack-hooks session-start",
        "timeout": 10
      }]
    }]
  }
}
```

The `workflow_classification_reminder.py` hook is an exception — it remains a Python file because it has no Rust implementation and is invoked as a direct Python script, not via `amplihack-hooks`.

## The Bootstrap Sequence

The install sequence is ordered to prevent the "missing dependency at runtime" failure mode:

```
validate_python()           ← fail fast if Python or amplihack SDK is missing
deploy_binaries()           ← place amplihack-hooks at ~/.local/bin BEFORE writing settings
ensure_settings_json()      ← now safe to write absolute path to amplihack-hooks
verify_hooks()              ← confirm all scripts are staged
write_manifest()            ← record what was deployed for uninstall
```

Python validation runs first because Python is required at hook runtime (for `workflow_classification_reminder.py` and the Python bridge scripts). Deploying binaries before writing `settings.json` ensures that the path written into `settings.json` already exists on disk.

## Relationship to the Python Installer

The Python installer (`amplihack.install`) defines `HOOK_CONFIGS` — a list of 6 hook registrations. The Rust implementation defines `AMPLIHACK_HOOK_SPECS` — 7 specs (6 hook registrations, with `UserPromptSubmit` split into two entries). The counts match because `SessionStop` is not a registered hook; it exists as infrastructure for Copilot but is not in `~/.claude/settings.json`.

Both installers write the same logical hooks. The difference is that the Rust installer:

1. Validates Python before writing anything
2. Deploys the `amplihack-hooks` binary
3. Uses binary subcommand format (`amplihack-hooks session-start`) instead of Python script paths for the Rust-implemented hooks

## See Also

- [Idempotent Installation](./idempotent-installation.md) — how repeated installs are safe
- [Hook Specifications](../reference/hook-specifications.md) — the canonical 7-hook table
- [Install from a Local Repository](../howto/local-install.md) — offline install workflow
