#!/usr/bin/env bash
# Issue #1278: git's own environment must not leak into what the hooks run.
#
# git exports GIT_DIR (and, per hook, GIT_INDEX_FILE / GIT_PREFIX /
# GIT_WORK_TREE) into every hook. Those variables outrank the working
# directory when git picks a repository, so anything the hook starts --
# cargo, a test binary, a git subprocess -- operates on the hook's repository
# instead of the one it was pointed at. That made the pre-push `cargo test`
# gate unpassable, which is exactly the pressure that produces `--no-verify`.
#
# This test locks in all three halves of the fix:
#   1. scripts/git-hook-cargo.sh removes the leaked variables.
#   2. Its list stays identical to amplihack_git::REPOSITORY_ENV_VARS.
#   3. scripts/check-git-command-sanitised.sh actually rejects a raw
#      Command::new("git"), so the Rust-side fix cannot be silently opted out of.
#   4. The build cache the wrapper picks is specific to the checkout (#1381).
#      One hardcoded CARGO_TARGET_DIR shared by every worktree and parallel
#      agent on a host makes cargo reuse another branch's compiled metadata
#      against your source, so the hook fails on somebody else's code.
#
# Run: bash tests/issue_1278_git_env_isolation.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WRAPPER="$REPO_ROOT/scripts/git-hook-cargo.sh"
GUARD="$REPO_ROOT/scripts/check-git-command-sanitised.sh"
HELPER_SRC="$REPO_ROOT/crates/amplihack-git/src/lib.rs"
PRECOMMIT="$REPO_ROOT/.pre-commit-config.yaml"

pass=0
fail=0
TMPROOT="$(mktemp -d)"
trap 'rm -rf "$TMPROOT"' EXIT

# ---------------------------------------------------------------------------
# Scrub the inherited git environment BEFORE building any fixture.
#
# This is not ceremony. It is the bug under test biting the test itself:
# `git init <path>` honours an ambient GIT_DIR and *ignores its path argument*.
# Run from a git hook -- the exact environment this file exists to describe --
# the fixture repositories below are never created where the test believes they
# are. The control assertion then points GIT_DIR at a directory that was never
# created, fails, and reports "git no longer honours GIT_DIR the way this test
# assumes", which is the precise opposite of what happened.
#
# CI runs with a clean environment, so that gap is invisible there and shows up
# only for the people this change is written for. A test about an environment
# leak has to be correct in the presence of the leak.
#
# The list is read out of the Rust constant so it cannot drift away from
# amplihack_git::REPOSITORY_ENV_VARS.
# ---------------------------------------------------------------------------
mapfile -t rust_vars < <(
    sed -n '/^pub const REPOSITORY_ENV_VARS/,/^];$/p' "$HELPER_SRC" \
        | grep -o '"GIT_[A-Z_]*"' | tr -d '"' | sort
)
if [ "${#rust_vars[@]}" -gt 0 ]; then
    unset "${rust_vars[@]}"
fi

# Never touch the real HOME: git reads config from it and can be made to write
# there. Every fixture in this file is disposable; the developer's HOME is not.
export HOME="$TMPROOT/home"
mkdir -p "$HOME"

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    if [ $# -gt 1 ] && [ -n "$2" ]; then
        printf '      %s\n' "$2"
    fi
    fail=$((fail + 1))
}

assert() {
    local desc="$1"
    shift
    if "$@"; then
        record_pass "$desc"
    else
        record_fail "$desc" "command failed: $*"
    fi
}

assert_eq() {
    local desc="$1"
    local expected="$2"
    local actual="$3"
    if [ "$actual" = "$expected" ]; then
        record_pass "$desc"
    else
        record_fail "$desc" "expected: <$expected>; actual: <$actual>"
    fi
}

# A hijacked fixture must fail loudly as a setup error rather than quietly as a
# wrong result three assertions later. `git init` returns success either way.
assert_repo_created() {
    local dir="$1"
    if [ -d "$dir/.git" ]; then
        record_pass "fixture: $(basename "$dir") is a real repository"
    else
        record_fail "fixture: $(basename "$dir") is a real repository" \
            "no $dir/.git -- an inherited GIT_DIR hijacked \`git init\`"
    fi
}

assert "scripts/git-hook-cargo.sh exists" test -x "$WRAPPER"
assert "scripts/check-git-command-sanitised.sh exists" test -x "$GUARD"

# ---------------------------------------------------------------------------
# 1. The wrapper removes every repository-selecting variable it is handed.
# ---------------------------------------------------------------------------

# rust_vars was parsed and scrubbed at the top of this file, before any fixture
# existed. Confirm it actually found something: an empty list would silently
# turn every check below into a no-op.
assert "REPOSITORY_ENV_VARS parsed from the Rust source" test "${#rust_vars[@]}" -gt 0

leaked=""
for var in "${rust_vars[@]}"; do
    # Set every one, then ask the wrapper what the child actually sees.
    seen="$(env "$var=/nonexistent/leaked" "$WRAPPER" printenv "$var" 2>/dev/null)"
    if [ -n "$seen" ]; then
        leaked="$leaked $var"
    fi
done
assert_eq "git-hook-cargo.sh scrubs every REPOSITORY_ENV_VARS entry" "" "$leaked"

