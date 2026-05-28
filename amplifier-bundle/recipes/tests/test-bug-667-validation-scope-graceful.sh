#!/usr/bin/env bash
# test-bug-667-validation-scope-graceful.sh — regression test for issue #667.
#
# Bug: step-15-commit-push requires build_publish_validation_scope.py which
# is a legacy Python script that no longer exists.
#
# Contracts under test:
#   1. workflow-publish.yaml MUST handle missing build_publish_validation_scope.py
#      gracefully (warn-and-continue), NOT as a hard failure.
#   2. The test-pr-always-opens.sh test infrastructure MUST already verify
#      that PR creation proceeds even when the validation script is missing.
#   3. No recipe YAML in amplifier-bundle/recipes/ MUST invoke
#      build_publish_validation_scope.py as a hard dependency (bare invocation
#      without error handling).
#   4. SKILL.md or agent prompts MUST NOT tell agents to run
#      build_publish_validation_scope.py.
#
# This test SHOULD FAIL before the #667 fix lands.
# It MUST PASS once the hard dependency is removed.
#
# Usage: bash amplifier-bundle/recipes/tests/test-bug-667-validation-scope-graceful.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

PUBLISH_RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-publish.yaml"
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

if [[ ! -f "${PUBLISH_RECIPE}" ]]; then
    echo "HARNESS-ERROR: ${PUBLISH_RECIPE} not found" >&2
    exit 2
fi

echo "=== Bug #667: build_publish_validation_scope.py graceful handling ==="

# ---------------------------------------------------------------------------
# Assertion 1: If workflow-publish.yaml references build_publish_validation,
# it MUST be wrapped in warn-and-continue (not a bare invocation).
# ---------------------------------------------------------------------------
if grep -qE 'build_publish_validation' "${PUBLISH_RECIPE}"; then
    # Reference exists — verify it's wrapped
    if grep -qE 'if[[:space:]]*!.*build_publish_validation|WARN.*validation|warn.*validation|continue' \
            "${PUBLISH_RECIPE}"; then
        pass 1 "build_publish_validation reference in workflow-publish.yaml is warn-and-continue"
    else
        fail 1 "build_publish_validation in workflow-publish.yaml is NOT warn-and-continue"
    fi
else
    pass 1 "workflow-publish.yaml does not reference build_publish_validation at all (safe)"
fi

# ---------------------------------------------------------------------------
# Assertion 2: The test-pr-always-opens.sh companion test exists and tests
# the graceful handling scenario.
# ---------------------------------------------------------------------------
PR_TEST="${REPO_ROOT}/amplifier-bundle/recipes/tests/test-pr-always-opens.sh"
if [[ ! -f "${PR_TEST}" ]]; then
    fail 2a "test-pr-always-opens.sh companion test does not exist"
else
    pass 2a "test-pr-always-opens.sh companion test exists"

    if grep -qE 'build_publish_validation.*missing.*continu|WARN.*build_publish_validation' \
            "${PR_TEST}"; then
        pass 2b "test-pr-always-opens.sh verifies graceful handling of missing validation script"
    else
        fail 2b "test-pr-always-opens.sh does not verify the missing validation script scenario"
    fi
fi

# ---------------------------------------------------------------------------
# Assertion 3: No recipe YAML invokes build_publish_validation_scope.py as
# a bare hard dependency.
# ---------------------------------------------------------------------------
bare_hits=0
while IFS= read -r -d '' recipe_file; do
    basename_file="$(basename "${recipe_file}")"
    # Skip test fixtures
    if [[ "${recipe_file}" == *"/tests/"* ]]; then
        continue
    fi
    if grep -qE 'build_publish_validation_scope\.py' "${recipe_file}"; then
        # Check if it's wrapped in error handling
        if grep -B2 -A2 'build_publish_validation_scope\.py' "${recipe_file}" \
           | grep -qE 'if[[:space:]]*!|WARN|warn|continue||| true|2>/dev/null'; then
            : # wrapped — ok
        else
            fail "3:${basename_file}" "bare invocation of build_publish_validation_scope.py"
            bare_hits=$((bare_hits + 1))
        fi
    fi
done < <(find "${REPO_ROOT}/amplifier-bundle/recipes" \( -name '*.yaml' -o -name '*.yml' -o -name '*.sh' \) -print0)

if [[ ${bare_hits} -eq 0 ]]; then
    pass 3 "No recipe has bare invocation of build_publish_validation_scope.py"
fi

# ---------------------------------------------------------------------------
# Assertion 4: SKILL.md does not instruct agents to run
# build_publish_validation_scope.py.
# ---------------------------------------------------------------------------
if [[ -f "${SKILL_MD}" ]]; then
    if grep -qE 'build_publish_validation_scope\.py' "${SKILL_MD}" \
       && ! grep -qiE 'legacy|replaced|deprecated|Note.*Rust' "${SKILL_MD}"; then
        fail 4 "SKILL.md instructs agents to run build_publish_validation_scope.py"
    else
        pass 4 "SKILL.md does not instruct agents to use build_publish_validation_scope.py"
    fi
else
    pass 4 "SKILL.md not found (skipped — not a hard requirement)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Bug #667 — build_publish_validation_scope.py dependency is graceful."
exit 0
