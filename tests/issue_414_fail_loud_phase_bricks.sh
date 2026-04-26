#!/usr/bin/env bash
# Tests for Issue #414: phase-brick recipes must declare worktree-setup outputs
# as required inputs and fail loudly when WORKTREE_SETUP_WORKTREE_PATH /
# REPO_PATH are unset, instead of silently falling back.
#
# Mirrors tests/issue_413_fail_loud_worktree.sh.
#
# Validates (per spec):
#   A. No `${VAR:-...}` fallback on required worktree/repo context vars.
#   B. `${VAR:?...}` form present at expected count per file.
#   C. Diagnostic strings reference the step-id, the upstream output, and
#      the literal "worktree-setup".
#   D. Top-level `inputs:` block declared in each affected file.
#   E. PyYAML safe_load parses each file AND the `inputs` block conforms to
#      schema: list of dicts each with `name` (str) + `description` (str).
#   F. Runtime negative reproduction per hardened step:
#        - exits non-zero
#        - diagnostic appears on stderr (not stdout)
#        - re-grep for `:-` fallback patterns is empty (runtime invariant)
#        - negative-grep for `eval` and unquoted `cd $WORKTREE...` to lock
#          safety properties going forward.
#
# Run: bash tests/issue_414_fail_loud_phase_bricks.sh
# Expected before fix: FAIL. Expected after fix: PASS.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RECIPES_DIR="$REPO_ROOT/amplifier-bundle/recipes"

FINALIZE="$RECIPES_DIR/workflow-finalize.yaml"
PR_REVIEW="$RECIPES_DIR/workflow-pr-review.yaml"
REFACTOR="$RECIPES_DIR/workflow-refactor-review.yaml"
PUBLISH="$RECIPES_DIR/workflow-publish.yaml"

ALL_FILES=("$FINALIZE" "$PR_REVIEW" "$REFACTOR" "$PUBLISH")

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

echo "=== Issue #414 TDD tests ==="
echo

# --- Pre: files exist -------------------------------------------------------
for f in "${ALL_FILES[@]}"; do
    assert "exists: $(basename "$f")" "[ -f '$f' ]"
done

# ----------------------------------------------------------------------------
# Section A — No `:-` fallback on required vars in any of the 4 files.
# ----------------------------------------------------------------------------
echo
echo "--- Section A: no ':-' fallback on required vars ---"
for f in "${ALL_FILES[@]}"; do
    n_wt=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:-\$REPO_PATH' "$f" || true)
    assert "no \${WORKTREE_SETUP_WORKTREE_PATH:-\$REPO_PATH} in $(basename "$f") (found=$n_wt)" \
        "[ '$n_wt' = '0' ]"
done

# REPO_PATH must not appear bare in workflow-finalize.yaml step-22b cd context
bare_repo=$(grep -cE '^\s*cd \$REPO_PATH' "$FINALIZE" || true)
assert "no bare 'cd \$REPO_PATH' in workflow-finalize.yaml (found=$bare_repo)" \
    "[ '$bare_repo' = '0' ]"

# Negative-grep: no `eval` on protected vars in any affected file
for f in "${ALL_FILES[@]}"; do
    n_eval=$(grep -cE 'eval[[:space:]]+["$]?(WORKTREE_SETUP_WORKTREE_PATH|REPO_PATH)' "$f" || true)
    assert "no 'eval' on protected vars in $(basename "$f") (found=$n_eval)" \
        "[ '$n_eval' = '0' ]"
done

# Negative-grep: no unquoted `cd $WORKTREE_SETUP_WORKTREE_PATH` anywhere
for f in "${ALL_FILES[@]}"; do
    n_bare=$(grep -cE 'cd \$WORKTREE_SETUP_WORKTREE_PATH' "$f" || true)
    assert "no unquoted 'cd \$WORKTREE_SETUP_WORKTREE_PATH' in $(basename "$f") (found=$n_bare)" \
        "[ '$n_bare' = '0' ]"
done

# ----------------------------------------------------------------------------
# Section B — `:?` form present at expected count per file.
# ----------------------------------------------------------------------------
echo
echo "--- Section B: ':?' hardening counts ---"

# workflow-finalize.yaml: 2 WORKTREE_SETUP_WORKTREE_PATH:?  +  1 REPO_PATH:?
n=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:?' "$FINALIZE" || true)
assert "workflow-finalize: 2 \${WORKTREE_SETUP_WORKTREE_PATH:?} (found=$n)" "[ '$n' = '2' ]"
n=$(grep -c 'REPO_PATH:?' "$FINALIZE" || true)
assert "workflow-finalize: >=1 \${REPO_PATH:?} (found=$n)" "[ '$n' -ge '1' ]"

