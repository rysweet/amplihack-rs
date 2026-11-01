#!/usr/bin/env bash
# Issue #1771: symlinked pre-commit build caches must be reclaimed safely, and
# the current cache must carry a CACHEDIR.TAG.
#
# scripts/git-hook-cargo.sh reclaims stale per-checkout caches (#1440), but it
# listed them with `find -type d`, so a cache that is a symbolic link to a
# directory on another volume was never seen, and its multi-gigabyte target
# was never freed. A dangling link at the current cache path made cargo fail.
# The entry list was also read one line at a time, so an entry name holding a
# newline split into two paths, and the second one was handed to `rm -rf`
# relative to the working directory.
#
# This test pins the contract of the fix:
#   1.  a stale link to a Cargo target dir: link and target both removed;
#   2.  a stale link whose target holds a live cargo lock (in debug/ or at the
#       root): both kept;
#   3.  a dangling prefixed link: removed once the link is older than the TTL,
#       kept while it is newer (its volume may only be unmounted);
#   4.  a stale link to a directory that is not a Cargo target dir: kept;
#   5.  the current cache is tagged with Cargo's CACHEDIR.TAG, an existing tag
#       is never rewritten, and a failed tag write never fails the hook;
#   6.  a dangling link at the current cache path is removed before the build;
#   7.  a fresh link to a Cargo target dir: kept;
#   8.  an entry name holding a newline cannot redirect `rm -rf`;
#   9.  age is measured to depth 2, so a recently built cache is kept;
#   10. a CACHEDIR.TAG that is a dangling link is never followed;
#   11. links to HOME, the cache root, its ancestors or the checkout's
#       ancestors are never deleted, even when they look like Cargo dirs;
#   12. an exported CDPATH and a relative link target change nothing;
#   13. TTL 0, or a TTL that is not a positive integer, disables the sweep,
#       and an invalid TTL is reported once;
#   14. an explicit CARGO_TARGET_DIR is never swept, cleaned or tagged.
#
# Ownership checks (a link or target owned by another user is kept) cannot be
# exercised without a second account, so they are documented, not tested here.
#
# Every case gets its own fake HOME and cache root under a private temp dir.
# Nothing here reads or writes the real HOME, ~/.claude, ~/.amplihack or the
# tmux server, and AMPLIHACK_CLAUDE_PROFILE_ROOT never reaches the wrapper.
#
# Run: bash tests/issue_1771_precommit_symlinked_caches.sh
# (also run under CI shell: bash --noprofile --norc -e -o pipefail <this>)

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRAPPER="$REPO_ROOT/scripts/git-hook-cargo.sh"
PREFIX="amplihack-precommit-target-"
OLD_STAMP="202001010000"

pass=0
fail=0

TMPROOT="$(mktemp -d 2>/dev/null)" || TMPROOT=""
case "$TMPROOT" in
    "" | / | "$HOME" | "$HOME"/)
        echo "refusing to run: mktemp -d gave <$TMPROOT>" >&2
        exit 1
        ;;
esac
if [ ! -d "$TMPROOT" ]; then
    echo "refusing to run: <$TMPROOT> is not a directory" >&2
    exit 1
fi
# Some cases make directories read-only; restore write access before removal.
trap 'chmod -R u+w "$TMPROOT" 2>/dev/null; rm -rf "$TMPROOT"' EXIT

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
    if [ -n "${2:-}" ]; then
        printf '     %s\n' "$2" >&2
    fi
    fail=$((fail + 1))
}

# check <description> <detail-on-failure> <test command...>
check() {
    local what="$1" detail="$2"
    shift 2
    if "$@"; then
        record_pass "$what"
    else
        record_fail "$what" "$detail"
    fi
}

is_link() { [ -L "$1" ]; }
exists() { [ -e "$1" ]; }
absent() { [ ! -e "$1" ] && [ ! -L "$1" ]; }
is_dir() { [ -d "$1" ]; }

if [ -x "$WRAPPER" ]; then
    record_pass "scripts/git-hook-cargo.sh exists and is executable"
else
    record_fail "scripts/git-hook-cargo.sh exists and is executable" "$WRAPPER"
    echo "passed: $pass, failed: $fail"
    exit 1
fi

VOL="$TMPROOT/vol"
REPO="$TMPROOT/checkout"
mkdir -p "$VOL" "$REPO" "$TMPROOT/tmp"
git init --quiet "$REPO" >/dev/null 2>&1 || true

