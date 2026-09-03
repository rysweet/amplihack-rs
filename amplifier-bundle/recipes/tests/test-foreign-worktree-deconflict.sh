#!/usr/bin/env bash
# test-foreign-worktree-deconflict.sh — TDD spec for the foreign-worktree
# branch-deconfliction fix (concurrency bug in workflow-worktree.yaml).
#
# ROOT CAUSE (issue #200 family / concurrency): in step-04-setup-worktree's
# State-2 idempotency path (branch exists, worktree missing at THIS recipe's
# WORKTREE_PATH, COMMITS_AHEAD>0) the script runs `git branch -D "${BRANCH_NAME}"`
# (~L334). When that same BRANCH_NAME is currently checked out by a DIFFERENT,
# concurrently-running recipe's worktree (a FOREIGN worktree at another path),
# `git branch -D` fails with "cannot delete branch used by worktree" and the
# whole recipe aborts. WORKTREE_EXISTS (~L308, `grep -Fx`) only matches this
# recipe's EXACT path, so a foreign worktree owning the branch elsewhere is
# invisible to the state machine.
#
# FIX (approved design): a self-contained, NON-DESTRUCTIVE shell brick
# `amplifier-bundle/tools/workflow_worktree_deconflict.sh` parses
# `git worktree list --porcelain`, detects when the candidate branch is owned by
# a FOREIGN worktree (checked out at a normalized path != the intended one), and
# returns a fresh, provably-free branch name via a bounded (<=5) retry loop.
# step-04-setup-worktree invokes it best-effort (graceful no-op if absent, #829/
# #840 precedent) BEFORE the idempotency state machine, reassigning
# BRANCH_NAME/WORKTREE_PATH so the deconflicted name flows into the JSON output.
# The consensus recipe (`consensus-issue-worktree.yaml` step3-setup-worktree)
# carries the equivalent guidance in its worktree-manager prompt (verified by
# static parity, since recipe-runner does not execute prompt bash).
#
# Helper CLI contract (defined here, TDD-first):
#   workflow_worktree_deconflict.sh resolve <candidate_branch> <intended_worktree_path> [repo_path]
#     stdout: exactly one line — the resolved branch (unchanged, or deconflicted)
#     stderr: human diagnostics
#     exit 0: a usable branch name was resolved
#     exit 1: bad args, or all bounded retries (<=5) exhausted without a free name
# Env knob:
#   AMPLIHACK_DECONFLICT_MAX_RETRIES  upper bound on suffix retries; validated
#                                     ^[0-9]+$ AND clamped to a hard ceiling of 5
#                                     (override may only LOWER the bound).
#
# This test SHOULD FAIL before the fix lands (helper missing, YAML not wired,
# consensus not mirrored) and MUST PASS once helper + step-04 call-site +
# consensus mirror exist.
#
# Usage: bash amplifier-bundle/recipes/tests/test-foreign-worktree-deconflict.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"

WORKTREE_YAML="${RECIPES}/workflow-worktree.yaml"
CONSENSUS_YAML="${RECIPES}/consensus-issue-worktree.yaml"
HELPER="${TOOLS}/workflow_worktree_deconflict.sh"

PASS_COUNT=0
FAIL_COUNT=0

pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

if [[ ! -f "${WORKTREE_YAML}" ]]; then
    echo "HARNESS-ERROR: required recipe not found: ${WORKTREE_YAML}" >&2
    exit 2
fi
if [[ ! -f "${CONSENSUS_YAML}" ]]; then
    echo "HARNESS-ERROR: required recipe not found: ${CONSENSUS_YAML}" >&2
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

echo "=== Foreign-worktree branch deconfliction (concurrency-safety) ==="

STEP04="$(extract_step "${WORKTREE_YAML}" "step-04-setup-worktree")"

# ===========================================================================
# Part A — Static / contract checks (helper + YAML integration + parity).
# ===========================================================================

# A1: helper exists.
if [[ -f "${HELPER}" ]]; then
    pass "A1-exists" "deconflict helper present at tools/workflow_worktree_deconflict.sh"
