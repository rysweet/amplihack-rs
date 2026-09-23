#!/usr/bin/env bash
# Run a cargo command from inside a git hook (issues #1278, #1381, #1440).
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
# cargo takes an exclusive flock on <target>/.cargo-lock for the duration of a
# build, so asking for that same lock is exact and costs nothing. Where flock(1)
# is missing we fall back to `lsof +D`, which does look at this directory (it
# correctly reported open_fds=0 for the caches on /tmp). With neither tool we
# decline to delete: deleting a cache out from under a live build produces
# failures that look like code defects, which is far more expensive than a stale
# directory left on disk.
cache_in_use() {
    local dir="$1"
    if command -v flock >/dev/null 2>&1; then
        if [ ! -e "$dir/.cargo-lock" ]; then
            return 1
        fi
        flock --nonblock --exclusive "$dir/.cargo-lock" true 2>/dev/null && return 1
        return 0
    fi
    if command -v lsof >/dev/null 2>&1; then
        [ -n "$(lsof -t +D "$dir" 2>/dev/null)" ] && return 0
        return 1
    fi
    return 0
}

# Age-based (LRU) reclaim. Every worktree gets its own hash, so parallel agents
# multiply these fast; without this the only thing that ever frees the space is
# a human noticing a full disk.
reclaim_stale_caches() {
    local root="$1" keep="$2" dir
    [ "$CACHE_TTL_DAYS" -gt 0 ] 2>/dev/null || return 0
    [ -d "$root" ] || return 0
    while IFS= read -r dir; do
        [ -n "$dir" ] || continue
        [ "$dir" = "$keep" ] && continue
        cache_in_use "$dir" && continue
        printf 'amplihack: reclaiming build cache untouched for %s+ days: %s\n' \
            "$CACHE_TTL_DAYS" "$dir" >&2
        rm -rf -- "$dir" 2>/dev/null || true
    done < <(find "$root" -mindepth 1 -maxdepth 1 -type d \
        -name "$CACHE_PREFIX*" -mtime "+$CACHE_TTL_DAYS" 2>/dev/null)
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
    _cache_root="$(cache_root)"
    _cache_dir="$_cache_root/$CACHE_PREFIX$(checkout_hash)"
    export CARGO_TARGET_DIR="$_cache_dir"
    reclaim_stale_caches "$_cache_root" "$_cache_dir"
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
