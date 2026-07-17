#!/usr/bin/env bash
# test-issue-955-git-identity-fallback.sh — regression test for issue #955.
#
# Bug: the recipe git-identity.sh helper was resolved with only TWO candidate
# paths and then hard-failed with `exit 2` when neither existed. This killed
# every commit/checkpoint step whenever AMPLIHACK_HOME pointed at a downstream
# repo checkout lacking `amplifier-bundle/` (e.g. the Simard project), even
# though the helper WAS installed at ${HOME}/.amplihack/amplifier-bundle/tools/
# git-identity.sh. Observed impact: default-workflow runs completed
# design+TDD+implement+verify but produced NO commit and NO PR because the
# checkpoint step exited 2.
#
# Fix: mirror the sibling RUNTIME_ARTIFACT_HELPER resolution — append three
# fallbacks BEFORE the terminal `exit 2`, in this order:
#   $(pwd)/...
#   ${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh
#   ${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh
# The final `exit 2` is retained ONLY after ALL candidates are exhausted
# (fail-visible, never silent-degrade).
#
# Contracts under test:
#   A. BEHAVIOURAL — with AMPLIHACK_HOME and the repo top-level both lacking
#      `amplifier-bundle/`, the extracted chain resolves the helper via the
#      ${HOME}/.amplihack fallback and exits 0 (no `exit 2`).
#   B. BEHAVIOURAL — when NONE of the candidate paths exist, the chain still
#      hard-fails with `exit 2` and emits the ERROR (fail-visible preserved).
#   C. STATIC — every recipe that resolves git-identity contains the
#      ${HOME}/.amplihack fallback candidate; the 7 single-line gate sites also
#      retain their terminal `exit 2`.
#
# This test SHOULD FAIL before the #955 fix lands (chain has only 2 paths).
# It MUST PASS once the three fallbacks are appended.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-955-git-identity-fallback.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

TDD="${RECIPES}/workflow-tdd.yaml"

SINGLE_LINE_RECIPES=(
    "consensus-pr-feedback.yaml"
    "workflow-tdd.yaml"
    "workflow-refactor-review.yaml"
    "workflow-publish.yaml"
    "workflow-finalize.yaml"
    "consensus-publish.yaml"
    "workflow-pr-review.yaml"
)

FALLBACK_CANDIDATE='${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh'

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

for f in "${SINGLE_LINE_RECIPES[@]}"; do
    if [[ ! -f "${RECIPES}/${f}" ]]; then
        echo "HARNESS-ERROR: required recipe not found: ${RECIPES}/${f}" >&2
        exit 2
    fi
done

# ---------------------------------------------------------------------------
# Extract the real single-line git-identity resolution chain from workflow-tdd
# and swap the final `. "$HELPER"` sourcing for an echo so we can observe the
# resolved path without actually sourcing the helper under test.
# ---------------------------------------------------------------------------
CHAIN="$(grep -m1 -E 'GIT_IDENTITY_HELPER="\$\{AMPLIHACK_HOME' "${TDD}" | sed -E 's/^[[:space:]]+//')"
if [[ -z "${CHAIN}" ]]; then
    echo "HARNESS-ERROR: could not extract git-identity chain from ${TDD}" >&2
    exit 2
fi
CHAIN_PROBE="$(printf '%s' "${CHAIN}" | sed -E 's/; \. "\$GIT_IDENTITY_HELPER"$/; echo "RESOLVED=$GIT_IDENTITY_HELPER"/')"

WORK="$(mktemp -d -t issue-955-XXXXXX)"
trap 'rm -rf "${WORK}"' EXIT

# A git repo sandbox that deliberately LACKS amplifier-bundle/.
SANDBOX_REPO="${WORK}/downstream-repo"
mkdir -p "${SANDBOX_REPO}"
git -C "${SANDBOX_REPO}" init -q
# AMPLIHACK_HOME that also lacks amplifier-bundle/.
EMPTY_HOME="${WORK}/amplihack-home-empty"
mkdir -p "${EMPTY_HOME}"

# ---------------------------------------------------------------------------
# A. Positive: ${HOME}/.amplihack fallback resolves; exit 0, no `exit 2`.
# ---------------------------------------------------------------------------
FAKE_HOME="${WORK}/fake-home"
mkdir -p "${FAKE_HOME}/.amplihack/amplifier-bundle/tools"
printf '#!/usr/bin/env bash\n: # stub git-identity helper\n' \
    >"${FAKE_HOME}/.amplihack/amplifier-bundle/tools/git-identity.sh"

set +e
OUT_A="$(cd "${SANDBOX_REPO}" && env -u REPO_PATH HOME="${FAKE_HOME}" \
    AMPLIHACK_HOME="${EMPTY_HOME}" bash -c "${CHAIN_PROBE}" 2>"${WORK}/err_a")"
RC_A=$?
set -e

EXPECTED_A="${FAKE_HOME}/.amplihack/amplifier-bundle/tools/git-identity.sh"
if [[ ${RC_A} -eq 0 && "${OUT_A}" == "RESOLVED=${EXPECTED_A}" ]]; then
    pass "A-fallback-resolves" "helper resolved via \${HOME}/.amplihack fallback (rc=0)"
else
    fail "A-fallback-resolves" \
        "expected rc=0 + RESOLVED=${EXPECTED_A}; got rc=${RC_A}, out='${OUT_A}'"
fi

# ---------------------------------------------------------------------------
# B. Negative: no candidate exists anywhere -> still hard-fails exit 2.
# ---------------------------------------------------------------------------
BARE_HOME="${WORK}/bare-home"
mkdir -p "${BARE_HOME}"
set +e
cd "${SANDBOX_REPO}" && env -u REPO_PATH HOME="${BARE_HOME}" \
    AMPLIHACK_HOME="${EMPTY_HOME}" bash -c "${CHAIN_PROBE}" >/dev/null 2>"${WORK}/err_b"
RC_B=$?
set -e

if [[ ${RC_B} -eq 2 ]] && grep -q "git identity helper not found" "${WORK}/err_b"; then
    pass "B-all-missing-exit2" "all candidates missing -> exit 2 + ERROR (fail-visible)"
else
    fail "B-all-missing-exit2" \
        "expected rc=2 + ERROR; got rc=${RC_B}, err='$(cat "${WORK}/err_b")'"
fi

# ---------------------------------------------------------------------------
# C. Static: every recipe carries the .amplihack fallback; the 7 single-line
#    gate sites retain their terminal exit 2.
# ---------------------------------------------------------------------------
for f in "${SINGLE_LINE_RECIPES[@]}"; do
    p="${RECIPES}/${f}"
    if grep -Fq "${FALLBACK_CANDIDATE}" "${p}"; then
        pass "C-fallback-${f}" "${f} contains \${HOME}/.amplihack git-identity fallback"
    else
        fail "C-fallback-${f}" "${f} missing \${HOME}/.amplihack git-identity fallback"
    fi
    if grep -Eq 'git identity helper not found.*exit[[:space:]]+2' "${p}"; then
        pass "C-exit2-${f}" "${f} retains terminal git-identity exit 2"
    else
        fail "C-exit2-${f}" "${f} lost terminal git-identity exit 2"
    fi
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
[[ ${FAIL_COUNT} -eq 0 ]] || exit 1
exit 0
