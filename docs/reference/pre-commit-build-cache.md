# Pre-commit Build Cache Reference

> [Home](../index.md) > Reference > Pre-commit Build Cache

The Cargo hooks in `.pre-commit-config.yaml` run through
`scripts/git-hook-cargo.sh`. The wrapper picks a build cache for the current
checkout, removes stale caches left by other checkouts, marks the current cache
with `CACHEDIR.TAG`, runs the command, and reports free disk space if the
command fails. This page describes each of those rules.

Related issues: #1278 (git environment), #1381 (per-checkout cache), #1440
(cache location and age-based reclaim), #1771 (symlinked caches and cache
tagging).

## Contents

- [Usage](#usage)
- [Location](#location)
- [Per-checkout hashing](#per-checkout-hashing)
- [Variables](#variables)
- [Order of operations](#order-of-operations)
- [Reclaim rules](#reclaim-rules)
- [Dangling links](#dangling-links)
- [CACHEDIR.TAG](#cachedirtag)
- [Explicit CARGO_TARGET_DIR](#explicit-cargo_target_dir)
- [Messages](#messages)
- [Fallback to TMPDIR](#fallback-to-tmpdir)
- [Tests](#tests)

## Usage

The wrapper takes a command and its arguments, and exits with the command's
status:

```bash
scripts/git-hook-cargo.sh cargo clippy --locked --all-targets -- -D warnings
```

The hooks call it in the same way, for example:

```yaml
entry: scripts/git-hook-cargo.sh cargo run --locked --bin amplihack -- hygiene artifact-guard --repo . --mode pre-commit
```

## Location

When `CARGO_TARGET_DIR` is not set, the wrapper exports:

```text
${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/amplihack-precommit-target-<hash>
```

The directory under `amplihack/` that holds these entries is the **cache
root**. Every entry whose name starts with `amplihack-precommit-target-` is
treated as a cache owned by the wrapper.

The wrapper does not create the cache directory itself. If the path does not
exist, Cargo creates it on the first build and writes its own `CACHEDIR.TAG`.

## Per-checkout hashing

`<hash>` is the first 12 hexadecimal characters of the SHA-256 of the checkout
root, as reported by `git rev-parse --show-toplevel` after the `GIT_*`
variables are removed. Each worktree and clone therefore gets its own cache, and
a given checkout reuses its cache across runs. The digest comes from
`sha256sum`, or from `shasum -a 256` where `sha256sum` is not installed (macOS).
If `git rev-parse` fails, the current working directory is hashed instead.

To find the cache for the current checkout:

```bash
root="$(git rev-parse --show-toplevel)"
if command -v sha256sum >/dev/null 2>&1; then
    hash="$(printf '%s' "$root" | sha256sum | cut -c1-12)"
else
    hash="$(printf '%s' "$root" | shasum -a 256 | cut -c1-12)"
fi
cache="${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/amplihack-precommit-target-$hash"
echo "$cache"
```

This prints the path under the normal cache root. If the wrapper fell back to
the temporary directory, the cache is under `${TMPDIR:-/tmp}` instead (see
[Fallback to TMPDIR](#fallback-to-tmpdir)).

## Variables

| Variable | Default | Meaning |
|---|---|---|
| `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS` | `7` | A cache is stale when nothing near its root has been modified for more than this many days (see [Staleness](#staleness)). `0` disables the reclaim sweep. |
| `AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB` | `5` | After a failed command, free space below this many GiB on the cache's filesystem is reported as a disk problem. |
| `CARGO_TARGET_DIR` | unset | When set, the wrapper uses it unchanged. See [Explicit CARGO_TARGET_DIR](#explicit-cargo_target_dir). |

A value that is not a positive integer, such as `-1`, `3.5` or `week`, disables
the sweep in the same way as `0`. A value containing anything other than digits
also prints one warning, so that a typo does not leave old caches in place
without explanation.

Run a hook with a shorter TTL:

```bash
AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=2 pre-commit run cargo-clippy --all-files
```

Run a hook with the sweep disabled:

```bash
AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 pre-commit run cargo-clippy --all-files
```

When the sweep is disabled, whether by `0` or by a value that is not a positive
integer, the wrapper still removes a dangling link at the current cache path and
still tags the current cache. Neither step reclaims anything.

## Order of operations

When the wrapper owns the cache (`CARGO_TARGET_DIR` unset), it runs these steps
in order:

1. If the current cache path is a dangling link owned by the user, remove the
   link so Cargo can create a real directory there.
2. Reclaim stale caches under the cache root, never touching the current cache.
3. Write `CACHEDIR.TAG` into the current cache if it exists and has no tag.
4. Run the command.
5. If the command failed, report free space on the cache's filesystem.

## Reclaim rules

The sweep reads the cache root one entry at a time, NUL-delimited, so a name
containing a newline is read as one entry and cannot be split into a path
outside the root. Only entries directly under the cache root whose names start
with `amplihack-precommit-target-` are considered, and the cache root must be an
absolute path.

Each entry is either a directory or a symbolic link. Hosts that keep build
caches on a separate volume often replace each cache directory with a link:

```text
~/.cache/amplihack/amplihack-precommit-target-3f9a1c2b7d04 -> /mnt/build/amplihack-precommit-target-3f9a1c2b7d04
```

### Rules applied to every entry

An entry is skipped, and nothing is deleted, when any of these holds:

- It is the current checkout's cache.
- It is a link to something other than a directory, such as a regular file.
  A link whose target does not exist is handled separately; see
  [Dangling links](#dangling-links).
- Its resolved path is empty, `/`, `$HOME`, the cache root, or the current
  cache.
- Its resolved path is an ancestor of the cache root, `$HOME`, the current
  cache, or the repository top level.
- The link or its resolved target is not owned by the current user.
- It is not stale (see below).
- A build is using it (see below).

Paths are resolved physically (`cd -P` and `pwd -P`) with `CDPATH` cleared, so
an exported `CDPATH` cannot redirect resolution.

### Staleness

A cache is stale when nothing at depth 0, 1 or 2 below its resolved directory
has been modified for more than TTL days. In `find` terms, the cache is kept if
`find <target> -maxdepth 2 -mtime -<TTL+1>` returns anything; with the default
TTL of 7 that is `-mtime -8`. This matches the `-mtime +<TTL>` test used for
directories since #1440, so a cache last modified exactly seven days ago is
kept, and one untouched for eight full days or more is reclaimed. Depth 2
covers `debug/deps`, `debug/.fingerprint`, `release/build` and similar
directories, which Cargo updates whenever a build writes output. Age is always
measured on the resolved directory, not on the link, because the link's own
timestamp does not change when the target is built.

Staleness depends on modification, not on use. A build that finds nothing to
recompile writes nothing, so a cache that is only read by no-op builds still
becomes stale after the TTL.

Example: a target whose root and `debug/` were last modified in 2020, but whose
`debug/deps` changed yesterday, is not stale and is kept.

### In-use detection

Cargo holds an exclusive `flock` on a `.cargo-lock` file for the duration of a
build. The wrapper probes `<target>/.cargo-lock` and every
`<target>/*/.cargo-lock`, which covers `debug/`, `release/` and other profile
directories. If any of those locks is held, the cache is kept.

When `flock` is not installed, the wrapper falls back to `lsof +D` on the
target. When neither tool is installed, every cache is treated as in use and
nothing is reclaimed.

### Directories

A stale, unused, user-owned directory is removed with `rm -rf`.

### Links

A stale, unused link is removed only when its target looks like a Cargo target
directory. The wrapper first deletes the target, then the link:

```text
rm -rf -- /mnt/build/amplihack-precommit-target-3f9a1c2b7d04
rm -f  -- ~/.cache/amplihack/amplihack-precommit-target-3f9a1c2b7d04
```

Deleting only the link would leave the multi-gigabyte target on the other
volume, which is the leak described in #1771.

A directory looks like a Cargo target directory when at least one of these
holds:

- It contains `CACHEDIR.TAG` whose first line is
  `Signature: 8a477f597d28d172789f06886806bc55`.
- It contains `.rustc_info.json`.
- It contains a `debug/` or `release/` directory that itself contains
  `.fingerprint/` or `.cargo-lock`.

A bare `debug/` or `release/` directory is not enough.

If the target does not look like a Cargo target directory, the wrapper keeps
both the link and the target, and prints one line to stderr (see
[Messages](#messages)). Remove such an entry by hand once you have checked what
the target holds.

### Accepted risk

A user-owned link to a Cargo target that is still wanted, but whose files have
not been modified for longer than the TTL, is deleted together with its target.
Raise `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS` or set it to `0` if that matters on
your host.

## Dangling links

A dangling link is a link whose target no longer exists, for example after the
build volume was wiped.

- Under the cache root, the sweep removes a dangling prefixed link owned by the
  user once the link itself has not been modified for more than the TTL. A
  newer dangling link is kept without a message, because its target may be on
  a volume that is only unmounted for now; removing the link would leave the
  target unreachable when the volume returns. Age is read from the link with
  `find <link> -maxdepth 0 -mtime -<TTL+1>`, which does not follow the link.
  This step is part of the sweep, so `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0`
  disables it.
- At the current cache path, the wrapper removes a dangling link owned by the
  user before running the command, whatever the TTL. Cargo then sees a missing
  path and creates a real directory. If the link was dangling only because its
  volume was unmounted, the build therefore runs on the disk that holds the
  cache root, and the old target on the other volume is no longer linked; once
  the volume is back, remove that target by hand or link it again.

If the current cache path is a link owned by another user, the wrapper prints a
warning and neither removes nor tags it. The command still runs.

## CACHEDIR.TAG

`CACHEDIR.TAG` marks a directory as a rebuildable cache so that backup tools
such as `tar --exclude-caches`, restic and borg skip it. Cargo writes the tag
only into directories it creates, so a target reached through a link, or one
created by another tool, may have none.

After the sweep, the wrapper writes the tag into the current cache when the path
is a real directory, or a link owned by the user that resolves to a directory.
The file content is Cargo's three lines:

```text
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/
```

Rules:

- An existing `CACHEDIR.TAG` is never overwritten, including a custom one.
- If `CACHEDIR.TAG` is a link, even a dangling one, nothing is written, so the
  wrapper never writes through a link.
- If the cache path does not exist yet, nothing is written; Cargo tags the
  directory when it creates it.
- A failed write, for example on a read-only directory, is ignored and does not
  fail the hook.

Check the tag on the current cache, using `$cache` from the snippet in
[Per-checkout hashing](#per-checkout-hashing):

```bash
IFS= read -r first < "$cache/CACHEDIR.TAG" && printf '%s\n' "$first"
# Output: Signature: 8a477f597d28d172789f06886806bc55
```

## Explicit CARGO_TARGET_DIR

When `CARGO_TARGET_DIR` is already set, the wrapper uses it as given and does
not:

- reclaim any cache;
- remove a dangling link at that path;
- write `CACHEDIR.TAG`.

It still removes the `GIT_*` variables and still reports disk space after a
failed command.

```bash
CARGO_TARGET_DIR=/mnt/build/my-target scripts/git-hook-cargo.sh cargo build --locked
```

## Messages

All messages go to stderr with the `amplihack:` prefix. Paths are printed with
`%q`, so names containing control characters are shown escaped.

`<ttl>` is the value of `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS`.

| Event | Message |
|---|---|
| Directory reclaimed | `amplihack: reclaiming build cache untouched for <ttl>+ days: <path>` |
| Link and target reclaimed | `amplihack: reclaiming build cache untouched for <ttl>+ days: <link> -> <resolved>` |
| Dangling link removed | `amplihack: removing dangling build cache link: <link>` |
| TTL value is not a plain number | `amplihack: ignoring invalid AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=<value>; cache sweep disabled` |
| Link kept because the target is not Cargo-shaped | `amplihack: kept <link>: target <resolved> does not look like a Cargo target dir` |
| Current cache path is a link owned by another user | `amplihack: warning: build cache <link> is a link owned by another user; leaving it alone` |
| `CACHEDIR.TAG` could not be written | `amplihack: could not write CACHEDIR.TAG in <path>; continuing` |
| A removal failed | `amplihack: could not remove <path>` or `amplihack: could not fully remove <path>` |
| Command failed with enough free space | `amplihack: build cache <path> -- <n> GiB free on <mount>.` |
| Command failed with free space below the minimum | a framed `OUT OF DISK SPACE -- this is not a compile error.` report |

## Fallback to TMPDIR

The wrapper falls back to the temporary directory when either of these holds:

- `XDG_CACHE_HOME` and `HOME` are both unset or empty.
- `mkdir -p "$base/amplihack"` fails, where `$base` is
  `${XDG_CACHE_HOME:-$HOME/.cache}`.

The cache root is then `${TMPDIR:-/tmp}`, and the current cache is:

```text
${TMPDIR:-/tmp}/amplihack-precommit-target-<hash>
```

All the rules above still apply there. The ownership checks matter most in this
case, because other users can create prefixed links in a shared `/tmp`; the
wrapper never follows or deletes a link it does not own.

## Tests

`tests/issue_1771_precommit_symlinked_caches.sh` covers linked caches, dangling
links, in-use locks under `debug/`, non-Cargo targets, tagging, the guards
against deleting `$HOME`, the cache root or the checkout, and `CDPATH`. The
ownership checks need a second user account and are not exercised in CI.
`tests/issue_1440_precommit_cache_location.sh` covers the location and the
directory sweep. The `ci.yml` workflow runs both tests on
every pull request.

## See also

- [Artifact Guard](../artifact-guard.md): the pre-commit hook that runs through
  this wrapper.
