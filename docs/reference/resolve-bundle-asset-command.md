# amplihack resolve-bundle-asset — Command Reference

## Synopsis

```
amplihack resolve-bundle-asset <ASSET>
```

## Description

Resolves a named bundle asset or a relative path under `amplifier-bundle/` to
an absolute filesystem path. Prints the resolved path to stdout on success.

This command replaces `amplihack runtime_assets` in recipe shell
steps, providing the same path-resolution logic without a Python dependency.

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<ASSET>` | yes | A named asset key (e.g. `multitask-orchestrator`) or a relative path starting with `amplifier-bundle/`. |

### Named Assets

| Name | Resolves to |
|------|-------------|
| `hooks-dir` | `amplifier-bundle/tools/amplihack/hooks/` |
| `helper-path` | `amplifier-bundle/tools/orch_helper.py` (fallback: `amplifier-bundle/tools/amplihack/orch_helper.py`) |
| `multitask-orchestrator` | `amplifier-bundle/bin/multitask-orchestrator.sh` |

`hooks-dir` and `helper-path` were re-registered in issue #614 to restore
compatibility with `smart-orchestrator.yaml` preflight steps (lines 58, 74)
which resolve these assets during recipe startup. `helper-path` tries two
candidate paths in order and returns the first that exists on disk.

### Relative Paths

When `<ASSET>` is not a named key, it is treated as a relative path. The path
must:

- Start with `amplifier-bundle/`
- Not contain `.` or `..` segments
- Not be absolute (no leading `/` or `~`)
- Contain only safe characters: `A-Z a-z 0-9 _ - . /`

## Examples

```sh
# Resolve a named asset
amplihack resolve-bundle-asset multitask-orchestrator
# Output: /home/user/.amplihack/amplifier-bundle/bin/multitask-orchestrator.sh

# Resolve the hooks directory (used by smart-orchestrator preflight)
amplihack resolve-bundle-asset hooks-dir
# Output: /home/user/.amplihack/amplifier-bundle/tools/amplihack/hooks

# Resolve the orchestrator helper script
amplihack resolve-bundle-asset helper-path
# Output: /home/user/.amplihack/amplifier-bundle/tools/orch_helper.py

# Resolve a relative path under amplifier-bundle/
amplihack resolve-bundle-asset amplifier-bundle/tools/statusline.sh
# Output: /home/user/.amplihack/amplifier-bundle/tools/statusline.sh

# Attempt to resolve an unknown named asset
amplihack resolve-bundle-asset nonexistent-thing
# Stderr: ERROR: Unknown asset name "nonexistent-thing". Expected one of: hooks-dir, helper-path, multitask-orchestrator
# Exit: 1
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Asset resolved — absolute path printed to stdout |
| `1` | Asset not found — the named key or relative path does not exist on disk, **or** the argument is an unregistered named asset (no `/` and not in the named-asset table) |
| `2` | Invalid input — the relative path failed validation (null bytes, unsafe characters, path traversal, missing `amplifier-bundle/` prefix) |

### Unregistered Named Assets

If `<ASSET>` contains no `/` and is not a registered named asset key, the
command returns exit code **1** (not found) with a diagnostic listing valid
named assets. This prevents unknown names from falling through to
relative-path validation — which would reject them with exit 2 because they
lack the `amplifier-bundle/` prefix.

```sh
$ amplihack resolve-bundle-asset unknown-asset
ERROR: Unknown asset name "unknown-asset". Expected one of: hooks-dir, helper-path, multitask-orchestrator
$ echo $?
1
```

This distinction matters for recipes that use `|| true` guards: exit 1
(not found) is a normal condition; exit 2 (invalid input) indicates a bug.

## Security Constraints

- Path traversal (`..`) is rejected at validation time.
- Only characters in `A-Z a-z 0-9 _ - . /` are allowed.
- Resolved paths are canonicalized; symlinks are followed but must resolve
  within the expected base directory.

## Related

- [Install Completeness Verification](./install-completeness.md) — What must be staged and verified during install
- [Install Manifest](./install-manifest.md) — What gets installed to `~/.amplihack/.claude`
- [recipe Command](./recipe-command.md) — Recipes that use `resolve-bundle-asset` in shell steps