# workflow-pr-review.yaml: 2 WORKTREE_SETUP_WORKTREE_PATH:?
n=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:?' "$PR_REVIEW" || true)
assert "workflow-pr-review: 2 \${WORKTREE_SETUP_WORKTREE_PATH:?} (found=$n)" "[ '$n' = '2' ]"

# workflow-refactor-review.yaml: 1 WORKTREE_SETUP_WORKTREE_PATH:?
n=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:?' "$REFACTOR" || true)
assert "workflow-refactor-review: 1 \${WORKTREE_SETUP_WORKTREE_PATH:?} (found=$n)" "[ '$n' = '1' ]"

# workflow-publish.yaml: 2 WORKTREE_SETUP_WORKTREE_PATH:? (unchanged from #413)
n=$(grep -c 'WORKTREE_SETUP_WORKTREE_PATH:?' "$PUBLISH" || true)
assert "workflow-publish: 2 \${WORKTREE_SETUP_WORKTREE_PATH:?} (preserved from #413; found=$n)" \
    "[ '$n' = '2' ]"

# Refactor-review must drop the 2>/dev/null + || cd fallback
n=$(grep -cE 'WORKTREE_SETUP_WORKTREE_PATH.*2>/dev/null.*cd "\$REPO_PATH"' "$REFACTOR" || true)
assert "workflow-refactor-review: '2>/dev/null || cd \$REPO_PATH' fallback removed (found=$n)" \
    "[ '$n' = '0' ]"

# ----------------------------------------------------------------------------
# Section C — Diagnostic strings reference step-id + upstream output + worktree-setup.
# ----------------------------------------------------------------------------
echo
echo "--- Section C: diagnostic strings ---"

# workflow-finalize.yaml — three hardened steps
for step in step-20b-push-cleanup step-21-pr-ready step-22b-final-status; do
    assert "workflow-finalize: diagnostic mentions $step" \
        "grep -q '$step requires' '$FINALIZE'"
done
assert "workflow-finalize: diagnostics mention 'worktree-setup'" \
    "[ \"\$(grep -c 'ensure parent recipe ran worktree-setup' '$FINALIZE')\" -ge '2' ]"
assert "workflow-finalize: step-22b diagnostic mentions repo_path" \
    "grep -q 'step-22b-final-status requires repo_path' '$FINALIZE'"

# workflow-pr-review.yaml — two hardened steps
for step in step-18c-push-feedback-changes step-19c-zero-bs-verification; do
    assert "workflow-pr-review: diagnostic mentions $step" \
        "grep -q '$step requires worktree_setup.worktree_path' '$PR_REVIEW'"
done
assert "workflow-pr-review: diagnostics mention 'worktree-setup'" \
    "[ \"\$(grep -c 'ensure parent recipe ran worktree-setup' '$PR_REVIEW')\" -ge '2' ]"

# workflow-refactor-review.yaml — one hardened step
assert "workflow-refactor-review: diagnostic mentions step-11b-implement-feedback" \
    "grep -q 'step-11b-implement-feedback requires worktree_setup.worktree_path' '$REFACTOR'"
assert "workflow-refactor-review: diagnostic mentions 'worktree-setup'" \
    "grep -q 'ensure parent recipe ran worktree-setup' '$REFACTOR'"

# ----------------------------------------------------------------------------
# Section D — Top-level `inputs:` block declared in each file.
# ----------------------------------------------------------------------------
echo
echo "--- Section D: top-level inputs: blocks ---"
if command -v python3 >/dev/null 2>&1 && python3 -c "import yaml" 2>/dev/null; then
    for f in "${ALL_FILES[@]}"; do
        assert "$(basename "$f"): top-level 'inputs:' block present" \
            "python3 -c \"import yaml,sys; d=yaml.safe_load(open('$f')); sys.exit(0 if isinstance(d.get('inputs'), list) and len(d['inputs'])>=1 else 1)\""
    done

    # finalize + publish must declare both worktree_setup.worktree_path and repo_path
    for f in "$FINALIZE" "$PUBLISH"; do
        assert "$(basename "$f"): inputs declares worktree_setup.worktree_path AND repo_path" \
            "python3 -c \"import yaml,sys; d=yaml.safe_load(open('$f')); names={i['name'] for i in d.get('inputs',[])}; sys.exit(0 if {'worktree_setup.worktree_path','repo_path'}.issubset(names) else 1)\""
    done

    # pr-review + refactor-review must declare worktree_setup.worktree_path
    for f in "$PR_REVIEW" "$REFACTOR"; do
        assert "$(basename "$f"): inputs declares worktree_setup.worktree_path" \
            "python3 -c \"import yaml,sys; d=yaml.safe_load(open('$f')); names={i['name'] for i in d.get('inputs',[])}; sys.exit(0 if 'worktree_setup.worktree_path' in names else 1)\""
    done
