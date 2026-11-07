---
title: Framework source selection reference
description: Which source amplihack install, update and self-heal stage the framework bundle from, how versions are compared, and which checks and modes staging applies.
last_updated: 2026-11-05
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Framework source selection reference

`amplihack install`, `amplihack update` and self-heal stage
`~/.amplihack/amplifier-bundle` from a source whose version equals the binary's
version. A source with a different version is used only with an explicit
`--local`.

## Contents

- [Sources each command consults](#sources-each-command-consults)
- [How a source's version is read](#how-a-sources-version-is-read)
- [Source rules](#source-rules)
- [Release tag fetch](#release-tag-fetch)
- [Staging order](#staging-order)
- [Fetch backoff](#fetch-backoff)
- [Manifest check](#manifest-check)
- [File modes](#file-modes)
- [What happens on each failure](#what-happens-on-each-failure)
- [Messages](#messages)
- [Known limits](#known-limits)
- [Contributor API](#contributor-api)

## Sources each command consults

Each command tries its sources in the order shown.

| Command | Sources, in order |
|---|---|
| `amplihack install --local <PATH>` | `<PATH>` only |
| `amplihack install` | `AMPLIHACK_HOME`, current-directory walk-up, executable walk-up, compile-time workspace root, `~/.amplihack`, release tag fetch |
| `amplihack install --force-refresh` | `AMPLIHACK_HOME`, release tag fetch |
| `amplihack update` | Runs the new binary's `install --force-refresh` |
| Self-heal and the launch bootstrap's framework re-stage | The same five local sources as `install`, then release tag fetch |
| The launch bootstrap's slash-command restore | The same five local sources as `install`. It never fetches. |

`--local` takes priority over `--force-refresh` when both are given.

Compatibility validation runs first, then the version check. A source that is
invalid or fails the
[compatibility check](framework-bundle-compatibility.md) is skipped with a note,
whatever its version.

`AMPLIHACK_HOME` is treated as the `~/.amplihack` source when both resolve to
the same directory. It never causes a refusal in that case, because sessions
set this variable themselves and the user may not have chosen it.
`--force-refresh` does not use that directory.

`AMPLIHACK_HOME` is only ever a source here. Install always stages into
`$HOME/.amplihack`, wherever `AMPLIHACK_HOME` points.

## How a source's version is read

`~/.amplihack` is judged by its stamp, `.installed-version`. Every other local
source is checked in this order:

1. `[workspace.package].version` in the source's `Cargo.toml`, when that file
   is a regular file of at most 64 KiB and names `crates/amplihack-cli`.
2. `git describe --exact-match --tags --match 'v*'`, with a 5-second limit and
   at most 256 bytes of output.
3. Otherwise the version is unknown. A value that doesn't parse as semver also
   counts as unknown.

A fetched release tree is read from `Cargo.toml` only. If that gives no version,
the fetch has failed.

Versions are compared with semver precedence, against the version the binary
reports with `amplihack --version`. Build metadata is ignored, so `1.2.0` and
`1.2.0+snapshot.7deb23da4` count as the same version.

## Source rules

An unknown version is treated the same as a mismatch.

| Source | Version matches | Mismatch or unknown |
|---|---|---|
| `--local <PATH>` | Stage; print path and version | Stage; print path, version and a warning. This also lets you stage a newer tree. |
| `AMPLIHACK_HOME`, plain `install` | Stage; print path and version | **Refuse** |
| Current-directory walk-up, plain `install` | Stage; print path and version | **Refuse** |
| `AMPLIHACK_HOME`, `--force-refresh` | Stage; print path and version | Print a note, then fetch the release tag |
| Any local source, self-heal or launch bootstrap | Stage | Print a note, then try the next source |
| Executable walk-up or compile-time root | Stage | Print a note, then try the next source |
| `~/.amplihack` | Stage | Older, newer or unreadable stamp: print a note, then fetch the release tag |

A refusal has these properties:

- It exits non-zero and names the source path, source version and binary
  version, and suggests `--local`.
- It happens before binaries are deployed, so nothing on disk changes.
- It happens before the `--interactive` wizard, so you aren't asked any
  questions first.
- The first valid source decides. A mismatched `AMPLIHACK_HOME` refuses even if
  the current directory would match.

After `amplihack install --local <PATH>`, the stamp records the binary's
version, so self-heal doesn't replace the bundle you chose.

## Release tag fetch

The fetch targets exactly `refs/tags/v<binary version>`. It never uses `main`, a
branch or an older tag.

**When a build may fetch:** only when `AMPLIHACK_RELEASE_VERSION` was set at
build time and the version has no `+` build metadata. Dev and snapshot builds
make no network request and tell you to use `amplihack install --local`.

**Routes:**

1. `git init` in an empty directory, then
   `git fetch --depth 1 https://github.com/rysweet/amplihack-rs refs/tags/v<version>`.
   No remote is configured, and the URL is fixed in the binary.
2. If git fails or isn't installed, download
   `https://github.com/rysweet/amplihack-rs/archive/refs/tags/v<version>.tar.gz`.

**Time limits**, shared across both routes:

| Caller | Limit |
|---|---|
| Explicit `install` and `update` | 180 seconds |
| Self-heal and the launch bootstrap | 60 seconds |

**Tarball limits:**

| Limit | Value |
|---|---|
| Download size | 256 MiB |
| Unpacked size | 1 GiB |
| Archive entries | 100,000 |
| Decompressed stream, including metadata entries | Unpacked size plus 4 KiB per allowed entry (about 1.4 GiB) |

Exceeding a limit counts as a failed fetch and leaves the bundle unchanged.

**Requirements on each route:**

- The download uses HTTPS only. Its final URL, after redirects, must be on the
  allowed host list, which includes `codeload.github.com` for this download
  only.
- The fetched tree's version must equal the binary's. A mismatch or an unknown
  version is a hard failure, and no older tree is tried.
- Git runs with inherited `GIT_*` location variables, `GIT_SSH_COMMAND`,
  `GIT_PROXY_COMMAND`, `GIT_ASKPASS`, `SSH_ASKPASS` and `GIT_TEMPLATE_DIR`
  removed. It also runs with hooks and `core.fsmonitor` turned off, no credential
  helper, no prompts and no standard input. These settings apply to every git
  call install makes, including `git describe`.
- Relative `PATH` entries are skipped when looking for `git`.

## Staging order

1. Remove any leftover staging directory.
2. Create `amplifier-bundle.staging` with mode `0700`.
3. Copy the source's `amplifier-bundle/` into it.
4. Compare the copy with the source ([manifest check](#manifest-check)).
5. Set modes across the tree ([file modes](#file-modes)). The staging root gets
   `0755` last.
6. Run `secure_merge_guard`, which applies the merge guard's specific modes.
7. Run the compatibility check on the staged copy.
8. Swap the staged copy into place as `amplifier-bundle`.

If any step after the first fails, the staging directory is deleted, the
installed bundle stays as it was and the stamp doesn't advance. A symlink at the
source's `amplifier-bundle` root is an error.

Self-heal stages under `~/.amplihack/.install.lock`, including its fetch.
Explicit `install` doesn't take that lock.

## Fetch backoff

A tag fetch that fails, from any install, writes `~/.amplihack/.tag-fetch-failed`:

```text
1.2.99
1762300800
```

The first line is the binary version that failed to fetch; the second is the
time of the failure in Unix seconds. For 10 minutes after that time, a
self-heal or launch-bootstrap install for the same version skips the fetch and
fails with `SourceFetchFailed::Failed`, so self-heal keeps a compatible bundle
with its usual warning. The holder of the install lock writes the marker before
releasing the lock, so the launches queued behind it read the marker and don't
retry.

| Case | Fetch runs? |
|---|---|
| No marker, or the marker is for another version | Yes |
| Marker for this version, under 10 minutes old | Self-heal: no. Explicit `install` and `update`: yes. |
| Marker older than 10 minutes, dated in the future, malformed or over 256 bytes | Yes |

A successful fetch removes the marker. A build that may not fetch never writes
one.

## Manifest check

After the copy, install records the relative path and SHA-256 of every regular
file in the source bundle and in the staged copy, then compares the two lists.

| Finding | Result |
|---|---|
| A file is missing from staging | Install fails |
| Staging has an extra file | Install fails |
| A file's hash differs | Install fails |

The check covers the whole bundle, including recipes, tools, skills and agents.
It uses the same walk as the copy, so these aren't part of either list:

- `__pycache__`, `.pytest_cache` and `node_modules` directories
- `.pyc` and `.pyo` files
- symlinks and special files

The error lists up to 20 paths per category and the total for each category.
Control characters in paths are escaped.

This check shows that staging matches its source. It is not a check against a
manifest published with the release: there is no such manifest yet, so a
same-version source with local edits passes. Issue #1955 item 4 asked for a
check against the release's manifest; what ships is the narrower
copy-against-source check, and the release manifest is a follow-up. See [Known limits](#known-limits).

## File modes

On Unix, staging sets modes itself. Source modes affect only whether a file is
executable, and the umask has no effect.

| Entry | Mode |
|---|---|
| Directory, including the bundle root | `0755` |
| File with any execute bit in the source | `0755` |
| Any other file | `0644` |

Nothing under the bundle is writable by group or others. This holds after every
install, update and self-heal re-stage. The `recipe run` merge guard from #1551
accepts the tree with no further `chmod`.

To check an installed bundle:

```sh
find ~/.amplihack/amplifier-bundle -perm /022
```

A clean bundle prints nothing.

## What happens on each failure

In this table, `update` means its post-update install. The binary download comes
first and always needs network access.

| Failure | `install` and `update` | Self-heal |
|---|---|---|
| Local source mismatch | `AMPLIHACK_HOME` or current directory: refuse. `--force-refresh`: note, then fetch. | Note, then next source |
| Tag fetch fails, or the build may not fetch | Exit non-zero; bundle and stamp unchanged; names `install --local` | Installed bundle compatible: warn, leave the stamp, run the command. Bundle missing or incompatible: exit 1. |
| Fetched tree has the wrong or an unknown version | Exit non-zero | Exit 1 |
| Manifest check fails | Exit non-zero | Exit 1 |

`amplihack update` exits non-zero when its post-update install fails. The new
binary is already in place by then, so run `amplihack install` once the cause is
fixed, or let self-heal retry at the next launch.

When self-heal keeps a bundle, the unchanged stamp makes a later launch try
again. After a failed fetch, launches for the same version skip the fetch for 10
minutes ([fetch backoff](#fetch-backoff)). On a build that may not fetch, that attempt looks only at local sources,
so the warning repeats until a same-version source appears or you run
`amplihack install --local`. See
[Self-heal failure modes](../features/self-heal-asset-restage.md#failure-mode).

## Messages

**Staging from a source with the same version:**

```text
📦 Using framework source /home/alice/src/amplihack-rs (v1.2.99, from AMPLIHACK_HOME)
```

**Staging from `--local` with a different version:**

```text
📦 Using framework source /home/alice/src/amplihack-old (v1.2.90, from --local)
⚠️  This source is v1.2.90 but this binary is v1.2.99. Staging it because --local was given.
```

An unreadable version prints as `unknown` in both lines.

**Refusal:**

```text
error: refusing to stage the framework bundle from /home/alice/src/amplihack-rs (AMPLIHACK_HOME):
  source version: v1.2.0
  binary version: v1.2.99
Run `amplihack install --local /home/alice/src/amplihack-rs` to stage this tree anyway.
```

**Skipped source:**

```text
ℹ️  Ignoring the framework source at /home/alice/src/amplihack-rs (AMPLIHACK_HOME): its version is v1.2.0 and this binary is v1.2.99.
```

**Tag fetch:**

```text
📥 Fetching the framework bundle from release tag v1.2.99 because no local source matches this binary.
```

With `--force-refresh`, the reason reads `because --force-refresh was given.`

A path with spaces or shell characters is single-quoted in the
`install --local` hint, so the command can be pasted as printed.

**Failed fetch:**

```text
error: could not fetch the framework bundle from release tag v1.2.99: network request timed out
The installed bundle and version stamp were not changed.
Run `amplihack install --local <PATH>` to install from a checkout.
```

**Build that may not fetch:**

```text
error: this build (1.2.0+snapshot.7deb23da4) cannot fetch a release tag, and no local source matches it.
Run `amplihack install --local <PATH>` to install from a checkout.
```

**Self-heal keeping a compatible bundle after a failed fetch (one line):**

```text
amplihack: warning: could not fetch the framework bundle for v1.2.99 (network request timed out); keeping the installed bundle. A later launch will retry.
```

**Self-heal keeping a compatible bundle on a build that may not fetch (one line):**

```text
amplihack: warning: this build (1.2.0+snapshot.7deb23da4) cannot fetch a release tag, and no local source matches it; keeping the installed bundle. Run `amplihack install --local <PATH>` to replace it.
```

**Manifest failure:**

```text
error: the staged bundle does not match its source /home/alice/src/amplihack-rs (1 missing, 0 extra, 1 changed):
  missing: recipes/default-workflow.yaml
  changed: tools/autodrive_merge_gate.sh
```

Install never prints `Bundled framework source not found`.

## Known limits

**Version checks**

- **`main` checkouts and release binaries:** `main` declares version `1.2.0`, and
  each release tag declares its own version, such as `1.2.99`. A release binary
  therefore refuses every `main` checkout on plain `install`, however recent it
  is. `Cargo.toml` is read first, so `git describe` isn't reached. Use `--local`
  to stage a `main` checkout on purpose.
- **Dev and snapshot builds:** a `1.2.0` build matches every `main` checkout,
  however old, because build metadata is ignored. The gate fully protects only
  release binaries.
- **Same-version checkouts:** a checkout with uncommitted or post-tag changes
  passes. The manifest compares staging with its source, not with the tag or a
  manifest published with the release, so it proves a faithful copy and nothing
  about the content. A release-published manifest is a follow-up to #1955. The
  output names the source so you can see this.
- **Self-declared versions:** the gate checks what a source says about itself.
  It doesn't prove where the source came from.
- **Current directory during self-heal:** self-heal can stage from a
  same-version current directory. A hostile repository can declare a matching
  version, and hooks staged from it keep running after you leave that directory.
- **`install --local` with an older tree:** the stamp then equals the binary's
  version, so `~/.amplihack` qualifies as a source even though its content is
  older. This follows from your explicit choice.

**Fetching**

- **No content pin:** the fetched tree isn't checked against a published hash.
  Its version is checked, and its copy is checked against itself.
- **Git route limits:** the git route is limited by time only. The size and
  entry limits apply to the tarball route.
- **Hung networks:** a hung network can add up to 60 seconds to a launch. The
  [fetch backoff](#fetch-backoff) limits this to one launch per 10 minutes: the
  launches queued behind a failed fetch skip it. A launch that starts after the
  window ends waits again.
- **Blocked networks:** someone who can block the network can keep a host on an
  older compatible bundle. A warning prints on each launch.

**Kept bundles**

- **Offline hosts:** an offline host can run a new binary with an older
  compatible bundle, with a warning, until a fetch succeeds.
- **Snapshot hosts:** a host that moves between snapshots has no valid source.
  It keeps the old bundle with a warning until you run `install --local`.
- **Codex:** while an older bundle is kept, the Codex reconcile is skipped with a
  warning. If the stamp already matches the binary and no source is found, the
  reconcile still fails with an error.
- **Codex launches pay for the gate:** every launch with Codex selected runs
  the version gate over the local sources. A source with no usable `Cargo.toml`
  version runs `git describe`, which can add up to 5 seconds. Sources it turns
  down go to the debug log; nothing prints unless a refusal occurs.

**Slash commands**

- **No restore without a local source:** a launch that finds
  `~/.claude/commands/amplihack/` missing restores it only from a same-version
  local source. With none, the commands stay missing, and the only record is a
  warning in the trace log. Run `amplihack install` to restore them.

**Staging**

- **Partial install after a failed check:** when the manifest check fails during
  staging, binaries and `.claude` may already be new while the bundle and stamp
  stay old. Self-heal retries.
- **Stale stamp with `AMPLIHACK_HOME` at `~/.amplihack`:** if the current
  directory is an older checkout, install refuses and names that directory. You
  may not have thought of it as the source.
- **Concurrent installs:** explicit install takes no lock and shares a staging
  path with self-heal. A race usually ends in a manifest failure, so self-heal
  can exit 1 while an explicit install runs.
- **Symlinks:** symlinks in the source aren't staged.

## Contributor API

All paths are under `crates/amplihack-cli/src/`.

| Item | File | Purpose |
|---|---|---|
| `SourcePolicy` | `commands/install/framework_source.rs` | Which rules apply: local override, force refresh, explicit or implicit. |
| `SourceVersion` | `commands/install/framework_source.rs` | `Known` is built only from a successful semver parse; otherwise unknown. |
| `read_source_version` | `commands/install/framework_source.rs` | Reads a source's version as described above. |
| `acquire_framework_source`, `AcquiredSource` | `commands/install/framework_source.rs` | Decides the source. All inputs are parameters. |
| `TagFetcher` | `commands/install/tag_fetch.rs` | Fetches one exact release tag. Injected in tests, with no production env var. |
| `SourceFetchFailed` | `commands/install/tag_fetch.rs` | The only error self-heal may continue past, and only with a compatible bundle. |
| `recent_failure`, `record_failure`, `clear` | `commands/install/fetch_backoff.rs` | The [fetch backoff](#fetch-backoff) marker. |
| `of_source`, `of_staged`, `compare` | `commands/install/bundle_manifest.rs` | Builds and compares manifests. `compare` is pure. |
| `walk_tree`, `WalkScope`, `WalkEntry` | `commands/install/filesystem.rs` | The one walker used by the copy and the manifest. |
| `normalize_bundle_modes` | `commands/install/directories.rs` | Applies [the mode rule](#file-modes). Returns an error on a symlink. |
| `run_install_with_fetcher` | `commands/install/mod.rs` | The install entry point tests use. `run_install_scoped` wraps it. |
| `http_get_bounded` | `update/network.rs` | HTTPS download with an overall time limit and a final-URL check. |

Focused tests:

```sh
cargo test -p amplihack-cli issue_1955
```

## See also

- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md) -
  what to do when install refuses a source or a check fails
- [Install from a local repository](../howto/local-install.md) - the `--local`
  route
- [Framework bundle compatibility](framework-bundle-compatibility.md) - the
  recipe contract every source must meet
- [Self-heal asset re-stage](../features/self-heal-asset-restage.md) - the
  version-stamp check at launch
- [Post-update install re-exec](../features/update-reexec-new-binary.md) - how
  `amplihack update` runs the new binary's install
- [Install command reference](install-command.md) - all install flags and phases
