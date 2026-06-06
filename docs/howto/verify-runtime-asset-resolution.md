---
title: Verify runtime asset resolution
description: Confirm native amplihack asset resolution for helper, session, hooks, and multitask assets.
last_updated: 2026-06-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Verify runtime asset resolution

Use this guide to confirm that a Rust amplihack install resolves the legacy
Python-era asset names and the current native multitask asset from the same
runtime bundle.

## Prerequisites

- `amplihack` is installed and on `PATH`
- The installed bundle root contains `amplifier-bundle/`
- Optional: `amplihack-asset-resolver` is installed for child-process checks

## Check the installed binary

```bash
amplihack --version
which amplihack
```

The binary should be the user-local or release binary you expect to test. If a
stale system binary appears first, repair the path order with
[Repair install/update PATH conflicts](./repair-install-update-path-conflicts.md).

## Resolve every parity asset

Run the canonical ordered list of runtime assets:

```bash
for asset in hooks-dir helper-path session-tree-path multitask-orchestrator; do
  path="$(amplihack resolve-bundle-asset "$asset")"
  printf '%s -> %s\n' "$asset" "$path"
  test -e "$path"
done
```

Expected output shape:

```text
hooks-dir -> /home/alice/.amplihack/amplifier-bundle/tools/amplihack/hooks
helper-path -> /home/alice/.amplihack/amplifier-bundle/bin/multitask-orchestrator.sh
session-tree-path -> /home/alice/.amplihack/amplifier-bundle/tools/amplihack/session
multitask-orchestrator -> /home/alice/.amplihack/amplifier-bundle/bin/multitask-orchestrator.sh
```

## Check expected file types

```bash
test -f "$(amplihack resolve-bundle-asset helper-path)"
test -d "$(amplihack resolve-bundle-asset session-tree-path)"
test -d "$(amplihack resolve-bundle-asset hooks-dir)"
test -f "$(amplihack resolve-bundle-asset multitask-orchestrator)"
```

All commands exit `0` when the installed bundle matches the resolver contract.

## Verify a custom bundle root

Set `AMPLIHACK_HOME` when validating a non-default install root:

```bash
AMPLIHACK_HOME="$HOME/.amplihack" amplihack resolve-bundle-asset helper-path
```

`AMPLIHACK_HOME` must point to the directory that contains
`amplifier-bundle/`, not to `amplifier-bundle/` itself.

## Verify child-process resolution

Recipe runners and launchers expose the standalone resolver through
`AMPLIHACK_ASSET_RESOLVER` when the binary is available:

```bash
if [ -n "${AMPLIHACK_ASSET_RESOLVER:-}" ]; then
  "$AMPLIHACK_ASSET_RESOLVER" helper-path
fi
```

You can also call the resolver binary directly:

```bash
amplihack-asset-resolver session-tree-path
```

Both forms print the same absolute paths as `amplihack resolve-bundle-asset`.
The standalone resolver also derives its no-argument usage text from the same
Rust named-asset table, so usage output and accepted names stay in sync.

## Interpret failures

| Symptom | Meaning | Fix |
| ------- | ------- | --- |
| `Unknown asset name` | The binary does not know the named asset. | Update `amplihack` and rerun `amplihack install`. |
| `Bundle asset not found` | The name is registered, but the file or directory is missing from every runtime root. | Set `AMPLIHACK_HOME` to the installed bundle root or reinstall amplihack. |
| Exit code `2` | The argument is an invalid relative path. | Use one of the named assets above or a safe `amplifier-bundle/...` path. |

## Related

- [resolve-bundle-asset Command Reference](../reference/resolve-bundle-asset-command.md)
- [Environment Variables](../reference/environment-variables.md#amplihack_asset_resolver)
- [Install Manifest](../reference/install-manifest.md)