# The tag cargo writes into every target dir it creates.
EXPECTED_TAG="$TMPROOT/expected-CACHEDIR.TAG"
printf '%s\n' \
    'Signature: 8a477f597d28d172789f06886806bc55' \
    '# This file is a cache directory tag created by cargo.' \
    '# For information about cache directory tags see https://bford.info/cachedir/' \
    > "$EXPECTED_TAG"
TAG_SIGNATURE='Signature: 8a477f597d28d172789f06886806bc55'

# new_case <name>: a private HOME and cache root, so one case cannot sweep
# another case's fixtures.
new_case() {
    C_HOME="$TMPROOT/$1/home"
    C_CACHE="$C_HOME/.cache"
    C_ROOT="$C_CACHE/amplihack"
    C_CWD="$REPO"
    C_CDPATH=""
    mkdir -p "$C_ROOT"
}

# run_wrapper <ttl> [NAME=VALUE...] <command...>
# Runs the wrapper from the fixture checkout with a fake HOME. Extra
# NAME=VALUE words are passed through env, so a case can set CARGO_TARGET_DIR.
run_wrapper() {
    local ttl="$1"
    shift
    (
        cd "$C_CWD" || exit 97
        if [ -n "$C_CDPATH" ]; then
            export CDPATH="$C_CDPATH"
        else
            unset CDPATH
        fi
        env -u CARGO_TARGET_DIR -u AMPLIHACK_CLAUDE_PROFILE_ROOT \
            HOME="$C_HOME" XDG_CACHE_HOME="$C_CACHE" TMPDIR="$TMPROOT/tmp" \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS="$ttl" \
            AMPLIHACK_PRECOMMIT_CACHE_MIN_FREE_GIB=0 \
            "$@"
    )
}

# The path the wrapper chooses for this checkout in the current case. TTL 0
# so the probe itself never sweeps.
target_dir_for() {
    run_wrapper 0 "$WRAPPER" printenv CARGO_TARGET_DIR 2>/dev/null
}

# age_tree <path>: set every entry, links included, to 2020.
age_tree() {
    find "$1" -exec touch -h -t "$OLD_STAMP" {} + 2>/dev/null
}

# make_target cargo|plain <dir>: build a fixture target and age all of it.
make_target() {
    mkdir -p "$2"
    case "$1" in
        cargo)
            mkdir -p "$2/debug/.fingerprint" "$2/debug/deps"
            : > "$2/.rustc_info.json"
            : > "$2/debug/deps/libfixture.rlib"
            ;;
        plain)
            mkdir -p "$2/debug"
            : > "$2/sentinel"
            ;;
    esac
    age_tree "$2"
}

# Check the platform can back-date files; every reclaim case depends on it.
probe="$TMPROOT/age-probe"
: > "$probe"
if ! touch -h -t "$OLD_STAMP" "$probe" 2>/dev/null \
    || [ -z "$(find "$probe" -mtime +30 2>/dev/null)" ]; then
    record_fail "touch -h -t can back-date fixtures" \
        "this platform cannot age files, so no reclaim case can run"
    echo "passed: $pass, failed: $fail"
    exit 1
fi

have_lock_tool=0
if command -v flock >/dev/null 2>&1 || command -v lsof >/dev/null 2>&1; then
    have_lock_tool=1
fi

# ---------------------------------------------------------------------------
# 1. A stale link to a Cargo target dir: target and link both removed.
# ---------------------------------------------------------------------------
new_case c1
t1="$VOL/c1-target"
l1="$C_ROOT/${PREFIX}c1stale00000"
make_target cargo "$t1"
ln -s "$t1" "$l1"
out1="$(run_wrapper 7 "$WRAPPER" true 2>&1)" || true
if [ "$have_lock_tool" -eq 1 ]; then
    check "1: a stale link to a Cargo target dir is removed" \
        "$l1 still exists; links were invisible to the sweep (#1771). stderr: ${out1:0:300}" \
        absent "$l1"
    check "1: ...and the target it pointed at is reclaimed" \
        "$t1 still exists; removing only the link frees nothing" \
        absent "$t1"
else
    printf 'skip 1: neither flock nor lsof available, so caches are always kept\n'
fi

