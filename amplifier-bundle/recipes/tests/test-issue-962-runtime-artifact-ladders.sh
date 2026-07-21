#!/usr/bin/env bash
# test-issue-962-runtime-artifact-ladders.sh — regression test for issue #962:
# NORMALIZE every bundled-helper resolution ladder in the touched recipes to the
# canonical 5-tier order so a helper that exists at the install root is ALWAYS
# found regardless of cwd / repo_path / gitignored-worktree-bundle.
#
# Canonical tier order (mirrors the landed #955 chain):
#   ${AMPLIHACK_HOME} -> ${REPO_PATH} -> $(pwd)
#     -> ${HOME}/.copilot -> ${HOME}/.amplihack
#
# EXCEPTION (issue #684): step-18c in workflow-pr-review.yaml requires the
# worktree and MUST NOT reference ${REPO_PATH}; its runtime-artifact helper uses
# the git-toplevel variant (${AMPLIHACK_HOME} -> git-toplevel -> $(pwd) ->
# ${HOME}/.copilot -> ${HOME}/.amplihack) — asserted separately below.
#
# Sites normalized by this PR (each must gain the missing tiers):
#   - workflow-finalize.yaml      workflow_pr_ready.sh           (2-tier if-form)
#   - workflow-finalize.yaml      workflow_final_status.sh       (2-tier if-form)
#   - workflow-finalize.yaml      workflow_agentic_finalization.sh (2-tier if-form)
#   - workflow-publish.yaml       workflow_publish_pr.sh         (2-tier if-form)
#   - workflow-worktree.yaml      workflow_worktree_sweep.sh     (2-tier)
# Already-canonical sites are asserted as a GUARD so they cannot regress:
#   - workflow-tdd.yaml / workflow-finalize.yaml / workflow-refactor-review.yaml
#     / workflow-publish.yaml   workflow_runtime_artifacts.sh
#
# Contracts under test:
#   STATIC-tier: every listed (recipe, helper) site carries the canonical
#        $(pwd), ${REPO_PATH:-$(pwd)}, ~/.copilot and ~/.amplihack tiers.
#   STATIC-git-toplevel: the pr-review step-18c runtime-artifact ladder carries
#        the git-toplevel/cwd/~/.copilot/~/.amplihack tiers and NO ${REPO_PATH}.
#   DYNAMIC-install-root: the pr-review runtime-artifact ladder resolves the
#        helper when the bundle lives ONLY under ${HOME}/.amplihack (the real
#        #962 gitignored-worktree install-root case) with no REPO_PATH tier.
#
# This test SHOULD FAIL before the fix and MUST PASS after.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-962-runtime-artifact-ladders.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

echo "=== Issue #962: canonical 5-tier resolution ladders across bookkeeping helper sites ==="

# Each entry: "<recipe>|<helper-filename>" (REPO_PATH-canonical sites).
# NOTE: workflow-pr-review.yaml|workflow_runtime_artifacts.sh is intentionally
# ABSENT here — it uses the git-toplevel variant (issue #684) asserted below.
SITES=(
    "workflow-finalize.yaml|workflow_pr_ready.sh"
    "workflow-finalize.yaml|workflow_final_status.sh"
    "workflow-finalize.yaml|workflow_agentic_finalization.sh"
    "workflow-finalize.yaml|workflow_runtime_artifacts.sh"
    "workflow-publish.yaml|workflow_publish_pr.sh"
    "workflow-publish.yaml|workflow_runtime_artifacts.sh"
    "workflow-worktree.yaml|workflow_worktree_sweep.sh"
    "workflow-tdd.yaml|workflow_runtime_artifacts.sh"
    "workflow-refactor-review.yaml|workflow_runtime_artifacts.sh"
    "workflow-tdd.yaml|workflow_implementation_evidence.sh"
    "workflow-terminal-state.yaml|workflow_pr_scope.sh"
    "workflow-terminal-state.yaml|workflow_final_status.sh"
)

for entry in "${SITES[@]}"; do
    recipe="${entry%%|*}"
    fn="${entry##*|}"
    file="${RECIPES}/${recipe}"
    if [[ ! -f "${file}" ]]; then
        echo "HARNESS-ERROR: recipe not found: ${file}" >&2
        exit 2
    fi
    for tier in \
        "\$(pwd)/amplifier-bundle/tools/${fn}" \
        "\${REPO_PATH:-\$(pwd)}/amplifier-bundle/tools/${fn}" \
        "\${HOME:-/root}/.copilot/amplifier-bundle/tools/${fn}" \
        "\${HOME:-/root}/.amplihack/amplifier-bundle/tools/${fn}"; do
        if grep -qF "${tier}" "${file}"; then
            pass "STATIC-tier:${recipe}:${fn}" "carries tier ${tier}"
        else
            fail "STATIC-tier:${recipe}:${fn}" "missing canonical tier ${tier}"
        fi
    done
done

