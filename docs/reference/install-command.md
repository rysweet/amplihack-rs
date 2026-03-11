# amplihack install / uninstall — Command Reference

## Synopsis

```
amplihack install [--local <PATH>]
amplihack uninstall
```

## amplihack install

Bootstraps the amplihack environment on the current machine. On first run, it performs full setup: validates Python, deploys native binaries, stages framework assets, and registers Claude Code hooks. Subsequent runs are idempotent — they update existing registrations in place without duplication.

### Options

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--local <PATH>` | `PathBuf` | absent | Install from a local directory instead of cloning from GitHub. The path must exist, be a directory, and contain a `.claude` subdirectory (at `<PATH>/.claude` or `<PATH>/../.claude`). |

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Install completed successfully |
| `1` | Python validation failed (see stderr for details) |
| `1` | `amplihack-hooks` binary not found after 5-step search |
| `1` | `--local` path does not exist or is not a directory |
| `1` | `--local` path does not contain a `.claude` directory |
| `1` | Git clone failed (non-local mode only) |

### Install Phases

```
amplihack install
│
├── 1. validate_python()          — python3 present AND 'import amplihack' works
├── 2. git clone OR --local path  — obtain framework source
├── 3. deploy_binaries()          — copy amplihack + amplihack-hooks to ~/.local/bin
├── 4. copy framework assets      — stage .claude/ tree to ~/.amplihack/.claude/
├── 5. create_runtime_dirs()      — create runtime/ subdirs with 0o755 permissions
├── 6. ensure_settings_json()     — backup settings.json, register hooks, set permissions
├── 7. verify_hooks()             — confirm all 7 hook scripts exist on disk
└── 8. write_manifest()           — write amplihack-manifest.json for uninstall
```

Phase 1 (Python validation) is fail-fast: if it fails, no changes are made to disk.

### Environment Variables

These variables are read during install. All are optional; the installer works without any of them set.

| Variable | Effect |
|----------|--------|
| `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` | Override the path used for `amplihack-hooks`. Useful in tests and CI. If set but the path does not exist, resolution falls through to Step 2. See [Binary Resolution](./binary-resolution.md). |
| `AMPLIHACK_HOME` | Override `~/.amplihack` staging root (default: `$HOME/.amplihack`). |

### Output

Successful install prints a phase-by-phase progress summary:

```
✓ Python 3.11.4 detected
✓ amplihack Python package detected
✓ Cloning from https://github.com/rysweet/amplihack...
✓ Deployed amplihack → ~/.local/bin/amplihack
✓ Deployed amplihack-hooks → ~/.local/bin/amplihack-hooks
✓ Staged framework assets (47 files, 12 directories)
✓ Created runtime directories
✓ Backed up ~/.claude/settings.json → settings.json.backup.1741651200
✓ Registered 7 Claude Code hooks
✓ Verified hook scripts
✓ Wrote install manifest
amplihack installed successfully.
```

If `~/.local/bin` is not in `$PATH`, an advisory is printed (install still succeeds):

```
⚠️  ~/.local/bin is not in $PATH
    Add: export PATH="$HOME/.local/bin:$PATH"
```

---

## amplihack uninstall

Removes all files, directories, binaries, and hook registrations tracked by the install manifest.

### Options

None.

### Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Uninstall completed (or nothing to remove) |
| `1` | A listed path resolved outside an allowed base directory (security check) |

### Uninstall Phases

```
amplihack uninstall
│
├── Read ~/.amplihack/.claude/install/amplihack-manifest.json
│   (if missing: warn and use hardcoded fallback list)
│
├── Phase 1 — Remove manifest files
│   └── validates each path is under ~/.amplihack/.claude/
│
├── Phase 2 — Remove manifest directories (deepest first)
│   └── validates each path is under ~/.amplihack/.claude/
│
├── Phase 3 — Remove manifest binaries
│   └── validates each path is under ~/.local/bin/
│
└── Phase 4 — Remove hook registrations from ~/.claude/settings.json
    └── retains XPIA and non-amplihack entries