else
    fail "A1-exists" "missing helper: ${HELPER}"
fi

# A2: helper hardened — set -euo pipefail.
if [[ -f "${HELPER}" ]] && grep -qE 'set -euo pipefail' "${HELPER}"; then
    pass "A2-strict" "helper uses 'set -euo pipefail'"
else
    fail "A2-strict" "helper missing 'set -euo pipefail'"
fi

# A3: helper advertises the 'resolve' operation.
if [[ -f "${HELPER}" ]] && grep -qE '\bresolve\b' "${HELPER}"; then
    pass "A3-resolve" "helper implements the 'resolve' operation"
else
    fail "A3-resolve" "helper does not implement a 'resolve' operation"
fi

# A4: ownership detection parses worktrees via porcelain (never fragile scraping).
if [[ -f "${HELPER}" ]] && grep -qE 'git worktree list --porcelain' "${HELPER}"; then
    pass "A4-porcelain" "helper enumerates worktrees via 'git worktree list --porcelain'"
else
    fail "A4-porcelain" "helper does not use 'git worktree list --porcelain'"
fi

# A5: every candidate is validated with the authoritative git ref checker.
if [[ -f "${HELPER}" ]] && grep -qE 'git check-ref-format' "${HELPER}"; then
    pass "A5-checkref" "helper validates candidates via 'git check-ref-format'"
else
    fail "A5-checkref" "helper does not validate candidates via 'git check-ref-format'"
fi

# A6: HARD non-destructive invariant — the helper must contain ZERO destructive
# commands. It renames THIS run's branch; it must never delete/reset/force the
# foreign run's branch or worktree.
if [[ -f "${HELPER}" ]]; then
    if grep -qE 'git[[:space:]]+branch[[:space:]]+-[dD]' "${HELPER}" \
       || grep -qE 'git[[:space:]]+worktree[[:space:]]+remove' "${HELPER}" \
       || grep -qE 'reset[[:space:]]+--hard' "${HELPER}" \
       || grep -qE 'checkout[[:space:]]+-f' "${HELPER}" \
       || grep -qE 'push[[:space:]].*(--force|--delete|-f\b)' "${HELPER}" \
       || grep -qE 'rm[[:space:]]+-[rRfial]*[rf]' "${HELPER}"; then
        fail "A6-nondestructive" "helper contains a DESTRUCTIVE command (must be read-only introspection)"
    else
        pass "A6-nondestructive" "helper contains no destructive git/fs commands (non-destructive invariant holds)"
    fi
else
    fail "A6-nondestructive" "helper missing — cannot verify non-destructive invariant"
fi

# A7: bounded retry — the helper honors AMPLIHACK_DECONFLICT_MAX_RETRIES and
# encodes a hard ceiling of 5 (DoS bound; override may only lower it).
if [[ -f "${HELPER}" ]] && grep -qE 'AMPLIHACK_DECONFLICT_MAX_RETRIES' "${HELPER}" \
   && grep -qE '\b5\b' "${HELPER}"; then
    pass "A7-bounded" "helper honors AMPLIHACK_DECONFLICT_MAX_RETRIES with a hard ceiling of 5"
else
    fail "A7-bounded" "helper missing bounded-retry knob / hard ceiling of 5"
fi

# A8: chosen name is provably free on ALL THREE axes — local ref, remote ref,
# and not checked out by ANY worktree.
if [[ -f "${HELPER}" ]] \
   && grep -qE 'refs/heads' "${HELPER}" \
   && grep -qE 'refs/remotes/origin' "${HELPER}"; then
    pass "A8-freshness" "helper checks local + remote refs (freshness axes referenced)"
else
    fail "A8-freshness" "helper does not check both refs/heads and refs/remotes/origin"
fi

# A9: helper normalizes paths (pwd -P) so a symlinked/'..'-laden intended path
# cannot masquerade as same-path or foreign.
if [[ -f "${HELPER}" ]] && grep -qE 'pwd -P' "${HELPER}"; then
    pass "A9-normalize" "helper normalizes paths via 'pwd -P' before comparison"
