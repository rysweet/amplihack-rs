#!/usr/bin/env bash
# test-bug-666-stale-python-refs.sh — regression test for issue #666.
#
# Bug: smart-orchestrator preflight references orch_helper.py and session_tree.py
# which were replaced by native Rust equivalents.
#
# Contracts under test:
#   1. SKILL.md MUST NOT instruct agents to use orch_helper.py — it MUST
#      reference `amplihack orch helper` (native Rust).
#   2. SKILL.md MUST NOT instruct agents to use session_tree.py — it MUST
#      reference the native recursion guard (`AMPLIHACK_MAX_DEPTH` env var).
#   3. reference.md MUST NOT list ci_status.py or github_issue.py as active
#      tools — these are replaced by `gh` CLI / native Rust.
#   4. dependency-resolver README MUST NOT tell agents to invoke ci_status.py.
#   5. runtime-topology README MUST reference native recursion guard, not
#      session_tree.py.
#   6. dev-orchestrator tutorial MUST reference `amplihack orch helper`, not
#      the legacy orch_helper.py as a current tool.
#   7. Historical docs (tutorials, references, audits) that mention legacy Python
#      files MUST include a deprecation note indicating Rust replacement.
#
# This test SHOULD FAIL before the #666 fix lands.
# It MUST PASS once all stale references are updated.
#
# Usage: bash amplifier-bundle/recipes/tests/test-bug-666-stale-python-refs.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"

SKILL_MD="${REPO_ROOT}/amplifier-bundle/skills/amplihack-expert/SKILL.md"
REFERENCE_MD="${REPO_ROOT}/amplifier-bundle/skills/amplihack-expert/reference.md"
DEP_RESOLVER_README="${REPO_ROOT}/amplifier-bundle/skills/dependency-resolver/README.md"
RUNTIME_TOPO_README="${REPO_ROOT}/docs/atlas/runtime-topology/README.md"
DEV_ORCH_TUTORIAL="${REPO_ROOT}/docs/tutorials/dev-orchestrator-tutorial.md"

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

# Precondition checks
for f in "${SKILL_MD}" "${REFERENCE_MD}" "${DEP_RESOLVER_README}" \
         "${RUNTIME_TOPO_README}" "${DEV_ORCH_TUTORIAL}"; do
    if [[ ! -f "${f}" ]]; then
        echo "HARNESS-ERROR: ${f} not found" >&2
        exit 2
    fi
done

echo "=== Bug #666: Stale Python tool references ==="

# ---------------------------------------------------------------------------
# Assertion 1: SKILL.md references native Rust orch helper, not orch_helper.py
# ---------------------------------------------------------------------------
if grep -qE 'amplihack orch helper' "${SKILL_MD}"; then
    pass 1a "SKILL.md references 'amplihack orch helper' (native Rust)"
else
    fail 1a "SKILL.md does not reference 'amplihack orch helper'"
fi

if grep -qE 'orch_helper\.py' "${SKILL_MD}" \
   && ! grep -qE 'replaces.*orch_helper\.py|legacy.*orch_helper\.py|replaced.*orch_helper\.py' "${SKILL_MD}"; then
    fail 1b "SKILL.md still references orch_helper.py as a current tool"
else
    pass 1b "SKILL.md does not present orch_helper.py as a current tool"
fi

# ---------------------------------------------------------------------------
# Assertion 2: SKILL.md references native recursion guard, not session_tree.py
# ---------------------------------------------------------------------------
if grep -qE 'AMPLIHACK_MAX_DEPTH' "${SKILL_MD}"; then
    pass 2a "SKILL.md references AMPLIHACK_MAX_DEPTH (native recursion guard)"
else
    fail 2a "SKILL.md does not reference AMPLIHACK_MAX_DEPTH"
fi

if grep -qE 'session_tree\.py' "${SKILL_MD}" \
   && ! grep -qE 'replaces.*session_tree\.py|legacy.*session_tree\.py|replaced.*session_tree\.py' "${SKILL_MD}"; then
    fail 2b "SKILL.md still references session_tree.py as a current tool"
