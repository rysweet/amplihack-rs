# Bootstrap Parity

## What Is Bootstrap Parity?

Bootstrap parity means the Rust CLI's `amplihack install` command performs the same first-install setup that the Python installer previously required: validating the Python environment, deploying native binaries, and registering all Claude Code hooks — without any separate manual steps.

Before bootstrap parity, a user installing amplihack-rs had to:

1. Run `amplihack install` (staged framework assets, wired hooks using Python script paths)
2. Separately figure out that `amplihack-hooks` also needed to be on `PATH`
3. Separately understand that hook commands needed to reference the Rust binary, not the Python scripts
4. Still rely on Python-only helpers such as bundle asset resolution during downstream tool execution

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

The workflow-classification reminder is now implemented in Rust and dispatched via `amplihack-hooks workflow-classification-reminder`, preserving the earlier 5-second budget while removing that Python hook from runtime execution.

Fresh native installs no longer need staged `tools/amplihack/hooks/*.py` assets. Reinstall still recognizes those historical Python command paths so older settings files are upgraded in place to the native binary format.

Bundle asset resolution also now has a native path: the Rust installer can deploy `amplihack-asset-resolver`, and launched tools / `amplihack recipe run` now receive `AMPLIHACK_ASSET_RESOLVER` pointing at that binary when it is available.

`session-start` has also shed its Python-only behaviors: version mismatch checks and stale global-hook migration now run natively in Rust, and the stale session-start memory bridge has been removed.

`user-prompt-submit` now performs agent-memory lookup natively in Rust as well, using the existing Rust memory-store readers instead of shelling out through a Python bridge.

`session-stop` now follows the same pattern: instead of delegating to a stale Python `MemoryCoordinator.store()` bridge, the Rust hook derives session-end learnings from explicit hook payload fields or transcript JSONL and writes them through the native SQLite / LadybugDB memory backends.

`stop/reflection` is native on the Rust side too: transcript parsing, repository-context detection, redirect-history loading, and reflection-prompt assembly now happen in Rust, and the hook invokes headless Claude CLI directly instead of bouncing through a Python SDK bridge.

## The Bootstrap Sequence

The install sequence is ordered to prevent the "missing dependency at runtime" failure mode:

```
deploy_binaries()           ← place amplihack-hooks (+ asset resolver when available) at ~/.local/bin BEFORE writing settings
ensure_settings_json()      ← now safe to write absolute path to amplihack-hooks
verify_framework_assets()   ← confirm required staged framework assets exist
write_manifest()            ← record what was deployed for uninstall
```

The install path is now fully native for the live runtime. `deploy_binaries()` still happens before writing `settings.json` so the hook binary path already exists on disk when registration occurs.

## Relationship to the Python Installer

The Python installer (`amplihack.install`) defines `HOOK_CONFIGS` — a list of 6 hook registrations. The Rust implementation defines `AMPLIHACK_HOOK_SPECS` — 7 specs (6 hook registrations, with `UserPromptSubmit` split into two entries). The counts match because `SessionStop` is not a registered hook; it exists as infrastructure for Copilot but is not in `~/.claude/settings.json`.

Both installers write the same logical hooks. The difference is that the Rust installer:

1. Validates Python before writing anything
2. Deploys the `amplihack-hooks` binary
3. Deploys `amplihack-asset-resolver` when present
4. Uses binary subcommand format (`amplihack-hooks session-start`) instead of Python script paths for the Rust-implemented hooks

## See Also

- [Idempotent Installation](./idempotent-installation.md) — how repeated installs are safe
- [Hook Specifications](../reference/hook-specifications.md) — the canonical 7-hook table
- [Install from a Local Repository](../howto/local-install.md) — offline install workflow