else
    fail "A9-normalize" "helper does not normalize paths via 'pwd -P'"
fi

# A10: helper shellcheck-clean (only when shellcheck is installed).
if command -v shellcheck >/dev/null 2>&1; then
    if [[ -f "${HELPER}" ]] && shellcheck -S warning "${HELPER}" >/dev/null 2>&1; then
        pass "A10-shellcheck" "helper passes shellcheck -S warning"
    else
        fail "A10-shellcheck" "helper fails shellcheck (or is missing)"
    fi
else
    echo "  SKIP[A10-shellcheck]: shellcheck not installed"
fi

# A11: step-04-setup-worktree invokes the deconflict helper.
if [[ -z "${STEP04}" ]]; then
    fail "A11-invoke" "could not extract step-04-setup-worktree from workflow-worktree.yaml"
elif printf '%s\n' "${STEP04}" | grep -qE 'workflow_worktree_deconflict\.sh'; then
    pass "A11-invoke" "step-04 invokes workflow_worktree_deconflict.sh"
else
    fail "A11-invoke" "step-04 does not invoke workflow_worktree_deconflict.sh"
fi

# A12: the inline call is best-effort (graceful no-op): guarded by a file-
# existence test and/or `|| true`/`|| echo` so a missing helper never aborts.
if printf '%s\n' "${STEP04}" | grep -E 'workflow_worktree_deconflict\.sh' \
       | grep -qE '(\|\||-f )'; then
    pass "A12-best-effort" "step-04 deconflict call is best-effort (guarded / '|| ...')"
else
    fail "A12-best-effort" "step-04 deconflict call is not guarded best-effort"
fi

# A13: CALL-SITE ORDERING — the deconflict invocation must run BEFORE the
# idempotency state machine (BRANCH_EXISTS probe) and BEFORE the State-2
# `git branch -D`. This is the core of the fix: detect foreign ownership before
# any destructive/reuse decision.
if [[ -n "${STEP04}" ]]; then
    # One awk per position, reading the whole step body. The
    # `head`-terminated pipelines these replace left `printf` writing into a
    # closed pipe once ${STEP04} outgrew the pipe buffer; under `pipefail` every
    # position then collapsed to "" and this ordering check — the core of the
    # #1391 fix — silently became vacuous (issue #1434).
    DECON_LINE="$(awk 'n == 0 && /workflow_worktree_deconflict\.sh/ { print FNR; n = 1 }' <<<"${STEP04}")"
    BEXISTS_LINE="$(awk 'n == 0 && /^[[:space:]]*BRANCH_EXISTS=/ { print FNR; n = 1 }' <<<"${STEP04}")"
    BRANCHD_LINE="$(awk 'n == 0 && /git branch -D/ { print FNR; n = 1 }' <<<"${STEP04}")"
    if [[ -n "${DECON_LINE}" && -n "${BEXISTS_LINE}" && "${DECON_LINE}" -lt "${BEXISTS_LINE}" ]] \
       && { [[ -z "${BRANCHD_LINE}" ]] || [[ "${DECON_LINE}" -lt "${BRANCHD_LINE}" ]]; }; then
        pass "A13-ordering" "deconflict runs before BRANCH_EXISTS probe and State-2 'git branch -D'"
    else
        fail "A13-ordering" "deconflict is not positioned before the state machine (decon=${DECON_LINE:-none}, branch_exists=${BEXISTS_LINE:-none}, branch-D=${BRANCHD_LINE:-none})"
    fi
else
    fail "A13-ordering" "step-04 not extracted — cannot verify call-site ordering"
fi

# A14: deconflicted name reassigns BRANCH_NAME so it flows into the JSON output.
if printf '%s\n' "${STEP04}" | grep -qE 'BRANCH_NAME=.*RESOLVED|BRANCH_NAME="\$RESOLVED_BRANCH"|RESOLVED_BRANCH'; then
    pass "A14-surfaced" "step-04 reassigns BRANCH_NAME from the resolved value (surfaces in JSON output)"
else
    fail "A14-surfaced" "step-04 does not reassign BRANCH_NAME from the deconflict result"