else
    pass 2b "SKILL.md does not present session_tree.py as a current tool"
fi

# ---------------------------------------------------------------------------
# Assertion 3: reference.md does not list ci_status.py / github_issue.py as
# active tools — should reference gh CLI or native Rust instead
# ---------------------------------------------------------------------------
if grep -qE 'ci_status\.py|github_issue\.py' "${REFERENCE_MD}"; then
    fail 3 "reference.md still lists ci_status.py or github_issue.py as active tools"
else
    pass 3 "reference.md no longer lists legacy Python API tools"
fi

if grep -qiE 'gh CLI|gh pr|gh issue|amplihack orch' "${REFERENCE_MD}"; then
    pass 3b "reference.md references modern tooling (gh CLI or native Rust)"
else
    fail 3b "reference.md does not reference gh CLI or native Rust replacements"
fi

# ---------------------------------------------------------------------------
# Assertion 4: dependency-resolver README does not tell agents to use ci_status.py
# ---------------------------------------------------------------------------
if grep -qE 'ci_status\.py' "${DEP_RESOLVER_README}"; then
    fail 4 "dependency-resolver README still references ci_status.py"
else
    pass 4 "dependency-resolver README no longer references ci_status.py"
fi

# ---------------------------------------------------------------------------
# Assertion 5: runtime-topology README references native recursion guard
# ---------------------------------------------------------------------------
if grep -qE 'AMPLIHACK_MAX_DEPTH|native recursion guard' "${RUNTIME_TOPO_README}"; then
    pass 5a "runtime-topology README references native recursion guard"
else
    fail 5a "runtime-topology README does not mention native recursion guard"
fi

if grep -qE 'session_tree\.py' "${RUNTIME_TOPO_README}"; then
    fail 5b "runtime-topology README still references session_tree.py"
else
    pass 5b "runtime-topology README no longer references session_tree.py"
fi

# ---------------------------------------------------------------------------
# Assertion 6: dev-orchestrator tutorial references native Rust orch helper
# ---------------------------------------------------------------------------
if grep -qE 'amplihack orch helper' "${DEV_ORCH_TUTORIAL}"; then
    pass 6 "dev-orchestrator tutorial references 'amplihack orch helper'"
else
    fail 6 "dev-orchestrator tutorial does not reference 'amplihack orch helper'"
fi

# ---------------------------------------------------------------------------
# Assertion 7: Historical docs with orch_helper.py mentions include deprecation note
# ---------------------------------------------------------------------------
HISTORICAL_DOCS=(
    "${REPO_ROOT}/docs/tutorials/workflow-publish-import-validation.md"
    "${REPO_ROOT}/docs/reference/workflow-publish-import-validation.md"
    "${REPO_ROOT}/docs/reference/resolve-bundle-asset-command.md"
    "${REPO_ROOT}/docs/recipes/P1_WORKFLOW_RELIABILITY_FIXES.md"
    "${REPO_ROOT}/docs/audits/recipe-runner-quality-robustness-audit.md"
    "${REPO_ROOT}/docs/howto/configure-workflow-publish-import-validation.md"
)

hist_pass=true
for doc in "${HISTORICAL_DOCS[@]}"; do
    if [[ ! -f "${doc}" ]]; then
        continue
    fi
    basename_doc="$(basename "${doc}")"
    if grep -qE 'orch_helper\.py|build_publish_validation.*\.py' "${doc}"; then
        if grep -qiE 'Note.*Rust|Note.*replaced|Note.*native|Note.*May 2026|deprecated|legacy.*replaced|now replaced|replaces legacy|native Rust|Legacy:' "${doc}"; then
            pass "7:${basename_doc}" "has deprecation note alongside legacy references"
        else
            fail "7:${basename_doc}" "mentions legacy Python but lacks deprecation note"
            hist_pass=false
        fi
    fi
done
if $hist_pass; then
    pass 7 "All historical docs with legacy refs include deprecation notes"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Bug #666 — all stale Python tool references replaced with native Rust."
exit 0
