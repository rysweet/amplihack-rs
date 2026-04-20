# Idempotent Installation

Running `amplihack install` more than once is safe. The second and subsequent runs update the installation in place — they do not create duplicate hook entries, do not overwrite unrelated settings, and do not corrupt the manifest.

## How Idempotency Works

### Hook Registrations

When `update_hook_paths()` processes an `AMPLIHACK_HOOK_SPECS` entry, it checks whether a matching wrapper already exists in the `settings.json` hook array before writing:

```
For each hook spec:
  if wrapper_matches(existing_entry, spec):
    replace existing entry in place   ← update, not duplicate
  else:
    append new entry to hook array    ← first install
```

The match is type-directed, based on the `HookCommandKind` of the spec:

| Kind | Match condition |
|------|----------------|
| `BinarySubcmd` | `command` contains both `amplihack-hooks` (the filename) and the subcommand string (e.g., `session-start`), or it is a legacy `tools/amplihack/hooks/*.py` path for the same hook |

Matching by semantic identity — not by full absolute path — means that moving the binary (e.g., during a `deploy_binaries()` run that updates `~/.local/bin`) causes the existing entry to be updated with the new path rather than a second entry being appended.

The `BinarySubcmd` match requires **both** the `amplihack-hooks` filename _and_ the specific subcommand argument (e.g., `session-start`). A user hook command that happens to contain the string `amplihack-hooks` but uses a different subcommand will not be matched and will not be modified.

### UserPromptSubmit Ordering

The two `UserPromptSubmit` entries have a required order:

1. `amplihack-hooks workflow-classification-reminder` (timeout 5 s)
2. `amplihack-hooks user-prompt-submit` (timeout 10 s)

On an idempotent run, each entry is matched and replaced in place, which preserves the original insertion position. Claude Code executes `UserPromptSubmit` hooks in array order, so this ordering guarantee ensures the reminder fires before the preference injection.

### Binary Deployment

`deploy_binaries()` checks ownership before overwriting an existing binary at `~/.local/bin/amplihack-hooks` or `~/.local/bin/amplihack`. A file owned by the current user is overwritten. A file owned by another user causes an error (to prevent privilege escalation via binary replacement).

### Manifest

Each install writes a fresh manifest, replacing the previous one. The manifest always reflects the state after the most recent successful install.

## What Is Never Duplicated

- Hook array entries (matched and replaced in place)
- Allowed tools in `settings.json.permissions.allow`
- Additional directories in `settings.json.additionalDirectories`
- Files in `~/.amplihack/.claude/` (overwritten by `copy_dir_recursive`)
- Runtime directories (already exist, `create_dir_all` is a no-op)

## Upgrading

Running `amplihack install` after updating the amplihack-rs binary is the upgrade mechanism. Framework assets are now bundled in the amplihack-rs source tree (issue #254), so updating the binary automatically updates the framework:

```sh
# Update the binary (framework assets update automatically)
cargo install --git https://github.com/rysweet/amplihack-rs amplihack-cli

# Re-install to stage updated assets and update hook paths
amplihack install
```

Or from a local checkout:

```sh
cd ~/src/amplihack-rs
git pull
cargo build --release
amplihack install --local .
```

Hook command strings in `settings.json` are updated to point to the newly deployed binary.

## Example: Idempotent Run Output

The second run of `amplihack install` produces output identical to the first, but each phase reports "updated" rather than "created":

```
✓ Python 3.11.4 detected
✓ amplihack Python package detected
✓ Using bundled framework assets...
✓ Updated amplihack → ~/.local/bin/amplihack
✓ Updated amplihack-hooks → ~/.local/bin/amplihack-hooks
✓ Staged framework assets (47 files, 12 directories)
✓ Runtime directories already exist
✓ Backed up ~/.claude/settings.json → settings.json.backup.1741651200
✓ Updated 7 Claude Code hook registrations (in place)
✓ Verified staged framework assets
✓ Updated install manifest
amplihack installed successfully.
```

## See Also

- [Bootstrap Parity](./bootstrap-parity.md) — why the install sequence is ordered the way it is
- [Hook Specifications](../reference/hook-specifications.md) — the 7 hooks and their idempotency matching rules
- [amplihack install reference](../reference/install-command.md) — full command reference