# ...and leaves the commit identity alone, which selects an author, not a repo.
identity="$(env GIT_AUTHOR_NAME=preserved "$WRAPPER" printenv GIT_AUTHOR_NAME 2>/dev/null)"
assert_eq "git-hook-cargo.sh preserves GIT_AUTHOR_NAME" "preserved" "$identity"

# ---------------------------------------------------------------------------
# 2. The hooks that build or run code go through the wrapper.
# ---------------------------------------------------------------------------

for hook in artifact-guard cargo-clippy cargo-test; do
    entry="$(awk -v id="      - id: $hook" '
        $0 == id { found = 1; next }
        found && /^        entry:/ { print; exit }
    ' "$PRECOMMIT")"
    case "$entry" in
        *scripts/git-hook-cargo.sh*)
            record_pass "hook '$hook' runs through git-hook-cargo.sh" ;;
        *)
            record_fail "hook '$hook' runs through git-hook-cargo.sh" \
                "entry was: ${entry:-<not found>}" ;;
    esac
done

# ---------------------------------------------------------------------------
# 3. The guard rejects a raw Command::new("git") and accepts a clean tree.
# ---------------------------------------------------------------------------

clean_tree="$TMPROOT/clean/crates/demo/src"
mkdir -p "$clean_tree"
cat > "$clean_tree/lib.rs" <<'RS'
pub fn head() -> std::io::Result<std::process::Output> {
    amplihack_git::command().args(["rev-parse", "HEAD"]).output()
}
RS
assert "guard accepts a tree that uses amplihack_git::command()" \
    "$GUARD" "$TMPROOT/clean"

dirty_tree="$TMPROOT/dirty/crates/demo/src"
mkdir -p "$dirty_tree"
cat > "$dirty_tree/lib.rs" <<'RS'
pub fn head() -> std::io::Result<std::process::Output> {
    std::process::Command::new("git").args(["rev-parse", "HEAD"]).output()
}
RS
if "$GUARD" "$TMPROOT/dirty" >/dev/null 2>&1; then
    record_fail "guard rejects a raw Command::new(\"git\")" "guard exited 0"
else
    record_pass "guard rejects a raw Command::new(\"git\")"
fi

assert "guard passes on the real repository" "$GUARD"

# ---------------------------------------------------------------------------
# 4. The behaviour itself: an ambient GIT_DIR must not follow git around.
#
# This is the bug in one command. A directory that is not a repository must
# stay not-a-repository even when the environment names one somewhere else.
# ---------------------------------------------------------------------------

decoy="$TMPROOT/decoy"
plain="$TMPROOT/plain"
mkdir -p "$decoy" "$plain"
git init --quiet "$decoy" >/dev/null 2>&1
assert_repo_created "$decoy"

# Control: unsanitised, the ambient GIT_DIR hijacks the probe. If this ever
# stops being true the assertion below stops proving anything, so assert it.
if GIT_DIR="$decoy/.git" git -C "$plain" rev-parse --git-dir >/dev/null 2>&1; then
    record_pass "control: an ambient GIT_DIR hijacks an unsanitised git command"
else
    record_fail "control: an ambient GIT_DIR hijacks an unsanitised git command" \
        "git no longer honours GIT_DIR the way this test assumes"
fi

# Sanitised the way the wrapper does it: the plain directory stays plain.
if GIT_DIR="$decoy/.git" "$WRAPPER" git -C "$plain" rev-parse --git-dir >/dev/null 2>&1; then
    record_fail "a scrubbed environment leaves a non-repo a non-repo" \
        "git still resolved a repository for $plain"
else
    record_pass "a scrubbed environment leaves a non-repo a non-repo"
fi

# ---------------------------------------------------------------------------
# 5. The build cache is per-checkout (issue #1381).
#
# A constant CARGO_TARGET_DIR is the obvious "simplification" and it is exactly
# the bug: two checkouts sharing one cache makes cargo compile your source
# against another branch's metadata.
# ---------------------------------------------------------------------------

# Two distinct checkouts must not resolve to the same cache.
one="$TMPROOT/checkout-one"
two="$TMPROOT/checkout-two"
git init --quiet "$one" >/dev/null 2>&1
git init --quiet "$two" >/dev/null 2>&1
assert_repo_created "$one"
assert_repo_created "$two"

target_dir_for() {
    # `env -u` so an ambient CARGO_TARGET_DIR cannot mask the default.
    (cd "$1" && env -u CARGO_TARGET_DIR "$WRAPPER" printenv CARGO_TARGET_DIR)
}

dir_one="$(target_dir_for "$one")"
dir_two="$(target_dir_for "$two")"

if [ -n "$dir_one" ] && [ "$dir_one" != "$dir_two" ]; then
    record_pass "separate checkouts get separate build caches"
else
    record_fail "separate checkouts get separate build caches" \
        "both resolved to <$dir_one>"
fi

# ...but one checkout must stay stable across runs, or the cache is pointless.
assert_eq "the same checkout reuses its own cache" "$dir_one" "$(target_dir_for "$one")"

# An explicit value is a caller's stated intent and must win. Ambient state
# overriding explicit intent is the bug this whole change is about.
explicit="$(cd "$one" && CARGO_TARGET_DIR=/explicit/path "$WRAPPER" printenv CARGO_TARGET_DIR)"
assert_eq "an explicit CARGO_TARGET_DIR is not overridden" "/explicit/path" "$explicit"

echo
echo "passed: $pass, failed: $fail"
[ "$fail" -eq 0 ]
