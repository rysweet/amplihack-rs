#!/usr/bin/env bash
# test-issue-840-worktree-leak-proof.sh — TDD spec for issue #840.
#
# Issue #840: make default-workflow worktree setup leak-proof and self-healing.
# The idempotent reuse/reset/recreate state machine in workflow-worktree.yaml
# (BRANCH_EXISTS/WORKTREE_EXISTS, ~lines 268-314) already prevents the original
# "cannot delete branch X used by worktree at PATH" error. This test pins the
# TWO remaining acceptance criteria:
#
#   1. Orphan-worktree sweep — best-effort prune of orphaned amplihack worktrees
#      left by prior FAILED runs, so a re-run converges (succeeds) instead of
#      leaking. Stale + no-unmerged-meaningful-diff orphans are removed; fresh
#      or unmerged-meaningful worktrees are PRESERVED (archived first, never
#      destroyed).
#   2. Cleanup-on-failure — a failed/aborted run prunes its own worktree+branch,
#      but only AFTER archiving/pushing any unique commits (never destroy
#      unmerged meaningful work).
#
# Design (from the approved spec): the destructive lifecycle logic lives in a
# self-contained helper `amplifier-bundle/tools/workflow_worktree_sweep.sh` to
# keep workflow-worktree.yaml strictly < 400 lines (and to leave the recipe
# step inventory unchanged). step-04-setup-worktree invokes it best-effort
# (graceful no-op if the helper is absent, per #829 precedent).
#
# Helper CLI contract (defined here, TDD-first):
#   workflow_worktree_sweep.sh sweep <repo_path>
#   workflow_worktree_sweep.sh cleanup_on_failure <repo_path> <branch>
# Env knobs:
#   AMPLIHACK_WORKTREE_STALE_SECS   orphan staleness threshold (default 86400).
#                                   Validated ^[0-9]+$; bad value -> default.
#   AMPLIHACK_WORKTREE_BASE_REF     base ref for the unmerged-diff gate; if
#                                   unset the helper resolves origin/HEAD ->
#                                   origin/main -> origin/master -> main/master.
#
# This test SHOULD FAIL before #840 lands (helper missing, YAML not yet calling
# it) and MUST PASS once the helper + step-04 inline call exist.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-840-worktree-leak-proof.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"

WORKTREE_YAML="${RECIPES}/workflow-worktree.yaml"
HELPER="${TOOLS}/workflow_worktree_sweep.sh"

PASS_COUNT=0
FAIL_COUNT=0

pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

if [[ ! -f "${WORKTREE_YAML}" ]]; then
    echo "HARNESS-ERROR: required recipe not found: ${WORKTREE_YAML}" >&2
    exit 2
fi

# Scratch workspace; cleaned on exit.
TEST_TMP="$(mktemp -d)"
cleanup() { rm -rf "${TEST_TMP}"; }
trap cleanup EXIT

