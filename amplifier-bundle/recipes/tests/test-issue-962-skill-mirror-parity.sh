#!/usr/bin/env bash
# test-issue-962-skill-mirror-parity.sh — regression test for issue #962:
# keep the default-workflow SKILL.md mirrors byte-identical (prior #849/#852
# churn was caused by mirror DRIFT between the two SKILL copies).
#
# Contract under test:
#   PARITY:    the two SKILL.md mirrors are byte-identical.
#              - amplifier-bundle/skills/default-workflow/SKILL.md
#              - docs/claude/skills/default-workflow/SKILL.md
#
# The POLICY and DOC parts, which pinned words in the mirrors and a doc's
# place in mkdocs.yml, were removed under #3129: tests check values and
# behaviour, never wording (Ryan, 23 Nov 2026).
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-skill-mirror-parity.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

SKILL_A="${REPO_ROOT}/amplifier-bundle/skills/default-workflow/SKILL.md"
SKILL_B="${REPO_ROOT}/docs/claude/skills/default-workflow/SKILL.md"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962: SKILL mirror parity ==="

for f in "${SKILL_A}" "${SKILL_B}"; do
    [[ -f "${f}" ]] || { echo "HARNESS-ERROR: SKILL mirror not found: ${f}" >&2; exit 2; }
done

# --- PARITY ----------------------------------------------------------------
if diff -q "${SKILL_A}" "${SKILL_B}" >/dev/null 2>&1; then
    pass "PARITY" "both SKILL.md mirrors are byte-identical"
else
    fail "PARITY" "SKILL.md mirrors have drifted apart (diff not empty)"
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962: the SKILL mirrors are byte-identical."
exit 0
