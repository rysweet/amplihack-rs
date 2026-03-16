# Install Manifest Reference

The install manifest is a JSON file written by `amplihack install` that records every path the installer created or modified. `amplihack uninstall` reads this file to know exactly what to remove.

## Location

```
~/.amplihack/.claude/install/amplihack-manifest.json
```

## Schema

```json
{
  "files": ["string"],
  "dirs": ["string"],
  "binaries": ["string"],
  "hook_registrations": ["string"]
}
```

All four fields are required in new manifests. All four have `#[serde(default)]` so old manifests without `binaries` or `hook_registrations` are read without error (those fields default to empty arrays).

### `files` — `Vec<String>`

Relative paths (from `~/.amplihack/.claude/`) of every file staged by the installer. Written as POSIX relative paths, e.g.:

```json
"files": [
  "AMPLIHACK.md",
  "agents/amplihack/some-agent.md",
  "commands/amplihack/analyze.md",
  "tools/statusline.sh"
]
```

Fresh native installs do not stage `tools/amplihack/hooks/*.py`. Hook execution is registered via `amplihack-hooks <subcommand>` and tracked separately in the `hook_registrations` field. Older manifests from pre-port installs may still contain legacy Python hook paths; uninstall continues to tolerate those historical entries.

During uninstall, each path is resolved as `~/.amplihack/.claude/<relative>`. Paths that resolve outside this base directory are rejected.

### `dirs` — `Vec<String>`

Relative paths (from `~/.amplihack/.claude/`) of every directory created by the installer. Uninstall removes them deepest-first to avoid `rmdir: Directory not empty` errors.

```json
"dirs": [
  "runtime/security",
  "runtime/analysis",
  "runtime/metrics",
  "runtime/logs",
  "runtime",
  "tools/amplihack",
  "agents/amplihack",
  "commands/amplihack",
  "install"
]
```

### `binaries` — `Vec<String>`

Absolute paths of every binary deployed to `~/.local/bin/`:

```json
"binaries": [
  "/home/alice/.local/bin/amplihack",
  "/home/alice/.local/bin/amplihack-hooks"
]
```

Uninstall validates that each path is under `~/.local/bin/` before deleting.

### `hook_registrations` — `Vec<String>`

Deduplicated list of Claude Code event names for which amplihack registered hooks. Used by uninstall to target the correct event arrays in `settings.json`.

```json
"hook_registrations": [
  "SessionStart",
  "Stop",
  "PreToolUse",
  "PostToolUse",
  "UserPromptSubmit",
  "PreCompact"
]
```

## Full Example

```json
{
  "files": [
    "AMPLIHACK.md",
    "agents/amplihack/some-agent.md",
    "commands/amplihack/analyze.md",
    "tools/statusline.sh",
    "install/amplihack-manifest.json"
  ],
  "dirs": [
    "runtime/security",
    "runtime/analysis",
    "runtime/metrics",
    "runtime/logs",
    "runtime",
    "tools/amplihack",
    "agents/amplihack",
    "commands/amplihack",
    "install"
  ],
  "binaries": [
    "/home/alice/.local/bin/amplihack",
    "/home/alice/.local/bin/amplihack-hooks"
  ],
  "hook_registrations": [
    "SessionStart",
    "Stop",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "PreCompact"
  ]
}
```

## Backup Metadata

In addition to the manifest, the installer writes a backup metadata file when it backs up an existing `settings.json`:

```
~/.amplihack/.claude/runtime/sessions/install_<unix_seconds>_backup.json
```

```json
{
  "timestamp": 1741651200,
  "backup_path": "/home/alice/.claude/settings.json.backup.1741651200",
  "original_path": "/home/alice/.claude/settings.json"
}
```

Backup files and backup metadata files are created with `0o600` permissions (owner read/write only). They are not world-readable — on a multi-user system this prevents other users from reading your `settings.json` backup, which contains your full tool allow-list and hook commands.

These metadata files are informational. They are not read by uninstall and are not removed by `amplihack uninstall`.

## Backward Compatibility

The manifest format is backward compatible. Old manifests with only `files` and `dirs` are read successfully — `binaries` and `hook_registrations` default to empty arrays. Uninstall skips phases 3 and 4 if the corresponding fields are empty.

## See Also

- [How to Uninstall amplihack](../howto/uninstall.md) — how the manifest is used
- [amplihack install reference](./install-command.md) — when and how the manifest is written
