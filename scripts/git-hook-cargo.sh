#!/usr/bin/env bash
# Run a cargo command from inside a git hook (issues #1278, #1381, #1440, #1771).
#
# Three kinds of ambient global state silently override what the caller meant.
# This script removes all three, in an order that matters -- see below. The
# fourth section covers caches that are symbolic links (issue #1771).
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
# 4. symlinked caches and cache tagging (issue #1771)
#
# Hosts that keep build output on a separate volume replace each cache
# directory with a symbolic link into that volume. The #1440 sweep listed
# entries with `find -type d`, which does not match a link, so linked caches
# were never reclaimed and their targets filled the other volume. A dangling
# link at the current path, left behind when the volume was wiped, made cargo
# fail. And the entry list was read one line at a time, so a name holding a
# newline split into two paths and the second reached `rm -rf` relative to the
# working directory. The rules now are:
#
#   a. Entries are read NUL-delimited and must lie under an absolute cache root.
#   b. A link is followed, or removed, only when the current user owns both the
#      link and its target. A shared TMPDIR fallback is where this matters.
#   c. A stale link is reclaimed only when its target looks like a cargo target
#      dir (cargo's CACHEDIR.TAG, .rustc_info.json, or a debug/ or release/ dir
#      holding .fingerprint/ or .cargo-lock). Then the target goes first and the
#      link second; removing only the link frees nothing. Otherwise both stay.
#   d. Age is measured on the resolved directory to depth 2, because the link's
#      own timestamp does not move when its target is built, and cargo updates
#      debug/deps and debug/.fingerprint rather than the target root.
#   e. cargo keeps its build lock in the profile directory (debug/, release/),
#      not only at the target root, so both places are probed.
#   f. Nothing that resolves to /, HOME, the cache root, the current cache, or
#      an ancestor of those or of the checkout is ever deleted.
#   g. A dangling link at the current path is removed before the build, even
#      with the sweep disabled, so cargo can create a real directory there.
#      A dangling sibling link is removed only once the link itself is older
#      than the TTL, so a cache on a volume that is unmounted for a while is
#      not orphaned.
#   h. The current cache gets cargo's CACHEDIR.TAG when it exists without one,
#      so backup tools skip it. A missing cache is left for cargo to create,
#      an existing tag (or a link named CACHEDIR.TAG) is never written through,
#      and a failed write never fails the hook.
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

