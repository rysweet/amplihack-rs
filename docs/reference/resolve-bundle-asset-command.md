---
title: resolve-bundle-asset command reference
description: Resolve amplihack bundle assets and legacy parity aliases from native Rust tools.
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# amplihack resolve-bundle-asset - Command Reference

## Synopsis

```
amplihack resolve-bundle-asset <ASSET>
```

The standalone resolver binary accepts the same single argument:

```
amplihack-asset-resolver <ASSET>
```

## Description

Resolves a named bundle asset or a relative path under `amplifier-bundle/` to
an absolute filesystem path. Prints the resolved path to stdout on success.

This is the native Rust asset resolver for recipe shell steps, hooks, and
child tools. It keeps the legacy Python asset names that recipes and helper
scripts still use, while resolving them to the current Rust/runtime assets.

## Arguments

| Argument | Required | Description |
| -------- | -------- | ----------- |
| `<ASSET>` | yes | A named asset key (e.g. `multitask-orchestrator`) or a relative path starting with `amplifier-bundle/`. |

### Named Assets

| Name | Resolves to | Expected type | Purpose |
| ---- | ----------- | ------------- | ------- |
| `helper-path` | `amplifier-bundle/bin/multitask-orchestrator.sh` | file | Compatibility alias for legacy orchestration-helper callers. |
| `session-tree-path` | `amplifier-bundle/tools/amplihack/session` | directory | Compatibility anchor for callers that still request the old session-tree asset name. |
| `hooks-dir` | `amplifier-bundle/tools/amplihack/hooks` | directory | Compatibility alias for hook configuration assets used by launcher and recipe preflight paths. |
| `multitask-orchestrator` | `amplifier-bundle/bin/multitask-orchestrator.sh` | file | Native multitask orchestrator wrapper. |

Named assets are resolved against runtime roots in priority order:

1. `AMPLIHACK_HOME`
2. `~/.amplihack`
3. The nearest ancestor of the current directory containing `amplifier-bundle/`
   or `.claude/`
4. The compiled workspace/package root
5. The current working directory

The first existing candidate wins. Missing named assets fail with exit code
`1`; invalid relative paths fail with exit code `2`.

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

# Resolve the hooks directory
amplihack resolve-bundle-asset hooks-dir
# Output: /home/user/.amplihack/amplifier-bundle/tools/amplihack/hooks

# Resolve the legacy helper alias to the native orchestrator wrapper
amplihack resolve-bundle-asset helper-path
# Output: /home/user/.amplihack/amplifier-bundle/bin/multitask-orchestrator.sh

# Resolve the legacy session-tree anchor
amplihack resolve-bundle-asset session-tree-path
# Output: /home/user/.amplihack/amplifier-bundle/tools/amplihack/session

# Resolve a relative path under amplifier-bundle/
amplihack resolve-bundle-asset amplifier-bundle/tools/statusline.sh
# Output: /home/user/.amplihack/amplifier-bundle/tools/statusline.sh

# Resolve through the standalone child-process API
amplihack-asset-resolver helper-path
# Output: /home/user/.amplihack/amplifier-bundle/bin/multitask-orchestrator.sh

# Attempt to resolve an unknown named asset
amplihack resolve-bundle-asset nonexistent-thing
# Stderr: ERROR: Unknown asset name "nonexistent-thing". Expected one of: hooks-dir, helper-path, session-tree-path, multitask-orchestrator
# Exit: 1
```

## Configuration

### `AMPLIHACK_HOME`

Set `AMPLIHACK_HOME` when the bundle is installed somewhere other than
`~/.amplihack`:

```sh
AMPLIHACK_HOME=/opt/amplihack amplihack resolve-bundle-asset helper-path
# Output: /opt/amplihack/amplifier-bundle/bin/multitask-orchestrator.sh
```

The value must point at the directory that contains `amplifier-bundle/`.

### `AMPLIHACK_ASSET_RESOLVER`

Launchers and recipe runners set `AMPLIHACK_ASSET_RESOLVER` to the absolute
path of `amplihack-asset-resolver` when the standalone binary is available.
Child tools can call it without knowing where `amplihack` itself is installed:

```sh
"$AMPLIHACK_ASSET_RESOLVER" amplifier-bundle/recipes/smart-orchestrator.yaml
```

See [Environment Variables](./environment-variables.md#amplihack_asset_resolver)
for resolver discovery order.

## Rust API

The CLI and standalone binary share the same resolver module.

| Function | Purpose |
| -------- | ------- |
| `resolve_named_asset(name)` | Resolve one of the named assets above to an existing path. |
| `resolve_asset(relative_path)` | Resolve a validated `amplifier-bundle/...` relative path. |
| `validate_relative_path(relative_path)` | Reject traversal, absolute paths, unsafe characters, and paths outside `amplifier-bundle/`. |
| `run_cli(arg)` | Dispatch one CLI argument and return the process exit code. |

## Exit Codes

| Code | Meaning |
| ---- | ------- |
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
ERROR: Unknown asset name "unknown-asset". Expected one of: hooks-dir, helper-path, session-tree-path, multitask-orchestrator
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

- [Verify Runtime Asset Resolution](../howto/verify-runtime-asset-resolution.md) — Confirm helper, session, hooks, and orchestrator assets resolve in an install
- [Install Completeness Verification](./install-completeness.md) — What must be staged and verified during install
- [Install Manifest](./install-manifest.md) — What gets installed to `~/.amplihack/.claude`
- [recipe Command](./recipe-command.md) — Recipes that use `resolve-bundle-asset` in shell steps