# ---------------------------------------------------------------------------
# 2. A stale link whose target a live build holds: both kept.
#    The lock is held on this shell's own fd, as in issue_1440. The file is
#    opened for append so the open does not refresh its mtime.
# ---------------------------------------------------------------------------
if command -v flock >/dev/null 2>&1; then
    new_case c2
    t2="$VOL/c2-target"
    l2="$C_ROOT/${PREFIX}c2locked0000"
    make_target cargo "$t2"
    : > "$t2/debug/.cargo-lock"
    age_tree "$t2"
    ln -s "$t2" "$l2"
    exec 9>>"$t2/debug/.cargo-lock"
    if flock --nonblock --exclusive 9; then
        run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
        check "2: a link whose target holds a live lock in debug/ is kept" \
            "$l2 was removed under a running build" is_link "$l2"
        check "2: ...and its target is kept" \
            "$t2 was deleted under a running build" is_dir "$t2"
    else
        record_fail "2: could take the fixture lock" "flock on fd 9 failed"
    fi
    exec 9>&-

    # 2b. The same with the lock at the target root.
    new_case c2b
    t2b="$VOL/c2b-target"
    l2b="$C_ROOT/${PREFIX}c2blocked000"
    make_target cargo "$t2b"
    : > "$t2b/.cargo-lock"
    age_tree "$t2b"
    ln -s "$t2b" "$l2b"
    exec 9>>"$t2b/.cargo-lock"
    if flock --nonblock --exclusive 9; then
        run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
        check "2b: a link whose target holds a live root lock is kept" \
            "$l2b was removed under a running build" is_link "$l2b"
        check "2b: ...and its target is kept" \
            "$t2b was deleted under a running build" is_dir "$t2b"
    else
        record_fail "2b: could take the fixture lock" "flock on fd 9 failed"
    fi
    exec 9>&-
else
    printf 'skip 2: flock unavailable, live-build protection unverified\n'
fi

# ---------------------------------------------------------------------------
# 3. A dangling prefixed link is removed once the link itself is older than
#    the TTL. A fresh one is kept: its volume may only be unmounted, and
#    removing the link would orphan the target when the volume returns.
# ---------------------------------------------------------------------------
new_case c3
l3="$C_ROOT/${PREFIX}c3dangling00"
f3="$C_ROOT/${PREFIX}c3danglingnw"
ln -s "$VOL/c3-never-existed" "$l3"
touch -h -t "$OLD_STAMP" "$l3"
ln -s "$VOL/c3-unmounted" "$f3"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "3: an old dangling prefixed link is removed" \
    "$l3 is still there" absent "$l3"
check "3: a fresh dangling prefixed link is kept" \
    "$f3 was removed" is_link "$f3"

# ---------------------------------------------------------------------------
# 4. A stale link to a directory that is not a Cargo target dir: all kept.
# ---------------------------------------------------------------------------
new_case c4
t4="$VOL/c4-plain"
l4="$C_ROOT/${PREFIX}c4plain00000"
make_target plain "$t4"
ln -s "$t4" "$l4"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "4: a link to a non-Cargo directory is kept" "$l4 was removed" is_link "$l4"
check "4: ...its target is kept" "$t4 was deleted" is_dir "$t4"
check "4: ...and the files in it are kept" "$t4/sentinel was deleted" \
    exists "$t4/sentinel"

# A plain debug/ directory alone does not make a Cargo target dir.
new_case c4b
t4b="$VOL/c4b-debug-only"
l4b="$C_ROOT/${PREFIX}c4bdebug0000"
mkdir -p "$t4b/debug" "$t4b/release"
: > "$t4b/debug/notes.txt"
age_tree "$t4b"
ln -s "$t4b" "$l4b"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "4b: a target with only bare debug/ and release/ dirs is kept" \
    "$t4b was deleted; a plain debug/ is not a Cargo marker" \
    exists "$t4b/debug/notes.txt"

# ---------------------------------------------------------------------------
# 5. CACHEDIR.TAG on the current cache.
# ---------------------------------------------------------------------------
new_case c5
cur5="$(target_dir_for)" || cur5=""
if [ -z "$cur5" ]; then
    record_fail "5: the wrapper reports its cache path" "got nothing"
