#!/usr/bin/env bash
# test-issue-962-implementation-evidence-fallback.sh — regression test for issue
# #962 (root cause of #964), FLAVOR A: helper-path resolution too narrow.
#
# Bug: the `implementation-terminal-evidence` step in workflow-tdd.yaml resolves
# the evidence helper with only TWO candidate paths and then HARD-FAILS `exit 2`
# if neither exists:
#     HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
#     [ -f "$HELPER" ] || HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_implementation_evidence.sh"
#     [ -f "$HELPER" ] || { echo "ERROR: ... not found: $HELPER" >&2; exit 2; }
#     bash "$HELPER"
# When cwd/repo_path is a downstream PRODUCT repo whose amplifier-bundle/tools/
# is gitignored (a fresh `git worktree add` yields an EMPTY tools/ dir),
# resolution never reaches the real install root
# (~/.amplihack/amplifier-bundle/tools/...) and the step aborts AFTER
# implementation + verification already succeeded — killing smart-orchestrator
# and DISCARDING the branch.
#
# Fix (this PR): apply the canonical 5-tier resolution ladder mirroring the
# landed #955 git-identity / RUNTIME_ARTIFACT_HELPER siblings, and — because
# this is a BOOKKEEPING/EVIDENCE gate, not a code-verification gate — DEGRADE
# VISIBLY (WARNING + exit 0) when the helper is unresolvable, so a bookkeeping
# step can NEVER discard validated implementation work (task invariant #3).
#
# Contracts under test:
#   POS-amplihack: helper installed ONLY at ${HOME}/.amplihack resolves + runs
#        (exit 0, helper output surfaces) when AMPLIHACK_HOME/REPO_PATH/cwd all
#        lack the bundle.
#   POS-copilot:   same via the ${HOME}/.copilot tier.
#   DEGRADE/NO-DISCARD: helper truly absent EVERYWHERE MUST NOT `exit 2` and
#        abort — it MUST print a visible WARNING and exit 0 so the committed,
#        verified work is preserved.
#   STATIC-tiers: the step body carries the canonical $(pwd), ~/.copilot and
#        ~/.amplihack tiers for workflow_implementation_evidence.sh.
#   STATIC-nodiscard: the step body no longer contains a bare terminal `exit 2`
#        for the unresolvable-helper case (it degrades visibly instead).
#
# This test SHOULD FAIL before the #962 fix lands and MUST PASS after.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-implementation-evidence-fallback.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
RECIPE="${RECIPES}/workflow-tdd.yaml"
STEP_ID="implementation-terminal-evidence"
HELPER_FILE="workflow_implementation_evidence.sh"

