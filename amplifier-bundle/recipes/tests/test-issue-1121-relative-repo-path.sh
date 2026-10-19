#!/usr/bin/env bash
# test-issue-1121-relative-repo-path.sh — TDD spec for issue #1121.
#
# Issue #1121: workflow-worktree.yaml step-04-setup-worktree spuriously
# HARD-FAILS with `git worktree add ... already exists` (exit 128) — taking the
# whole recipe run to status: failure — when invoked with a RELATIVE repo_path
# (e.g. `-c repo_path="."`) and a worktree for the target branch is already
# registered and present on disk.
#
# ROOT CAUSE (two combining defects in the new-branch three-state idempotency
# guard, ~lines 287-358 of workflow-worktree.yaml):
#   A. NON-CANONICAL PATH: with repo_path=".", WORKTREE_PATH becomes
#      "./worktrees/<branch>", but `git worktree list --porcelain` emits ABSOLUTE
#      canonical paths ("/abs/repo/worktrees/<branch>"). The `grep -Fx` exact
#      match therefore NEVER matches a registered worktree, so a present+
#      registered worktree is misclassified as "missing" (State 2 instead of
#      State 1 reuse).
#   B. NO EXISTING-DIRECTORY / PRUNE GUARD in the State-2 and State-3
#      `git worktree add` branches, so the misclassification (or any leftover
#      dir on disk) becomes a hard exit-128 instead of converging.
#
# REQUIRED FIXES (both):
#   1. Canonicalize repo_path once, right after the `cd "$REPO_PATH"` +
#      is-inside-work-tree validation:  REPO_PATH="$(pwd -P)".
#   2. Route every `git worktree add` through a guarded, idempotent helper
#      (prune + reuse-if-branch-matches else remove+re-add) so setup converges
#      to exit 0 instead of exit 128. Never `git worktree add --force`.
#
# This test SHOULD FAIL before #1121 lands (scenarios exit 128; the canonicalize
# static check is absent) and MUST PASS once both fixes are in place.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1121-relative-repo-path.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RECIPES="${REPO_ROOT}/amplifier-bundle/recipes"

WORKTREE_YAML="${RECIPES}/workflow-worktree.yaml"

# Branch that the step derives from the fixed context vars below. Kept explicit
# (rather than re-deriving the slug pipeline) so the scenarios are readable.
#   BRANCH_PREFIX=fix  ISSUE_NUMBER=1121  TASK_DESCRIPTION="reuse me" -> slug "reuse-me"
TARGET_BRANCH="fix/issue-1121-reuse-me"
STEP_TASK_DESC="reuse me"
STEP_BRANCH_PREFIX="fix"
STEP_ISSUE_NUMBER="1121"

PASS_COUNT=0
FAIL_COUNT=0

pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

if [[ ! -f "${WORKTREE_YAML}" ]]; then
    echo "HARNESS-ERROR: required recipe not found: ${WORKTREE_YAML}" >&2
    exit 2
fi

# Scratch workspace; cleaned on exit. HOME is repointed here when running the
# extracted step body so the best-effort sweep/deconflict helper lookups under
# ~/.amplihack and ~/.copilot reliably miss (hermetic no-op, per #829).
TEST_TMP="$(mktemp -d)"
cleanup() { rm -rf "${TEST_TMP}"; }
trap cleanup EXIT

# ---------------------------------------------------------------------------
# extract_step <file> <step-id>
# Prints the contiguous block from `- id: "<step-id>"` up to (but excluding) the
# next top-level `  - id:` marker. Same idiom as test-issue-840.
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# extract_step_body <file> <step-id>
# Prints the executable bash body of a `command: |` block, dedented (the block
# is indented 6 spaces under the 4-space `command:` key). Stops at the next
# 4-space YAML key (e.g. `    output:`). This lets us run the REAL step logic
# against a temp repo instead of brittle string-matching runtime output.
# ---------------------------------------------------------------------------
extract_step_body() {
    local file="$1" step_id="$2"
    extract_step "${file}" "${step_id}" | awk '
        /^    command:[[:space:]]*\|/ { grab = 1; next }
        grab && /^    [A-Za-z_-]+:/  { grab = 0 }
        grab { line = $0; sub(/^      /, "", line); print line }
    '
}

# ---------------------------------------------------------------------------
# build_repo <name> -> echoes path to a fresh repo with origin + main commit.
# Mirrors test-issue-840's build_repo.
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# run_step_body <repo> -> runs the extracted step body inside <repo> with a
# RELATIVE repo_path ("."). Captures combined output in RUN_OUT and exit code
# in RUN_RC. Never aborts the harness (RC captured under set -e via `if`).
# ---------------------------------------------------------------------------
STEP_SCRIPT="${TEST_TMP}/step-04-body.sh"

run_step_body() {
    local repo="$1"
    if RUN_OUT="$(
        cd "${repo}" && env -i \
            PATH="${PATH}" \
            HOME="${TEST_TMP}" \
            REPO_PATH="." \
            TASK_DESCRIPTION="${STEP_TASK_DESC}" \
            BRANCH_PREFIX="${STEP_BRANCH_PREFIX}" \
            ISSUE_NUMBER="${STEP_ISSUE_NUMBER}" \
            bash "${STEP_SCRIPT}" 2>&1
    )"; then
        RUN_RC=0
    else
        RUN_RC=$?
    fi
}

echo "=== Issue #1121: relative repo_path must not spuriously exit-128 ==="

STEP04="$(extract_step "${WORKTREE_YAML}" "step-04-setup-worktree")"
if [[ -z "${STEP04}" ]]; then
    echo "HARNESS-ERROR: could not extract step-04-setup-worktree from ${WORKTREE_YAML}" >&2
    exit 2
fi