else
    t5="$VOL/c5-current"
    mkdir -p "$t5"
    ln -s "$t5" "$cur5"

    # 5a. A linked, untagged current cache is tagged.
    rc5=0
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || rc5=$?
    check "5a: the wrapper still succeeds" "exit $rc5" [ "$rc5" -eq 0 ]
    check "5a: the current cache link is kept" "$cur5 was removed" is_link "$cur5"
    first5=""
    if [ -f "$t5/CACHEDIR.TAG" ]; then
        IFS= read -r first5 < "$t5/CACHEDIR.TAG" || true
    fi
    check "5a: CACHEDIR.TAG is written through the current cache link" \
        "first line was <$first5>" [ "$first5" = "$TAG_SIGNATURE" ]
    check "5a: ...with the three lines cargo writes" \
        "content differs from cargo's tag" \
        cmp -s "$EXPECTED_TAG" "$t5/CACHEDIR.TAG"

    # 5b. An existing tag is never rewritten.
    printf '%s\n' "$TAG_SIGNATURE" '# custom tag kept by a user' > "$t5/CACHEDIR.TAG"
    cp "$t5/CACHEDIR.TAG" "$TMPROOT/c5-saved-tag"
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
    check "5b: an existing CACHEDIR.TAG is left unchanged" \
        "the tag was rewritten" cmp -s "$TMPROOT/c5-saved-tag" "$t5/CACHEDIR.TAG"

    # 5c. A tag that cannot be written never fails the hook.
    if [ "$(id -u)" -eq 0 ]; then
        printf 'skip 5c: running as root, a read-only directory stays writable\n'
    else
        rm -f "$t5/CACHEDIR.TAG"
        chmod a-w "$t5"
        rc5c=0
        run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || rc5c=$?
        chmod u+w "$t5"
        check "5c: a failed tag write does not fail the hook" \
            "exit $rc5c" [ "$rc5c" -eq 0 ]
        check "5c: ...and leaves no tag behind" \
            "$t5/CACHEDIR.TAG exists" absent "$t5/CACHEDIR.TAG"
    fi
fi

# 5d. A real directory at the current path is tagged too.
new_case c5d
cur5d="$(target_dir_for)" || cur5d=""
if [ -n "$cur5d" ]; then
    mkdir -p "$cur5d"
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
    check "5d: a real current cache directory is tagged" \
        "no matching CACHEDIR.TAG in $cur5d" \
        cmp -s "$EXPECTED_TAG" "$cur5d/CACHEDIR.TAG"
else
    record_fail "5d: the wrapper reports its cache path" "got nothing"
fi

# 5e. A current cache that does not exist yet is left for cargo to create.
new_case c5e
cur5e="$(target_dir_for)" || cur5e=""
if [ -n "$cur5e" ]; then
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
    check "5e: the wrapper does not create a missing cache directory" \
        "$cur5e was created" absent "$cur5e"
fi

# ---------------------------------------------------------------------------
# 6. A dangling link at the current cache path is removed before the command
#    runs, even with the sweep disabled.
# ---------------------------------------------------------------------------
new_case c6
cur6="$(target_dir_for)" || cur6=""
if [ -n "$cur6" ]; then
    ln -s "$VOL/c6-gone" "$cur6"
    # shellcheck disable=SC2016 # expanded by the wrapped shell, not here
    seen6="$(run_wrapper 0 "$WRAPPER" sh -c \
        'if [ -L "$CARGO_TARGET_DIR" ]; then echo link; else echo nolink; fi' \
        2>/dev/null)" || seen6=""
    check "6: the build never sees a dangling link at CARGO_TARGET_DIR" \
        "the wrapped command saw <$seen6>" [ "$seen6" = "nolink" ]
    check "6: ...and the dangling link is gone afterwards" \
        "$cur6 is still present" absent "$cur6"
    check "6: ...and its old target was not created" \
        "$VOL/c6-gone exists" absent "$VOL/c6-gone"
else
    record_fail "6: the wrapper reports its cache path" "got nothing"
fi

# ---------------------------------------------------------------------------
# 7. A fresh link to a Cargo target dir: kept.
# ---------------------------------------------------------------------------
new_case c7
t7="$VOL/c7-fresh"
l7="$C_ROOT/${PREFIX}c7fresh00000"
mkdir -p "$t7/debug/.fingerprint" "$t7/debug/deps"
: > "$t7/.rustc_info.json"
ln -s "$t7" "$l7"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "7: a fresh link to a Cargo target dir is kept" "$l7 was removed" is_link "$l7"
check "7: ...and its target is kept" "$t7 was deleted" is_dir "$t7"

