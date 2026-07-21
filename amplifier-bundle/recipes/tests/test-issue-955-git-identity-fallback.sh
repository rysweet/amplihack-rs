#!/usr/bin/env bash
# test-issue-955-git-identity-fallback.sh — regression test for issue #955.
#
# Bug: the git-identity.sh helper is resolved with only TWO candidate paths and
# then hard-fails with `exit 2` if neither exists:
#     GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh"
#     [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"
#     [ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }
# This kills EVERY commit/checkpoint step whenever AMPLIHACK_HOME points at a
# downstream repo checkout that lacks amplifier-bundle/ — even though the helper
# IS installed at ${HOME}/.amplihack/amplifier-bundle/tools/git-identity.sh.
#
# Fix: mirror the sibling RUNTIME_ARTIFACT_HELPER resolution — append THREE more
# guarded fallbacks before the terminal exit 2:
#     ... || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"
#     ... || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"
#     ... || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"
# The terminal `exit 2` stays LAST (fail-visible, never silent-degrade).
#
# Contracts under test:
#   POS: When AMPLIHACK_HOME and the repo top-level both LACK amplifier-bundle/,
#        the chain MUST still resolve the helper via the ${HOME}/.amplihack
#        fallback, source it, and NOT exit 2.
#   POS2: The ${HOME}/.copilot fallback also resolves.
#   NEG: When the helper is absent from ALL candidate paths, the chain MUST
#        still fail visibly with exit 2 + "git identity helper not found"
#        (fail-visible control — never silent-degrade).
#   STATIC: Every recipe that emits "git identity helper not found" MUST carry
#        the three canonical tail fallbacks in its git-identity chain.
#   STATIC2: workflow-publish.yaml multi-line block MUST also gain the three
#        fallbacks while remaining exit-2-free (it never had an exit 2).
#   GUARD: Exactly 7 single-line sites retain their terminal git-identity
#        `exit 2`; the multi-line publish block stays exit-2-free.
#
# This test SHOULD FAIL before the #955 fix lands (only 2 candidates present).
# It MUST PASS once the three tail fallbacks are appended everywhere.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-955-git-identity-fallback.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

# Every recipe that emits "git identity helper not found" (7 single-line sites).
RECIPE_FILES=(
    "workflow-finalize.yaml"
    "workflow-refactor-review.yaml"
    "workflow-pr-review.yaml"
    "workflow-tdd.yaml"
    "workflow-publish.yaml"
    "consensus-publish.yaml"
    "consensus-pr-feedback.yaml"
)

# The three canonical tail fallbacks that MUST be appended (mirrors the sibling
# RUNTIME_ARTIFACT_HELPER chain, retargeted at git-identity.sh).
FALLBACK_PWD='$(pwd)/amplifier-bundle/tools/git-identity.sh'
FALLBACK_COPILOT='${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh'
FALLBACK_AMPLIHACK='${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh'

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

for f in "${RECIPE_FILES[@]}"; do
    if [[ ! -f "${RECIPES}/${f}" ]]; then
        echo "HARNESS-ERROR: required recipe not found: ${RECIPES}/${f}" >&2
        exit 2
    fi
done

echo "=== Issue #955: git-identity helper resolution robustness ==="

# ---------------------------------------------------------------------------
# Dynamic fixture: an isolated environment where NEITHER AMPLIHACK_HOME NOR the
# repo top-level contains amplifier-bundle/, reproducing the downstream-checkout
# scenario (e.g. the Simard project) from the bug report.
# ---------------------------------------------------------------------------
TMP_ROOT="$(mktemp -d)"
cleanup() { rm -rf "${TMP_ROOT}"; }
trap cleanup EXIT

TMP_REPO="${TMP_ROOT}/repo"                 # git top-level, NO amplifier-bundle/
TMP_AH="${TMP_ROOT}/downstream"             # AMPLIHACK_HOME, NO amplifier-bundle/
HOME_AMPLIHACK="${TMP_ROOT}/home_amplihack" # has ~/.amplihack/.../git-identity.sh
HOME_COPILOT="${TMP_ROOT}/home_copilot"     # has ~/.copilot/.../git-identity.sh
HOME_EMPTY="${TMP_ROOT}/home_empty"         # nothing installed anywhere