# extract_step <file> <step-id>
# Prints the contiguous block from the matching `- id: "<step-id>"` line up to
# (but not including) the next top-level `  - id:` step marker.
extract_step() {
    local file="$1" step_id="$2"
    awk -v target="${step_id}" '
        BEGIN { inblk = 0 }
        /^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/ {
            line = $0
            sub(/^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/, "", line)
            sub(/".*$/, "", line)
            if (line == target) { inblk = 1; print; next }
            else if (inblk) { inblk = 0 }
        }
        inblk { print }
    ' "${file}"
}

echo "=== Issue #840: leak-proof, self-healing worktree setup ==="

# ===========================================================================
# Part A — Static / contract checks (helper + YAML integration).
# ===========================================================================

# A1: helper exists.
if [[ -f "${HELPER}" ]]; then
    pass "A1-exists" "sweep helper present at tools/workflow_worktree_sweep.sh"
else
    fail "A1-exists" "missing helper: ${HELPER}"
fi

# A2: helper hardened — set -euo pipefail.
if [[ -f "${HELPER}" ]] && grep -qE 'set -euo pipefail' "${HELPER}"; then
    pass "A2-strict" "helper uses 'set -euo pipefail'"
else
    fail "A2-strict" "helper missing 'set -euo pipefail'"
fi

# A3: helper advertises both modes.
if [[ -f "${HELPER}" ]] && grep -qE '\bsweep\b' "${HELPER}" \
   && grep -qE 'cleanup_on_failure' "${HELPER}"; then
    pass "A3-modes" "helper implements sweep + cleanup_on_failure modes"
else
    fail "A3-modes" "helper does not implement both sweep and cleanup_on_failure modes"
fi

# A4: helper parses worktrees only via porcelain (no fragile `ls` scraping).
if [[ -f "${HELPER}" ]] && grep -qE 'git worktree list --porcelain' "${HELPER}"; then
    pass "A4-porcelain" "helper enumerates worktrees via 'git worktree list --porcelain'"
else
    fail "A4-porcelain" "helper does not use 'git worktree list --porcelain'"
fi

# A5: helper honors the staleness env knob.
if [[ -f "${HELPER}" ]] && grep -qE 'AMPLIHACK_WORKTREE_STALE_SECS' "${HELPER}"; then
    pass "A5-stale-env" "helper honors AMPLIHACK_WORKTREE_STALE_SECS"
else
    fail "A5-stale-env" "helper ignores AMPLIHACK_WORKTREE_STALE_SECS"
fi

# A6: helper enforces the unmerged-meaningful-diff gate (rev-list count BASE..HEAD).
if [[ -f "${HELPER}" ]] && grep -qE 'rev-list --count' "${HELPER}"; then
    pass "A6-diff-gate" "helper computes unmerged-diff gate via rev-list --count"
else
    fail "A6-diff-gate" "helper missing unmerged-diff safety gate (rev-list --count)"
fi

# A7: helper shellcheck-clean (only when shellcheck is installed).
if command -v shellcheck >/dev/null 2>&1; then
    if [[ -f "${HELPER}" ]] && shellcheck -S warning "${HELPER}" >/dev/null 2>&1; then
        pass "A7-shellcheck" "helper passes shellcheck -S warning"
    else
        fail "A7-shellcheck" "helper fails shellcheck (or is missing)"
    fi
else
    echo "  SKIP[A7-shellcheck]: shellcheck not installed"
fi

# A8: step-04-setup-worktree invokes the sweep helper.
STEP04="$(extract_step "${WORKTREE_YAML}" "step-04-setup-worktree")"
if [[ -z "${STEP04}" ]]; then
    fail "A8-step" "could not extract step-04-setup-worktree from workflow-worktree.yaml"
else
    if printf '%s\n' "${STEP04}" | grep -qE 'workflow_worktree_sweep\.sh'; then
        pass "A8-invoke" "step-04 invokes workflow_worktree_sweep.sh"
    else
        fail "A8-invoke" "step-04 does not invoke workflow_worktree_sweep.sh"
    fi
fi

# A9: the inline sweep call is best-effort (graceful no-op): guarded by a
# file-existence test and/or `|| true` so a missing helper never aborts setup.
if printf '%s\n' "${STEP04}" | grep -E 'workflow_worktree_sweep\.sh' \
       | grep -qE '(\|\| true|-f )'; then
    pass "A9-best-effort" "step-04 sweep call is best-effort (guarded / '|| true')"
else
    fail "A9-best-effort" "step-04 sweep call is not guarded best-effort"
fi

# A10: HARD CONSTRAINT — workflow-worktree.yaml strictly < 400 lines.
YAML_LINES=$(wc -l < "${WORKTREE_YAML}")
if [[ "${YAML_LINES}" -lt 400 ]]; then
    pass "A10-400" "workflow-worktree.yaml is ${YAML_LINES} lines (< 400)"
else
    fail "A10-400" "workflow-worktree.yaml is ${YAML_LINES} lines (>= 400 — brick limit breached)"
fi

# ===========================================================================
# Part B — Scenario checks against a real temp git repo.
# These exercise the helper's runtime behaviour. They require the helper to
# exist; before #840 lands they fail at the guard below (expected, TDD).
# ===========================================================================

# build_repo <name> -> echoes path to a fresh repo with origin + main commit.
build_repo() {
    local name="$1"
    local remote="${TEST_TMP}/${name}-remote.git"
    local work="${TEST_TMP}/${name}"
    git init --quiet --bare "${remote}"
    git clone --quiet "${remote}" "${work}" 2>/dev/null
    git -C "${work}" config user.email "t@example.com"
    git -C "${work}" config user.name "Test"
    git -C "${work}" checkout -q -b main 2>/dev/null || git -C "${work}" checkout -q main
    echo "base" > "${work}/README.md"
    git -C "${work}" add README.md
    git -C "${work}" commit -q -m "base commit"
    git -C "${work}" push -q -u origin main 2>/dev/null || true
    git -C "${work}" remote set-head origin main 2>/dev/null || true
    printf '%s\n' "${work}"
}

run_sweep() { # run_sweep <repo> [extra-env-prefixed args via env]
    bash "${HELPER}" sweep "$1"
}

if [[ ! -f "${HELPER}" ]]; then
    echo ""
    echo "--- Scenario checks SKIPPED: helper not yet implemented (TDD red) ---"
    # Record the skip as failures so the suite is RED until #840 is implemented.
    for s in B1-best-effort B2-stale-removed B3-fresh-kept B4-unmerged-preserved \
             B5-no-escape B6-cleanup-archives B7-converges; do
        fail "${s}" "scenario requires ${HELPER} (not implemented)"
    done
else
    export AMPLIHACK_WORKTREE_BASE_REF="origin/main"

    # --- B1: best-effort — sweep on a clean repo (no orphans) exits 0. ---
    REPO="$(build_repo b1)"
    if run_sweep "${REPO}" >/dev/null 2>&1; then
        pass "B1-best-effort" "sweep on clean repo exits 0"
    else
        fail "B1-best-effort" "sweep on clean repo returned non-zero"
    fi

    # --- B2: STALE + no-diff orphan is removed (dir + branch + prune). ---
    REPO="$(build_repo b2)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/orphan-clean" \
        -b feat/orphan-clean origin/main
    AMPLIHACK_WORKTREE_STALE_SECS=0 run_sweep "${REPO}" >/dev/null 2>&1 || true
    if [[ ! -e "${REPO}/worktrees/feat/orphan-clean" ]] \
       && ! git -C "${REPO}" worktree list --porcelain | grep -qF "worktree ${REPO}/worktrees/feat/orphan-clean" \
       && [[ -z "$(git -C "${REPO}" branch --list feat/orphan-clean)" ]]; then
        pass "B2-stale-removed" "stale clean orphan worktree + branch pruned"
    else
        fail "B2-stale-removed" "stale clean orphan was not fully pruned"
    fi

    # --- B3: FRESH orphan (default threshold) is PRESERVED. ---
    REPO="$(build_repo b3)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/fresh" \
        -b feat/fresh origin/main
    # default AMPLIHACK_WORKTREE_STALE_SECS=86400; freshly created -> not stale.
    run_sweep "${REPO}" >/dev/null 2>&1 || true
    if [[ -e "${REPO}/worktrees/feat/fresh" ]] \
       && [[ -n "$(git -C "${REPO}" branch --list feat/fresh)" ]]; then
        pass "B3-fresh-kept" "fresh (non-stale) orphan preserved"
    else
        fail "B3-fresh-kept" "fresh orphan was wrongly pruned"
    fi

    # --- B4: STALE but UNMERGED-MEANINGFUL orphan is preserved/archived,
    #         never silently destroyed. The commit MUST remain reachable. ---
    REPO="$(build_repo b4)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/unmerged" \
        -b feat/unmerged origin/main
    echo "unique work" > "${REPO}/worktrees/feat/unmerged/work.txt"
    git -C "${REPO}/worktrees/feat/unmerged" add work.txt
    git -C "${REPO}/worktrees/feat/unmerged" commit -q -m "unique unmerged work"
    UNMERGED_SHA="$(git -C "${REPO}/worktrees/feat/unmerged" rev-parse HEAD)"
    AMPLIHACK_WORKTREE_STALE_SECS=0 run_sweep "${REPO}" >/dev/null 2>&1 || true
    # Safety invariant: the unique commit must still be reachable somewhere
    # (branch still present, or archived under refs/amplihack-archive/*).
    if git -C "${REPO}" cat-file -e "${UNMERGED_SHA}^{commit}" 2>/dev/null; then
        reachable=no
        if [[ -n "$(git -C "${REPO}" branch --list feat/unmerged)" ]]; then
            reachable=yes
        elif git -C "${REPO}" for-each-ref --format='%(objectname)' refs/amplihack-archive/ 2>/dev/null \
                | grep -qx "${UNMERGED_SHA}"; then
            reachable=yes
        elif git -C "${REPO}" rev-parse --verify --quiet "refs/amplihack-archive/feat/unmerged" >/dev/null 2>&1; then
            reachable=yes
        fi
        if [[ "${reachable}" == "yes" ]]; then
            pass "B4-unmerged-preserved" "unmerged-meaningful orphan preserved/archived (commit reachable)"
        else
            fail "B4-unmerged-preserved" "unmerged commit ${UNMERGED_SHA} not reachable after sweep (data loss)"
        fi
    else
        fail "B4-unmerged-preserved" "unmerged commit ${UNMERGED_SHA} was destroyed by sweep (DATA LOSS)"
    fi

    # --- B5: containment — sweep touches ONLY registered worktrees under
    #         worktrees/. A sentinel outside, and an unregistered dir inside
    #         worktrees/, both survive. ---
    REPO="$(build_repo b5)"
    mkdir -p "${REPO}/IMPORTANT_KEEP"
    echo keep > "${REPO}/IMPORTANT_KEEP/data.txt"
    mkdir -p "${REPO}/worktrees/not-a-worktree"
    echo keep > "${REPO}/worktrees/not-a-worktree/data.txt"
    AMPLIHACK_WORKTREE_STALE_SECS=0 run_sweep "${REPO}" >/dev/null 2>&1 || true
    if [[ -f "${REPO}/IMPORTANT_KEEP/data.txt" ]] \
       && [[ -f "${REPO}/worktrees/not-a-worktree/data.txt" ]]; then
        pass "B5-no-escape" "sweep only acts on registered worktrees; sentinels survived"
    else
        fail "B5-no-escape" "sweep removed a non-registered path (containment breach)"
    fi

    # --- B6: cleanup_on_failure archives unique commits BEFORE pruning. ---
    REPO="$(build_repo b6)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/failed-run" \
        -b feat/failed-run origin/main
    echo "wip" > "${REPO}/worktrees/feat/failed-run/wip.txt"
    git -C "${REPO}/worktrees/feat/failed-run" add wip.txt
    git -C "${REPO}/worktrees/feat/failed-run" commit -q -m "wip from failed run"
    WIP_SHA="$(git -C "${REPO}/worktrees/feat/failed-run" rev-parse HEAD)"
    bash "${HELPER}" cleanup_on_failure "${REPO}" feat/failed-run >/dev/null 2>&1 || true
    # The unique commit must survive the prune (archived), and the live worktree
    # should be cleaned up so a re-run does not collide.
    if git -C "${REPO}" cat-file -e "${WIP_SHA}^{commit}" 2>/dev/null \
       && [[ ! -e "${REPO}/worktrees/feat/failed-run" ]]; then
        pass "B6-cleanup-archives" "cleanup_on_failure archived unique commit then pruned worktree"
    else
        fail "B6-cleanup-archives" "cleanup_on_failure lost work or failed to prune worktree"
    fi

    # --- B7: convergence — after a simulated prior failed run leaks a stale
    #         clean worktree+branch, sweep prunes it so a fresh `worktree add`
    #         at the same path/branch succeeds (no collision error). ---
    REPO="$(build_repo b7)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/leak" \
        -b feat/leak origin/main
    AMPLIHACK_WORKTREE_STALE_SECS=0 run_sweep "${REPO}" >/dev/null 2>&1 || true
    if git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/leak" \
           -b feat/leak origin/main 2>/dev/null; then
        pass "B7-converges" "re-add after sweep succeeds (setup converges, no leak collision)"
    else
        fail "B7-converges" "re-add after sweep failed — leaked worktree/branch blocked convergence"
    fi
fi

# ===========================================================================
# Summary
# ===========================================================================
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Issue #840 — worktree setup is leak-proof, self-healing, and data-safe."
exit 0