[[ -f "${RECIPE}" ]] || { echo "HARNESS-ERROR: recipe not found: ${RECIPE}" >&2; exit 2; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962 (Flavor A): implementation-terminal-evidence resolution + no-discard ==="

# extract_step_command <recipe> <step-id> — print the dedented bash body of the
# named step's `command: |` block.
extract_step_command() {
    local recipe="$1" step="$2"
    awk -v step="$step" '
        index($0, "id: \"" step "\"") { instep=1 }
        instep && $0 ~ /command: \|/ { incmd=1; next }
        incmd {
            if ($0 ~ /^    [a-zA-Z_]+:/ || $0 ~ /^  - id:/) { exit }
            sub(/^      /, "")
            print
        }
    ' "${recipe}"
}

BODY="$(extract_step_command "${RECIPE}" "${STEP_ID}")"
if [[ -z "${BODY}" ]]; then
    echo "HARNESS-ERROR: could not extract command body for step ${STEP_ID}" >&2
    exit 2
fi

# ---------------------------------------------------------------------------
# Isolated fixture: NEITHER AMPLIHACK_HOME NOR REPO_PATH NOR the repo top-level
# NOR cwd contains amplifier-bundle/ (the downstream gitignored-worktree case).
# ---------------------------------------------------------------------------
TMP_ROOT="$(mktemp -d)"
cleanup() { rm -rf "${TMP_ROOT}"; }
trap cleanup EXIT

TMP_REPO="${TMP_ROOT}/repo"                 # git top-level + cwd, NO bundle
TMP_AH="${TMP_ROOT}/downstream"             # AMPLIHACK_HOME, NO bundle
HOME_AMPLIHACK="${TMP_ROOT}/home_amplihack" # ~/.amplihack/.../<helper>
HOME_COPILOT="${TMP_ROOT}/home_copilot"     # ~/.copilot/.../<helper>
HOME_EMPTY="${TMP_ROOT}/home_empty"         # nothing installed anywhere

mkdir -p "${TMP_REPO}" "${TMP_AH}" "${HOME_EMPTY}"
git -C "${TMP_REPO}" init -q

install_stub() {
    local dir="$1"
    mkdir -p "${dir}"
    cat > "${dir}/${HELPER_FILE}" <<'STUB'
#!/usr/bin/env bash
# Test stub standing in for the real implementation-evidence helper.
echo "STUB_IMPL_EVIDENCE_RAN"
STUB
}
install_stub "${HOME_AMPLIHACK}/.amplihack/amplifier-bundle/tools"
install_stub "${HOME_COPILOT}/.copilot/amplifier-bundle/tools"

# run_body <home> — execute the extracted step body inside the fixture with the
# given HOME. Returns the body's exit code; stdout+stderr captured by caller.
run_body() {
    local home="$1"
    (
        cd "${TMP_REPO}" || exit 3
        export HOME="${home}"
        export AMPLIHACK_HOME="${TMP_AH}"
        unset REPO_PATH
        printf '%s\n' "${BODY}" | bash
    )
}

# --- POS-amplihack ---------------------------------------------------------
set +e
out="$(run_body "${HOME_AMPLIHACK}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -q 'STUB_IMPL_EVIDENCE_RAN'; then
    pass "POS-amplihack" "resolves via \${HOME}/.amplihack tier and runs the helper (rc=0)"
else
    fail "POS-amplihack" "did not resolve/run via \${HOME}/.amplihack (rc=${rc}): ${out}"
fi

# --- POS-copilot -----------------------------------------------------------
set +e
out="$(run_body "${HOME_COPILOT}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -q 'STUB_IMPL_EVIDENCE_RAN'; then
    pass "POS-copilot" "resolves via \${HOME}/.copilot tier and runs the helper (rc=0)"
else
    fail "POS-copilot" "did not resolve/run via \${HOME}/.copilot (rc=${rc}): ${out}"
fi

# --- DEGRADE / NO-DISCARD --------------------------------------------------
# Helper absent everywhere: a bookkeeping/evidence step MUST NOT abort the
# recipe (no `exit 2`). It MUST degrade VISIBLY (WARNING) and exit 0 so the
# committed, verified implementation is preserved.
set +e
out="$(run_body "${HOME_EMPTY}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -qi 'WARNING'; then
    pass "DEGRADE-nodiscard" "unresolvable helper degrades visibly (WARNING) and exits 0 — work preserved"
else
    fail "DEGRADE-nodiscard" "unresolvable helper did not degrade visibly with exit 0 (rc=${rc}): ${out}"
fi

# --- STATIC: canonical tiers present ---------------------------------------
for tier in \
    "\$(pwd)/amplifier-bundle/tools/${HELPER_FILE}" \
    "\${HOME:-/root}/.copilot/amplifier-bundle/tools/${HELPER_FILE}" \
    "\${HOME:-/root}/.amplihack/amplifier-bundle/tools/${HELPER_FILE}"; do
    if printf '%s\n' "${BODY}" | grep -qF "${tier}"; then
        pass "STATIC-tier" "carries tier ${tier}"
    else
        fail "STATIC-tier" "missing canonical tier ${tier}"
    fi
done

# --- STATIC: no bare terminal exit 2 (degrade, not abort) ------------------
if printf '%s\n' "${BODY}" | grep -qE 'exit[[:space:]]+2'; then
    fail "STATIC-nodiscard" "step still contains a bare terminal 'exit 2' for the evidence gate (must degrade)"
else
    pass "STATIC-nodiscard" "no bare terminal 'exit 2' — evidence gate degrades instead of aborting"
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962 Flavor A — implementation-terminal-evidence resolves via the full ladder and never discards work."
exit 0
