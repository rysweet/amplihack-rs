#!/usr/bin/env bash
# test-issue-962-step17a-testing-evidence-gate.sh — regression test for issue
# #962, FLAVOR B: a bookkeeping gate hard-fails and cascades even when the code
# work is DONE and the step is legitimately N/A.
#
# Bug: `step-17a-testing-evidence-gate` in workflow-pr-review.yaml exits 1 the
# moment $LOCAL_TESTING_GATE is empty:
#     if [ -z "$GATE_OUTPUT" ] || [ "$GATE_OUTPUT" = "''" ]; then ... exit 1 ; fi
# On a real run (Simard actor-binding fix) this fired IMMEDIATELY AFTER the
# preceding agent reported "19/19 tests pass" + {"verdict":"WORK_VERIFIED"} and
# the step itself concluded "No work needed" — yet it still exited non-zero and
# cascaded up through workflow-pr-review -> default-workflow -> whole recipe
# FAILED, throwing away a fully-implemented + tested + committed fix.
#
# Fix (this PR): distinguish THREE explicit outcomes (design A4):
#   (1) applicable + evidence present  -> normal PASS (exit 0)
#   (2) N/A (empty gate) OR evidence tool unresolvable -> VISIBLE degrade
#       (WARNING + exit 0) so validated work is preserved (NO-DISCARD invariant)
#   (3) GENUINE code-verification failure explicitly reported in the evidence
#       (e.g. "VERDICT: FAILED") -> stays FATAL (exit non-zero)
#
# Contracts under test:
#   POS:            benign, populated gate output -> exit 0, "PASSED".
#   DEGRADE/N-A:    empty gate ($LOCAL_TESTING_GATE unset/'') -> WARNING + exit 0
#                   (this is the #962 regression — currently exit 1).
#   FAIL-VISIBLE:   evidence containing an explicit failure verdict
#                   ("VERDICT: FAILED") -> exit non-zero (genuine code failure
#                   stays fatal; only bookkeeping-absence is degraded).
#
# This test SHOULD FAIL before the fix (empty gate currently exits 1; an
# explicit failure verdict currently passes because any non-empty value passes).
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-step17a-testing-evidence-gate.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-pr-review.yaml"
STEP_ID="step-17a-testing-evidence-gate"

[[ -f "${RECIPE}" ]] || { echo "HARNESS-ERROR: recipe not found: ${RECIPE}" >&2; exit 2; }

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962 (Flavor B): step-17a-testing-evidence-gate three-outcome contract ==="

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

# run_gate <gate-value> — run the extracted step body with LOCAL_TESTING_GATE set.
run_gate() {
    local gate="$1"
    ( export LOCAL_TESTING_GATE="${gate}"; printf '%s\n' "${BODY}" | bash )
}

# --- POS: populated, benign gate passes ------------------------------------
POS_GATE="Step 13: Local Testing Results
Detected toolchains: cargo.
Strategy: cargo test. Executed: 19/19 tests pass."
set +e
out="$(run_gate "${POS_GATE}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -qi 'PASSED'; then
    pass "POS" "populated benign gate passes (rc=0)"
else
    fail "POS" "populated benign gate did not pass (rc=${rc}): ${out}"
fi

# --- DEGRADE / N-A: empty gate must NOT abort ------------------------------
# The #962 regression: an empty gate on a change that legitimately needs no
# local-testing evidence must degrade visibly, not abort and discard work.
set +e
out="$(run_gate "" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -qi 'WARNING'; then
    pass "DEGRADE-na" "empty gate degrades visibly (WARNING) and exits 0 — no cascade/discard"
else
    fail "DEGRADE-na" "empty gate did not degrade visibly with exit 0 (rc=${rc}): ${out}"
fi

# --- FAIL-VISIBLE: explicit failure verdict stays fatal --------------------
FAIL_GATE="Step 13: Local Testing Results
Executed: cargo test. VERDICT: FAILED — 3 of 19 tests failing."
set +e
out="$(run_gate "${FAIL_GATE}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -ne 0 ]]; then
    pass "FAIL-VISIBLE" "explicit failure verdict in evidence stays fatal (rc=${rc})"
else
    fail "FAIL-VISIBLE" "explicit failure verdict was NOT treated as fatal (rc=0): ${out}"
fi

# --- POS-benign-failword: a benign summary that merely CONTAINS the word ----
# "failed" (e.g. "0 tests failed, 19 passed") must NOT be misread as a failure
# verdict. A loose /tests failed/ match here would re-introduce the exact
# work-discarding abort #962 removes, so it must PASS, not go fatal.
BENIGN_GATE="Step 13: Local Testing Results
Executed: cargo test. Summary: 0 tests failed, 19 passed."
set +e
out="$(run_gate "${BENIGN_GATE}" 2>&1)"; rc=$?
set -e
if [[ ${rc} -eq 0 ]] && printf '%s\n' "${out}" | grep -qi 'PASSED'; then
    pass "POS-benign-failword" "benign 'N tests failed, M passed' summary is not misread as fatal (rc=0)"
else
    fail "POS-benign-failword" "benign summary containing the word 'failed' was wrongly treated as non-pass (rc=${rc}): ${out}"
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962 Flavor B — step-17a distinguishes pass / degrade / fatal without discarding verified work."
exit 0