extract_step_body "${WORKTREE_YAML}" "step-04-setup-worktree" > "${STEP_SCRIPT}"
if [[ ! -s "${STEP_SCRIPT}" ]]; then
    echo "HARNESS-ERROR: extracted step body is empty (command block extraction failed)" >&2
    exit 2
fi

# ===========================================================================
# Part A — Static / contract checks.
# ===========================================================================

# A1: FIX #1 — step-04 canonicalizes repo_path via `pwd -P` after the cd, so the
# grep -Fx registration check matches git's absolute porcelain output.
# Here-strings, not `printf | grep -q`: grep -q exits at its first match, the
# writer then takes SIGPIPE on the large step body, and under pipefail the
# pipeline reports failure even though the pattern matched (CI flake on #1639).
if grep -qE 'REPO_PATH="\$\(pwd -P\)"' <<<"${STEP04}"; then
    pass "A1-canonicalize" "step-04 canonicalizes repo_path with pwd -P after cd"
else
    fail "A1-canonicalize" "step-04 does not canonicalize repo_path (REPO_PATH=\"\$(pwd -P)\" missing)"
fi

# A2: FIX #2 — every `git worktree add` is routed through the mandated guarded,
# idempotent helper (prune + reuse-if-branch-matches else remove+re-add). Accept
# the inline helper function or the externalized tools/ add-helper script. An
# ACTUAL executed `git worktree prune` line (not a comment/echo advising it) also
# satisfies the guard. Comment/error-string mentions of "git worktree prune"
# (issue #858 advice) must NOT count — hence the leading-whitespace/`;` anchor.
if grep -qE '(\bwt_add_idempotent\b|workflow_worktree_add|^[[:space:]]*git worktree prune\b)' <<<"${STEP04}"; then
    pass "A2-add-guard" "new-branch add path routes through a guarded idempotent-add helper"
else
    fail "A2-add-guard" "new-branch add path lacks a guarded idempotent-add helper (still bare 'git worktree add')"
fi

# A3: HARD CONSTRAINT (brick limit) — workflow-worktree.yaml strictly < 400 lines.
YAML_LINES=$(wc -l < "${WORKTREE_YAML}")
if [[ "${YAML_LINES}" -lt 400 ]]; then
    pass "A3-400" "workflow-worktree.yaml is ${YAML_LINES} lines (< 400)"
else
    fail "A3-400" "workflow-worktree.yaml is ${YAML_LINES} lines (>= 400 — brick limit breached)"
fi

# A4: guard must NOT reach for the unconditional `git worktree add --force`,
# which can silently clobber a legitimately-populated directory.
if grep -qE 'git worktree add --force' <<<"${STEP04}"; then
    fail "A4-no-force-add" "step-04 uses 'git worktree add --force' (can clobber populated dir)"
else
    pass "A4-no-force-add" "step-04 avoids unconditional 'git worktree add --force'"
fi

# ===========================================================================
# Part B — Real-repo scenarios (the faithful reproduction of #1121).
# ===========================================================================

# --- B1: registered + present worktree for the branch, run with repo_path="."
#         MUST be reused (State 1), exit 0, and never print "already exists". ---
REPO="$(build_repo s1)"
git -C "${REPO}" worktree add -q "${REPO}/worktrees/${TARGET_BRANCH}" \
    -b "${TARGET_BRANCH}" origin/main
run_step_body "${REPO}"
if [[ "${RUN_RC}" -ne 0 ]]; then
    fail "B1-exit0" "step exited ${RUN_RC} (expected 0) for a registered+present worktree with repo_path=. (this is the #1121 exit-128 bug)"
elif printf '%s\n' "${RUN_OUT}" | grep -qiF "already exists"; then
    fail "B1-exit0" "step printed 'already exists' (spurious worktree-add collision)"
else
    pass "B1-exit0" "registered+present worktree with repo_path=. exits 0, no 'already exists'"
fi
# B1b: it must be classified State 1 (reuse) -> created=false in the JSON output.
if printf '%s\n' "${RUN_OUT}" | grep -qE '"created":[[:space:]]*false'; then
    pass "B1-reuse" "registered worktree REUSED (created=false, State 1)"
else
    fail "B1-reuse" "registered worktree was NOT reused (expected \"created\": false in JSON output)"
fi

# --- B2: directory present on disk for the target branch but WITHOUT a
#         registration (stale/partial state or manually-created dir). The guard
#         must still converge to exit 0 (remove+re-add), never exit 128. ---
REPO="$(build_repo s2)"
mkdir -p "${REPO}/worktrees/${TARGET_BRANCH}"
echo "leftover from a partially-pruned prior run" \
    > "${REPO}/worktrees/${TARGET_BRANCH}/stale.txt"
run_step_body "${REPO}"
if [[ "${RUN_RC}" -ne 0 ]]; then
    fail "B2-converge" "step exited ${RUN_RC} (expected 0) for a present-but-unregistered dir (must converge, not exit 128)"
elif printf '%s\n' "${RUN_OUT}" | grep -qiF "already exists"; then
    fail "B2-converge" "step printed 'already exists' for a present-but-unregistered dir"
else
    pass "B2-converge" "present-but-unregistered dir converges to exit 0 (no 128)"
fi
# B2b: after convergence the path is a real, registered worktree on the branch.
if git -C "${REPO}" worktree list --porcelain \
       | grep -qFx "worktree ${REPO}/worktrees/${TARGET_BRANCH}"; then
    pass "B2-registered" "converged worktree is registered on ${TARGET_BRANCH}"
else
    fail "B2-registered" "converged path is not a registered worktree"
fi

# ===========================================================================
# Summary
# ===========================================================================
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"

if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi

echo "PASS: Issue #1121 — relative repo_path setup is canonical, idempotent, and exit-0 convergent."
exit 0
