#!/usr/bin/env bash
# Issue #1440: the pre-commit build caches must not fill /tmp, must be
# reclaimed, and a full disk must never present itself as a compile error.
#
# scripts/git-hook-cargo.sh gives every checkout its own multi-gigabyte
# CARGO_TARGET_DIR (#1381). Nothing ever removed them and they lived on /tmp,
# so they accumulated -- 12 dirs / 122 GB, then 5 / 57 GB, then 7 / 62 GB in
# one session, with /tmp at 98-99% full while /home had 300+ GB free. cargo
# then failed with
#
#     error: failed to write ... dep-graph.part.bin: No space left on device
#     error: could not compile `amplihack-hive` (lib) due to 1 previous error
#
# which names the compiler for what is a storage failure. Two agents diagnosed
# it independently from scratch.
#
# This test locks in the three halves of the fix:
#   1. the default cache lives under ~/.cache, never under /tmp,
#      and an explicit CARGO_TARGET_DIR still wins;
#   2. stale caches are reclaimed, and a cache a live build holds is NOT --
#      deleting one of those produces failures that look like code defects;
#   3. a failed build reports free space on the cache's filesystem, loudly
#      when the disk is the reason.
#
# Run: bash tests/issue_1440_precommit_cache_location.sh
# (also run under CI's shell: bash --noprofile --norc -e -o pipefail <this>)

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRAPPER="$REPO_ROOT/scripts/git-hook-cargo.sh"

pass=0
fail=0
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

# The bug under test in #1278 bites this test too: an inherited GIT_DIR makes
# `git init <path>` ignore its path argument, so scrub before building fixtures.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_COMMON_DIR \
    GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_QUARANTINE_PATH \
    GIT_NAMESPACE GIT_CEILING_DIRECTORIES GIT_DISCOVERY_ACROSS_FILESYSTEM \
    GIT_INDEX_VERSION 2>/dev/null || true

record_pass() {
    printf 'ok   %s\n' "$1"
    pass=$((pass + 1))
}

record_fail() {
    printf 'FAIL %s\n' "$1" >&2
    [ -n "${2:-}" ] && printf '     %s\n' "$2" >&2
    fail=$((fail + 1))
}

assert_eq() {
    if [ "$2" = "$3" ]; then
        record_pass "$1"
    else
        record_fail "$1" "expected <$2>, got <$3>"
    fi
}

assert_contains() {
    case "$3" in
        *"$2"*) record_pass "$1" ;;
        *) record_fail "$1" "output did not contain <$2>: ${3:0:400}" ;;
    esac
}

assert_absent() {
    case "$3" in
        *"$2"*) record_fail "$1" "output unexpectedly contained <$2>" ;;
        *) record_pass "$1" ;;
    esac
}

if [ -x "$WRAPPER" ]; then
    record_pass "scripts/git-hook-cargo.sh exists and is executable"
else
    record_fail "scripts/git-hook-cargo.sh exists and is executable" "$WRAPPER"
    echo "passed: $pass, failed: $fail"
    exit 1
fi

FAKE_HOME="$TMPROOT/home"
FAKE_CACHE="$FAKE_HOME/.cache"
CACHE_ROOT="$FAKE_CACHE/amplihack"
PREFIX="amplihack-precommit-target-"
mkdir -p "$FAKE_HOME"

# Fixture checkouts. Two of them, because the per-checkout cache from #1381 has
# to survive this change.
one="$TMPROOT/checkout-one"
two="$TMPROOT/checkout-two"
mkdir -p "$one" "$two"
git init --quiet "$one" >/dev/null 2>&1 || true
git init --quiet "$two" >/dev/null 2>&1 || true

# Ask the wrapper what target directory it would use. `env -u` so an ambient
# CARGO_TARGET_DIR cannot mask the default; TTL 0 so this probe never sweeps.
target_dir_for() {
    (
        cd "$1" || exit 1
        env -u CARGO_TARGET_DIR \
            HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" TMPDIR=/tmp \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
            "$WRAPPER" printenv CARGO_TARGET_DIR
    ) 2>/dev/null
}

# ---------------------------------------------------------------------------
# 1. Location: ~/.cache, not /tmp.
# ---------------------------------------------------------------------------

dir_one="$(target_dir_for "$one")" || dir_one=""
dir_two="$(target_dir_for "$two")" || dir_two=""

if [ -n "$dir_one" ]; then
    record_pass "the wrapper still exports a default CARGO_TARGET_DIR"
else
    record_fail "the wrapper still exports a default CARGO_TARGET_DIR" "got nothing"
fi