# Reclaim caches whose directory has not been touched in this many days. cargo
# writes into the target dir throughout a build, so an untouched-for-days
# directory belongs to a checkout nobody is building any more. 0 disables the
# sweep.
CACHE_TTL_DAYS="${AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS:-7}"
# Anything other than a plain decimal integer (-1, 3.5, week) disables the sweep
# like 0 does, with one warning so that a typo does not silently bring back the
# #1771 leak. Leading zeros are dropped so that shell arithmetic cannot read
# 08 as octal, and more than nine digits is treated as "never stale".
case "$CACHE_TTL_DAYS" in
    '' | *[!0-9]*)
        printf 'amplihack: ignoring invalid AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=%q; cache sweep disabled\n' \
            "$CACHE_TTL_DAYS" >&2
        CACHE_TTL_DAYS=0
        ;;
    *)
        if [ "${#CACHE_TTL_DAYS}" -gt 9 ]; then
            CACHE_TTL_DAYS=0
        else
            CACHE_TTL_DAYS=$((10#$CACHE_TTL_DAYS))
        fi
        ;;
esac
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
# cargo takes an exclusive flock on .cargo-lock for the duration of a build, so
# asking for that same lock is exact and costs nothing. The lock lives in the
# profile directory (<target>/debug/.cargo-lock, <target>/release/.cargo-lock)
# as well as at the target root, so every one of those is probed (#1771). Where
# flock(1) is missing we fall back to `lsof +D`, which does look at this
# directory (it correctly reported open_fds=0 for the caches on /tmp). With
# neither tool we decline to delete: deleting a cache out from under a live
# build produces failures that look like code defects, which is far more
# expensive than a stale directory left on disk.
cache_in_use() {
    local dir="$1" lock
    if command -v flock >/dev/null 2>&1; then
        # Without nullglob an unmatched pattern stays literal; the -e test
        # skips it.
        for lock in "$dir/.cargo-lock" "$dir"/*/.cargo-lock; do
            [ -e "$lock" ] || continue
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

# Print the physical path of an absolute directory, following links, or print
# nothing. CDPATH is cleared so an exported value cannot redirect the cd, and a
# relative path is refused outright.
resolve_dir() {
    case "$1" in
        /*) ;;
        *) return 0 ;;
    esac
    (CDPATH='' cd -P -- "$1" 2>/dev/null && pwd -P) || true
}

# Does this directory look like a cargo target dir? A bare debug/ or release/
# is not enough: plenty of non-cargo trees have one.
is_cargo_target() {
    local dir="$1" sig="" profile
    if [ -f "$dir/CACHEDIR.TAG" ] && [ ! -L "$dir/CACHEDIR.TAG" ]; then
        IFS= read -r sig < "$dir/CACHEDIR.TAG" 2>/dev/null || true
        [ "$sig" = "Signature: 8a477f597d28d172789f06886806bc55" ] && return 0
    fi
    [ -f "$dir/.rustc_info.json" ] && return 0
    for profile in debug release; do
        [ -d "$dir/$profile" ] || continue
        [ -d "$dir/$profile/.fingerprint" ] && return 0
        [ -e "$dir/$profile/.cargo-lock" ] && return 0
    done
    return 1
}

# Stale means nothing within two levels of the directory was modified in the
# last CACHE_TTL_DAYS days. cargo rewrites debug/deps and debug/.fingerprint on
# every build but may leave the target root alone, so the root's own mtime is
# not enough. `-mtime -N` is the complement of the `-mtime +TTL` test #1440
# used. Callers must have checked CACHE_TTL_DAYS is a positive integer.
# If find cannot read a subdirectory, its errors are discarded and the cache can
# be classed as stale; deletion still requires ownership, a Cargo-shaped target
# and a free lock.
is_stale() {
    [ -z "$(find "$1" -maxdepth 2 -mtime -$((CACHE_TTL_DAYS + 1)) -print -quit 2>/dev/null)" ]
}

# Has a link itself not been created or touched in the last CACHE_TTL_DAYS
# days? find does not follow links by default, so this reads the link's own
# mtime, which is all a dangling link has.
link_is_stale() {
    [ -z "$(find "$1" -maxdepth 0 -mtime -$((CACHE_TTL_DAYS + 1)) -print -quit 2>/dev/null)" ]
}

# Is this path, measured without following a link, owned by the current user?
# find does not follow links by default, so for a link this is the link's own
# owner. CI runs as one user, so a foreign-owned fixture cannot be built there;
# the test documents this rather than exercising it. MY_UID is set once in the
# main block; it is called for every entry, and `id -u` is a fork each time.
owned_by_me() {
    [ -n "$(find "$1" -maxdepth 0 -user "$MY_UID" 2>/dev/null)" ]
}

# Write cargo's CACHEDIR.TAG into an existing directory that has none. cargo
# writes the tag only into a target dir it creates, and this script's caches
# often exist before cargo runs (made by an earlier build, or turned into a link
# by a host wrapper), so issue #1771 requires the script to write it. Nothing
# is written when CACHEDIR.TAG exists or is a link, dangling or not, and the
# write itself runs under noclobber so it can neither follow a link created in
# the meantime nor replace a file. A failed write (read-only directory, full
# disk) is reported and otherwise ignored: a missing tag must not fail a hook.
write_cachedir_tag() {
    local dir="$1"
    [ -d "$dir" ] || return 0
    if [ -e "$dir/CACHEDIR.TAG" ] || [ -L "$dir/CACHEDIR.TAG" ]; then
        return 0
    fi
    (
        set -C
        printf '%s\n' \
            'Signature: 8a477f597d28d172789f06886806bc55' \
            '# This file is a cache directory tag created by cargo.' \
            '# For information about cache directory tags see https://bford.info/cachedir/' \
            > "$dir/CACHEDIR.TAG"
    ) 2>/dev/null || printf 'amplihack: could not write CACHEDIR.TAG in %q; continuing\n' \
        "$dir" >&2 || true
}

# Must this resolved path never be deleted?
#   is_protected_path <path> <current-cache> <guard>...
# True for an empty path, /, a path inside the current cache, and a path equal
# to or an ancestor of the current cache or any guard (cache root, HOME,
# checkout). Empty guards are ignored.
is_protected_path() {
    local path="$1" current="$2" guard
    shift
    case "$path" in
        '' | /) return 0 ;;
    esac
    if [ -n "$current" ]; then
        case "$path/" in
            "$current"/?*) return 0 ;;
        esac
    fi
    for guard in "$@"; do
        [ -n "$guard" ] || continue
        [ "$path" = "$guard" ] && return 0
        case "$guard/" in
            "$path"/*) return 0 ;;
        esac
    done
    return 1
}

# Age-based (LRU) reclaim. Every worktree gets its own hash, so parallel agents
# multiply these fast; without this the only thing that ever frees the space is
# a human noticing a full disk.
#
# Entries are directories or links (#1771). A link is reclaimed by deleting its
# target and then the link, but only when the target looks like a cargo target
# dir; see section 4 of the header for the full rule set.
reclaim_stale_caches() {
    local root="$1" keep="$2" entry resolved
    local real_root real_home="" real_keep="" real_top="" top
    [ "$CACHE_TTL_DAYS" -gt 0 ] || return 0
    root="${root%/}"
    case "$root" in
        /?*) ;;
        *) return 0 ;;
    esac
    [ -d "$root" ] || return 0
    real_root="$(resolve_dir "$root")"
    [ -n "$real_root" ] || return 0
    if [ -n "${HOME:-}" ]; then
        real_home="$(resolve_dir "$HOME")"
    fi
    if [ -d "$keep" ]; then
        real_keep="$(resolve_dir "$keep")"
    else
        real_keep="$(resolve_dir "$(dirname "$keep")")"
        [ -n "$real_keep" ] && real_keep="$real_keep/$(basename "$keep")"
    fi
    top="$(git rev-parse --show-toplevel 2>/dev/null)" || top=""
    [ -n "$top" ] && real_top="$(resolve_dir "$top")"

    # find lists only directories and links, and every path starts with the
    # absolute root checked above. The case below repeats what find already
    # guarantees; it is kept as the S1 guard in front of rm -rf.
    while IFS= read -r -d '' entry; do
        case "$entry" in
            "$root"/?*) ;;
            *) continue ;;
        esac
        [ "$entry" = "$keep" ] && continue
        owned_by_me "$entry" || continue
        if [ -L "$entry" ]; then
            if [ ! -e "$entry" ]; then
                # The target may be on a volume that is only unmounted for now.
                # Removing the link at once would orphan the target when the
                # volume returns, so wait until the link itself is past the TTL.
                link_is_stale "$entry" || continue
                printf 'amplihack: removing dangling build cache link: %q\n' "$entry" >&2
                rm -f -- "$entry" 2>/dev/null \
                    || printf 'amplihack: could not remove %q\n' "$entry" >&2
                continue
            fi
            # A link to a regular file is not a cache; leave it alone.
            [ -d "$entry" ] || continue
        fi

        resolved="$(resolve_dir "$entry")"
        is_protected_path "$resolved" "$real_keep" "$real_root" "$real_home" "$real_top" \
            && continue
        owned_by_me "$resolved" || continue
        is_stale "$resolved" || continue
        cache_in_use "$resolved" && continue

        if [ -L "$entry" ]; then
            if ! is_cargo_target "$resolved"; then
                printf 'amplihack: kept %q: target %q does not look like a Cargo target dir\n' \
                    "$entry" "$resolved" >&2
                continue
            fi
            printf 'amplihack: reclaiming build cache untouched for %s+ days: %q -> %q\n' \
                "$CACHE_TTL_DAYS" "$entry" "$resolved" >&2
            if rm -rf -- "$resolved" 2>/dev/null; then
                rm -f -- "$entry" 2>/dev/null \
                    || printf 'amplihack: could not remove %q\n' "$entry" >&2
            else
                printf 'amplihack: could not fully remove %q; link %q kept\n' \
                    "$resolved" "$entry" >&2
            fi
        else
            printf 'amplihack: reclaiming build cache untouched for %s+ days: %q\n' \
                "$CACHE_TTL_DAYS" "$entry" >&2
            rm -rf -- "$entry" 2>/dev/null \
                || printf 'amplihack: could not fully remove %q\n' "$entry" >&2
        fi
    done < <(find "$root" -mindepth 1 -maxdepth 1 \( -type d -o -type l \) \
        -name "$CACHE_PREFIX*" -print0 2>/dev/null)
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
# Reclaim only ever runs over caches this script owns, so an explicit directory
# is never swept.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
    export CARGO_TARGET_DIR
    CACHE_OWNED=0
else
    CACHE_OWNED=1
    MY_UID="$(id -u)"
    _cache_root="$(cache_root)"
    _cache_dir="$_cache_root/$CACHE_PREFIX$(checkout_hash)"
    export CARGO_TARGET_DIR="$_cache_dir"
    # Issue #1771, in this order: clear a dangling link at the current path so
    # cargo can create a real directory, reclaim stale siblings, then tag the
    # current cache. A link owned by somebody else is never touched.
    _cache_dir_ours=1
    if [ -L "$_cache_dir" ]; then
        if ! owned_by_me "$_cache_dir"; then
            _cache_dir_ours=0
            printf 'amplihack: warning: build cache %q is a link owned by another user; leaving it alone\n' \
                "$_cache_dir" >&2
        elif [ ! -e "$_cache_dir" ]; then
            printf 'amplihack: removing dangling build cache link: %q\n' "$_cache_dir" >&2
            rm -f -- "$_cache_dir" 2>/dev/null \
                || printf 'amplihack: could not remove %q\n' "$_cache_dir" >&2
        fi
    fi
    reclaim_stale_caches "$_cache_root" "$_cache_dir"
    if [ "$_cache_dir_ours" -eq 1 ]; then
        write_cachedir_tag "$_cache_dir"
    fi
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
