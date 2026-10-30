# Pre-commit Build Cache

**Type**: Reference (Information-Oriented)

The pre-commit and pre-push hooks run cargo through
`scripts/git-hook-cargo.sh`. The wrapper gives each checkout its own
`CARGO_TARGET_DIR`, removes caches that nobody has built in for some time, tags
each cache so that backup and clean-up tools recognise it, and removes a cache
link whose target has gone. This page lists those rules (issues #1381, #1440
and #1771).

## Contents

- [Cache location](#cache-location)
- [Settings](#settings)
- [Order of operations](#order-of-operations)
- [Stale-cache sweep](#stale-cache-sweep)
- [Cache tag](#cache-tag)
- [Dangling link at the cache path](#dangling-link-at-the-cache-path)
- [Explicit CARGO_TARGET_DIR](#explicit-cargo_target_dir)
- [Reclaiming space by hand](#reclaiming-space-by-hand)
- [Limitations](#limitations)
- [Related](#related)

## Cache location

Unless the caller sets `CARGO_TARGET_DIR`, the wrapper sets it to:

```text
${XDG_CACHE_HOME:-$HOME/.cache}/amplihack/amplihack-precommit-target-<hash>
```

`<hash>` is the first 12 hexadecimal characters of the SHA-256 digest of the
checkout root, as printed by `git rev-parse --show-toplevel`, or of the working
directory when git cannot answer. Every clone and worktree therefore has its
own cache, and a checkout reuses its cache from one run to the next.

The directory that holds the caches is the **cache root**. When neither
`XDG_CACHE_HOME` nor `HOME` is set, or `<base>/amplihack` cannot be created,
the cache root is `${TMPDIR:-/tmp}` itself.

The wrapper creates the cache root but never the cache directory. Cargo creates
the cache directory on the first build in a checkout.

Print the cache path for the current checkout:

```console
$ AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 scripts/git-hook-cargo.sh printenv CARGO_TARGET_DIR
/home/dev/.cache/amplihack/amplihack-precommit-target-48029401020f
```

The hash differs for each checkout. `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0` turns
the sweep off for this run; the cache tag and the dangling-link repair still
run.

## Settings

| Variable | Default | Effect |
| --- | --- | --- |
| `AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS` | `7` | Age limit for the sweep, in days. A cache qualifies when it is older than this limit by the [age rule](#age-rule), which counts whole days as `find -mtime +N` does, so `7` means 8 days or more. `0` turns the sweep off. A value containing anything other than the digits 0 to 9, such as `+7`, `7d` or a value with a leading space, also turns the sweep off. Unset or empty means `7`. |
| `AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB` | `5` | When the wrapped command fails and the cache's filesystem has less than this many GiB free, the wrapper prints an `OUT OF DISK SPACE` report. Otherwise it prints one line giving the free space. |
| `CARGO_TARGET_DIR` | unset | When set and not empty, passed to the command unchanged, and the wrapper skips the sweep, the cache tag and the dangling-link repair. See [Explicit CARGO_TARGET_DIR](#explicit-cargo_target_dir). |

## Order of operations

`scripts/git-hook-cargo.sh <command> [args...]` does the following:

1. Removes git's repository variables, such as `GIT_DIR` and `GIT_INDEX_FILE`,
   from the environment (#1278).
2. Chooses the cache path and exports it as `CARGO_TARGET_DIR`, unless the
   caller has already set that variable.
3. Sweeps stale caches in the cache root, when the TTL is a positive whole
   number.
4. Removes a dangling link at the chosen cache path.
5. Writes `CACHEDIR.TAG` into the chosen cache directory when it has none.
6. Runs the command.
7. After a failure, reports the free space on the cache's filesystem.

Steps 3 to 5 apply only to a cache path the wrapper chose. Steps 4 and 5 run
whatever the TTL.

The command's exit status is the wrapper's exit status. Every wrapper message
goes to stderr, so stdout contains only the command's output. A run prints
nothing when the command succeeds, the cache exists and has a tag, and nothing
needs reclaiming or repairing.

## Stale-cache sweep

### Entries considered

The sweep looks only at the top level of the cache root, and only at entries
whose names begin with `amplihack-precommit-target-`. It lists:

- directories whose own modification time is older than the TTL; and
- symbolic links that you own, whatever the age of the link itself.

"You" means the effective user running the hook. `find` writes the list
NUL-separated. The sweep skips any listed path whose text does not begin with
`<cache root>/amplihack-precommit-target-`. This is a string test on the path
as listed, made without looking at the file system again, so a newline in an
entry's name cannot direct a removal at another path. The sweep classifies each
entry as a directory, a link or a dangling link only when it reaches that
entry, because an earlier removal in the same sweep can change what a link
points at.

The listing is only a first filter. The rules in the sections below decide what
is removed, and every one of them applies the [age rule](#age-rule) and the
[in-use check](#in-use-check), except the rule for dangling links.

The sweep never touches this run's cache or any entry without the prefix.
Outside the cache root, it removes only the resolved directory of a link that
passes every check in [Symbolic links](#symbolic-links).

### Age rule

A cache is older than the TTL only when each of the following paths that
exists was last modified more than TTL whole days ago, as `find -mtime +N`
counts it:

- the cache directory itself, or for a link the resolved directory;
- its `debug` directory;
- its `release` directory;
- its `.rustc_info.json` file.

A path that does not exist is ignored. If any existing path was modified within
the limit, the cache is kept.

Cargo writes its build output inside `debug/` and `release/`. The cache
directory's own modification time changes only when an entry directly inside it
is added or removed, so a cache that is built every day can still have a
top-level modification time several weeks old.

### In-use check

A cache is in use when a build holds one of cargo's lock files in it. Cargo
holds the lock file in the profile directory, such as `debug/.cargo-lock`, for
the length of a build, and `<target triple>/debug/.cargo-lock` as well for a
cross build. It does not hold a `.cargo-lock` at the top of the cache. The
wrapper therefore probes every lock file that exists at these three depths:

```text
<dir>/.cargo-lock
<dir>/*/.cargo-lock
<dir>/*/*/.cargo-lock
```

For a link, `<dir>` is the resolved directory. For each path, the wrapper first
tests that the file exists, and then requests an exclusive lock with
`flock --nonblock --exclusive`, releasing it at once. If any request fails, the
cache is in use. The existence test comes first because `flock` creates a
missing file, and a lock on a file it has just created always succeeds. A cache
that has none of these files is not in use.

Where `flock` is not installed, the wrapper asks `lsof +D <dir>` instead, and
the cache is in use when `lsof` lists any process. With neither tool, every
cache counts as in use.

### Plain directories

A prefix-named directory that is not a link is removed with `rm -rf` when:

1. it is not this run's cache;
2. it is older than the TTL by the [age rule](#age-rule); and
3. it is not in use by the [in-use check](#in-use-check).

Plain directories are not checked for cargo-target markers. Each removal prints
one line:

```text
amplihack: reclaiming build cache untouched for 7+ days: /home/dev/.cache/amplihack/amplihack-precommit-target-9b0d4e3f12a6
```

### Symbolic links

Some hosts keep cargo target directories on a separate volume: a local cargo
wrapper replaces each new target directory with a link to a directory such as
`/d0/dev/cargo-targets/<name>`. The cache entry is then a link, and the space
is used by the directory it points at. This page calls that directory the
**resolved directory**: the physical path that `cd -P` reaches through the
link, as printed by `pwd -P`.

The sweep checks each prefix-named link in this order and stops at the first
condition that does not hold. A failure at check 2 removes the link, as
described in [Dangling links](#dangling-links). A failure at any other check
leaves the link and its target in place and prints nothing.

| # | Condition |
| --- | --- |
| 1 | You own the link. The owner is read from the link itself, not from its target. |
| 2 | The link's target exists. |
| 3 | The target is a directory. A link to a file is never removed. |
| 4 | You own the cache root. In a shared root such as `/tmp`, links are never followed. |
| 5 | The link resolves to a physical path. |
| 6 | The resolved path is absolute and is not `/`. |
| 7 | The resolved directory is not the cache root or an ancestor of it. |
| 8 | The resolved directory is not `$HOME`, the hook's working directory (normally the checkout), or an ancestor of either. |
| 9 | The resolved directory is not this run's cache, a directory inside it, or a directory that contains it. |
| 10 | You own the resolved directory. |
| 11 | The resolved directory is older than the TTL by the [age rule](#age-rule). The link's own age is ignored. |
| 12 | The resolved directory has a cargo-target marker (see [Cargo-target markers](#cargo-target-markers)). |
| 13 | The resolved directory is not in use by the [in-use check](#in-use-check). |
| 14 | The resolved path still names the directory the link points at. This rejects a link that changed during the sweep and a path that ended in a newline. |

When every condition holds, the wrapper prints one line naming the link and the
resolved directory, removes the resolved directory with `rm -rf`, and then
removes the link with `rm -f` once the directory has gone. It never runs
`rm -rf` on the link path.

```text
amplihack: reclaiming build cache untouched for 7+ days: /home/dev/.cache/amplihack/amplihack-precommit-target-00e7e95cdab9 -> /d0/dev/cargo-targets/amplihack-amplihack-precommit-target-00e7e95cdab9-d3a0998953ff
```

### Cargo-target markers

The resolved directory of a link counts as a cargo target directory when it
contains at least one of these:

| Marker | Kind | Written by |
| --- | --- | --- |
| `CACHEDIR.TAG` | file, any content | cargo when it creates the directory, or this wrapper (see [Cache tag](#cache-tag)) |
| `.rustc_info.json` | file | cargo, during a build |
| `debug/.fingerprint` | directory | cargo, for a debug build |
| `release/.fingerprint` | directory | cargo, for a release build |

A bare `debug/` or `release/` directory is not a marker.

### Dangling links

A prefix-named link that you own and whose target no longer exists is removed
with `rm -f`, whatever its age:

```text
amplihack: removing dangling build cache link: /home/dev/.cache/amplihack/amplihack-precommit-target-0e941ec7b3c9
```

Dangling links owned by another user are left in place. The sweep is the only
step that removes dangling links in the cache root, so a TTL of `0` leaves
them. A dangling link at this run's own cache path is handled separately, as
described in [Dangling link at the cache path](#dangling-link-at-the-cache-path).

## Cache tag

Before the command runs, the wrapper writes cargo's cache directory tag into
the chosen cache directory if it has none. The file is
`<cache path>/CACHEDIR.TAG`; when the cache path is a link, the file is written
into the resolved directory. It contains three lines, each ending in a newline:

```text
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/
```

Cargo writes this file only when it creates the target directory itself. A
directory created beforehand, for example by a host wrapper that places it on
another volume, would otherwise never be tagged. Tools that honour the tag
include `tar --exclude-caches`, `borg create --exclude-caches` and
`restic backup --exclude-caches`.

The tag is written only when all of the following hold:

- the caller did not set `CARGO_TARGET_DIR`;
- the cache path exists and is a directory or a link to one;
- you own that directory and, when the cache path is a link, the link as well;
- nothing exists at `CACHEDIR.TAG` in that directory, not even a dangling link.

The file is opened with noclobber (`set -C`), so the wrapper never overwrites
an existing tag, whatever its content, or one that another process writes at
the same moment. It does not create the cache directory or change its mode or
owner. Tagging prints nothing and runs whatever the TTL.

Show the tag for the current checkout:

```console
$ cat "$(AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 scripts/git-hook-cargo.sh printenv CARGO_TARGET_DIR)/CACHEDIR.TAG"
Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/
```

> **Warning:** Point the cache path, or a link at it, only at a directory that
> is used for nothing else. The tag is written into the resolved directory, so
> backup tools that honour it skip that whole directory. The tag is also a
> cargo-target marker, so once the directory is older than the TTL, a sweep
> run from another checkout can remove it.

## Dangling link at the cache path

When the chosen cache path is a link whose target no longer exists, cargo
cannot create its target directory and the build fails. The wrapper therefore
removes such a link before the command runs, provided you own the link:

```text
amplihack: removed dangling build cache link before the build: /home/dev/.cache/amplihack/amplihack-precommit-target-48029401020f
```

Cargo then creates a fresh directory at the cache path and writes its own
`CACHEDIR.TAG` into it. On a host whose cargo wrapper places target directories
on another volume, that wrapper may create a new link instead.

A dangling link owned by another user is left in place, and the build fails as
it did before. A cache path that is a file, or a link to a file, is also left
alone. The repair runs whatever the TTL.

## Explicit CARGO_TARGET_DIR

A `CARGO_TARGET_DIR` that the caller sets to a non-empty value is passed to the
command unchanged:

```console
$ CARGO_TARGET_DIR="$HOME/.cache/artifact-guard-target" scripts/git-hook-cargo.sh printenv CARGO_TARGET_DIR
/home/dev/.cache/artifact-guard-target
```

The wrapper then does not sweep any cache root, write a cache tag, or remove a
dangling link at that path. An untagged directory or a dangling link at an
explicit path is left as it is. The free-space report after a failure still
applies, without its `rm -rf` hint.

## Reclaiming space by hand

### Run the sweep with a short limit

The wrapper is the only scripted route. From any checkout:

```bash
AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=1 scripts/git-hook-cargo.sh true
```

This applies every rule on this page with a limit of one day, so it removes
idle caches whose paths under the [age rule](#age-rule) were all last modified
two or more days ago, and then runs `true`. `1` is the shortest limit,
because `0` turns the sweep off. Each removal prints one line on stderr.

### Remove one entry by hand

Work on one cache entry at a time, with no build running in that cache. Do not
loop or glob over the entries in the cache root.

1. Set the cache root, list it and choose one entry:

    ```bash
    root="${XDG_CACHE_HOME:-$HOME/.cache}/amplihack"
    ls -l -- "$root"
    entry="$root/amplihack-precommit-target-00e7e95cdab9"
    ```

    If the wrapper fell back to `${TMPDIR:-/tmp}` (see
    [Cache location](#cache-location)), set `root` to that directory instead.
    The cache root in use is the directory part of the path that the
    `printenv` command in that section prints.

2. If `ls -ld -- "$entry"` shows a directory (the mode begins with `d`), check
   that no build holds it:

    ```bash
    find "$entry" -maxdepth 3 -name .cargo-lock -type f \
        ! -exec flock --nonblock --exclusive {} true \; -print
    ```

    The command prints each lock file that a build holds. It probes only lock
    files that exist, so it never creates one. It must print nothing. Where
    `flock` is not installed, `lsof +D "$entry"` must print nothing instead.
    Then remove the directory:

    ```bash
    rm -rf -- "$entry"
    ```

3. If the mode begins with `l`, the entry is a link. Check that its target
   exists:

    ```bash
    [ -e "$entry" ] && echo exists || echo dangling
    ```

    If this prints `dangling`, remove the link with `rm -- "$entry"` and stop.
    Otherwise, resolve the link:

    ```bash
    target="$(cd -P -- "$entry" && pwd -P)" && printf '%s\n' "$target"
    ```

    If this fails, the link points at a file or at a directory you cannot
    enter: leave it in place and stop. Also stop if it prints `/`, or a
    directory that is or contains your home directory, a checkout or the cache
    root.

4. Check that you own the resolved directory and that it has a cargo-target
   marker:

    ```bash
    ls -ld -- "$target"
    ls -d -- "$target/CACHEDIR.TAG" "$target/.rustc_info.json" \
        "$target/debug/.fingerprint" "$target/release/.fingerprint" 2>/dev/null
    ```

    The first command must show your user name as the owner. The second must
    print at least one path.

5. Check that no build holds the directory:

    ```bash
    find "$target" -maxdepth 3 -name .cargo-lock -type f \
        ! -exec flock --nonblock --exclusive {} true \; -print
    ```

    As in step 2, this must print nothing. Then remove the directory, and
    remove the link once the directory has gone:

    ```bash
    rm -rf -- "$target"
    [ ! -e "$target" ] && rm -- "$entry"
    ```

Do not run `rm -rf` on the link path, and do not add a trailing `/` to it.

## Limitations

- The out-of-space report suggests `rm -rf <cache root>/amplihack-precommit-target-*`.
  For caches that are links, that command removes only the links and leaves
  their directories on the other volume. The report also prints the cache
  root without quotes. Use one of the routes in
  [Reclaiming space by hand](#reclaiming-space-by-hand) instead.
- If the disk fills while the tag is being written, a partial `CACHEDIR.TAG`
  remains, because the wrapper never overwrites an existing tag. Delete it once
  space is free; the next run writes a complete one.
- The ownership checks use `[ -O ]` and `find -user`. When the hook runs as
  root, these pass for every path that root owns, system directories included,
  and fail for paths owned by other users. For a root-owned link target, checks
  6 to 9 in [Symbolic links](#symbolic-links) and the cargo-target markers of
  check 12 are then what keep the sweep inside cache directories. The age rule
  and the in-use check still apply.
- `rm -rf` descends into any filesystem mounted inside a cache directory, for
  plain directories and resolved directories alike.
- The checks and the removal name the resolved directory by its path. A process
  that can write to a parent directory of that path can replace one of its
  components between the checks and the `rm -rf`, which then removes whatever
  the path names at that moment. Check 14 narrows this window but does not
  close it.
- The in-use check and the removal are separate steps. A build that starts
  between them loses its cache and fails.
- The age rule reads four modification times. A build that adds or removes
  nothing directly inside the cache directory, `debug/` or `release/`, and does
  not rewrite `.rustc_info.json`, leaves all four unchanged. This can happen
  with `cargo check` or `cargo clippy`. A cache used only in that way can be
  reclaimed while idle, and is rebuilt on its next use.
- With neither `flock` nor `lsof` installed, the sweep removes only dangling
  links.
- Links owned by another user, and links in a cache root you do not own, are
  never followed, so their directories are never reclaimed. This includes the
  TMPDIR fallback, whose root, such as `/tmp`, normally belongs to root. A
  per-user directory for the fallback is left to a follow-up change.
- Messages print paths as they are, without escaping. A cache name or link
  target that contains a newline or a terminal control character appears
  unaltered on stderr, and can make one message look like two.

## Related

- [Artifact Guard](../artifact-guard.md): the pre-commit hook that runs
  through this wrapper and requires build output outside the repository.
- [Developing amplihack](../howto/develop-amplihack.md): local builds, tests
  and hook troubleshooting.
- `tests/issue_1440_precommit_cache_location.sh` covers the cache location,
  the sweep of plain directories and the free-space report.
  `tests/issue_1771_precommit_cache_symlinks.sh` covers links, tags, dangling
  links, the age rule and the in-use check.