mkdir -p "${TMP_REPO}" "${TMP_AH}" "${HOME_EMPTY}"
git -C "${TMP_REPO}" init -q

# Install stub helpers only under the "home" trees (NOT under AMPLIHACK_HOME or
# the repo top-level) so resolution must reach the appended tail fallbacks.
install_stub() {
    local dir="$1"
    mkdir -p "${dir}"
    cat > "${dir}/git-identity.sh" <<'STUB'
#!/usr/bin/env bash
# Test stub sourced in place of the real git-identity helper.
echo "STUB_GITID_SOURCED"
amplihack_prepare_git_commit_identity() { :; }
STUB
}
install_stub "${HOME_AMPLIHACK}/.amplihack/amplifier-bundle/tools"
install_stub "${HOME_COPILOT}/.copilot/amplifier-bundle/tools"

# extract_gitid_chain <recipe> — print the single-line git-identity resolution
# chain (the one that ends in the terminal `exit 2`), leading whitespace removed.
extract_gitid_chain() {
    local recipe="$1" line
    line="$(grep -E 'GIT_IDENTITY_HELPER="[^"]*git rev-parse --show-toplevel' "${recipe}" \
            | grep -F 'exit 2' | head -1)"
    line="${line#"${line%%[![:space:]]*}"}"
    printf '%s' "${line}"
}

# run_gitid_chain <recipe> <home> — evaluate a recipe's git-identity chain inside
# the isolated fixture and echo the resolved path. Runs in a subshell so a
# terminal `exit 2` surfaces as the subshell exit code.
run_gitid_chain() {
    local recipe="$1" home="$2" chain
    chain="$(extract_gitid_chain "${RECIPES}/${recipe}")"
    if [[ -z "${chain}" ]]; then
        echo "HARNESS: no git-identity chain found in ${recipe}" >&2
        return 3
    fi
    (
        cd "${TMP_REPO}" || exit 3
        export HOME="${home}"
        export AMPLIHACK_HOME="${TMP_AH}"
        unset REPO_PATH
        eval "${chain}"
        printf 'RESOLVED=%s\n' "${GIT_IDENTITY_HELPER}"
    )
}

# ---------------------------------------------------------------------------
# POS: ${HOME}/.amplihack fallback resolves for every recipe.
# ---------------------------------------------------------------------------
for f in "${RECIPE_FILES[@]}"; do
    set +e
    out="$(run_gitid_chain "${f}" "${HOME_AMPLIHACK}" 2>&1)"
    rc=$?
    set -e
    if [[ ${rc} -eq 0 ]] \
       && printf '%s\n' "${out}" | grep -q 'STUB_GITID_SOURCED' \
       && printf '%s\n' "${out}" | grep -qF "${HOME_AMPLIHACK}/.amplihack/amplifier-bundle/tools/git-identity.sh"; then
        pass "POS-amplihack:${f}" "resolves via \${HOME}/.amplihack fallback and sources helper"
    else
        fail "POS-amplihack:${f}" "did not resolve via \${HOME}/.amplihack fallback (rc=${rc}): ${out}"
    fi
done

# ---------------------------------------------------------------------------
# POS2: ${HOME}/.copilot fallback resolves for every recipe.
# ---------------------------------------------------------------------------
for f in "${RECIPE_FILES[@]}"; do
    set +e
    out="$(run_gitid_chain "${f}" "${HOME_COPILOT}" 2>&1)"
    rc=$?
    set -e
    if [[ ${rc} -eq 0 ]] \
       && printf '%s\n' "${out}" | grep -q 'STUB_GITID_SOURCED' \
       && printf '%s\n' "${out}" | grep -qF "${HOME_COPILOT}/.copilot/amplifier-bundle/tools/git-identity.sh"; then
        pass "POS-copilot:${f}" "resolves via \${HOME}/.copilot fallback and sources helper"
    else
        fail "POS-copilot:${f}" "did not resolve via \${HOME}/.copilot fallback (rc=${rc}): ${out}"
    fi
done

# ---------------------------------------------------------------------------
# NEG: fail-visible control — helper truly absent everywhere MUST still exit 2.
# ---------------------------------------------------------------------------
for f in "${RECIPE_FILES[@]}"; do
    set +e
    out="$(run_gitid_chain "${f}" "${HOME_EMPTY}" 2>&1)"
    rc=$?
    set -e
    if [[ ${rc} -eq 2 ]] \
       && printf '%s\n' "${out}" | grep -q 'git identity helper not found'; then
        pass "NEG-failvisible:${f}" "absent helper still exits 2 with ERROR (fail-visible)"
    else
        fail "NEG-failvisible:${f}" "did not fail visibly with exit 2 (rc=${rc}): ${out}"
    fi
done

# ---------------------------------------------------------------------------
# STATIC: each recipe's git-identity chain carries all three tail fallbacks.
# ---------------------------------------------------------------------------
count_fallback() {
    # Literal-substring occurrence count within a file.
    grep -Fc "$2" "$1" 2>/dev/null || true
}

for f in "${RECIPE_FILES[@]}"; do
    file="${RECIPES}/${f}"
    # workflow-publish.yaml carries TWO git-identity chains (single-line + the
    # multi-line block), so each fallback must appear at least twice there.
    min=1
    [[ "${f}" == "workflow-publish.yaml" ]] && min=2

    for cand_name in PWD COPILOT AMPLIHACK; do
        case "${cand_name}" in
            PWD) cand="${FALLBACK_PWD}" ;;
            COPILOT) cand="${FALLBACK_COPILOT}" ;;
            AMPLIHACK) cand="${FALLBACK_AMPLIHACK}" ;;
        esac
        n="$(count_fallback "${file}" "${cand}")"
        if [[ "${n}" -ge "${min}" ]]; then
            pass "STATIC-${cand_name}:${f}" "carries ${cand_name} fallback (>= ${min})"
        else
            fail "STATIC-${cand_name}:${f}" "missing ${cand_name} fallback '${cand}' (found ${n}, need >= ${min})"
        fi
    done
