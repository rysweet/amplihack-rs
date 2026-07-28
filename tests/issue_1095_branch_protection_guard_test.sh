#!/usr/bin/env bash
# TDD tests for issue #1095 — branch-protection strict-mode detection guard.
#
# Contract:
#   scripts/check-branch-protection.sh reads required_status_checks.strict for
#   the protected branch (via `gh api --jq`, structured extraction only) and:
#     - exits 0 only when strict is exactly "true";
#     - exits 1 when strict is "false" (or empty/other), printing an error that
#       says it expected 'true';
#     - exits 1 with a "not configured" error when the GH_TOKEN secret is
#       absent (loud fail, never a silent pass);
#     - exits 1 when the underlying `gh api` call fails;
#     - resolves the repo slug via `gh repo view` when GITHUB_REPOSITORY is
#       unset (local / manual runs), then applies the same strict check.
#
# These tests are fully self-contained: a fake `gh` is placed early on PATH so
# the script never contacts the real GitHub API. The fake is driven by the
# FAKE_STRICT environment variable.
#
# Run: bash tests/issue_1095_branch_protection_guard_test.sh

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GUARD="$REPO_ROOT/scripts/check-branch-protection.sh"

pass=0
fail=0
TMPROOT=""

cleanup() { [ -n "$TMPROOT" ] && rm -rf "$TMPROOT"; }
trap cleanup EXIT

record_pass() {
    echo "PASS: $1"
    pass=$((pass + 1))
}

record_fail() {
    echo "FAIL: $1"
    fail=$((fail + 1))
}

TMPROOT="$(mktemp -d)"
BINDIR="$TMPROOT/bin"
mkdir -p "$BINDIR"

# Fake `gh`: handles both the protection query and the repo-view fallback.
# FAKE_STRICT drives behaviour:
#   true|false|<other>  -> echoed as the strict value for the api call
#   apierror            -> the api call exits non-zero (simulates 403 / bad repo)
cat >"$BINDIR/gh" <<'FAKE'
#!/usr/bin/env bash
set -uo pipefail
if [ "${1:-}" = "api" ]; then
    if [ "${FAKE_STRICT:-}" = "apierror" ]; then
        echo "HTTP 403: Must have admin rights to Repository." >&2
        exit 1
    fi
    echo "${FAKE_STRICT:-}"
    exit 0
fi
if [ "${1:-}" = "repo" ] && [ "${2:-}" = "view" ]; then
    echo "octo/example"
    exit 0
fi
echo "fake gh: unhandled args: $*" >&2
exit 2
FAKE
chmod +x "$BINDIR/gh"

# Common environment for invoking the guard with the fake gh on PATH.
run_guard() {
    # Args are passed as VAR=value env assignments plus stderr capture path.
    local errfile="$1"
    shift
    PATH="$BINDIR:$PATH" GITHUB_REPOSITORY="octo/example" \
        env "$@" bash "$GUARD" 2>"$errfile"
}

# Variant that runs the guard with GITHUB_REPOSITORY unset, forcing the script
# down the `gh repo view` slug-resolution fallback.
run_guard_no_repo() {
    local errfile="$1"
    shift
    PATH="$BINDIR:$PATH" \
        env -u GITHUB_REPOSITORY "$@" bash "$GUARD" 2>"$errfile"
}

test_guard_exists() {
    if [ -x "$GUARD" ]; then
        record_pass "check-branch-protection.sh exists and is executable"
    else
        record_fail "check-branch-protection.sh missing or not executable at $GUARD"
    fi
}

test_strict_true_passes() {
    local err="$TMPROOT/err_true"
    if run_guard "$err" GH_TOKEN=x FAKE_STRICT=true; then
        record_pass "strict=true -> exit 0"
    else
        record_fail "strict=true should exit 0 (stderr: $(cat "$err"))"
    fi
}

test_strict_false_fails() {
    local err="$TMPROOT/err_false"
    if run_guard "$err" GH_TOKEN=x FAKE_STRICT=false; then
        record_fail "strict=false should exit non-zero but exited 0"
    else
        if grep -q "expected 'true'" "$err"; then
            record_pass "strict=false -> non-zero and stderr mentions expected 'true'"
        else
            record_fail "strict=false: stderr missing \"expected 'true'\" (got: $(cat "$err"))"
        fi
    fi
}

test_missing_token_fails() {
    local err="$TMPROOT/err_token"
    # GH_TOKEN explicitly empty; FAKE_STRICT=true proves it fails on token, not value.
    if run_guard "$err" GH_TOKEN= FAKE_STRICT=true; then
        record_fail "missing GH_TOKEN should exit non-zero but exited 0"
    else
        if grep -q "not configured" "$err"; then
            record_pass "missing token -> non-zero and stderr mentions 'not configured'"
        else
            record_fail "missing token: stderr missing 'not configured' (got: $(cat "$err"))"
        fi
    fi
}

test_api_error_fails() {
    local err="$TMPROOT/err_api"
    if run_guard "$err" GH_TOKEN=x FAKE_STRICT=apierror; then
        record_fail "api error should exit non-zero but exited 0"
    else
        record_pass "api error -> non-zero exit"
    fi
}

test_slug_fallback_resolves_and_passes() {
    local err="$TMPROOT/err_slug"
    # GITHUB_REPOSITORY unset -> script must call `gh repo view` (fake returns
    # octo/example) and then honour the strict value like any other run.
    if run_guard_no_repo "$err" GH_TOKEN=x FAKE_STRICT=true; then
        record_pass "GITHUB_REPOSITORY unset -> gh repo view fallback -> exit 0"
    else
        record_fail "slug fallback with strict=true should exit 0 (stderr: $(cat "$err"))"
    fi
}

test_guard_exists
test_strict_true_passes
test_strict_false_fails
test_missing_token_fails
test_api_error_fails
test_slug_fallback_resolves_and_passes

echo "=== Results: $pass passed, $fail failed ==="
[ "$fail" -eq 0 ] || exit 1
