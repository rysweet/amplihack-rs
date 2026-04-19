# amplihack resolve-bundle-asset — Command Reference

## Synopsis

```
amplihack resolve-bundle-asset <ASSET>
```

## Description

Resolves a named bundle asset or a relative path under `amplifier-bundle/` to
an absolute filesystem path. Prints the resolved path to stdout on success.

This command replaces `python3 -m amplihack.runtime_assets` in recipe shell
steps, providing the same path-resolution logic without a Python dependency.

## Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<ASSET>` | yes | A named asset key (e.g. `helper-path`, `session-tree-path`, `hooks-dir`) or a relative path starting with `amplifier-bundle/`. |

### Named Assets

| Name | Resolves to |
|------|-------------|
| `helper-path` | `amplifier-bundle/tools/orch_helper.py` |
| `session-tree-path` | `amplifier-bundle/tools/session_tree.py` |
| `hooks-dir` | `.claude/tools/amplihack/hooks` or `amplifier-bundle/tools/amplihack/hooks` |

For `hooks-dir`, multiple candidate paths are tried in order; the first that
exists is returned.

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
amplihack resolve-bundle-asset helper-path
# Output: /home/user/.amplihack/.claude/amplifier-bundle/tools/orch_helper.py

# Resolve a relative path under amplifier-bundle/
amplihack resolve-bundle-asset amplifier-bundle/tools/session_tree.py
# Output: /home/user/.amplihack/.claude/amplifier-bundle/tools/session_tree.py
```

## Exit Codes

| Code | Meaning |
|------|---------|
| `0` | Asset resolved — absolute path printed to stdout |
| `1` | Asset not found — the named key or relative path does not exist on disk |
| `2` | Invalid input — the asset name or path failed validation (null bytes, unsafe characters, path traversal, missing `amplifier-bundle/` prefix) |

## Security Constraints

- Path traversal (`..`) is rejected at validation time.
- Only characters in `A-Z a-z 0-9 _ - . /` are allowed.
- Resolved paths are canonicalized; symlinks are followed but must resolve
  within the expected base directory.

## Related

- [Install Manifest](./install-manifest.md) — What gets installed to `~/.amplihack/.claude`
- [recipe Command](./recipe-command.md) — Recipes that use `resolve-bundle-asset` in shell steps
