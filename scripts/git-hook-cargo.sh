#!/usr/bin/env bash
# Run a cargo command from inside a git hook (issues #1278, #1381, #1440, #1771).
#
# Three kinds of ambient global state silently override what the caller meant.
# This script removes all three, in an order that matters -- see below.
#
# 1. git's repository selection (issue #1278)
#
# git exports the repository it is working on into every hook it runs: GIT_DIR,
# and depending on the hook GIT_INDEX_FILE, GIT_PREFIX, GIT_WORK_TREE and
# friends. cargo inherits them, every test binary inherits them, and every git
# subprocess those binaries spawn inherits them too. Those variables outrank the
# working directory when git picks a repository, so a test that builds a temp
# repo and shells out to git silently operates on amplihack's own repository
# instead -- and the pre-push `cargo test` gate becomes unpassable, which is
# what trains people into `--no-verify`.
#
# Removing them restores the plain, correct meaning of "the repository is the
# one you pointed at". Identity variables (GIT_AUTHOR_*, GIT_COMMITTER_*) are
# deliberately left alone: they select an author, not a repository.
#
# This list is the shell twin of amplihack_git::REPOSITORY_ENV_VARS.
# tests/issue_1278_git_env_isolation.sh reads that constant out of the Rust
# source and checks this script scrubs every entry, so adding a variable to one
# side without the other fails CI.
#
# 2. the build cache (issue #1381)
#
# The hooks used to share one hardcoded CARGO_TARGET_DIR across every worktree,
# clone and parallel agent on the host. cargo then reused another branch's
# compiled metadata against your source, producing failures that belong to
# somebody else's code -- an E0061 for a function signature that does not exist
# on your branch, or E0460 "possibly newer version of crate" on doctests.
#
# The path below is therefore derived from the checkout root and MUST STAY
# checkout-specific. Shortening it back to a constant is the obvious
# "simplification" and it reintroduces the bug; two checkouts sharing a cache is
# the whole defect. A given checkout still reuses its own cache across runs,
# which is the reason to set the variable at all.
#
# 3. the disk those caches live on (issue #1440)
#
# Per-checkout caches are 6-16 GB each and nothing ever removed them, so they
# accumulated on /tmp -- observed three times in one session at 12 dirs/122 GB,
# 5/57 GB and 7/62 GB, with /tmp at 98-99% full. cargo then died with
#
#     error: failed to write ... dep-graph.part.bin: No space left on device
#     error: could not compile `amplihack-hive` (lib) due to 1 previous error
#
# which reads as a compiler problem and is a full disk. Two agents diagnosed it
# from scratch in one night. Three things follow, and each is load-bearing:
#
#   a. The default cache lives under ${XDG_CACHE_HOME:-$HOME/.cache}, not under
#      /tmp. /home had 300+ GB free every single time /tmp was full, and
#      ~/.cache is the conventional home for a rebuildable cache. On hosts with
#      no writable HOME (some containers) we fall back to TMPDIR rather than
#      failing the hook.
#   b. Stale sibling caches are reclaimed -- see reclaim_stale_caches().
#   c. A failing build reports the free space on the cache's filesystem, so
#      "no space left" can never again present itself as "could not compile".
#
# 4. caches that are links to another volume (issue #1771)
#
# Some hosts wrap cargo so that each new target directory becomes a symbolic
# link to a directory on a larger volume. The sweep in (3b) listed candidates
# with `find -type d`, which does not match links, so one host built up 73 of
# these caches holding 321 GiB and none was ever reclaimed. cargo writes
# CACHEDIR.TAG only into a directory it creates itself, so none was tagged
# either, and a link whose target had gone made the next build in that checkout
# fail. docs/reference/pre-commit-build-cache.md lists every rule below.
#
#   d. The sweep follows prefix-named links. Entries are listed NUL-separated
#      and must carry the prefix, so a newline in a name cannot redirect a
#      removal. A link is followed only when the user owns the link, the cache
#      root and the resolved directory. The resolved directory is skipped when
#      it is or contains the cache root, HOME or the checkout, overlaps this
#      run's cache, or no longer names the directory the link points at.
#      Otherwise, when it is stale, idle and carries a cargo marker
#      (CACHEDIR.TAG, .rustc_info.json, debug/.fingerprint or
#      release/.fingerprint), it is removed, and then the link.
#   e. Staleness reads debug/, release/ and .rustc_info.json as well as the
#      cache directory itself, for plain directories and links alike, and a
#      cache is in use when a build holds any .cargo-lock up to two levels
#      down. cargo writes and locks inside the profile directories; the top
#      level alone says nothing about the last build.
#   f. A prefix-named link the user owns whose target has gone is removed by
#      the sweep, whatever its age.
#   g. Before the build, a dangling link at the chosen path is removed so that
#      cargo can create the directory again, and a chosen directory with no
#      CACHEDIR.TAG is given cargo's tag so that backup and clean-up tools skip
#      it. Both run whatever the TTL and only on a path the user owns, and
#      neither touches an explicit CARGO_TARGET_DIR.
set -euo pipefail