# ---------------------------------------------------------------------------
# 8. An entry name holding a newline cannot redirect deletion.
#    Read line by line, the name below splits into a path that does not exist
#    and the bare word victim8, which `rm -rf` resolves against the checkout.
# ---------------------------------------------------------------------------
new_case c8
victim8="$REPO/victim8"
mkdir -p "$victim8"
: > "$victim8/keep-me"
age_tree "$victim8"
evil8="$C_ROOT/${PREFIX}c8evil"$'\n'"victim8"
mkdir -p "$evil8"
age_tree "$evil8"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "8: a newline in an entry name cannot delete a directory outside the cache root" \
    "$victim8 was deleted by a split entry name" exists "$victim8/keep-me"

# ---------------------------------------------------------------------------
# 9. Age is measured to depth 2: a target whose root and debug/ are old but
#    whose debug/deps was just written is in use, not stale.
# ---------------------------------------------------------------------------
new_case c9
t9="$VOL/c9-target"
l9="$C_ROOT/${PREFIX}c9recent0000"
make_target cargo "$t9"
touch "$t9/debug/deps"
ln -s "$t9" "$l9"
d9="$C_ROOT/${PREFIX}c9recentdir0"
make_target cargo "$d9"
touch "$d9/debug/deps"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "9a: a linked target with a recently written debug/deps is kept" \
    "$t9 was deleted" exists "$t9/debug/deps/libfixture.rlib"
check "9a: ...and so is its link" "$l9 was removed" is_link "$l9"
check "9b: a cache directory with a recently written debug/deps is kept" \
    "$d9 was deleted on the age of its root alone" \
    exists "$d9/debug/deps/libfixture.rlib"

# ---------------------------------------------------------------------------
# 10. A CACHEDIR.TAG that is a dangling link is never followed.
# ---------------------------------------------------------------------------
new_case c10
cur10="$(target_dir_for)" || cur10=""
victim10="$TMPROOT/victim10"
if [ -n "$cur10" ]; then
    mkdir -p "$cur10"
    ln -s "$victim10" "$cur10/CACHEDIR.TAG"
    rc10=0
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || rc10=$?
    check "10: a dangling CACHEDIR.TAG link is not written through" \
        "$victim10 was created" absent "$victim10"
    check "10: ...the link is left as it was" \
        "$cur10/CACHEDIR.TAG changed" is_link "$cur10/CACHEDIR.TAG"
    check "10: ...and the hook still succeeds" "exit $rc10" [ "$rc10" -eq 0 ]
else
    record_fail "10: the wrapper reports its cache path" "got nothing"
fi

# ---------------------------------------------------------------------------
# 11. Links to protected directories are never followed into `rm -rf`, even
#     when those directories look like Cargo target dirs and are old.
#     XDG_CACHE_HOME sits outside HOME here so that HOME, the cache root, its
#     parent and the checkout parent can all be aged independently.
# ---------------------------------------------------------------------------
base11="$TMPROOT/c11"
C_HOME="$base11/home"
C_CACHE="$base11/xdg"
C_ROOT="$C_CACHE/amplihack"
C_CWD="$base11/repos/repo"
C_CDPATH=""
mkdir -p "$C_HOME" "$C_ROOT" "$C_CWD"
git init --quiet "$C_CWD" >/dev/null 2>&1 || true
for d11 in "$C_HOME" "$C_CACHE" "$C_ROOT" "$base11/repos" "$C_CWD"; do
    mkdir -p "$d11/debug/.fingerprint"
    : > "$d11/.rustc_info.json"
    : > "$d11/protected-marker"
done
ln -s "$C_HOME" "$C_ROOT/${PREFIX}c11home00000"
ln -s "$C_CACHE" "$C_ROOT/${PREFIX}c11xdg000000"
ln -s "$C_ROOT" "$C_ROOT/${PREFIX}c11root00000"
ln -s "$base11/repos" "$C_ROOT/${PREFIX}c11repos0000"
ln -s "$C_CWD" "$C_ROOT/${PREFIX}c11checkout0"
age_tree "$base11"
run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
check "11: a link to HOME never deletes HOME" \
    "$C_HOME was deleted" exists "$C_HOME/protected-marker"
check "11: a link to an ancestor of the cache root never deletes it" \
    "$C_CACHE was deleted" exists "$C_CACHE/protected-marker"
check "11: a link to the cache root never deletes it" \
    "$C_ROOT was deleted" exists "$C_ROOT/protected-marker"
check "11: a link to an ancestor of the checkout never deletes it" \
    "$base11/repos was deleted" exists "$base11/repos/protected-marker"
