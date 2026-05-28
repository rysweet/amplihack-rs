#!/usr/bin/env bash
# test-bug-668-rate-limit-resilience.sh — regression test for issue #668.
#
# Bug: When Copilot hits a rate limit during an agent step, the recipe runner
# treats it as a hard failure. Non-critical steps like step-06c should use
# continue_on_error: true so documentation polish doesn't abort the recipe.
#
# Contracts under test:
#   1. workflow-design.yaml step-06c-documentation-refinement MUST have
#      continue_on_error: true.
#   2. Critical steps (step-07, step-08, step-09) MUST NOT have
#      continue_on_error: true (they must still fail hard).
#   3. SKILL.md MUST document rate-limit resilience in a Known Failure Points
#      section.
#   4. The Known Failure Points section MUST mention continue_on_error as the
#      recommended fix for non-critical steps.
#
# This test SHOULD FAIL before the #668 fix lands.
# It MUST PASS once continue_on_error is added and docs are updated.
#
# Usage: bash amplifier-bundle/recipes/tests/test-bug-668-rate-limit-resilience.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

WORKFLOW_DESIGN="${REPO_ROOT}/amplifier-bundle/recipes/workflow-design.yaml"
SKILL_MD="${REPO_ROOT}/amplifier-bundle/skills/amplihack-expert/SKILL.md"

PASS_COUNT=0
FAIL_COUNT=0

pass() {
    PASS_COUNT=$((PASS_COUNT + 1))
    echo "  PASS[$1]: $2"
}

fail() {
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo "  FAIL[$1]: $2" >&2
}

for f in "${WORKFLOW_DESIGN}" "${SKILL_MD}"; do
    if [[ ! -f "${f}" ]]; then
        echo "HARNESS-ERROR: ${f} not found" >&2
        exit 2
    fi
done

echo "=== Bug #668: Rate-limit resilience ==="

# ---------------------------------------------------------------------------
# Assertion 1: step-06c-documentation-refinement MUST have continue_on_error: true
# ---------------------------------------------------------------------------
# We parse the YAML to find the step and check its continue_on_error field.
# Use a grep-based approach for reliability (avoid YAML parser dependency issues).

# First, find the step-06c block and check for continue_on_error
step_06c_block=$(awk '
    /id:.*step-06c-documentation-refinement/ { found=1; next }
    found && /^[[:space:]]*- id:/ { found=0 }
    found { print }
' "${WORKFLOW_DESIGN}")

if [[ -z "${step_06c_block}" ]]; then
    fail 1 "step-06c-documentation-refinement not found in workflow-design.yaml"
else
    if echo "${step_06c_block}" | grep -qE 'continue_on_error:[[:space:]]*true'; then
        pass 1 "step-06c has continue_on_error: true"
    else
        fail 1 "step-06c does NOT have continue_on_error: true"
    fi
fi

# Also verify at the YAML level that the key appears within a few lines of
# the step ID (sanity check against wrong indentation or comments).
if grep -A5 'step-06c-documentation-refinement' "${WORKFLOW_DESIGN}" \
   | grep -qE 'continue_on_error:[[:space:]]*true'; then
    pass 1b "continue_on_error: true confirmed near step-06c ID in raw YAML"
else
    fail 1b "continue_on_error: true NOT found near step-06c ID in raw YAML"
fi

# ---------------------------------------------------------------------------
# Assertion 2: Critical steps MUST NOT have continue_on_error: true
# ---------------------------------------------------------------------------
CRITICAL_STEPS=(
    "step-07-tdd-write-tests"
    "step-08-implementation"
    "step-09-verification"
)

critical_fail=false
for step_id in "${CRITICAL_STEPS[@]}"; do
    step_block=$(awk -v sid="${step_id}" '
        $0 ~ "id:.*" sid { found=1; next }
        found && /^[[:space:]]*- id:/ { found=0 }
        found { print }
    ' "${WORKFLOW_DESIGN}")

    if echo "${step_block}" | grep -qE 'continue_on_error:[[:space:]]*true'; then
        fail "2:${step_id}" "critical step has continue_on_error: true (DANGEROUS)"
        critical_fail=true
    fi
done

if ! $critical_fail; then
    pass 2 "No critical step (07/08/09) has continue_on_error: true"
fi

# ---------------------------------------------------------------------------
# Assertion 3: SKILL.md has a Known Failure Points section
# ---------------------------------------------------------------------------
if grep -qE '^##[[:space:]]*Known Failure Points' "${SKILL_MD}"; then
    pass 3 "SKILL.md has a 'Known Failure Points' section"
else
    fail 3 "SKILL.md does not have a 'Known Failure Points' section"
fi

# ---------------------------------------------------------------------------
# Assertion 4: Known Failure Points section mentions rate-limit and
# continue_on_error as the recommended fix
# ---------------------------------------------------------------------------
# Extract everything from "Known Failure Points" to end of file
kfp_section=$(awk '/^##[[:space:]]*Known Failure Points/,0 { print }' "${SKILL_MD}")

if [[ -z "${kfp_section}" ]]; then
    fail 4a "Could not extract Known Failure Points section"
    fail 4b "Skipped (depends on 4a)"
else
    if echo "${kfp_section}" | grep -qiE 'rate.?limit'; then
        pass 4a "Known Failure Points mentions rate-limit"
    else
        fail 4a "Known Failure Points does not mention rate-limit"
    fi

    if echo "${kfp_section}" | grep -qE 'continue_on_error'; then
        pass 4b "Known Failure Points documents continue_on_error as fix"
    else
        fail 4b "Known Failure Points does not document continue_on_error"
    fi

    if echo "${kfp_section}" | grep -qiE 'step-06c|documentation.refinement|doc.*polish'; then
        pass 4c "Known Failure Points specifically mentions the non-critical doc step"
    else
        fail 4c "Known Failure Points does not mention the specific non-critical step"
    fi
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Bug #668 — rate-limit resilience configured for non-critical steps."
exit 0