unset \
    GIT_ALTERNATE_OBJECT_DIRECTORIES \
    GIT_CEILING_DIRECTORIES \
    GIT_COMMON_DIR \
    GIT_DIR \
    GIT_DISCOVERY_ACROSS_FILESYSTEM \
    GIT_INDEX_FILE \
    GIT_INDEX_VERSION \
    GIT_NAMESPACE \
    GIT_OBJECT_DIRECTORY \
    GIT_PREFIX \
    GIT_QUARANTINE_PATH \
    GIT_WORK_TREE

# Reclaim caches that have not been touched in this many days. cargo writes into
# the target dir throughout a build, so an untouched-for-days cache belongs to a
# checkout nobody is building any more (older_than_ttl says what "touched"
# reads). 0 disables the sweep, and so does any value that is not a whole
# number, which would otherwise reach find as a malformed -mtime.
CACHE_TTL_DAYS="${AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS:-7}"
# Below this much free space on the cache's filesystem, a build failure is
# reported as a disk problem rather than a code problem.
CACHE_MIN_FREE_GIB="${AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB:-5}"
CACHE_PREFIX="amplihack-precommit-target-"

# Order matters: the checkout root is resolved only AFTER the scrub above.
# Asking git which repository we are in while GIT_DIR is still set would answer
# with the hook's repository rather than this checkout -- the very confusion
# this script exists to remove -- and every worktree driven by the same hook
# would collide on one cache again.
checkout_hash() {
    local root digest
    root="$(git rev-parse --show-toplevel 2>/dev/null)" || root="$PWD"
    # sha256sum is coreutils; shasum ships with macOS.
    if command -v sha256sum >/dev/null 2>&1; then
        digest="$(printf '%s' "$root" | sha256sum)"
    else
        digest="$(printf '%s' "$root" | shasum -a 256)"
    fi
    printf '%s' "${digest:0:12}"
}

# Where the per-checkout caches live. A rebuildable multi-gigabyte cache belongs
# in ~/.cache, not on /tmp: /tmp is small, shared by every process on the host,
# and filling it breaks everyone. TMPDIR remains the fallback for a host with no
# usable HOME -- a hook that refuses to run is worse than one on the old path.
cache_root() {
    local base
    base="${XDG_CACHE_HOME:-${HOME:+$HOME/.cache}}"
    if [ -n "$base" ] && mkdir -p "$base/amplihack" 2>/dev/null; then
        printf '%s' "$base/amplihack"
        return 0
    fi
    printf '%s' "${TMPDIR:-/tmp}"
}

# Is a build writing into this cache right now?
#
# NOT `fuser -m "$dir"`. That reports on the whole FILESYSTEM the directory sits
# on, not the directory, so on a shared mount it calls every cache busy and
# tells you nothing. It cost a round of misdiagnosis.
#
# cargo takes an exclusive flock for the duration of a build on the lock file in
# the profile directory, <target>/debug/.cargo-lock, and for a cross build on
# <target>/<triple>/debug/.cargo-lock as well. It holds nothing at the top of
# the target dir. Asking for those same locks is exact and costs nothing, so
# every .cargo-lock at the top and one and two levels down is probed. Only files
# that exist are probed: flock(1) creates a missing file, and a lock on a file
# it has just created always succeeds.
#
# Where flock(1) is missing we fall back to `lsof +D`, which does look at this
# directory (it correctly reported open_fds=0 for the caches on /tmp). With
# neither tool we decline to delete: deleting a cache out from under a live
# build produces failures that look like code defects, which is far more
# expensive than a stale directory left on disk.
cache_in_use() {
    local dir="$1" lock
    if command -v flock >/dev/null 2>&1; then
        for lock in "$dir/.cargo-lock" "$dir"/*/.cargo-lock "$dir"/*/*/.cargo-lock; do
            [ -f "$lock" ] || continue
            flock --nonblock --exclusive "$lock" true 2>/dev/null || return 0
        done
        return 1
    fi
    if command -v lsof >/dev/null 2>&1; then
        [ -n "$(lsof -t +D "$dir" 2>/dev/null)" ] && return 0
        return 1
    fi
    return 0
}