# ---------------------------------------------------------------------------
# STATIC-git-toplevel: pr-review step-18c runtime-artifact ladder must carry the
# git-toplevel variant tiers (NOT ${REPO_PATH}, per the #684 worktree invariant).
# ---------------------------------------------------------------------------
PR_REVIEW="${RECIPES}/workflow-pr-review.yaml"
for tier in \
    "\$(git rev-parse --show-toplevel 2>/dev/null)/amplifier-bundle/tools/workflow_runtime_artifacts.sh" \
    "\$(pwd)/amplifier-bundle/tools/workflow_runtime_artifacts.sh" \
    "\${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh" \
    "\${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; do
    if grep -qF "${tier}" "${PR_REVIEW}"; then
        pass "STATIC-git-toplevel:workflow_runtime_artifacts.sh" "carries tier ${tier}"
    else
        fail "STATIC-git-toplevel:workflow_runtime_artifacts.sh" "missing git-toplevel-variant tier ${tier}"
    fi
done
# Honor #684: the runtime-artifact ladder in step-18c must NOT reference REPO_PATH.
STEP_18C_ARTIFACT="$(grep -F 'RUNTIME_ARTIFACT_HELPER=' "${PR_REVIEW}")"
if printf '%s\n' "${STEP_18C_ARTIFACT}" | grep -qF ':-${REPO_PATH'; then
    fail "STATIC-no-repo-path:step-18c" "pr-review runtime-artifact ladder must NOT reference REPO_PATH (#684)"
else
    pass "STATIC-no-repo-path:step-18c" "pr-review runtime-artifact ladder omits REPO_PATH (#684 honored)"
fi

# ---------------------------------------------------------------------------
# DYNAMIC: pr-review runtime-artifact ladder must resolve when the bundle lives
# ONLY under ${HOME}/.amplihack — the real issue #962 case (downstream product
# repo, gitignored worktree bundle, helper present only at the install root).
# AMPLIHACK_HOME, git-toplevel, cwd and ~/.copilot intentionally LACK the helper,
# and REPO_PATH is set but must be IGNORED (step-18c omits that tier per #684).
# ---------------------------------------------------------------------------
TMP_ROOT="$(mktemp -d)"
cleanup() { rm -rf "${TMP_ROOT}"; }
trap cleanup EXIT

TMP_REPO="${TMP_ROOT}/repo"                        # cwd + git top-level, NO bundle
TMP_REPO_PATH="${TMP_ROOT}/repopath"               # REPO_PATH, HAS a decoy helper
TMP_HOME="${TMP_ROOT}/home"                         # ~/.amplihack HAS the helper

mkdir -p "${TMP_REPO}" \
         "${TMP_REPO_PATH}/amplifier-bundle/tools" \
         "${TMP_HOME}/.amplihack/amplifier-bundle/tools"
git -C "${TMP_REPO}" init -q
cat > "${TMP_HOME}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh" <<'STUB'
#!/usr/bin/env bash
preflight_known_workflow_runtime_artifacts() { echo "STUB_RUNTIME_ARTIFACTS_RAN"; }
STUB
# Decoy under REPO_PATH must be IGNORED (proves the ladder has no REPO_PATH tier).
cat > "${TMP_REPO_PATH}/amplifier-bundle/tools/workflow_runtime_artifacts.sh" <<'STUB'
#!/usr/bin/env bash
echo "DECOY_REPO_PATH_HELPER_SHOULD_NOT_RESOLVE" >&2; exit 99
STUB

# Extract the pr-review runtime-artifact resolution ladder + terminal guard
# (assignment lines and the `[ -f ... ] || { ... exit N; }` guard), excluding
# the later `. "$RUNTIME_ARTIFACT_HELPER"` sourcing line.
CHAIN="$(grep -E 'RUNTIME_ARTIFACT_HELPER=|\[ -f "\$RUNTIME_ARTIFACT_HELPER" \]' \
            "${PR_REVIEW}" \
         | grep -vE '^\s*\.\s' )"
CHAIN="$(printf '%s\n' "${CHAIN}" | sed -E 's/^[[:space:]]+//')"
if [[ -z "${CHAIN}" ]]; then
    echo "HARNESS-ERROR: could not extract pr-review runtime-artifact chain" >&2
    exit 2
fi

CHAIN_FILE="${TMP_ROOT}/pr_review_chain.sh"
{ printf '%s\n' "${CHAIN}"; printf 'printf "RESOLVED=%%s\\n" "$RUNTIME_ARTIFACT_HELPER"\n'; } > "${CHAIN_FILE}"

set +e
out="$(
    cd "${TMP_REPO}" || exit 3
    export HOME="${TMP_HOME}"
    unset AMPLIHACK_HOME
    export REPO_PATH="${TMP_REPO_PATH}"
    bash "${CHAIN_FILE}"
)"
rc=$?
set -e
if [[ ${rc} -eq 0 ]] \
   && printf '%s\n' "${out}" | grep -qF "RESOLVED=${TMP_HOME}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; then
    pass "DYNAMIC-install-root" "pr-review ladder resolves via the \${HOME}/.amplihack tier, ignoring the REPO_PATH decoy (rc=0)"
else
    fail "DYNAMIC-install-root" "pr-review ladder did not resolve via \${HOME}/.amplihack (rc=${rc}): ${out}"
fi

echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -gt 0 ]] && exit 1
echo "PASS: Issue #962 — all bookkeeping helper sites carry the canonical 5-tier ladder."
exit 0
