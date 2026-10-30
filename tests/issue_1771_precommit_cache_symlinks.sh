#!/usr/bin/env bash
# Issue #1771: pre-commit build caches that are symbolic links must be
# reclaimed, every cache the wrapper chooses must carry a cache tag, and a
# dangling link at the chosen cache path must not break the build.
#
# On the host in the issue, a local cargo wrapper turns each new target
# directory into a link to a directory on another volume. The sweep added for
# #1440 listed candidates with `find -type d`, which does not match links, so
# 73 caches holding 321 GiB built up and were never reclaimed. Cargo writes
# CACHEDIR.TAG only into a directory it creates itself, so none of those caches
# was tagged and tag-based clean-up tools skipped them. A link whose target had
# gone left CARGO_TARGET_DIR pointing at nothing, and the next build in that
# checkout failed.
#
# The rules under test are those in docs/reference/pre-commit-build-cache.md.
# Every fixture lives under one temporary root: HOME, XDG_CACHE_HOME and TMPDIR
# point inside it, and the "other volume" is $T/vol. The wrapped commands are
# true, printenv and sh. No cargo, no tmux and no real cache is used.
#
# Cases (P is the cache prefix, amplihack-precommit-target-):
#    1  a stale link to a cargo target: link and directory removed, both named
#    2  as 1, with debug/.cargo-lock held by a build: both kept
#    3  a dangling prefix-named link: removed
#    4  a stale directory with no cargo marker behind a link: kept, silently
#    5  tagging: a plain directory, through a link, an existing tag, and a
#       dangling link or a FIFO at the tag path
#    6  a dangling link at the chosen path: removed before the command runs
#    7  a fresh directory behind a link that is itself old: kept
#    8  a missing chosen cache directory: not created
#    9  an explicit CARGO_TARGET_DIR: no sweep, no tag, no repair
#   10  TTL 0 or a TTL that is not a whole number: no sweep, but tagging and
#       repair still run
#   11  a prefix-named link to a file, and a dangling link without the prefix
#   12  a link to a directory that contains the cache root: kept
#   13  a second run on a tagged, linked cache: silent
#   14  a sibling link to the directory behind this run's cache path: kept
#   15  a newline in an entry's name cannot direct a removal elsewhere
#   16  a link whose target name ends in a newline: kept
#   17  a link to the hook's working directory: kept
#   18  a link to HOME: kept
#   19  age rule for plain directories: debug/, release/, .rustc_info.json
#   20  age rule through a link: a fresh debug/ keeps the cache
#   21  in-use check one and two levels down, creating no lock files
#
# Cases that need GNU `touch -d`, `touch -h`, flock(1) or timeout(1) print a
# skip note where the tool is missing, as the #1440 test does on macOS.
#
# Run: bash tests/issue_1771_precommit_cache_symlinks.sh
# (CI runs it under bash --noprofile --norc -e -o pipefail)

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRAPPER="$REPO_ROOT/scripts/git-hook-cargo.sh"
P="amplihack-precommit-target-"

pass=0
fail=0

T0="$(mktemp -d)" || T0=""
case "$T0" in
    /?*) ;;
    *) echo "mktemp -d failed" >&2; exit 1 ;;
esac
[ -d "$T0" ] || { echo "mktemp -d gave no directory: $T0" >&2; exit 1; }
trap 'rm -rf -- "$T0"' EXIT

# The wrapper compares paths it resolves with `cd -P`, so every fixture uses the
# physical path of the temporary root (on macOS, /var is a link).
T="$(cd -P -- "$T0" && pwd -P)" || T=""
case "$T" in
    /?*) ;;
    *) echo "could not resolve $T0" >&2; exit 1 ;;
esac

# Everything the wrapper could write lands under $T. TMPDIR is the wrapper's
# fallback cache root when HOME is unusable, so it moves too. Test children
# must not see a real profile root or a real HOME (#1699).
export HOME="$T/home"
export XDG_CACHE_HOME="$T/home/.cache"
export TMPDIR="$T/tmp"
mkdir -p "$HOME" "$XDG_CACHE_HOME" "$TMPDIR" "$T/vol"
unset AMPLIHACK_CLAUDE_PROFILE_ROOT CARGO_TARGET_DIR 2>/dev/null || true

# As in #1440: an inherited GIT_DIR makes `git init <path>` ignore its path
# argument, so scrub before building fixtures.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_PREFIX GIT_COMMON_DIR \
    GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_QUARANTINE_PATH \
    GIT_NAMESPACE GIT_CEILING_DIRECTORIES GIT_DISCOVERY_ACROSS_FILESYSTEM \
    GIT_INDEX_VERSION 2>/dev/null || true