# The physical directory a path names, as `cd -P` reaches it (#1771). Fails for
# a dangling link or anything that is not a directory. readlink -f is GNU-only,
# so this is the portable form. An empty argument and a bare "-" are refused:
# `cd ''` stays where it is and `cd -P -- -` goes to OLDPWD, so either would
# resolve to some unrelated directory.
resolve_dir() {
    case "$1" in
        '' | -) return 1 ;;
    esac
    (CDPATH='' cd -P -- "$1" 2>/dev/null && pwd -P)
}

# Is the physical path $1 equal to $2 or below it? "/a/bc" is not below "/a/b".
path_within() {
    [ "$2" = / ] && return 0
    [ "$1" = "$2" ] && return 0
    case "$1" in
        "$2"/*) return 0 ;;
    esac
    return 1
}

# Does the user own the link itself? `[ -O ]` follows the link and reports the
# owner of its target, which is the wrong question for a link someone else may
# have planted.
owned_link() {
    [ -n "$(find "$1" -maxdepth 0 -type l -user "$EUID" -print 2>/dev/null)" ]
}

# Has nobody built in this cache for more than CACHE_TTL_DAYS days? The cache
# directory's own modification time changes only when an entry directly inside
# it is added or removed, so a cache built every day can look weeks old from the
# top. cargo writes its output inside debug/ and release/ and rewrites
# .rustc_info.json, so each of those that exists must be old too. find -mtime
# gives the same whole-day rounding as the listing in reclaim_stale_caches().
older_than_ttl() {
    local dir="$1" path
    [ -n "$(find "$dir" -maxdepth 0 -type d -mtime "+$CACHE_TTL_DAYS" -print 2>/dev/null)" ] ||
        return 1
    for path in "$dir/debug" "$dir/release" "$dir/.rustc_info.json"; do
        [ -e "$path" ] || continue
        [ -n "$(find "$path" -maxdepth 0 -mtime "+$CACHE_TTL_DAYS" -print 2>/dev/null)" ] ||
            return 1
    done
    return 0
}

# Does this directory look like a cargo target dir? A link can point anywhere,
# so its target is removed only if cargo has visibly written there. A bare
# debug/ or release/ is too common a name to count; the fingerprint directory
# inside it is cargo's own.
looks_like_cargo_target() {
    [ -f "$1/CACHEDIR.TAG" ] || [ -f "$1/.rustc_info.json" ] ||
        [ -d "$1/debug/.fingerprint" ] || [ -d "$1/release/.fingerprint" ]
}

# One prefix-named link in the cache root (#1771). The space a symlinked cache
# uses is in the directory it resolves to, often on another volume, so that is
# what is aged, checked and removed. Following a link is the one place this
# script can reach outside the cache root, so every check below must pass, in
# this order; the table in docs/reference/pre-commit-build-cache.md numbers
# them. ROOT_R, KEEP_R, HOME_R and PWD_R are physical paths resolved once per
# sweep, and FOLLOW is 1 only when the user owns the cache root.
reclaim_cache_link() {
    local link="$1" root_r="$2" keep_r="$3" home_r="$4" pwd_r="$5" follow="$6"
    local target
    # Another user's link is theirs to clean up, dangling or not.
    owned_link "$link" || return 0
    # A dangling link holds no space, and left in place at a checkout's cache
    # path it breaks that checkout's next build. Its age does not matter.
    if [ ! -e "$link" ]; then
        printf 'amplihack: removing dangling build cache link: %s\n' "$link" >&2
        rm -f -- "$link" 2>/dev/null || true
        return 0
    fi
    [ -d "$link" ] || return 0
    # In a root another user can write to, such as /tmp, anyone can plant a
    # prefix-named link, so none is followed.
    [ "$follow" = 1 ] || return 0
    target="$(resolve_dir "$link")" || return 0
    case "$target" in
        /?*) ;;
        *) return 0 ;;
    esac
    # Never the cache root or an ancestor of it, which would take this run's
    # cache and every sibling with it; never HOME or the checkout.
    path_within "$root_r" "$target" && return 0
    [ -n "$home_r" ] && path_within "$home_r" "$target" && return 0
    [ -n "$pwd_r" ] && path_within "$pwd_r" "$target" && return 0
    # Never this run's cache, from either side.
    path_within "$keep_r" "$target" && return 0
    path_within "$target" "$keep_r" && return 0
    # Ownership comes before anything reads inside the directory.
    [ -O "$target" ] || return 0
    older_than_ttl "$target" || return 0
    # Skipping a directory that is not a cargo target prints nothing; it would
    # otherwise print on every run.
    looks_like_cargo_target "$target" || return 0
    cache_in_use "$target" && return 0
    # Command substitution drops trailing newlines, and the link can change
    # while the checks run. Either way the path checked so far may no longer
    # name the link's directory, and then nothing is removed.
    [ "$target" -ef "$link" ] || return 0
    printf 'amplihack: reclaiming build cache untouched for %s+ days: %s -> %s\n' \
        "$CACHE_TTL_DAYS" "$link" "$target" >&2
    rm -rf -- "$target" 2>/dev/null || true
    # The link goes only once its directory has: a link left behind is cleaned
    # up as dangling on a later run, whereas a directory left without its link
    # would never be found again. rm -rf is never run on the link path: with a
    # trailing slash, some systems follow it into the directory behind it.
    if [ ! -e "$target" ]; then
        rm -f -- "$link" 2>/dev/null || true
    fi
    return 0
}

# Age-based (LRU) reclaim. Every worktree gets its own hash, so parallel agents
# multiply these fast; without this the only thing that ever frees the space is
# a human noticing a full disk.
#
# find lists stale prefix-named directories and every prefix-named link the user
# owns, whatever the link's own age (#1771). Each entry is classified only when
# the loop reaches it: an earlier removal in the same sweep can change what a
# link points at. The list is NUL-separated and each entry must still begin with
# the root and the prefix, so a newline in a name cannot turn into a relative
# path that rm would resolve against the checkout.
reclaim_stale_caches() {
    local root="$1" keep="$2" dir root_r keep_r home_r pwd_r follow=0
    case "$CACHE_TTL_DAYS" in
        '' | *[!0-9]*) return 0 ;;
    esac
    [ "$CACHE_TTL_DAYS" -gt 0 ] 2>/dev/null || return 0
    [ -d "$root" ] || return 0
    root_r="$(resolve_dir "$root")" || return 0
    # This run's cache may not exist yet; its physical path is then the one
    # cargo will create.
    keep_r="$(resolve_dir "$keep")" || keep_r="$root_r/${keep##*/}"
    home_r="$(resolve_dir "${HOME:-}")" || home_r=""
    pwd_r="$(pwd -P 2>/dev/null)" || pwd_r=""
    if [ -O "$root_r" ]; then
        follow=1
    fi
    while IFS= read -r -d '' dir; do
        case "$dir" in
            "$root/$CACHE_PREFIX"*) ;;
            *) continue ;;
        esac
        [ "$dir" = "$keep" ] && continue
        if [ -L "$dir" ]; then
            reclaim_cache_link "$dir" "$root_r" "$keep_r" "$home_r" "$pwd_r" "$follow"
        elif [ -d "$dir" ] && older_than_ttl "$dir"; then
            cache_in_use "$dir" && continue
            printf 'amplihack: reclaiming build cache untouched for %s+ days: %s\n' \
                "$CACHE_TTL_DAYS" "$dir" >&2
            rm -rf -- "$dir" 2>/dev/null || true
        fi
    done < <(find "$root" -mindepth 1 -maxdepth 1 -name "$CACHE_PREFIX*" \
        \( \( -type d -mtime "+$CACHE_TTL_DAYS" \) -o \( -type l -user "$EUID" \) \) \
        -print0 2>/dev/null)
    return 0
}

