#!/usr/bin/env bash
# test-issue-962-fatal-allowlist-preserved.sh — regression test for issue #962:
# the no-discard sweep must NOT over-degrade. Genuine CODE-verification gates
# stay FATAL; only bookkeeping/evidence gates are converted to visible-degrade.
#
# FATAL allowlist (MUST remain fatal — treated as correctness/provenance
# controls, not bookkeeping):
#   - workflow-tdd.yaml      step-08c-enforce-verdict  (HOLLOW_SUCCESS -> exit 1)
#   - workflow-pr-review.yaml step-19c-zero-bs-verification (-> exit 1)
#   - git-identity.sh resolution sites (7 single-line `exit 2`, per #955)
#
# CONVERTED to visible-degrade (MUST NOT abort/discard work):
#   - workflow-tdd.yaml      implementation-terminal-evidence  (no bare exit 2)
#   - workflow-pr-review.yaml step-17a-testing-evidence-gate    (empty -> WARN)
#
# CI registration: the new #962 regression tests MUST be wired into
# .github/workflows/ci.yml (an unrun test is zero-value, design A6).
#
# This test SHOULD FAIL before the fix (the two converted gates still abort,
# and CI does not yet run the new tests) and MUST PASS after.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-fatal-allowlist-preserved.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
CI_YML="${REPO_ROOT}/.github/workflows/ci.yml"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962: FATAL allowlist preserved; bookkeeping gates degraded; CI wired ==="

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

# ---------------------------------------------------------------------------
# FATAL allowlist — these gates MUST keep their fatal exit.
# ---------------------------------------------------------------------------
ENFORCE="$(extract_step_command "${RECIPES}/workflow-tdd.yaml" "step-08c-enforce-verdict")"
if printf '%s\n' "${ENFORCE}" | grep -qF 'HOLLOW_SUCCESS' \
   && printf '%s\n' "${ENFORCE}" | grep -qE 'exit[[:space:]]+1'; then
    pass "FATAL:step-08c" "enforce-verdict keeps HOLLOW_SUCCESS -> exit 1"
else
    fail "FATAL:step-08c" "enforce-verdict lost its fatal HOLLOW_SUCCESS exit 1"
fi

ZEROBS="$(extract_step_command "${RECIPES}/workflow-pr-review.yaml" "step-19c-zero-bs-verification")"
if printf '%s\n' "${ZEROBS}" | grep -qE 'exit[[:space:]]+1'; then
    pass "FATAL:step-19c" "zero-bs-verification keeps a fatal exit 1"
else
    fail "FATAL:step-19c" "zero-bs-verification lost its fatal exit 1"
fi

# git-identity: exactly 7 single-line fail-visible `exit 2` sites (per #955).
GITID_RECIPES=(workflow-finalize.yaml workflow-refactor-review.yaml workflow-pr-review.yaml \
               workflow-tdd.yaml workflow-publish.yaml consensus-publish.yaml consensus-pr-feedback.yaml)
total_gitid_exit2=0
for f in "${GITID_RECIPES[@]}"; do
    [[ -f "${RECIPES}/${f}" ]] || { echo "HARNESS-ERROR: missing ${f}" >&2; exit 2; }
    n="$(grep -E 'git identity helper not found' "${RECIPES}/${f}" 2>/dev/null | grep -cE 'exit[[:space:]]+2' || true)"
    total_gitid_exit2=$((total_gitid_exit2 + n))
done
if [[ "${total_gitid_exit2}" -eq 7 ]]; then
    pass "FATAL:git-identity" "7 git-identity fail-visible exit-2 sites retained"
else
    fail "FATAL:git-identity" "found ${total_gitid_exit2} git-identity exit-2 sites (expected 7)"
fi

# ---------------------------------------------------------------------------
# CONVERTED gates — MUST degrade, not abort.
# ---------------------------------------------------------------------------
IMPL="$(extract_step_command "${RECIPES}/workflow-tdd.yaml" "implementation-terminal-evidence")"
if printf '%s\n' "${IMPL}" | grep -qE 'exit[[:space:]]+2'; then
    fail "DEGRADE:impl-evidence" "implementation-terminal-evidence still has a fatal exit 2"
else
    pass "DEGRADE:impl-evidence" "implementation-terminal-evidence no longer aborts (no exit 2)"
fi
if printf '%s\n' "${IMPL}" | grep -qi 'WARNING'; then
    pass "DEGRADE:impl-evidence-warn" "implementation-terminal-evidence degrades visibly (WARNING)"
else
    fail "DEGRADE:impl-evidence-warn" "implementation-terminal-evidence lacks a visible WARNING degrade"
fi

STEP17A="$(extract_step_command "${RECIPES}/workflow-pr-review.yaml" "step-17a-testing-evidence-gate")"
if printf '%s\n' "${STEP17A}" | grep -qi 'WARNING'; then
    pass "DEGRADE:step-17a" "step-17a degrades visibly (WARNING) on the N/A path"
else
    fail "DEGRADE:step-17a" "step-17a lacks a visible WARNING degrade for the N/A/empty case"
fi

# ---------------------------------------------------------------------------
# CI registration — the new #962 tests must actually run in CI.
# ---------------------------------------------------------------------------
[[ -f "${CI_YML}" ]] || { echo "HARNESS-ERROR: ci.yml not found: ${CI_YML}" >&2; exit 2; }
NEW_TESTS=(
    test-issue-962-implementation-evidence-fallback.sh
    test-issue-962-step17a-testing-evidence-gate.sh
    test-issue-962-runtime-artifact-ladders.sh
    test-issue-962-fatal-allowlist-preserved.sh
    test-issue-962-skill-mirror-parity.sh
)
for t in "${NEW_TESTS[@]}"; do
    if grep -qF "${t}" "${CI_YML}"; then
        pass "CI-registered:${t}" "ci.yml runs ${t}"
    else
        fail "CI-registered:${t}" "ci.yml does not run ${t}"
    fi
done

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962 — FATAL allowlist intact, bookkeeping gates degraded, CI wired."
exit 0
