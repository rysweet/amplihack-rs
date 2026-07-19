#!/usr/bin/env bash
# test-issue-962-skill-mirror-parity.sh — regression test for issue #962:
# reconcile the default-workflow SKILL.md mirrors and the reference doc with the
# recipe reality, and keep them byte-identical (prior #849/#852 churn was caused
# by mirror DRIFT between the two SKILL copies).
#
# Contracts under test:
#   PARITY:    the two SKILL.md mirrors are byte-identical.
#              - amplifier-bundle/skills/default-workflow/SKILL.md
#              - docs/claude/skills/default-workflow/SKILL.md
#   POLICY:    BOTH mirrors document the fail-visible-vs-degraded-mode gate
#              policy — they must mention the "no-discard" invariant, the
#              "degrade" behavior, and the "fail-visible" terminal action.
#   DOC:       docs/reference/workflow-implementation-evidence.md exists and is
#              wired into the mkdocs nav.
#
# This test SHOULD FAIL before the fix (mirrors do not yet document the policy)
# and MUST PASS after.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-skill-mirror-parity.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

SKILL_A="${REPO_ROOT}/amplifier-bundle/skills/default-workflow/SKILL.md"
SKILL_B="${REPO_ROOT}/docs/claude/skills/default-workflow/SKILL.md"
DOC="${REPO_ROOT}/docs/reference/workflow-implementation-evidence.md"
MKDOCS="${REPO_ROOT}/mkdocs.yml"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962: SKILL mirror parity + gate-policy documentation ==="

for f in "${SKILL_A}" "${SKILL_B}"; do
    [[ -f "${f}" ]] || { echo "HARNESS-ERROR: SKILL mirror not found: ${f}" >&2; exit 2; }
done

# --- PARITY ----------------------------------------------------------------
if diff -q "${SKILL_A}" "${SKILL_B}" >/dev/null 2>&1; then
    pass "PARITY" "both SKILL.md mirrors are byte-identical"
else
    fail "PARITY" "SKILL.md mirrors have drifted apart (diff not empty)"
fi

# --- POLICY: both mirrors document the gate policy -------------------------
# Case-insensitive keyword presence in BOTH mirrors.
check_phrase() {
    local phrase="$1" label="$2"
    if grep -qiF "${phrase}" "${SKILL_A}" && grep -qiF "${phrase}" "${SKILL_B}"; then
        pass "POLICY:${label}" "both mirrors mention '${phrase}'"
    else
        fail "POLICY:${label}" "one or both mirrors missing '${phrase}'"
    fi
}
check_phrase "no-discard" "no-discard"
check_phrase "degrade"    "degrade"
check_phrase "fail-visible" "fail-visible"

# --- DOC: reference doc exists and is wired into mkdocs nav -----------------
if [[ -f "${DOC}" ]]; then
    pass "DOC:exists" "reference doc present"
else
    fail "DOC:exists" "docs/reference/workflow-implementation-evidence.md missing"
fi

if [[ -f "${MKDOCS}" ]] && grep -qF "workflow-implementation-evidence.md" "${MKDOCS}"; then
    pass "DOC:nav" "reference doc wired into mkdocs nav"
else
    fail "DOC:nav" "reference doc not referenced in mkdocs.yml"
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962 — SKILL mirrors reconciled and documenting the gate policy."
exit 0