done

# ---------------------------------------------------------------------------
# STATIC2 + GUARD: exactly 7 single-line git-identity `exit 2` sites; the
# workflow-publish.yaml multi-line block stays exit-2-free (only one file
# references the ERROR text more than once? No — each file has exactly one
# single-line ERROR site; publish's multi-line block never emits the ERROR).
# ---------------------------------------------------------------------------
total_exit2=0
for f in "${RECIPE_FILES[@]}"; do
    n="$(grep -E 'git identity helper not found' "${RECIPES}/${f}" 2>/dev/null \
         | grep -cE 'exit[[:space:]]+2' || true)"
    total_exit2=$((total_exit2 + n))
done
if [[ "${total_exit2}" -eq 7 ]]; then
    pass "GUARD-exit2-total" "exactly 7 single-line git-identity exit-2 sites retained"
else
    fail "GUARD-exit2-total" "found ${total_exit2} git-identity exit-2 sites (expected 7)"
fi

# The publish multi-line block must NOT gain an exit 2: the file references the
# ERROR text exactly once (its single-line site only).
publish_err="$(grep -cE 'git identity helper not found' "${RECIPES}/workflow-publish.yaml" 2>/dev/null || true)"
if [[ "${publish_err}" -eq 1 ]]; then
    pass "GUARD-publish-noexit2" "workflow-publish.yaml multi-line block stays exit-2-free"
else
    fail "GUARD-publish-noexit2" "workflow-publish.yaml has ${publish_err} git-identity ERROR sites (expected 1)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Issue #955 — git-identity helper resolves via the full fallback chain (fail-visible only when truly absent)."
exit 0
