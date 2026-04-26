#!/usr/bin/env bash
# Tests for Issue #413: workflow-publish.yaml must fail loud when
# WORKTREE_SETUP_WORKTREE_PATH is unset, instead of silently falling back to
# $REPO_PATH.
#
# These tests validate:
#   1. The two target lines (112 and 180) use ${VAR:?diagnostic} form.
#   2. No silent fallback ${VAR:-$REPO_PATH} remains in execution paths.
#   3. The diagnostic strings reference the correct step (15 / 16) and source.
#   4. Manual reproduction with `unset` produces the diagnostic on stderr and
#      a non-zero exit, never executing the cd.
#   5. Diagnostic echo lines (informational ${VAR:-(unset)}) are preserved.
#   6. YAML remains parseable.
#
# Run: bash tests/issue_413_fail_loud_worktree.sh
# Expected before fix: FAIL. Expected after fix: PASS.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FILE="$REPO_ROOT/amplifier-bundle/recipes/workflow-publish.yaml"

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

echo "=== Issue #413 TDD tests ==="
echo "Target file: $FILE"
echo

# --- Test 1: file exists ----------------------------------------------------
assert "workflow-publish.yaml exists" "[ -f '$FILE' ]"

# --- Test 2: no silent fallback for WORKTREE_SETUP_WORKTREE_PATH ------------
silent_count=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:-\$REPO_PATH' "$FILE" || true)
assert "no '\${WORKTREE_SETUP_WORKTREE_PATH:-\$REPO_PATH}' remains (found=$silent_count)" \
    "[ '$silent_count' = '0' ]"

# --- Test 3: fail-loud form present at exactly two execution sites ----------
fail_loud_count=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:?' "$FILE" || true)
assert "exactly two '\${WORKTREE_SETUP_WORKTREE_PATH:?...}' execution sites (found=$fail_loud_count)" \
    "[ '$fail_loud_count' = '2' ]"

# --- Test 4: step-15 diagnostic references step-15 and step-04 source -------
assert "step-15 diagnostic references step-15 and workflow-worktree" \
    "grep -q 'step-15 requires worktree_setup.worktree_path from step-04 (workflow-worktree)' '$FILE'"

# --- Test 5: step-16 diagnostic references step-16 and step-04 source -------
assert "step-16 diagnostic references step-16 and workflow-worktree" \
    "grep -q 'step-16 requires worktree_setup.worktree_path from step-04 (workflow-worktree)' '$FILE'"

# --- Test 6: cd uses quoted expansion (no bare $VAR) ------------------------
bare_cd=$(grep -cE 'cd \$WORKTREE_SETUP_WORKTREE_PATH' "$FILE" || true)
assert "no bare unquoted 'cd \$WORKTREE_SETUP_WORKTREE_PATH'" "[ '$bare_cd' = '0' ]"

# --- Test 7: diagnostic echo lines preserved (informational) ----------------
# These two are intentionally NOT changed (they are log output, not exec paths).
assert "informational echo with WORKTREE_SETUP_WORKTREE_PATH:-(unset) preserved" \
    "grep -q 'WORKTREE_SETUP_WORKTREE_PATH:-(unset)' '$FILE'"

# --- Test 8: set -euo pipefail present in both step blocks ------------------
# :? requires errexit to abort the recipe step.
step15_block=$(awk '/id: "step-15-commit-push"/,/id: "step-16-create-draft-pr"/' "$FILE")
assert "step-15 block has 'set -euo pipefail'" \
    "echo \"\$step15_block\" | grep -q 'set -euo pipefail'"

step16_block=$(awk '/id: "step-16-create-draft-pr"/,/^  - id:/' "$FILE" | tail -n +2 | awk 'NR==1{print; next} /^  - id:/{exit} {print}')
# Fallback simpler check: just grep within a window after step-16 marker.
assert "step-16 block has 'set -euo pipefail'" \
    "awk '/id: \"step-16-create-draft-pr\"/{f=1} f' '$FILE' | head -20 | grep -q 'set -euo pipefail'"

# --- Test 9: YAML parses cleanly --------------------------------------------
if command -v python3 >/dev/null 2>&1; then
    if python3 -c "import yaml" 2>/dev/null; then
        assert "YAML parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$FILE\"))'"
    else
        echo "SKIP: PyYAML not available"
    fi
else
    echo "SKIP: python3 not available"
fi

# --- Test 10: runtime fail-loud reproduction --------------------------------
# Validates the actual shell semantics of the diagnostic form used in the file.
diag='step-15 requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs'
out=$(unset WORKTREE_SETUP_WORKTREE_PATH; bash -c 'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?'"$diag"'}"' 2>&1)
rc=$?
assert "fail-loud reproduction exits non-zero" "[ '$rc' != '0' ]"
assert "fail-loud reproduction emits step-15 diagnostic on stderr" \
    "echo \"\$out\" | grep -q 'step-15 requires worktree_setup.worktree_path'"

# --- Test 11: success path unchanged when var is set ------------------------
tmpdir=$(mktemp -d)
WORKTREE_SETUP_WORKTREE_PATH="$tmpdir" bash -c \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?should-not-fire}" && pwd' \
    >/dev/null
rc=$?
rm -rf "$tmpdir"
assert "success path: cd succeeds when WORKTREE_SETUP_WORKTREE_PATH is set" "[ '$rc' = '0' ]"

# --- Summary ----------------------------------------------------------------
echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