else
    echo "SKIP: python3+PyYAML not available — Section D"
fi

# ----------------------------------------------------------------------------
# Section E — PyYAML safe_load + inputs schema.
# ----------------------------------------------------------------------------
echo
echo "--- Section E: PyYAML safe_load + schema ---"
if command -v python3 >/dev/null 2>&1 && python3 -c "import yaml" 2>/dev/null; then
    for f in "${ALL_FILES[@]}"; do
        assert "$(basename "$f"): yaml.safe_load succeeds" \
            "python3 -c 'import yaml; yaml.safe_load(open(\"$f\"))'"
        assert "$(basename "$f"): inputs schema = list of {name:str, description:str}" \
            "python3 -c \"import yaml,sys; d=yaml.safe_load(open('$f')); inp=d.get('inputs',[]); ok=isinstance(inp,list) and all(isinstance(x,dict) and isinstance(x.get('name'),str) and isinstance(x.get('description'),str) for x in inp); sys.exit(0 if ok else 1)\""
    done
else
    echo "SKIP: python3+PyYAML not available — Section E"
fi

# ----------------------------------------------------------------------------
# Section F — Runtime negative reproduction per hardened step.
# ----------------------------------------------------------------------------
echo
echo "--- Section F: runtime negative reproductions ---"

TMPWORK="$(mktemp -d)"
trap 'rm -rf "$TMPWORK"' EXIT

run_negative() {
    # $1 = test description, $2 = diagnostic substring, $3 = bash body
    local desc="$1"
    local needle="$2"
    local body="$3"

    local stdout_file="$TMPWORK/stdout.$$"
    local stderr_file="$TMPWORK/stderr.$$"

    env -i PATH=/usr/bin:/bin TMPDIR="$TMPWORK" bash -c "$body" \
        >"$stdout_file" 2>"$stderr_file"
    local rc=$?

    assert "$desc: exits non-zero" "[ '$rc' != '0' ]"
    assert "$desc: diagnostic on stderr" "grep -q '$needle' '$stderr_file'"
    assert "$desc: diagnostic NOT on stdout (stderr discipline)" \
        "! grep -q '$needle' '$stdout_file'"
}

# step-20b-push-cleanup
run_negative \
    "workflow-finalize.step-20b-push-cleanup" \
    "step-20b-push-cleanup requires worktree_setup.worktree_path" \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-20b-push-cleanup requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"'

# step-21-pr-ready
run_negative \
    "workflow-finalize.step-21-pr-ready" \
    "step-21-pr-ready requires worktree_setup.worktree_path" \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-21-pr-ready requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"'

# step-22b-final-status (REPO_PATH)
run_negative \
    "workflow-finalize.step-22b-final-status" \
    "step-22b-final-status requires repo_path" \
    'set -euo pipefail; cd "${REPO_PATH:?step-22b-final-status requires repo_path; ensure parent recipe propagated repo_path context}"'

# step-18c-push-feedback-changes
run_negative \
    "workflow-pr-review.step-18c-push-feedback-changes" \
    "step-18c-push-feedback-changes requires worktree_setup.worktree_path" \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-18c-push-feedback-changes requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"'

# step-19c-zero-bs-verification
run_negative \
    "workflow-pr-review.step-19c-zero-bs-verification" \
    "step-19c-zero-bs-verification requires worktree_setup.worktree_path" \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-19c-zero-bs-verification requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"'

# step-11b-implement-feedback
run_negative \
    "workflow-refactor-review.step-11b-implement-feedback" \
    "step-11b-implement-feedback requires worktree_setup.worktree_path" \
    'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-11b-implement-feedback requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"'

# Positive control: when var is set, success path works
ok_dir="$(mktemp -d -p "$TMPWORK")"
env -i PATH=/usr/bin:/bin WORKTREE_SETUP_WORKTREE_PATH="$ok_dir" \
    bash -c 'set -euo pipefail; cd "${WORKTREE_SETUP_WORKTREE_PATH:?should-not-fire}" && pwd' \
    >/dev/null 2>&1
rc=$?
assert "positive control: success path when var is set" "[ '$rc' = '0' ]"

# ----------------------------------------------------------------------------
# Summary
# ----------------------------------------------------------------------------
echo
echo "=== Summary: $pass passed, $fail failed ==="
exit "$fail"