check "11: a link to the checkout never deletes it" \
    "$C_CWD was deleted" exists "$C_CWD/protected-marker"

# ---------------------------------------------------------------------------
# 12. CDPATH and relative link targets do not change the outcome of case 1.
# ---------------------------------------------------------------------------
new_case c12
t12="$VOL/c12-target"
l12="$C_ROOT/${PREFIX}c12relative0"
make_target cargo "$t12"
# C_ROOT is TMPROOT/c12/home/.cache/amplihack, four levels below TMPROOT.
ln -s "../../../../vol/c12-target" "$l12"
mkdir -p "$TMPROOT/vol/vol/c12-target"
C_CDPATH="$TMPROOT:$TMPROOT/vol"
if [ "$have_lock_tool" -eq 1 ]; then
    run_wrapper 7 "$WRAPPER" true >/dev/null 2>&1 || true
    check "12: with CDPATH exported, a stale relative link is removed" \
        "$l12 still exists" absent "$l12"
    check "12: ...and its real target is reclaimed" \
        "$t12 still exists" absent "$t12"
    check "12: ...and a CDPATH look-alike is untouched" \
        "$TMPROOT/vol/vol/c12-target was deleted" is_dir "$TMPROOT/vol/vol/c12-target"
else
    printf 'skip 12: neither flock nor lsof available\n'
fi

# ---------------------------------------------------------------------------
# 13. TTL 0, or a TTL that is not a positive integer, disables the sweep,
#     dangling links elsewhere included.
# ---------------------------------------------------------------------------
for ttl13 in 0 week -1; do
    new_case "c13-$ttl13"
    t13="$VOL/c13-$ttl13-target"
    l13="$C_ROOT/${PREFIX}c13stale0000"
    g13="$C_ROOT/${PREFIX}c13dangling0"
    make_target cargo "$t13"
    ln -s "$t13" "$l13"
    ln -s "$VOL/c13-never" "$g13"
    touch -h -t "$OLD_STAMP" "$g13"
    rc13=0
    err13="$TMPROOT/c13-$ttl13.err"
    run_wrapper "$ttl13" "$WRAPPER" true >/dev/null 2>"$err13" || rc13=$?
    warn13="$(grep -c 'ignoring invalid AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS' "$err13" || true)"
    if [ "$ttl13" = 0 ]; then
        check "13: TTL <0> prints no warning" "warned $warn13 times" [ "$warn13" -eq 0 ]
    else
        check "13: TTL <$ttl13> is reported once" "warned $warn13 times" [ "$warn13" -eq 1 ]
    fi
    check "13: TTL <$ttl13> does not fail the hook" "exit $rc13" [ "$rc13" -eq 0 ]
    check "13: TTL <$ttl13> keeps a stale link" "$l13 was removed" is_link "$l13"
    check "13: TTL <$ttl13> keeps its target" "$t13 was deleted" is_dir "$t13"
    check "13: TTL <$ttl13> keeps a dangling link outside the current path" \
        "$g13 was removed" is_link "$g13"
done

# ---------------------------------------------------------------------------
# 14. An explicit CARGO_TARGET_DIR: no sweep, no tag, no link removal.
# ---------------------------------------------------------------------------
new_case c14
t14="$VOL/c14-target"
l14="$C_ROOT/${PREFIX}c14stale0000"
make_target cargo "$t14"
ln -s "$t14" "$l14"
explicit14="$VOL/c14-explicit"
mkdir -p "$explicit14"
run_wrapper 7 CARGO_TARGET_DIR="$explicit14" "$WRAPPER" true >/dev/null 2>&1 || true
check "14: an explicit CARGO_TARGET_DIR leaves stale links alone" \
    "$l14 was removed" is_link "$l14"
check "14: ...and their targets" "$t14 was deleted" is_dir "$t14"
check "14: ...and the explicit directory is not tagged" \
    "$explicit14/CACHEDIR.TAG was written" absent "$explicit14/CACHEDIR.TAG"
dangling14="$VOL/c14-dangling"
ln -s "$VOL/c14-never" "$dangling14"
run_wrapper 7 CARGO_TARGET_DIR="$dangling14" "$WRAPPER" true >/dev/null 2>&1 || true
check "14: a dangling explicit CARGO_TARGET_DIR is not removed" \
    "$dangling14 was removed" is_link "$dangling14"

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