# Make the chosen cache usable and recognisable before the build (#1771). Runs
# whatever the TTL, and only for a path this script chose.
#
# A dangling link here leaves CARGO_TARGET_DIR pointing at nothing and cargo
# fails to create its target dir, so the link goes; cargo then creates and tags
# a fresh directory. Another user's link is left for cargo to fail on.
#
# cargo writes CACHEDIR.TAG only into a directory it creates itself. When a host
# wrapper created the directory first, nothing tags it and backup and clean-up
# tools treat it as data, so the wrapper writes cargo's tag. It writes only into
# a directory the user owns, reached through a link the user owns, and never
# replaces or writes through anything already at the tag path: the existence
# test keeps a FIFO from blocking the open, and noclobber (O_EXCL) loses any
# race with a concurrent writer. The directory itself is never created here.
prepare_owned_cache() {
    local dir="$1" tag
    if [ -L "$dir" ] && [ ! -e "$dir" ]; then
        owned_link "$dir" || return 0
        if rm -f -- "$dir"; then
            printf 'amplihack: removed dangling build cache link before the build: %s\n' \
                "$dir" >&2
        fi
        return 0
    fi
    [ -d "$dir" ] || return 0
    [ -O "$dir" ] || return 0
    if [ -L "$dir" ]; then
        owned_link "$dir" || return 0
    fi
    tag="$dir/CACHEDIR.TAG"
    if [ -e "$tag" ] || [ -L "$tag" ]; then
        return 0
    fi
    (
        set -C
        printf '%s\n' \
            'Signature: 8a477f597d28d172789f06886806bc55' \
            '# This file is a cache directory tag created by cargo.' \
            '# For information about cache directory tags see https://bford.info/cachedir/' \
            >"$tag"
    ) 2>/dev/null || true
    return 0
}