# Ordered on purpose: a cache home that itself sits under the test's temp root
# is fine -- what must never happen is the cache landing on the temp filesystem
# *instead of* the cache home, which is the shape the bug had.
case "$dir_one" in
    "$CACHE_ROOT"/*)
        record_pass "the default build cache is not on the temp filesystem" ;;
    /tmp/*|/var/tmp/*|/var/folders/*|"${TMPDIR:-/tmp}"/*)
        record_fail "the default build cache is not on the temp filesystem" \
            "resolved to <$dir_one>; /tmp is small and shared, and filling it breaks every user on the host (#1440)" ;;
    *)
        record_pass "the default build cache is not on the temp filesystem" ;;
esac

case "$dir_one" in
    "$CACHE_ROOT"/*) record_pass "the default build cache lives under XDG_CACHE_HOME/amplihack" ;;
    *) record_fail "the default build cache lives under XDG_CACHE_HOME/amplihack" \
        "expected a path under <$CACHE_ROOT>, got <$dir_one>" ;;
esac

# With XDG unset it must still land under $HOME/.cache, not /tmp.
home_default="$(
    cd "$one" || exit 1
    env -u CARGO_TARGET_DIR -u XDG_CACHE_HOME \
        HOME="$FAKE_HOME" TMPDIR=/tmp AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
        "$WRAPPER" printenv CARGO_TARGET_DIR 2>/dev/null
)" || home_default=""
case "$home_default" in
    "$FAKE_HOME"/.cache/amplihack/*) record_pass "with no XDG_CACHE_HOME the cache falls back to \$HOME/.cache" ;;
    *) record_fail "with no XDG_CACHE_HOME the cache falls back to \$HOME/.cache" \
        "got <$home_default>" ;;
esac

# TMPDIR must not drag the cache back onto a temp filesystem.
tmpdir_default="$(
    cd "$one" || exit 1
    env -u CARGO_TARGET_DIR HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
        TMPDIR="$TMPROOT/tmpdir" AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
        "$WRAPPER" printenv CARGO_TARGET_DIR 2>/dev/null
)" || tmpdir_default=""
assert_eq "TMPDIR does not override the cache location" "$dir_one" "$tmpdir_default"

# ---------------------------------------------------------------------------
# 2. The #1381 contract still holds: per checkout, stable, explicit wins.
# ---------------------------------------------------------------------------

if [ -n "$dir_one" ] && [ "$dir_one" != "$dir_two" ]; then
    record_pass "separate checkouts still get separate build caches (#1381)"
else
    record_fail "separate checkouts still get separate build caches (#1381)" \
        "both resolved to <$dir_one>"
fi

assert_eq "the same checkout reuses its own cache" "$dir_one" "$(target_dir_for "$one")"

explicit="$(
    cd "$one" || exit 1
    CARGO_TARGET_DIR=/explicit/path HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
        "$WRAPPER" printenv CARGO_TARGET_DIR 2>/dev/null
)" || explicit=""
assert_eq "an explicit CARGO_TARGET_DIR is still honoured" "/explicit/path" "$explicit"

# ---------------------------------------------------------------------------
# 3. Reclaim.
#
# Stale caches go; fresh ones, the current one, foreign directories and -- above
# all -- a cache a live build is holding stay. Deleting that last one produces
# failures that look like code defects and cost a round of misdiagnosis.
# ---------------------------------------------------------------------------

mkdir -p "$CACHE_ROOT"
stale="$CACHE_ROOT/${PREFIX}stale0000abcd"
fresh="$CACHE_ROOT/${PREFIX}fresh0000abcd"
locked="$CACHE_ROOT/${PREFIX}locked000abc"
foreign="$CACHE_ROOT/some-other-directory"
mkdir -p "$stale/debug" "$fresh/debug" "$locked/debug" "$foreign"
: > "$locked/.cargo-lock"
mkdir -p "$dir_one"
: > "$dir_one/.cargo-lock"

age_ok=1
for d in "$stale" "$locked" "$foreign" "$dir_one"; do
    touch -d '30 days ago' "$d" 2>/dev/null || age_ok=0
done

if [ "$age_ok" -eq 1 ]; then
    # A live build holds an exclusive flock on <target>/.cargo-lock for the whole
    # build. Hold it here on our own fd: the sweep runs in a child process whose
    # own flock(1) opens the file fresh, so it sees genuine contention. No
    # sleeps, no races.
    holding=0
    if command -v flock >/dev/null 2>&1; then
        exec 9>"$locked/.cargo-lock"
        if flock --nonblock --exclusive 9; then
            holding=1
        fi
    fi

    sweep_out="$(
        cd "$one" || exit 1
        env -u CARGO_TARGET_DIR HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=7 \
            "$WRAPPER" true 2>&1
    )" || true

    if [ -d "$stale" ]; then
        record_fail "a cache untouched for 30 days is reclaimed" \
            "$stale still exists; nothing ever frees this space (#1440). sweep said: ${sweep_out:0:300}"
    else
        record_pass "a cache untouched for 30 days is reclaimed"
    fi

    if [ -d "$fresh" ]; then
        record_pass "a recently used cache is kept"
    else
        record_fail "a recently used cache is kept" "$fresh was deleted"
    fi

    if [ -d "$dir_one" ]; then
        record_pass "the cache this run is about to use is never swept"
    else
        record_fail "the cache this run is about to use is never swept" "$dir_one was deleted"
    fi

    if [ -d "$foreign" ]; then
        record_pass "directories outside the ${PREFIX}* namespace are left alone"
    else
        record_fail "directories outside the ${PREFIX}* namespace are left alone" \
            "$foreign was deleted"
    fi

    if [ "$holding" -eq 1 ]; then
        if [ -d "$locked" ]; then
            record_pass "a stale cache a live build holds is NOT deleted"
        else
            record_fail "a stale cache a live build holds is NOT deleted" \
                "$locked was deleted out from under a running build"
        fi
        exec 9>&-
    else
        printf 'skip flock(1) unavailable: live-build protection unverified\n'
    fi

    # An explicit CARGO_TARGET_DIR means the caller owns the storage; the
    # wrapper must not go sweeping directories it did not create.
    survivor="$CACHE_ROOT/${PREFIX}survivor0000"
    mkdir -p "$survivor"
    touch -d '30 days ago' "$survivor" 2>/dev/null || true
    (
        cd "$one" || exit 1
        env CARGO_TARGET_DIR="$TMPROOT/explicit-target" HOME="$FAKE_HOME" \
            XDG_CACHE_HOME="$FAKE_CACHE" AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=7 \
            "$WRAPPER" true
    ) >/dev/null 2>&1 || true
    if [ -d "$survivor" ]; then
        record_pass "no sweep happens when the caller set CARGO_TARGET_DIR"
    else
        record_fail "no sweep happens when the caller set CARGO_TARGET_DIR" \
            "$survivor was deleted"
    fi
else
    printf 'skip touch -d unavailable: reclaim behaviour unverified\n'
fi

# The caution that cost a round of misdiagnosis: `fuser -m` reports on the whole
# filesystem, not the directory, so it calls every cache on a shared mount busy.
if grep -vE '^[[:space:]]*#' "$WRAPPER" | grep -qE '(^|[^[:alnum:]_-])fuser([^[:alnum:]_-]|$)'; then
    record_fail "the reclaim path does not use fuser" \
        "fuser -m reports on the FILESYSTEM, not the directory -- use lsof +D or cargo's .cargo-lock"
else
    record_pass "the reclaim path does not use fuser"
fi

# ---------------------------------------------------------------------------
# 4. A failing build says what the disk is doing.
# ---------------------------------------------------------------------------

run_failing() { # $1 = min-free threshold in GiB
    (
        cd "$one" || exit 1
        env -u CARGO_TARGET_DIR HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
            AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB="$1" \
            "$WRAPPER" false 2>&1
    )
}

# A threshold nothing can satisfy stands in for the full disk.
if full_out="$(run_failing 999999999)"; then
    record_fail "a failing command still fails through the wrapper" "wrapper exited 0"
else
    record_pass "a failing command still fails through the wrapper"
fi
assert_contains "a full disk is named as a disk problem" "OUT OF DISK SPACE" "$full_out"
assert_contains "...and explicitly not as a compile error" "not a compile error" "$full_out"
assert_contains "...and names the cache directory" "$dir_one" "$full_out"
assert_contains "...and names the issue" "#1440" "$full_out"

# With space to spare a failure is a normal failure: one factual line, no siren.
plenty_out="$(run_failing 0)" || true
assert_contains "an ordinary failure still reports free space" "GiB free" "$plenty_out"
assert_absent "an ordinary failure does not cry disk-full" "OUT OF DISK SPACE" "$plenty_out"

# With a caller-supplied cache the wrapper still reports the disk, but must not
# offer to delete storage it does not own.
explicit_out="$(
    cd "$one" || exit 1
    env CARGO_TARGET_DIR="$TMPROOT/explicit-target" HOME="$FAKE_HOME" \
        XDG_CACHE_HOME="$FAKE_CACHE" AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB=999999999 \
        "$WRAPPER" false 2>&1
)" || true
assert_contains "an explicit cache dir still gets the disk diagnosis" \
    "OUT OF DISK SPACE" "$explicit_out"
assert_absent "...but the wrapper never offers to delete storage it does not own" \
    "rm -rf" "$explicit_out"

# A successful run must stay silent.
ok_out="$(
    cd "$one" || exit 1
    env -u CARGO_TARGET_DIR HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
        AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
        AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB=999999999 \
        "$WRAPPER" true 2>&1
)" || true
assert_eq "a successful run says nothing about the disk" "" "$ok_out"

# The wrapped command's exit status is the wrapper's exit status.
if (cd "$one" && env -u CARGO_TARGET_DIR HOME="$FAKE_HOME" XDG_CACHE_HOME="$FAKE_CACHE" \
        AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 "$WRAPPER" sh -c 'exit 42' >/dev/null 2>&1); then
    code=0
else
    code=$?
fi
assert_eq "the wrapped command's exit code is preserved" "42" "$code"

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
