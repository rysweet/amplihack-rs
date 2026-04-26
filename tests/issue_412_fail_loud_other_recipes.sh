#!/usr/bin/env bash
# Tests for Issue #412: the four sibling recipes that still use the
# `${WORKTREE_*:-…}` silent-fallback pattern (which #413 already fixed in
# workflow-publish.yaml) must be converted to fail-loud `${VAR:?…}` form
# per the Zero-BS contract.
#
# Targets (lines from current main, may shift after fix):
#   - workflow-finalize.yaml         L56, L141
#   - workflow-pr-review.yaml        L207, L303
#   - workflow-refactor-review.yaml  L190 (triple-fallback chain)
#   - workflow-publish.yaml          regression guard (already clean post-#413)
#
# These tests validate:
#   1. No `${WORKTREE_*:-…}` execution-path silent fallback remains in any
#      of the four recipes (informational `:-(unset)` echoes are exempt).
#   2. Each `cd "${WORKTREE_*:?…}"` diagnostic references workflow-worktree.
#   3. The infamous `2>/dev/null || cd "$REPO_PATH"` chain is gone from
#      workflow-refactor-review.yaml.
#   4. Each recipe still has `set -euo pipefail` somewhere in the modified
#      blocks (so :? actually aborts the step).
#   5. Each recipe parses cleanly as YAML.
#   6. Runtime semantics: a stripped `cd "${WORKTREE_SETUP_WORKTREE_PATH:?…}"`
#      with the env var unset exits non-zero and surfaces the diagnostic.
#
# Run: bash tests/issue_412_fail_loud_other_recipes.sh
# Expected before fix: FAIL. Expected after fix: PASS.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPES_DIR="$REPO_ROOT/amplifier-bundle/recipes"

FINALIZE="$RECIPES_DIR/workflow-finalize.yaml"
PR_REVIEW="$RECIPES_DIR/workflow-pr-review.yaml"
REFACTOR_REVIEW="$RECIPES_DIR/workflow-refactor-review.yaml"
PUBLISH="$RECIPES_DIR/workflow-publish.yaml"

ALL_TARGETS=("$FINALIZE" "$PR_REVIEW" "$REFACTOR_REVIEW" "$PUBLISH")

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

echo "=== Issue #412 TDD tests ==="
echo "Target dir: $RECIPES_DIR"
echo

# --- Test 1: all target files exist -----------------------------------------
for f in "${ALL_TARGETS[@]}"; do
    assert "$(basename "$f") exists" "[ -f '$f' ]"
done

# --- Test 2: no `${WORKTREE_*:-…}` silent fallback in any execution path ----
# Informational echo lines that intentionally use `:-(unset)` are exempt.
for f in "${ALL_TARGETS[@]}"; do
    name=$(basename "$f")
    silent_count=$(grep -nE '\$\{WORKTREE_[A-Z_]*:-' "$f" | grep -v '(unset)' | wc -l | tr -d ' ')
    if [ "$silent_count" != "0" ]; then
        echo "      offending lines in $name:"
        grep -nE '\$\{WORKTREE_[A-Z_]*:-' "$f" | grep -v '(unset)' | sed 's/^/        /'
    fi
    assert "no '\${WORKTREE_*:-…}' execution-path fallback in $name (found=$silent_count)" \
        "[ '$silent_count' = '0' ]"
done

# --- Test 3: known-fixed sites use :? fail-loud form -------------------------
finalize_failloud=$(grep -c '\${WORKTREE_SETUP_WORKTREE_PATH:?' "$FINALIZE" || true)
assert "workflow-finalize.yaml has at least 2 \${WORKTREE_SETUP_WORKTREE_PATH:?...} sites (found=$finalize_failloud)" \
    "[ '$finalize_failloud' -ge '2' ]"

pr_review_failloud=$(grep -c '\${WORKTREE_SETUP_WORKTREE_PATH:?' "$PR_REVIEW" || true)
assert "workflow-pr-review.yaml has at least 2 \${WORKTREE_SETUP_WORKTREE_PATH:?...} sites (found=$pr_review_failloud)" \
    "[ '$pr_review_failloud' -ge '2' ]"

refactor_failloud=$(grep -c '\${WORKTREE_SETUP_WORKTREE_PATH:?' "$REFACTOR_REVIEW" || true)
assert "workflow-refactor-review.yaml has at least 1 \${WORKTREE_SETUP_WORKTREE_PATH:?...} site (found=$refactor_failloud)" \
    "[ '$refactor_failloud' -ge '1' ]"

# --- Test 4: triple-fallback chain removed from refactor-review --------------
triple=$(grep -c '2>/dev/null || cd "\$REPO_PATH"' "$REFACTOR_REVIEW" || true)
assert "workflow-refactor-review.yaml: triple-fallback chain removed (found=$triple)" \
    "[ '$triple' = '0' ]"

# --- Test 5: diagnostics reference workflow-worktree -------------------------
for f in "$FINALIZE" "$PR_REVIEW" "$REFACTOR_REVIEW"; do
    name=$(basename "$f")
    # Every :? diagnostic should mention 'worktree' so the reviewer can
    # trace the missing producer.
    bad_diag=$(grep -oE '\$\{WORKTREE_SETUP_WORKTREE_PATH:\?[^}]+\}' "$f" \
               | grep -vci 'worktree' || true)
    assert "$name: every :? diagnostic mentions 'worktree' (bad=$bad_diag)" \
        "[ '$bad_diag' = '0' ]"
done

# --- Test 6: bare `cd $WORKTREE_*` (unquoted) absent -------------------------
for f in "${ALL_TARGETS[@]}"; do
    name=$(basename "$f")
    bare=$(grep -cE 'cd \$WORKTREE_' "$f" || true)
    assert "no bare unquoted 'cd \$WORKTREE_*' in $name (found=$bare)" \
        "[ '$bare' = '0' ]"
done

# --- Test 7: each modified file still has `set -euo pipefail` ----------------
for f in "${ALL_TARGETS[@]}"; do
    name=$(basename "$f")
    has=$(grep -c 'set -euo pipefail' "$f" || true)
    assert "$name retains 'set -euo pipefail' (count=$has)" \
        "[ '$has' -ge '1' ]"
done

# --- Test 8: YAML parses cleanly --------------------------------------------
if command -v python3 >/dev/null 2>&1 && python3 -c "import yaml" 2>/dev/null; then
    for f in "${ALL_TARGETS[@]}"; do
        name=$(basename "$f")
        assert "$name parses with yaml.safe_load" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$f\"))'"
    done
else
    echo "SKIP: python3 + PyYAML not available, skipping YAML parse checks"
fi

# --- Test 9: runtime fail-loud reproduction (semantic guard) -----------------
diag='step-XX requires worktree_setup.worktree_path from workflow-worktree; ensure parent recipe ran worktree-setup and propagated outputs'
out=$(unset WORKTREE_SETUP_WORKTREE_PATH; bash -c 'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?'"$diag"'}"' 2>&1)
rc=$?
assert "fail-loud reproduction exits non-zero" "[ '$rc' != '0' ]"
assert "fail-loud reproduction emits diagnostic mentioning workflow-worktree" \
    "echo \"\$out\" | grep -q 'workflow-worktree'"

# --- Test 10: success path unchanged when var is set ------------------------
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