```

### What Uninstall Preserves

- `~/.claude/settings.json` (the file itself — only amplihack entries are removed)
- `~/.claude/settings.json.backup.*` files
- XPIA hook registrations
- Any other tool's hook registrations

---

## Internal API (for contributors)

The functions below are in `crates/amplihack-cli/src/commands/install.rs`.

### `run_install(local: Option<PathBuf>) -> Result<()>`

Entry point called by the command dispatcher. Canonicalizes and validates `--local` path when provided. Delegates to `local_install()` directly (local mode) or after `git clone` (network mode).

### `run_uninstall() -> Result<()>`

Entry point for uninstall. Reads the manifest, then executes phases 1–4.

### `local_install(repo_root: &Path) -> Result<()>`

Core install logic. Runs all phases from Python validation through manifest writing. Calls `find_hooks_binary()` to locate `amplihack-hooks` before wiring hooks.

### `find_hooks_binary() -> Result<PathBuf>`

Locates the `amplihack-hooks` binary using a 5-step resolution:

1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` environment variable (must point to an existing executable)
2. Sibling of the currently running `amplihack` executable
3. `~/.local/bin/amplihack-hooks`
4. `~/.cargo/bin/amplihack-hooks`
5. `PATH` lookup via `which amplihack-hooks`

Returns an actionable error if none of the five locations yield an executable.

### `deploy_binaries(hooks_bin: &Path) -> Result<Vec<PathBuf>>`

Copies `amplihack` (current executable) and `amplihack-hooks` (resolved by `find_hooks_binary`) to `~/.local/bin` with `0o755` permissions. Checks file ownership before overwriting an existing file. Returns the list of deployed paths for inclusion in the manifest.

> **macOS note:** On macOS with System Integrity Protection (SIP) active, copying the running executable to `~/.local/bin` may produce a quarantined binary. See the [First-time install how-to](../howto/first-install.md#macos-sip-note) for the resolution step.

### `validate_python() -> Result<()>`

Runs two subprocess checks with hardcoded argument arrays (no shell interpolation):

```
python3 --version
python3 -c "import amplihack"
```

Exits with code 1 and a `❌` diagnostic message if either fails.

### `ensure_settings_json(staging_dir: &Path, timestamp: u64, hooks_bin: &Path) -> Result<(bool, Vec<String>)>`

Reads or creates `~/.claude/settings.json`. Creates a timestamped backup and backup metadata JSON (both with `0o600` permissions). Calls `validate_hook_command_string()` on each command before writing. Calls `update_hook_paths()` for amplihack hooks and (if XPIA is installed) for XPIA hooks. Returns `(settings_existed, registered_event_names)`.

### `validate_hook_command_string(cmd: &str) -> Result<()>`

Validates that a hook command string does not contain shell metacharacters (`|&;$\`(){}<!>#~*\`). Called by `ensure_settings_json()` and `update_hook_paths()` before any write to `settings.json`. Returns an error with the offending string identified if validation fails.

### `update_hook_paths(settings, hook_system, specs, hooks_dir, hooks_bin)`

Iterates `specs` and calls `validate_hook_command_string()` on each command string before upserting its hook wrapper into `settings["hooks"][event]`. Uses `wrapper_matches()` for idempotency. Preserves order — `workflow_classification_reminder.py` always precedes `user-prompt-submit` in the `UserPromptSubmit` array.

### `remove_hook_registrations(settings) -> Result<()>`

Removes hook array entries whose command string contains `amplihack-hooks` or `tools/amplihack/`. Preserves all other entries.

## See Also

- [Hook Specifications](./hook-specifications.md) — the 7 hooks registered by amplihack install
- [Install Manifest](./install-manifest.md) — manifest schema
- [Binary Resolution](./binary-resolution.md) — find_hooks_binary lookup detail
