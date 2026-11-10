---
title: Verify runtime asset resolution
description: Confirm native amplihack asset resolution for helper, session, hooks, and multitask assets.
last_updated: 2026-11-10
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

Resolving an asset only reads the bundle. `amplihack install` also treats
`AMPLIHACK_HOME` as a possible source, and refuses it if its version differs from
the binary's. See
[Framework source selection](../reference/framework-source-selection.md).

## Check the staged modes

Confirm nothing under the bundle is writable by group or others:

```bash
find "$HOME/.amplihack/amplifier-bundle" -perm /022
```

A clean bundle prints nothing. Confirm a resolved script is still executable:

```bash
test -x "$(amplihack resolve-bundle-asset multitask-orchestrator)" && echo ok
```

Expected output:

```text
ok
```

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

## Check the installed resolver is stripped

The resolver links only `amplihack-utils`, not `amplihack-cli`, and release
builds strip it. An installed copy should be a few megabytes and report
`stripped`:

```bash
resolver="$(readlink -f "$HOME/.local/bin/amplihack-asset-resolver")"
stat -c %s "$resolver"
file "$resolver"
```

Expected output shape on Linux x64:

```text
<size in bytes; a few MB, not hundreds>
/home/alice/.amplihack/releases/<release>/bin/amplihack-asset-resolver: ELF 64-bit LSB pie executable, x86-64, ..., stripped
```

The byte count varies by platform and release. A file of several hundred
megabytes reported as `with debug_info, not stripped` is a local build that
was installed before issue #2025; about 590 MB is typical. Replace it with
either of:

```bash
amplihack update                  # stages the stripped resolver from the release archive
./target/release/amplihack install  # from a release build in a source checkout
```

Do not run the installed `amplihack install` for this. The installed
`amplihack` runs from `~/.amplihack/releases/<release>/bin/`, so its sibling is
the unstripped file itself, and the install copies it into the new release.

Copies in older directories under `~/.amplihack/releases/` are left in place
until those releases are cleaned up; the link no longer points at them.

To confirm a source checkout builds a resolver without lbug:

```bash
cargo tree -p amplihack-asset-resolver-bin -i lbug
```

Cargo reports that `lbug` did not match any packages.

## Understand the build-directory warning

`amplihack install` falls back to the resolver on `PATH` only when there is
no resolver beside the running `amplihack`. It refuses a PATH copy that lives
in a cargo build directory (a path component that is exactly `target`, `debug`
or `release`, or that contains `target-`), including through a symlink:

```text
  ⚠️  Not deploying amplihack-asset-resolver from /home/alice/src/amplihack-rs/target/debug/amplihack-asset-resolver: it is in a build directory
```

The install still succeeds. The existing `~/.local/bin/amplihack-asset-resolver`
is not changed, and if there is none, launched tools fall back to
`amplihack resolve-bundle-asset`. To deploy a resolver from a source checkout,
build it in release mode and install from the same build:

```bash
cargo build --release -p amplihack -p amplihack-hooks-bin -p amplihack-asset-resolver-bin
./target/release/amplihack install
```

The installer uses the sibling `target/release/amplihack-asset-resolver`,
which is stripped. The sibling is not checked for build directories, so
`./target/debug/amplihack install` deploys the unstripped debug resolver. See the
[asset resolver source rule](../reference/install-command.md#asset-resolver-source-rule).

## Interpret failures

| Symptom | Meaning | Fix |
| ------- | ------- | --- |
| `Unknown asset name` | The binary does not know the named asset. | Update `amplihack` and rerun `amplihack install`. |
| `Bundle asset not found` | The name is registered, but the file or directory is missing from every runtime root. | Set `AMPLIHACK_HOME` to the installed bundle root or reinstall amplihack. |
| Exit code `2` | The argument is an invalid relative path. | Use one of the named assets above or a safe `amplifier-bundle/...` path. |
| Resolver is hundreds of MB, `not stripped` | A local debug build was installed before issue #2025. | Run `amplihack update`, or `./target/release/amplihack install` from a release build. |
| `Not deploying amplihack-asset-resolver from ...` | The only resolver found was on `PATH` in a build directory. | Install from a release build that has the resolver beside `amplihack`. |

## Related

- [resolve-bundle-asset Command Reference](../reference/resolve-bundle-asset-command.md)
- [Environment Variables](../reference/environment-variables.md#amplihack_asset_resolver)
- [Install Command Reference](../reference/install-command.md#asset-resolver-source-rule)
- [Install Manifest](../reference/install-manifest.md)
- [Framework Source Selection](../reference/framework-source-selection.md)