# `df` on the nearest existing ancestor: the cache directory itself may not
# exist yet, and asking about a missing path answers nothing.
report_cache_space() {
    local dir="$1" probe="$1" avail mount
    while [ -n "$probe" ] && [ "$probe" != "/" ] && [ ! -d "$probe" ]; do
        probe="$(dirname "$probe")"
    done
    avail=""
    mount=""
    read -r avail mount < <(df -Pk "$probe" 2>/dev/null | awk 'NR==2 {print $4, $6}') || true
    [ -n "$avail" ] || return 0

    local avail_gib
    avail_gib="$(awk -v k="$avail" 'BEGIN { printf "%.1f", k / 1048576 }')"

    if awk -v k="$avail" -v min="$CACHE_MIN_FREE_GIB" \
        'BEGIN { exit !(k / 1048576 < min) }'; then
        {
            echo
            echo "==============================================================="
            echo "amplihack: OUT OF DISK SPACE -- this is not a compile error."
            echo
            printf '  build cache : %s\n' "$dir"
            printf '  filesystem  : %s -- %s GiB free (want %s GiB)\n' \
                "$mount" "$avail_gib" "$CACHE_MIN_FREE_GIB"
            echo
            echo "  cargo reports a full disk as 'No space left on device"
            echo "  (os error 28)' and then 'could not compile <crate>'."
            echo "  Read that as storage, not as code (issue #1440)."
            echo
            if [ "$CACHE_OWNED" -eq 1 ]; then
                echo "  These caches are rebuildable. With no build running:"
                printf '    rm -rf %s/%s*\n' "$(dirname "$dir")" "$CACHE_PREFIX"
            fi
            echo "==============================================================="
            echo
        } >&2
    else
        printf 'amplihack: build cache %s -- %s GiB free on %s.\n' \
            "$dir" "$avail_gib" "$mount" >&2
    fi
}

# An explicitly provided CARGO_TARGET_DIR still wins: this is a default, not an
# override. Ambient state overriding explicit intent is the bug, not the fix.
# Reclaim, tagging and the dangling-link repair only ever run over caches this
# script owns, so an explicit directory is never swept, tagged or repaired.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    export CARGO_TARGET_DIR
    CACHE_OWNED=0
else
    CACHE_OWNED=1
    _cache_root="$(cache_root)"
    _cache_dir="$_cache_root/$CACHE_PREFIX$(checkout_hash)"
    export CARGO_TARGET_DIR="$_cache_dir"
    reclaim_stale_caches "$_cache_root" "$_cache_dir"
    prepare_owned_cache "$_cache_dir"
fi

# Deliberately not `exec`: the whole point of (3c) is to look at the disk after
# a failure, which a process that replaced itself cannot do.
set +e
"$@"
status=$?
set -e

if [ "$status" -ne 0 ]; then
    report_cache_space "$CARGO_TARGET_DIR"
fi

exit "$status"