fi

# A15: HARD CONSTRAINT — workflow-worktree.yaml strictly < 400 lines (brick budget).
YAML_LINES=$(wc -l < "${WORKTREE_YAML}")
if [[ "${YAML_LINES}" -lt 400 ]]; then
    pass "A15-400" "workflow-worktree.yaml is ${YAML_LINES} lines (< 400)"
else
    fail "A15-400" "workflow-worktree.yaml is ${YAML_LINES} lines (>= 400 — brick limit breached)"
fi

# A16: CONSENSUS PARITY — the consensus recipe's step3-setup-worktree prompt
# carries the equivalent deconfliction guidance (mirror in the same commit).
STEP3="$(extract_step "${CONSENSUS_YAML}" "step3-setup-worktree")"
if [[ -z "${STEP3}" ]]; then
    fail "A16-parity" "could not extract step3-setup-worktree from consensus-issue-worktree.yaml"
elif printf '%s\n' "${STEP3}" | grep -qE 'workflow_worktree_deconflict\.sh' \
     && printf '%s\n' "${STEP3}" | grep -qiE 'foreign|deconflict'; then
    pass "A16-parity" "consensus step3 prompt mirrors the deconfliction guidance"
else
    fail "A16-parity" "consensus step3 prompt is missing the deconfliction mirror"
fi

# A17: FAIL-LOUD CALL-SITE (review S1+S2) — the deconflict call must branch on
# the helper's exit code and abort ('deconfliction failed' + exit 1) instead of
# silently falling back to the CONFLICTING name via '2>/dev/null || echo "$BRANCH_NAME"'.
A17_OK=1
if ! printf '%s\n' "${STEP04}" | grep -qE 'deconfliction failed'; then
    A17_OK=0
fi
if printf '%s\n' "${STEP04}" | grep -qE '\|\|[[:space:]]*echo[[:space:]]+"\$\{?BRANCH_NAME\}?"'; then
    A17_OK=0
fi
if [[ -n "${STEP3}" ]]; then
    if ! printf '%s\n' "${STEP3}" | grep -qE 'deconfliction failed'; then
        A17_OK=0
    fi
    if printf '%s\n' "${STEP3}" | grep -qE '\|\|[[:space:]]*echo[[:space:]]+"\$\{?BRANCH_NAME\}?"'; then
        A17_OK=0
    fi
fi
if [[ "${A17_OK}" -eq 1 ]]; then
    pass "A17-fail-loud" "both call-sites branch on helper exit code and fail loud (no silent fallback to the conflicting name)"
else
    fail "A17-fail-loud" "a call-site still silences the helper and/or falls back to the conflicting BRANCH_NAME on failure"
fi

# ===========================================================================
# Part B — Behavioral checks against real temp git repos.
# These exercise the helper's runtime contract. They require the helper to
# exist; before the fix lands they fail at the guard below (expected, TDD).
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

# resolve <candidate> <intended_path> <repo> -> prints stdout (branch) only.
resolve() {
    bash "${HELPER}" resolve "$1" "$2" "$3" 2>/dev/null
}

# branch_checked_out_anywhere <repo> <branch> -> 0 if some worktree owns it.
branch_checked_out_anywhere() {
    git -C "$1" worktree list --porcelain \
        | grep -qxF "branch refs/heads/$2"
}

if [[ ! -f "${HELPER}" ]]; then
    echo ""
    echo "--- Scenario checks SKIPPED: helper not yet implemented (TDD red) ---"
    for s in B1-badargs B2-absent B3-same-path B4-normalized-same B5-leftover-branch \
             B6-foreign-new B7-foreign-nondestructive B8-provably-free B9-converges \
             B10-length-cap; do
        fail "${s}" "scenario requires ${HELPER} (not implemented)"
    done