# The wrapper removes directories. Refuse to run it against anything outside
# the temporary root.
case "$HOME" in
    "$T"/*) ;;
    *) echo "HOME is not under $T: $HOME" >&2; exit 1 ;;
esac
case "$XDG_CACHE_HOME" in
    "$T"/*) ;;
    *) echo "XDG_CACHE_HOME is not under $T: $XDG_CACHE_HOME" >&2; exit 1 ;;
esac

record_pass() {
    printf 'ok   %s\n' "$1"
    pass=$((pass + 1))
}

record_fail() {
    printf 'FAIL %s\n' "$1" >&2
    [ -n "${2:-}" ] && printf '     %s\n' "$2" >&2
    fail=$((fail + 1))
}

if [ -x "$WRAPPER" ]; then
    record_pass "scripts/git-hook-cargo.sh exists and is executable"
else
    record_fail "scripts/git-hook-cargo.sh exists and is executable" "$WRAPPER"
    echo "passed: $pass, failed: $fail"
    exit 1
fi

# ---------------------------------------------------------------------------
# Running the wrapper.
# ---------------------------------------------------------------------------

RUN_HOME="$HOME"
RUN_TARGET=""
run_out=""
run_err=""
run_status=0

# run_wrapper CHECKOUT CACHE_HOME TTL COMMAND...
# Runs the wrapper from CHECKOUT with XDG_CACHE_HOME set to CACHE_HOME, and
# leaves stdout, stderr and the exit status in run_out, run_err and
# run_status. A non-empty RUN_TARGET is passed as an explicit CARGO_TARGET_DIR.
run_wrapper() {
    local dir="$1" cache_home="$2" ttl="$3"
    shift 3
    run_status=0
    (
        cd "$dir" || exit 97
        env -u CARGO_TARGET_DIR ${RUN_TARGET:+"CARGO_TARGET_DIR=$RUN_TARGET"} \
            HOME="$RUN_HOME" XDG_CACHE_HOME="$cache_home" \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS="$ttl" \
            "$WRAPPER" "$@"
    ) >"$T/run.out" 2>"$T/run.err" </dev/null || run_status=$?
    run_out="$(cat "$T/run.out")" || run_out=""
    run_err="$(cat "$T/run.err")" || run_err=""
}

# chosen_for CHECKOUT CACHE_HOME prints the cache path the wrapper chooses for
# CHECKOUT, or nothing when that path is not inside CACHE_HOME/amplihack. It
# runs with TTL 0, which also creates the cache root. Call it before planting
# anything at the chosen path: every run now tags and repairs that path.
chosen_for() {
    local out
    out="$(
        cd "$1" || exit 1
        env -u CARGO_TARGET_DIR HOME="$RUN_HOME" XDG_CACHE_HOME="$2" \
            AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=0 \
            "$WRAPPER" printenv CARGO_TARGET_DIR 2>/dev/null </dev/null
    )" || out=""
    case "$out" in
        "$2/amplihack/$P"?*) printf '%s' "$out" ;;
    esac
    return 0
}

# begin CASE CHECKOUT sets cache_home, root and chosen for one case. Each case
# has its own cache home, so no case can see another's fixtures.
begin() {
    cache_home="$T/c$1/cache"
    root="$cache_home/amplihack"
    chosen="$(chosen_for "$2" "$cache_home")" || chosen=""
    if [ -z "$chosen" ]; then
        record_fail "case $1: the wrapper names a cache path under $root" \
            "it printed nothing usable"
        return 1
    fi
    return 0
}

# ---------------------------------------------------------------------------
# Assertions.
# ---------------------------------------------------------------------------

gone() { [ ! -e "$1" ] && [ ! -L "$1" ]; }
dangling() { [ -L "$1" ] && [ ! -e "$1" ]; }

assert_gone() {
    if gone "$2"; then
        record_pass "$1"
    else
        record_fail "$1" "still present: $2; stderr: ${run_err:0:300}"
    fi
}

# A real directory, not a link to one.
assert_dir() {
    if [ -d "$2" ] && [ ! -L "$2" ]; then
        record_pass "$1"
    else
        record_fail "$1" "not a directory: $2; stderr: ${run_err:0:300}"
    fi
}

assert_exists() {
    if [ -e "$2" ]; then
        record_pass "$1"
    else
        record_fail "$1" "missing: $2; stderr: ${run_err:0:300}"
    fi
}

assert_link() {
    if [ -L "$2" ]; then
        record_pass "$1"
    else
        record_fail "$1" "not a link: $2; stderr: ${run_err:0:300}"
    fi
}

assert_dangling() {
    if dangling "$2"; then
        record_pass "$1"
    else
        record_fail "$1" "not a dangling link: $2; stderr: ${run_err:0:300}"
    fi
}

assert_contains() {
    case "$3" in
        *"$2"*) record_pass "$1" ;;
        *) record_fail "$1" "output did not contain <$2>: ${3:0:300}" ;;
    esac
}

assert_empty() {
    if [ -z "$2" ]; then
        record_pass "$1"
    else
        record_fail "$1" "expected no output, got: ${2:0:300}"
    fi
}

assert_eq() {
    if [ "$2" = "$3" ]; then
        record_pass "$1"
    else
        record_fail "$1" "expected <$2>, got <${3:0:300}>"
    fi
}

assert_status() {
    if [ "$run_status" = "$2" ]; then
        record_pass "$1"
    else
        record_fail "$1" "exit status $run_status, expected $2; stderr: ${run_err:0:300}"
    fi
}

# Cargo's tag, written out here rather than copied from the script under test.
EXPECTED_TAG="$T/expected-tag"
printf '%s\n' \
    'Signature: 8a477f597d28d172789f06886806bc55' \
    '# This file is a cache directory tag created by cargo.' \
    '# For information about cache directory tags see https://bford.info/cachedir/' \
    >"$EXPECTED_TAG"

assert_tag() {
    if cmp -s "$EXPECTED_TAG" "$2"; then
        record_pass "$1"
    else
        record_fail "$1" "$2 does not hold exactly cargo's three tag lines"
    fi
}

# ---------------------------------------------------------------------------
# Fixtures.
# ---------------------------------------------------------------------------

# age PATH... moves each path's own modification time 30 days back. touch -d
# follows links, so never pass one here; age_link is for links.
age() {
    local p
    for p in "$@"; do
        touch -d '30 days ago' "$p" 2>/dev/null || true
    done
}

age_link() {
    touch -h -d '30 days ago' "$1" 2>/dev/null || true
}

# age_tree DIR ages every path the age rule reads. Creating anything directly
# inside one of them afterwards makes it fresh again, so call this last.
age_tree() {
    local d="$1" p
    for p in "$d/debug" "$d/release" "$d/.rustc_info.json"; do
        if [ -e "$p" ] && [ ! -L "$p" ]; then
            age "$p"
        fi
    done
    age "$d"
}

# make_target DIR leaves what a debug build leaves: a fingerprint directory and
# an unheld lock file in debug/.
make_target() {
    mkdir -p "$1/debug/.fingerprint"
    : >"$1/debug/.cargo-lock"
}

HAVE_TOUCH_D=0
mkdir -p "$T/probe/old"
age "$T/probe/old"
if [ -n "$(find "$T/probe/old" -maxdepth 0 -mtime +7 -print 2>/dev/null)" ]; then
    HAVE_TOUCH_D=1
fi

# touch -h must age the link and leave its target alone.
HAVE_TOUCH_H=0
mkdir -p "$T/probe/fresh"
ln -s fresh "$T/probe/link"
age_link "$T/probe/link"
if [ -n "$(find "$T/probe/link" -maxdepth 0 -type l -mtime +7 -print 2>/dev/null)" ] &&
    [ -z "$(find "$T/probe/fresh" -maxdepth 0 -mtime +7 -print 2>/dev/null)" ]; then
    HAVE_TOUCH_H=1
fi

HAVE_FLOCK=0
if command -v flock >/dev/null 2>&1; then
    HAVE_FLOCK=1
fi

# requires CASE TOOL... is true when every tool is present, and otherwise
# prints a skip note.
requires() {
    local label="$1" tool
    shift
    for tool in "$@"; do
        case "$tool" in
            touch-d) [ "$HAVE_TOUCH_D" -eq 1 ] && continue ;;
            touch-h) [ "$HAVE_TOUCH_H" -eq 1 ] && continue ;;
            flock) [ "$HAVE_FLOCK" -eq 1 ] && continue ;;
            timeout) command -v timeout >/dev/null 2>&1 && continue ;;
        esac
        printf 'skip %s: %s unavailable\n' "$label" "$tool"
        return 1
    done
    return 0
}

one="$T/one"
two="$T/two"
three="$T/three"
for c in "$one" "$two" "$three"; do
    mkdir -p "$c"
    git init --quiet "$c" >/dev/null 2>&1 || true
done

# The command sh runs in cases 6a and 10a: it succeeds only when the cache path
# is not a link and it can create a directory there, as cargo must.
CREATE_TARGET='[ ! -L "$CARGO_TARGET_DIR" ] && mkdir "$CARGO_TARGET_DIR"'

# ---------------------------------------------------------------------------
# 1. A stale link to a cargo target is reclaimed: the directory on the other
# volume goes, then the link, and the reclaim line names both. The command is
# printenv, so this also checks that no new output reaches stdout.
# ---------------------------------------------------------------------------

if requires "case 1" touch-d && begin 1 "$one"; then
    tgt="$T/vol/c1"
    link="$root/${P}c1"
    make_target "$tgt"
    age_tree "$tgt"
    ln -s "$tgt" "$link"
    run_wrapper "$one" "$cache_home" 7 printenv CARGO_TARGET_DIR
    assert_status "case 1: the wrapper exits with the command's status" 0
    assert_gone "case 1: the stale cache link is removed" "$link"
    assert_gone "case 1: the directory it pointed at is removed" "$tgt"
    assert_contains "case 1: stderr names the link" "$link" "$run_err"
    assert_contains "case 1: stderr names the resolved directory" "$tgt" "$run_err"
    assert_eq "case 1: stdout holds only the cache path" "$chosen" "$run_out"
fi

# ---------------------------------------------------------------------------
# 2. As case 1, but a build holds debug/.cargo-lock, which is where cargo keeps
# its lock. The lock is held on this shell's fd 9: the wrapper's flock opens
# the file afresh and meets it, with no sleeps or races. The file already
# exists, so opening it does not make debug/ fresh.
# ---------------------------------------------------------------------------

if requires "case 2" touch-d flock && begin 2 "$one"; then
    tgt="$T/vol/c2"
    link="$root/${P}c2"
    make_target "$tgt"
    age_tree "$tgt"
    ln -s "$tgt" "$link"
    exec 9>"$tgt/debug/.cargo-lock"
    if flock --nonblock --exclusive 9; then
        run_wrapper "$one" "$cache_home" 7 true
        assert_link "case 2: the link to a cache a build holds is kept" "$link"
        assert_dir "case 2: the cache a build holds is kept" "$tgt/debug/.fingerprint"
    else
        record_fail "case 2: the test holds debug/.cargo-lock" "flock on fd 9 failed"
    fi
    exec 9>&-
fi

# ---------------------------------------------------------------------------
# 3. A prefix-named link whose target has gone is removed, whatever its age.
# ---------------------------------------------------------------------------

if begin 3 "$one"; then
    link="$root/${P}c3"
    ln -s "$T/vol/c3-gone" "$link"
    run_wrapper "$one" "$cache_home" 7 true
    assert_status "case 3: the wrapper exits with the command's status" 0
    assert_gone "case 3: a dangling cache link is removed" "$link"
    assert_contains "case 3: stderr names the dangling link" "$link" "$run_err"
fi

# ---------------------------------------------------------------------------
# 4. A stale directory behind a prefix-named link, with notes and a bare debug/
# but no cargo marker, is not a cargo target. It is kept with its link, and the
# skip prints nothing, because it would otherwise print on every run.
# ---------------------------------------------------------------------------

if requires "case 4" touch-d && begin 4 "$one"; then
    tgt="$T/vol/c4"
    link="$root/${P}c4"
    mkdir -p "$tgt/debug"
    printf 'notes\n' >"$tgt/notes.txt"
    age_tree "$tgt"
    ln -s "$tgt" "$link"
    run_wrapper "$one" "$cache_home" 7 true
    assert_link "case 4: the link to a directory that is not a cargo target is kept" "$link"
    assert_exists "case 4: the directory and its contents are kept" "$tgt/notes.txt"
    assert_dir "case 4: a bare debug/ is not a cargo marker" "$tgt/debug"
    assert_empty "case 4: the skip prints nothing" "$run_err"
fi

# ---------------------------------------------------------------------------
# 5. Tagging. Cargo writes CACHEDIR.TAG only into a directory it creates, so
# the wrapper writes it into the chosen cache when it is missing, and never
# replaces or writes through anything already at that path.
# ---------------------------------------------------------------------------

if begin 5a "$one"; then
    mkdir -p "$chosen"
    run_wrapper "$one" "$cache_home" 7 true
    assert_tag "case 5a: a missing tag is written into a plain chosen directory" \
        "$chosen/CACHEDIR.TAG"
    assert_empty "case 5a: tagging prints nothing" "$run_err"
fi

if begin 5b "$one"; then
    tgt="$T/vol/c5b"
    make_target "$tgt"
    ln -s "$tgt" "$chosen"
    run_wrapper "$one" "$cache_home" 7 true
    assert_tag "case 5b: a missing tag is written into the resolved directory of a link" \
        "$tgt/CACHEDIR.TAG"
    assert_link "case 5b: the chosen path is still a link" "$chosen"
fi

if begin 5c "$one"; then
    mkdir -p "$chosen"
    printf 'Signature: 8a477f597d28d172789f06886806bc55\n# written by a host wrapper\n' \
        >"$chosen/CACHEDIR.TAG"
    cp "$chosen/CACHEDIR.TAG" "$T/c5c/tag-before"
    run_wrapper "$one" "$cache_home" 7 true
    if cmp -s "$T/c5c/tag-before" "$chosen/CACHEDIR.TAG"; then
        record_pass "case 5c: an existing tag with other content is unchanged byte for byte"
    else
        record_fail "case 5c: an existing tag with other content is unchanged byte for byte" \
            "$chosen/CACHEDIR.TAG was rewritten"
    fi
fi

if begin 5d "$one"; then
    mkdir -p "$chosen"
    ln -s "$T/c5d/elsewhere" "$chosen/CACHEDIR.TAG"
    run_wrapper "$one" "$cache_home" 7 true
    assert_dangling "case 5d: a dangling link at the tag path is left as it is" \
        "$chosen/CACHEDIR.TAG"
    assert_gone "case 5d: nothing is written through that link" "$T/c5d/elsewhere"
fi

# noclobber refuses to open an existing regular file, but not a FIFO, so a
# wrapper that opened one for writing would block until something read it.
# timeout(1) bounds the run; afterwards a reader is attached and closed so that
# a writer still blocked in open() cannot outlive the test.
if requires "case 5e" timeout && begin 5e "$one"; then
    mkdir -p "$chosen"
    if mkfifo "$chosen/CACHEDIR.TAG" 2>/dev/null; then
        fifo_status=0
        (
            cd "$one" || exit 97
            env -u CARGO_TARGET_DIR HOME="$RUN_HOME" XDG_CACHE_HOME="$cache_home" \
                AMPLIHACK_PRECOMMIT_CACHE_TTL_DAYS=7 timeout 20 "$WRAPPER" true
        ) >/dev/null 2>&1 </dev/null || fifo_status=$?
        if [ "$fifo_status" -eq 0 ]; then
            record_pass "case 5e: a FIFO at the tag path does not block the wrapper"
        else
            record_fail "case 5e: a FIFO at the tag path does not block the wrapper" \
                "exit status $fifo_status (124 means the wrapper blocked opening it)"
        fi
        if [ -p "$chosen/CACHEDIR.TAG" ]; then
            record_pass "case 5e: the FIFO is left in place"
        else
            record_fail "case 5e: the FIFO is left in place" "$chosen/CACHEDIR.TAG changed"
        fi
        { exec 7<>"$chosen/CACHEDIR.TAG"; exec 7<&-; } 2>/dev/null || true
    else
        printf 'skip case 5e: mkfifo failed\n'
    fi
fi

# ---------------------------------------------------------------------------
# 6. A dangling link at the chosen path is removed before the command runs, so
# the command can create a real directory there, as cargo would.
# ---------------------------------------------------------------------------

if begin 6a "$one"; then
    ln -s "$T/vol/c6a-gone" "$chosen"
    run_wrapper "$one" "$cache_home" 7 sh -c "$CREATE_TARGET"
    assert_status "case 6a: the command finds no link at the cache path and creates it" 0
    assert_dir "case 6a: the cache path is now a directory" "$chosen"
    assert_gone "case 6a: the old link target is not created" "$T/vol/c6a-gone"
fi

# The command here succeeds whatever the wrapper does, so the free-space report
# that follows a failure cannot be what names the path.
if begin 6b "$one"; then
    ln -s "$T/vol/c6b-gone" "$chosen"
    run_wrapper "$one" "$cache_home" 7 true
    assert_gone "case 6b: the dangling link at the cache path is removed" "$chosen"
    assert_contains "case 6b: stderr names the removed link" "$chosen" "$run_err"
fi

# ---------------------------------------------------------------------------
# 7. The link's own age is ignored: an old link to a fresh cargo target is kept.
# ---------------------------------------------------------------------------

if requires "case 7" touch-h && begin 7 "$one"; then
    tgt="$T/vol/c7"
    link="$root/${P}c7"
    make_target "$tgt"
    ln -s "$tgt" "$link"
    age_link "$link"
    run_wrapper "$one" "$cache_home" 7 true
    assert_link "case 7: an old link to a fresh cache is kept" "$link"
    assert_dir "case 7: the fresh cache behind it is kept" "$tgt/debug/.fingerprint"
fi

# ---------------------------------------------------------------------------
# 8. The wrapper creates the cache root but never the cache directory: cargo
# creates it, and tags it, on the first build.
# ---------------------------------------------------------------------------

if begin 8 "$one"; then
    run_wrapper "$one" "$cache_home" 7 true
    assert_gone "case 8: a missing chosen cache directory is not created" "$chosen"
fi

# ---------------------------------------------------------------------------
# 9. An explicit CARGO_TARGET_DIR belongs to the caller. A dangling link there
# is not removed, a directory there is not tagged, and the default cache root
# is not swept.
# ---------------------------------------------------------------------------

cache_home="$T/c9/cache"
root="$cache_home/amplihack"
mkdir -p "$root"
ln -s "$T/vol/c9-gone" "$root/${P}c9"
ln -s "$T/vol/c9-nowhere" "$T/c9/explicit-link"
mkdir -p "$T/c9/explicit-dir"

RUN_TARGET="$T/c9/explicit-link"
run_wrapper "$one" "$cache_home" 7 true
assert_status "case 9: the wrapper exits with the command's status" 0
assert_dangling "case 9: a dangling link at an explicit CARGO_TARGET_DIR is left alone" \
    "$T/c9/explicit-link"
assert_gone "case 9: nothing is created at that link's target" "$T/vol/c9-nowhere"
assert_dangling "case 9: the default cache root is not swept" "$root/${P}c9"

RUN_TARGET="$T/c9/explicit-dir"
run_wrapper "$one" "$cache_home" 7 true
assert_gone "case 9: an explicit directory is not tagged" "$T/c9/explicit-dir/CACHEDIR.TAG"
RUN_TARGET=""

# ---------------------------------------------------------------------------
# 10. TTL 0 turns the sweep off, as does a TTL that is not a whole number. The
# chosen path is still tagged and still repaired. Checkouts one and two share
# a cache root here, so one root holds an untagged chosen directory, a dangling
# chosen link and a dangling prefix-named link.
# ---------------------------------------------------------------------------

if begin 10a "$one"; then
    chosen_one="$chosen"
    chosen_two="$(chosen_for "$two" "$cache_home")" || chosen_two=""
    if [ -z "$chosen_two" ]; then
        record_fail "case 10a: the wrapper names a cache path for checkout two" \
            "it printed nothing usable"
    else
        mkdir -p "$chosen_one"
        ln -s "$T/vol/c10a-gone" "$chosen_two"
        ln -s "$T/vol/c10a-gone-too" "$root/${P}c10a"

        run_wrapper "$one" "$cache_home" 0 true
        assert_tag "case 10a: TTL 0 still tags the chosen directory" "$chosen_one/CACHEDIR.TAG"
        assert_dangling "case 10a: TTL 0 leaves a dangling prefix-named link" "$root/${P}c10a"

        run_wrapper "$two" "$cache_home" 0 sh -c "$CREATE_TARGET"
        assert_status "case 10a: TTL 0 still removes a dangling link at the chosen path" 0
        assert_dangling "case 10a: ...and still leaves the other dangling link" "$root/${P}c10a"
    fi
fi

if begin 10b "$one"; then
    mkdir -p "$chosen"
    ln -s "$T/vol/c10b-gone" "$root/${P}c10b"
    run_wrapper "$one" "$cache_home" 7d true
    assert_status "case 10b: a TTL of 7d does not stop the wrapper" 0
    assert_tag "case 10b: a TTL of 7d still tags the chosen directory" "$chosen/CACHEDIR.TAG"
    assert_dangling "case 10b: a TTL of 7d turns the sweep off" "$root/${P}c10b"
fi

# ---------------------------------------------------------------------------
# 11. Entries the sweep leaves alone: a prefix-named link to a file, and a
# dangling link whose name lacks the prefix.
# ---------------------------------------------------------------------------

if begin 11 "$one"; then
    printf 'not a cache\n' >"$T/vol/c11-file"
    age "$T/vol/c11-file"
    ln -s "$T/vol/c11-file" "$root/${P}c11-file"
    ln -s "$T/vol/c11-gone" "$root/not-a-cache-c11"
    run_wrapper "$one" "$cache_home" 7 true
    assert_link "case 11: a prefix-named link to a file is kept" "$root/${P}c11-file"
    assert_exists "case 11: the file is kept" "$T/vol/c11-file"
    assert_dangling "case 11: a dangling link without the prefix is kept" \
        "$root/not-a-cache-c11"
fi

# ---------------------------------------------------------------------------
# 12. A link to a directory that contains the cache root is never followed
# into a removal, even when that directory looks like a stale release build.
# begin creates the cache root inside $T/c12, so $T/c12 is aged after it.
# ---------------------------------------------------------------------------

if requires "case 12" touch-d; then
    mkdir -p "$T/c12/release/.fingerprint"
    if begin 12 "$one"; then
        link="$root/${P}c12"
        ln -s "$T/c12" "$link"
        age "$T/c12/release" "$T/c12"
        run_wrapper "$one" "$cache_home" 7 true
        assert_dir "case 12: a link target that contains the cache root is kept" \
            "$T/c12/release/.fingerprint"
        assert_dir "case 12: the cache root is kept" "$root"
        assert_link "case 12: the link is kept" "$link"
    fi
fi

# ---------------------------------------------------------------------------
# 13. The everyday path on a host like the one in the issue: the chosen path is
# a link to a cache on another volume. The first run tags it silently, and the
# second has nothing to do and prints nothing at all.
# ---------------------------------------------------------------------------

if begin 13 "$one"; then
    tgt="$T/vol/c13"
    make_target "$tgt"
    ln -s "$tgt" "$chosen"
    run_wrapper "$one" "$cache_home" 7 true
    first_output="$run_out$run_err"
    run_wrapper "$one" "$cache_home" 7 true
    assert_tag "case 13: the linked cache is tagged" "$tgt/CACHEDIR.TAG"
    assert_empty "case 13: the first run prints nothing" "$first_output"
    assert_empty "case 13: the second run prints nothing on stdout" "$run_out"
    assert_empty "case 13: the second run prints nothing on stderr" "$run_err"
    assert_link "case 13: the chosen path is still a link" "$chosen"
fi

# ---------------------------------------------------------------------------
# 14. A sibling link that resolves to the directory behind this run's cache
# path points at this run's cache, which the sweep never touches.
# ---------------------------------------------------------------------------

if requires "case 14" touch-d && begin 14 "$one"; then
    tgt="$T/vol/c14"
    make_target "$tgt"
    age_tree "$tgt"
    ln -s "$tgt" "$chosen"
    ln -s "$tgt" "$root/${P}c14"
    run_wrapper "$one" "$cache_home" 7 true
    assert_dir "case 14: the directory behind this run's cache path is kept" \
        "$tgt/debug/.fingerprint"
    assert_link "case 14: this run's cache link is kept" "$chosen"
    assert_link "case 14: the sibling link is kept" "$root/${P}c14"
fi

# ---------------------------------------------------------------------------
# 15. A newline in an entry's name must not split it into two paths. Read line
# by line, "<root>/<P>x<newline>victim" becomes the relative path "victim",
# which then names a directory in the hook's working directory.
# ---------------------------------------------------------------------------

if requires "case 15" touch-d && begin 15 "$one"; then
    mkdir -p "$one/victim"
    age "$one/victim"
    odd="$root/${P}x"$'\n'"victim"
    mkdir -p "$odd"
    age "$odd"
    run_wrapper "$one" "$cache_home" 7 true
    assert_dir "case 15: a directory in the checkout named after the second line is kept" \
        "$one/victim"
    assert_gone "case 15: the stale entry with a newline in its name is reclaimed" "$odd"
fi

# ---------------------------------------------------------------------------
# 16. A link whose target name ends in a newline. Command substitution drops
# trailing newlines, so the resolved path names a different directory: here, a
# stale cargo target beside the real one. The sweep must notice and keep both.
# ---------------------------------------------------------------------------

if requires "case 16" touch-d && begin 16 "$one"; then
    tgt="$T/vol/c16"
    make_target "$tgt"
    age_tree "$tgt"
    mkdir -p "$tgt"$'\n'
    ln -s "$tgt"$'\n' "$root/${P}c16"
    run_wrapper "$one" "$cache_home" 7 true
    assert_dir "case 16: the directory the shortened path names is kept" \
        "$tgt/debug/.fingerprint"
    assert_dir "case 16: the link's own target is kept" "$tgt"$'\n'
    assert_link "case 16: the link is kept" "$root/${P}c16"
fi

# ---------------------------------------------------------------------------
# 17. A link to the hook's working directory, which here holds a stale
# .rustc_info.json, is never followed into a removal.
# ---------------------------------------------------------------------------

if requires "case 17" touch-d && begin 17 "$three"; then
    printf '{}\n' >"$three/.rustc_info.json"
    age "$three/.rustc_info.json" "$three"
    ln -s "$three" "$root/${P}c17"
    run_wrapper "$three" "$cache_home" 7 true
    assert_dir "case 17: the checkout behind the link is kept" "$three/.git"
    assert_exists "case 17: its .rustc_info.json is kept" "$three/.rustc_info.json"
    assert_link "case 17: the link is kept" "$root/${P}c17"
fi

# ---------------------------------------------------------------------------
# 18. Likewise for HOME. This case gives the wrapper a HOME of its own, still
# inside the temporary root.
# ---------------------------------------------------------------------------

if requires "case 18" touch-d; then
    RUN_HOME="$T/c18/home"
    mkdir -p "$RUN_HOME"
    if begin 18 "$one"; then
        printf '{}\n' >"$RUN_HOME/.rustc_info.json"
        age "$RUN_HOME/.rustc_info.json" "$RUN_HOME"
        ln -s "$RUN_HOME" "$root/${P}c18"
        run_wrapper "$one" "$cache_home" 7 true
        assert_exists "case 18: HOME behind a link is kept" "$RUN_HOME/.rustc_info.json"
        assert_link "case 18: the link is kept" "$root/${P}c18"
    fi
    RUN_HOME="$HOME"
fi

# ---------------------------------------------------------------------------
# 19. The age rule for plain directories. Cargo writes into debug/ and
# release/ and rewrites .rustc_info.json, while the cache directory's own
# modification time changes only when an entry directly inside it is added or
# removed. A cache is stale only when every one of those paths is.
# ---------------------------------------------------------------------------

if requires "case 19" touch-d && begin 19 "$one"; then
    for fresh in none debug release info; do
        d="$root/${P}c19-$fresh"
        mkdir -p "$d/debug/.fingerprint" "$d/release"
        : >"$d/debug/.cargo-lock"
        printf '{}\n' >"$d/.rustc_info.json"
        case "$fresh" in
            debug) age "$d/release" "$d/.rustc_info.json" ;;
            release) age "$d/debug" "$d/.rustc_info.json" ;;
            info) age "$d/debug" "$d/release" ;;
            *) age "$d/debug" "$d/release" "$d/.rustc_info.json" ;;
        esac
        age "$d"
    done
    run_wrapper "$one" "$cache_home" 7 true
    assert_gone "case 19: a cache with every timestamp stale is reclaimed" \
        "$root/${P}c19-none"
    assert_contains "case 19: stderr names it" "$root/${P}c19-none" "$run_err"
    assert_dir "case 19: a fresh debug/ keeps a cache" "$root/${P}c19-debug"
    assert_dir "case 19: a fresh release/ keeps a cache" "$root/${P}c19-release"
    assert_dir "case 19: a fresh .rustc_info.json keeps a cache" "$root/${P}c19-info"
fi

# ---------------------------------------------------------------------------
# 20. The same rule through a link: an old resolved directory whose debug/ was
# written recently is a cache in use from day to day.
# ---------------------------------------------------------------------------

if requires "case 20" touch-d && begin 20 "$one"; then
    tgt="$T/vol/c20"
    make_target "$tgt"
    printf '{}\n' >"$tgt/.rustc_info.json"
    age "$tgt/.rustc_info.json" "$tgt"
    ln -s "$tgt" "$root/${P}c20"
    run_wrapper "$one" "$cache_home" 7 true
    assert_dir "case 20: a linked cache with a fresh debug/ is kept" "$tgt/debug/.fingerprint"
    assert_link "case 20: its link is kept" "$root/${P}c20"
fi

# ---------------------------------------------------------------------------
# 21. The in-use check looks where cargo keeps its locks: debug/.cargo-lock,
# and <triple>/debug/.cargo-lock for a cross build. flock(1) creates a missing
# file and then always gets the lock, so the probe must never create one.
# ---------------------------------------------------------------------------

if requires "case 21" touch-d flock && begin 21 "$one"; then
    plain="$root/${P}c21-plain"
    mkdir -p "$plain/debug/.fingerprint" "$plain/release"
    : >"$plain/debug/.cargo-lock"
    age_tree "$plain"
    tgt="$T/vol/c21"
    triple="$tgt/x86_64-unknown-linux-gnu"
    mkdir -p "$triple/debug/.fingerprint"
    : >"$triple/debug/.cargo-lock"
    printf '{}\n' >"$tgt/.rustc_info.json"
    age_tree "$tgt"
    ln -s "$tgt" "$root/${P}c21"
    exec 8>"$plain/debug/.cargo-lock"
    exec 9>"$triple/debug/.cargo-lock"
    if flock --nonblock --exclusive 8 && flock --nonblock --exclusive 9; then
        run_wrapper "$one" "$cache_home" 7 true
        assert_dir "case 21: a plain cache whose debug/.cargo-lock is held is kept" \
            "$plain/debug/.fingerprint"
        assert_dir "case 21: a linked cache whose <triple>/debug/.cargo-lock is held is kept" \
            "$triple/debug/.fingerprint"
        assert_link "case 21: the link to it is kept" "$root/${P}c21"
        assert_gone "case 21: no lock file is created at the top of the plain cache" \
            "$plain/.cargo-lock"
        assert_gone "case 21: no lock file is created in its release/" \
            "$plain/release/.cargo-lock"
        assert_gone "case 21: no lock file is created at the top of the linked cache" \
            "$tgt/.cargo-lock"
        assert_gone "case 21: no lock file is created in its target-triple directory" \
            "$triple/.cargo-lock"
    else
        record_fail "case 21: the test holds both lock files" "flock on fd 8 or 9 failed"
    fi
    exec 8>&- 9>&-
fi

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
