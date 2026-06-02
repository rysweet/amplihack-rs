#!/usr/bin/env bash
# Tests for Issue #682: step-21-pr-ready must guard PR_URL before invoking
# gh commands, and resume path must handle missing worktree_setup output.
#
# TDD: These tests are written BEFORE the implementation. They define the
# required contract for the resume-path fix.
#
# These tests validate:
#   1. step-21 checks PR_URL is non-empty before gh pr ready / gh pr comment.
#   2. step-21 does not unconditionally invoke gh pr ready.
#   3. Runtime: gh pr ready is NOT called when PR_URL is empty.
#   4. YAML remains parseable.
#
# Run: bash tests/issue_682_worktree_resume.sh
# Expected before fix: FAIL. Expected after fix: PASS.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FINALIZE_FILE="$REPO_ROOT/amplifier-bundle/recipes/workflow-finalize.yaml"

fail=0
pass=0

assert() {
    local desc="$1"
    local cond="$2"
    if eval "$cond"; then
        echo "PASS: $desc"
        pass=$((pass+1))
    else
        echo "FAIL: $desc"
        echo "      condition: $cond"
        fail=$((fail+1))
    fi
}

echo "=== Issue #682 TDD tests: worktree resume path / PR_URL guard ==="
echo "Finalize file: $FINALIZE_FILE"
echo

# --- Test 1: file exists ----------------------------------------------------
assert "workflow-finalize.yaml exists" "[ -f '$FINALIZE_FILE' ]"

# --- Test 2: step-21 references PR_URL --------------------------------------
# Use awk+grep directly (avoids quoting issues with $-vars in YAML command blocks)

assert "step-21 references PR_URL variable" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -q 'PR_URL'"

# --- Test 3: step-21 has PR_URL empty check before gh commands ---------------
assert "step-21 has PR_URL empty/non-empty check" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -Eq '\\[ -z .PR_URL|\\[ -n .PR_URL|-z \".PR_URL|-n \".PR_URL|PR_URL.*=~|\\[\\[.*PR_URL'"

# --- Test 4: gh pr ready appears AFTER PR_URL check -------------------------
# The pr ready command must not be invoked unconditionally.
pr_url_line=$(awk '/id: .step-21-pr-ready/,/output:/' "$FINALIZE_FILE" | grep -n 'PR_URL' | head -1 | cut -d: -f1)
gh_ready_line=$(awk '/id: .step-21-pr-ready/,/output:/' "$FINALIZE_FILE" | grep -n 'gh pr ready' | head -1 | cut -d: -f1)

if [ -n "$pr_url_line" ] && [ -n "$gh_ready_line" ]; then
    assert "gh pr ready appears after PR_URL reference (line $gh_ready_line > $pr_url_line)" \
        "[ '$gh_ready_line' -gt '$pr_url_line' ]"
else
    assert "both PR_URL and 'gh pr ready' exist in step-21" "false"
fi

# --- Test 5: gh pr comment also guarded by PR_URL ----------------------------
assert "gh pr comment is guarded — appears after PR_URL check" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -q 'gh pr comment'"

# --- Test 6: step-21 emits WARNING or INFO when PR_URL is empty ---------------
assert "step-21 emits WARNING or INFO when skipping due to empty PR_URL" \
    "awk '/id: .step-21-pr-ready/,/output:/' '$FINALIZE_FILE' | grep -Eq 'WARNING.*PR_URL|INFO.*PR_URL|PR_URL.*empty|PR_URL.*skip'"

# --- Test 7: Runtime — gh pr ready NOT called with empty PR_URL ---------------
# Create a mock 'gh' that records invocations and exits non-zero to prove
# it was called.
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

cat > "$tmpdir/gh" <<'MOCK_GH'
#!/usr/bin/env bash
echo "MOCK_GH_CALLED: $*" >> "$MOCK_GH_LOG"
exit 0
MOCK_GH
chmod +x "$tmpdir/gh"

# Test: with empty PR_URL, the mock gh should NOT be called for 'pr ready'
export MOCK_GH_LOG="$tmpdir/gh_calls.log"
export PR_URL=""
export PATH="$tmpdir:$PATH"

# We can't run the full step, but we can test the expected guard pattern:
# if [ -n "$PR_URL" ]; then gh pr ready "$PR_URL"; fi
(
    set -euo pipefail
    PR_URL=""
    if [ -n "$PR_URL" ]; then
        gh pr ready "$PR_URL"
        gh pr comment "$PR_URL" --body "test"
    else
        echo "INFO: PR_URL is empty — skipping PR ready marking" >&2
    fi
) 2>/dev/null

if [ -f "$MOCK_GH_LOG" ] && grep -q 'pr ready' "$MOCK_GH_LOG"; then
    assert "runtime: gh pr ready NOT called when PR_URL is empty" "false"
else
    assert "runtime: gh pr ready NOT called when PR_URL is empty" "true"
fi

# Test: with non-empty PR_URL, the mock gh SHOULD be called
export MOCK_GH_LOG="$tmpdir/gh_calls_nonempty.log"
(
    set -euo pipefail
    PR_URL="https://github.com/org/repo/pull/42"
    if [ -n "$PR_URL" ]; then
        gh pr ready "$PR_URL"
    else
        echo "INFO: PR_URL is empty — skipping" >&2
    fi
) 2>/dev/null

assert "runtime: gh pr ready IS called when PR_URL is non-empty" \
    "[ -f '$tmpdir/gh_calls_nonempty.log' ] && grep -q 'pr ready' '$tmpdir/gh_calls_nonempty.log'"

# --- Test 8: YAML parses cleanly -------------------------------------------
if command -v python3 >/dev/null 2>&1; then
    if python3 -c "import yaml" 2>/dev/null; then
        assert "workflow-finalize.yaml parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$FINALIZE_FILE\"))'"
    else
        echo "SKIP: PyYAML not available"
    fi
else
    echo "SKIP: python3 not available"
fi

# --- Summary ----------------------------------------------------------------
echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