else
    # --- B1: bad args → exit 1 (contract). ---
    REPO="$(build_repo b1)"
    if bash "${HELPER}" resolve "feat/x" >/dev/null 2>&1; then
        fail "B1-badargs" "missing intended-path arg should exit non-zero"
    else
        pass "B1-badargs" "bad/insufficient args exit non-zero"
    fi

    # --- B2: branch ABSENT → candidate returned unchanged, exit 0. ---
    REPO="$(build_repo b2)"
    OUT="$(resolve "feat/issue-1-brand-new" "${REPO}/worktrees/feat/issue-1-brand-new" "${REPO}")"
    if [[ "${OUT}" == "feat/issue-1-brand-new" ]]; then
        pass "B2-absent" "absent branch returns candidate unchanged"
    else
        fail "B2-absent" "absent branch should return unchanged, got '${OUT}'"
    fi

    # --- B3: SAME-PATH resume → unchanged (State-1 reuse preserved). ---
    REPO="$(build_repo b3)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/mine" -b feat/mine origin/main
    OUT="$(resolve "feat/mine" "${REPO}/worktrees/feat/mine" "${REPO}")"
    if [[ "${OUT}" == "feat/mine" ]]; then
        pass "B3-same-path" "branch checked out at intended path is same-path resume (unchanged)"
    else
        fail "B3-same-path" "same-path resume should return unchanged, got '${OUT}'"
    fi

    # --- B4: NORMALIZED same-path — non-canonical intended path (./, ..) that
    #         resolves to the checkout path is still same-path (unchanged). ---
    REPO="$(build_repo b4)"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/feat/norm" -b feat/norm origin/main
    OUT="$(resolve "feat/norm" "${REPO}/worktrees/feat/./norm" "${REPO}")"
    if [[ "${OUT}" == "feat/norm" ]]; then
        pass "B4-normalized-same" "non-canonical intended path normalizes to same-path (unchanged)"
    else
        fail "B4-normalized-same" "normalized same-path should return unchanged, got '${OUT}'"
    fi

    # --- B5: branch EXISTS but NO worktree owns it → unchanged (normal State-2;
    #         this recipe may safely manage its own leftover branch). ---
    REPO="$(build_repo b5)"
    git -C "${REPO}" branch feat/leftover origin/main
    OUT="$(resolve "feat/leftover" "${REPO}/worktrees/feat/leftover" "${REPO}")"
    if [[ "${OUT}" == "feat/leftover" ]]; then
        pass "B5-leftover-branch" "branch with no owning worktree returns unchanged (State-2 preserved)"
    else
        fail "B5-leftover-branch" "unowned leftover branch should return unchanged, got '${OUT}'"
    fi

    # --- B6: FOREIGN ownership → a NEW, distinct, valid branch is returned. ---
    #     Reproduce the exact bug: BRANCH is checked out by a FOREIGN worktree
    #     at a DIFFERENT path than this recipe's intended path, and the foreign
    #     branch is AHEAD of base (the State-2 `git branch -D` trigger).
    REPO="$(build_repo b6)"
    FOREIGN_WT="${REPO}/worktrees/foreign-holder"
    INTENDED="${REPO}/worktrees/feat/issue-200-fix-bug"
    git -C "${REPO}" worktree add -q "${FOREIGN_WT}" -b feat/issue-200-fix-bug origin/main
    echo "foreign work" > "${FOREIGN_WT}/w.txt"
    git -C "${FOREIGN_WT}" add w.txt
    git -C "${FOREIGN_WT}" commit -q -m "foreign unique commit"
    FOREIGN_SHA="$(git -C "${FOREIGN_WT}" rev-parse HEAD)"
    NEW_BRANCH="$(resolve "feat/issue-200-fix-bug" "${INTENDED}" "${REPO}")"
    if [[ -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "feat/issue-200-fix-bug" ]] \
       && git -C "${REPO}" check-ref-format --branch "${NEW_BRANCH}" >/dev/null 2>&1; then
        pass "B6-foreign-new" "foreign-owned branch deconflicts to a NEW valid branch ('${NEW_BRANCH}')"
    else
        fail "B6-foreign-new" "foreign ownership should yield a new valid branch, got '${NEW_BRANCH}'"
    fi

    # --- B7: NON-DESTRUCTIVE — resolving a foreign branch must leave the foreign
    #         branch, its worktree, and its unique commit fully intact. The bug's
    #         `git branch -D` on the foreign branch must NEVER happen. ---
    if [[ -d "${FOREIGN_WT}" ]] \
       && [[ -n "$(git -C "${REPO}" branch --list feat/issue-200-fix-bug)" ]] \
       && git -C "${REPO}" cat-file -e "${FOREIGN_SHA}^{commit}" 2>/dev/null \
       && branch_checked_out_anywhere "${REPO}" "feat/issue-200-fix-bug"; then
        pass "B7-foreign-nondestructive" "foreign branch + worktree + commit preserved (no 'git branch -D')"
    else
        fail "B7-foreign-nondestructive" "foreign branch/worktree/commit was disturbed by resolve (DATA LOSS)"
    fi

    # --- B8: the deconflicted name is PROVABLY FREE on all three axes. ---
    if [[ -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "feat/issue-200-fix-bug" ]]; then
        free=yes
        git -C "${REPO}" show-ref --verify --quiet "refs/heads/${NEW_BRANCH}" && free=no
        git -C "${REPO}" show-ref --verify --quiet "refs/remotes/origin/${NEW_BRANCH}" && free=no
        branch_checked_out_anywhere "${REPO}" "${NEW_BRANCH}" && free=no
        if [[ "${free}" == "yes" ]]; then
            pass "B8-provably-free" "deconflicted name has no local ref, no remote ref, no worktree"
        else
            fail "B8-provably-free" "deconflicted name '${NEW_BRANCH}' is not free on some axis"
        fi
    else
        fail "B8-provably-free" "no deconflicted name produced to verify freshness"
    fi

    # --- B9: CONVERGENCE — the recipe's normal create path works on the new
    #         name. Recompute WORKTREE_PATH and `git worktree add` at it, exactly
    #         as step-04 would. It must succeed with NO collision and NO need to
    #         touch the foreign branch. ---
    if [[ -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "feat/issue-200-fix-bug" ]]; then
        NEW_PATH="${REPO}/worktrees/${NEW_BRANCH}"
        if git -C "${REPO}" worktree add -q "${NEW_PATH}" -b "${NEW_BRANCH}" origin/main 2>/dev/null \
           && [[ -d "${NEW_PATH}" ]] \
           && [[ -d "${FOREIGN_WT}" ]]; then
            pass "B9-converges" "create path on deconflicted branch succeeds; foreign run untouched"
        else
            fail "B9-converges" "create path on deconflicted branch failed (no convergence)"
        fi
    else
        fail "B9-converges" "no deconflicted name produced to exercise the create path"
    fi

    # --- B10: LENGTH CAP on the rename path (review S4). A very long foreign
    #          candidate must deconflict to a name bounded by DECONFLICT_MAX_BRANCH_LEN
    #          (80) while staying valid and distinct — the suffix is preserved, the
    #          base is truncated. ---
    REPO="$(build_repo b10)"
    LONG_SLUG="$(printf 'a%.0s' $(seq 1 70))"
    LONG_BRANCH="feat/issue-200-${LONG_SLUG}"
    git -C "${REPO}" worktree add -q "${REPO}/worktrees/foreign-long" -b "${LONG_BRANCH}" origin/main
    LONG_OUT="$(resolve "${LONG_BRANCH}" "${REPO}/worktrees/feat/issue-200-mine" "${REPO}")"
    if [[ -n "${LONG_OUT}" && "${LONG_OUT}" != "${LONG_BRANCH}" ]] \
       && [[ "${#LONG_OUT}" -le 80 ]] \
       && git -C "${REPO}" check-ref-format --branch "${LONG_OUT}" >/dev/null 2>&1; then
        pass "B10-length-cap" "long foreign candidate deconflicts to a bounded (<=80) valid name (len=${#LONG_OUT})"
    else
        fail "B10-length-cap" "rename path did not cap length or produced invalid name: '${LONG_OUT}' (len=${#LONG_OUT})"
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

echo "PASS: foreign-worktree deconfliction is non-destructive, bounded, and concurrency-safe."
exit 0
