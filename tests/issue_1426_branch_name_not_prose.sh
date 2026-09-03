#!/usr/bin/env bash
# issue_1426_branch_name_not_prose.sh — regression spec for issue #1426.
#
# BUG: `step-04-setup-worktree` (amplifier-bundle/recipes/workflow-worktree.yaml)
# named its branch — and therefore its worktree DIRECTORY — by slugifying the
# first fifty characters of `task_description`:
#
#     TASK_SLUG=$(printf '%s' "$TASK_DESC" | tr … | cut -c1-50 …)
#     BRANCH_NAME="${BRANCH_PREFIX}/issue-${ISSUE_NUMBER}-${TASK_SLUG}"
#
# A task whose opening line was `Repository: /Users/ryan/src/mistt-qa/ws/142/
# jamestown (GitHub mistt-repo/jamestown).` produced
#
#     feat/issue-142-repository-usersryansrcmistt-qaws142jamestown-gith
#
# — path separators stripped, cut mid-word at "gith" — created as a SECOND
# branch beside `fix/142-band-edge-previous-slice`, which the very same task had
# pinned and told the run not to branch away from. The same shape once produced
# `feat/issue-1277-skip-workflow-launch-this-agent-is-already-executi`.
#
# FIX: two properties, both asserted here against the REAL extracted step body
# running against REAL git repositories.
#
#   1. EXPLICIT BRANCH WINS. When the task NAMES its branch, that branch is the
#      branch: it is reused if it exists, created verbatim if it does not, and
#      no competing `feat/issue-N-<prose>` branch is derived alongside it.
#   2. THE DERIVED NAME IS BOUNDED AND ISSUE-BASED. Absent an explicit branch the
#      name is keyed to the issue number with a short tail cut on a WORD
#      BOUNDARY, never a filesystem path and never fifty characters of prose.
#
# Part B runs the derived-name cases with NO amplifier-bundle reachable, because
# the derived name is LOAD-BEARING and must not change with the environment. A
# first cut of this fix put it behind a bundle helper with a hash fallback; the
# phase bricks run bundle-less by design, so the name silently became a hash, a
# re-run stopped recognising the worktree its predecessor had registered, and
# test-issue-1121-relative-repo-path.sh went red on exactly that. B0 pins the
# name that test's fixture depends on, so the two can never drift apart again.
#
# Usage: bash tests/issue_1426_branch_name_not_prose.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.
# Expected before the fix: FAIL. Expected after the fix: PASS.

# This script drives commands that exit non-zero BY DESIGN — the explicit-branch
# helper reports "no branch named / not an existing ref" as exit 10 — and reports its
# own verdict through FAIL_COUNT and the final exit. So `-e` must not leak in from the
# caller: CI's wrapper shell is `bash --noprofile --norc -e -o pipefail`, and a
# reviewer may well run this file with those flags directly. Without the explicit
# `set +e` the run dies at the first intentional non-zero and reports a phantom pass
# count. Every command status here is checked explicitly.
set +e
set -uo pipefail

# --- Hermetic isolation -----------------------------------------------------
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE || true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKTREE_YAML="${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml"
BRANCH_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_branch_name.sh"

[[ -f "${WORKTREE_YAML}" ]] || { echo "HARNESS-ERROR: missing ${WORKTREE_YAML}" >&2; exit 2; }

TEST_TMP="$(mktemp -d)"
export HOME="${TEST_TMP}/home"
mkdir -p "${HOME}"

cleanup() {
    local repo
    for repo in "${TEST_TMP}"/repo-*; do
        [[ -d "${repo}/.git" ]] || continue
        git -C "${repo}" worktree list --porcelain 2>/dev/null \
            | awk '$1=="worktree"{print $2}' \
            | while IFS= read -r wt; do
                [[ "${wt}" == "${TEST_TMP}"/* ]] || continue
                [[ "${wt}" == "${repo}" ]] && continue
                git -C "${repo}" worktree remove --force -- "${wt}" >/dev/null 2>&1 || true
            done
        git -C "${repo}" worktree prune >/dev/null 2>&1 || true
    done
    rm -rf "${TEST_TMP}"
}
trap cleanup EXIT

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); printf '  PASS[%s]: %b\n' "$1" "$2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); printf '  FAIL[%s]: %b\n' "$1" "$2" >&2; }

# --- The incident's task description ----------------------------------------
INCIDENT_PREAMBLE='Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).

Fix the band edge so the previous slice is used.'

INCIDENT_TASK="${INCIDENT_PREAMBLE}

BRANCH — already created and checked out in this worktree:
    fix/142-band-edge-previous-slice
Do not create it, do not switch to it, do not branch again."

PINNED_BRANCH='fix/142-band-edge-previous-slice'

# ===========================================================================
# Part A — the helper resolves an EXPLICITLY NAMED branch, and only that.
# ===========================================================================
echo "=== Issue #1426: branch names come from the branch or the issue, never prose ==="

if [[ ! -f "${BRANCH_HELPER}" ]]; then
    fail "A0-helper" "amplifier-bundle/tools/workflow_branch_name.sh is missing — the branch-name ladder was never extracted"
else
    pass "A0-helper" "tools/workflow_branch_name.sh exists"

    # explicit_of TASK -> prints the named branch (empty when none is named).
    explicit_of() { TASK_DESCRIPTION="$1" bash "${BRANCH_HELPER}" explicit 2>/dev/null; }

    GOT="$(explicit_of "${INCIDENT_TASK}")"
    if [[ "${GOT}" == "${PINNED_BRANCH}" ]]; then
        pass "A1-incident" "the pinned branch is recovered from the task: ${GOT}"
    else
        fail "A1-incident" "expected '${PINNED_BRANCH}', got '${GOT}'"
    fi

    GOT="$(explicit_of 'Branch: `fix/9-foo`.')"
    if [[ "${GOT}" == "fix/9-foo" ]]; then
        pass "A2-inline" "an inline 'Branch: <ref>' directive is recovered and undecorated"
    else
        fail "A2-inline" "expected 'fix/9-foo', got '${GOT}'"
    fi

    # False positives matter more than coverage here: a wrong hit hijacks the run.
    GOT="$(explicit_of 'Branch protection rules: enabled')"
    if [[ -z "${GOT}" ]]; then
        pass "A3-no-false-positive" "'Branch protection rules: enabled' names no branch"
    else
        fail "A3-no-false-positive" "prose was mistaken for a branch: '${GOT}'"
    fi

    GOT="$(explicit_of 'Branch: main')"
    if [[ -z "${GOT}" ]]; then
        pass "A4-reserved" "'Branch: main' is refused — that is the base, not the work branch"
    else
        fail "A4-reserved" "expected no branch, got '${GOT}'"
    fi

    GOT="$(explicit_of 'Please use the branch fix/9-foo when you are done')"
    if [[ -z "${GOT}" ]]; then
        pass "A5-not-a-directive" "'branch' inside a sentence is not a directive"
    else
        fail "A5-not-a-directive" "mid-sentence prose was read as a directive: '${GOT}'"
    fi
fi

build_repo() {
    local name="$1"
    local remote="${TEST_TMP}/remote-${name}.git"
    local work="${TEST_TMP}/repo-${name}"
    git init --quiet --bare "${remote}"
    git clone --quiet "${remote}" "${work}" 2>/dev/null
    git -C "${work}" config user.email "t@example.com"
    git -C "${work}" config user.name "Test"
    git -C "${work}" checkout -q -b main 2>/dev/null || git -C "${work}" checkout -q main
    echo base > "${work}/README.md"
    git -C "${work}" add README.md
    git -C "${work}" commit -q -m "base commit"
    git -C "${work}" push -q -u origin main 2>/dev/null || true
    git -C "${work}" remote set-head origin main >/dev/null 2>&1 || true
    printf '%s\n' "${work}"
}

extract_step_body() {
    awk -v target="$2" '
        /^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/ {
            line = $0
            sub(/^[[:space:]]*-[[:space:]]+id:[[:space:]]*"/, "", line)
            sub(/".*$/, "", line)
            if (line == target) { inblk = 1; next }
            else if (inblk) { inblk = 0 }
        }
        inblk && /^    command:[[:space:]]*\|/ { grab = 1; next }
        grab && /^    [A-Za-z_-]+:/ { grab = 0; inblk = 0 }
        grab { line = $0; sub(/^      /, "", line); print line }
    ' "$1"
}

STEP_SCRIPT="${TEST_TMP}/step-04-body.sh"
extract_step_body "${WORKTREE_YAML}" "step-04-setup-worktree" > "${STEP_SCRIPT}"
[[ -s "${STEP_SCRIPT}" ]] || { echo "HARNESS-ERROR: step-04 body extraction produced nothing" >&2; exit 2; }

# run_step <cwd> <issue> <task> [EXTRA=VAL ...] -> RUN_RC / RUN_JSON / RUN_ERR.
# Exit status is read from $? on its own line, never through a pipeline.
run_step() {
    local cwd="$1" issue="$2" task="$3"
    shift 3
    RUN_JSON="$(
        cd "${cwd}" && env -i \
            PATH="${PATH}" \
            HOME="${HOME}" \
            AMPLIHACK_HOME="${REPO_ROOT}" \
            AMPLIHACK_RUNTIME_ROOT="${TEST_TMP}/runtime" \
            REPO_PATH="." \
            TASK_DESCRIPTION="${task}" \
            BRANCH_PREFIX="feat" \
            ISSUE_NUMBER="${issue}" \
            "$@" \
            bash "${STEP_SCRIPT}" 2>"${TEST_TMP}/step.err"
    )"
    RUN_RC=$?
    RUN_ERR="$(cat "${TEST_TMP}/step.err")"
}

json_field() { printf '%s' "$1" | sed -n "s/.*\"$2\": *\"\([^\"]*\)\".*/\1/p"; }


# ===========================================================================
# Part B — the DERIVED name, produced with NO amplifier-bundle reachable.
#
# `run_step_bare` mirrors the environment of
# amplifier-bundle/recipes/tests/test-issue-1121-relative-repo-path.sh: env -i,
# HOME repointed at the scratch dir, no AMPLIHACK_HOME, a RELATIVE repo_path.
# Nothing under amplifier-bundle/tools is reachable, so whatever these cases
# assert is what the recipe itself computes.
# ===========================================================================

# run_step_bare <cwd> <issue> <prefix> <task> -> RUN_RC / RUN_JSON / RUN_ERR.
# Exit status is read from $? on its own line, never through a pipeline.
#
# `trap '' PIPE` is deliberate. An ignored-SIGPIPE disposition SURVIVES exec and is
# common in CI, and it is what turns a pipeline stage that stops reading early into a
# plain exit 1 instead of a signal death. Running the step body this way is the
# closer approximation of the runner (see B6).
run_step_bare() {
    local cwd="$1" issue="$2" prefix="$3" task="$4"
    RUN_JSON="$(
        cd "${cwd}" && trap '' PIPE && env -i \
            PATH="${PATH}" \
            HOME="${TEST_TMP}/nobundle" \
            REPO_PATH="." \
            TASK_DESCRIPTION="${task}" \
            BRANCH_PREFIX="${prefix}" \
            ISSUE_NUMBER="${issue}" \
            bash "${STEP_SCRIPT}" 2>"${TEST_TMP}/step.err"
    )"
    RUN_RC=$?
    RUN_ERR="$(cat "${TEST_TMP}/step.err")"
}
mkdir -p "${TEST_TMP}/nobundle"

# --- B0: the exact name test-issue-1121-relative-repo-path.sh pins ----------
REPO_B0="$(build_repo b0)"
run_step_bare "${REPO_B0}" 1121 fix "reuse me"
B0_BRANCH="$(json_field "${RUN_JSON}" branch_name)"
if [[ "${RUN_RC}" -eq 0 && "${B0_BRANCH}" == "fix/issue-1121-reuse-me" ]]; then
    pass "B0-1121-fixture" "bundle-less derivation still yields fix/issue-1121-reuse-me"
else
    fail "B0-1121-fixture" "rc=${RUN_RC} branch='${B0_BRANCH}' (expected 'fix/issue-1121-reuse-me' — test-issue-1121-relative-repo-path.sh depends on this exact name)\nstderr:\n${RUN_ERR}"
fi

# --- B1: re-running the same task reuses the worktree it created ------------
run_step_bare "${REPO_B0}" 1121 fix "reuse me"
B1_BRANCH="$(json_field "${RUN_JSON}" branch_name)"
if [[ "${RUN_RC}" -eq 0 && "${B1_BRANCH}" == "${B0_BRANCH}" ]] \
   && printf '%s' "${RUN_JSON}" | grep -qE '"created":[[:space:]]*false'; then
    pass "B1-idempotent" "a second run derives the same name and REUSES the worktree (created=false)"
else
    fail "B1-idempotent" "rc=${RUN_RC} branch='${B1_BRANCH}' json=${RUN_JSON}\nstderr:\n${RUN_ERR}"
fi

# --- B2..B5: the incident's own prose, bundle-less --------------------------
REPO_B2="$(build_repo b2)"
run_step_bare "${REPO_B2}" 142 feat "${INCIDENT_PREAMBLE}"
DERIVED="$(json_field "${RUN_JSON}" branch_name)"

if [[ "${RUN_RC}" -eq 0 && "${DERIVED}" == feat/issue-142-* ]]; then
    pass "B2-issue-keyed" "derived name is keyed to the issue: ${DERIVED}"
else
    fail "B2-issue-keyed" "rc=${RUN_RC} branch='${DERIVED}'\nstderr:\n${RUN_ERR}"
fi

if [[ "${DERIVED}" != *usersryan* && "${DERIVED}" != *jamestown* && "${DERIVED}" != *mistt* ]]; then
    pass "B3-no-paths" "the filesystem path in the task did not leak into the ref"
else
    fail "B3-no-paths" "path fragments leaked into the branch name: '${DERIVED}'"
fi

TAIL="${DERIVED#feat/issue-142-}"
if (( ${#TAIL} <= 24 )) && (( ${#DERIVED} <= 72 )); then
    pass "B4-bounded" "tail is ${#TAIL} characters and the ref is ${#DERIVED} (bounds: 24 / 72)"
else
    fail "B4-bounded" "tail '${TAIL}' is ${#TAIL} characters, ref ${#DERIVED}: ${DERIVED}"
fi

# Word-boundary truncation: every hyphen-separated piece of the tail must be a
# WHOLE word of the task description, never a fragment such as "gith".
BAD_WORD=""
LOWER_TASK="$(printf '%s' "${INCIDENT_PREAMBLE}" | tr '[:upper:]' '[:lower:]' | tr -c 'a-z0-9' ' ')"
for piece in ${TAIL//-/ }; do
    FOUND=0
    for word in ${LOWER_TASK}; do
        [[ "${word}" == "${piece}" ]] && { FOUND=1; break; }
    done
    (( FOUND )) || BAD_WORD="${piece}"
done
if [[ -z "${BAD_WORD}" ]]; then
    pass "B5-word-boundary" "every piece of '${TAIL}' is a whole word of the task, not a fragment"
else
    fail "B5-word-boundary" "'${BAD_WORD}' in '${TAIL}' is a mid-word truncation, not a word of the task"
fi

# --- B6: a very long description must not produce a very long ref (#1249/#1260)
REPO_B6="$(build_repo b6)"
HUGE="$(head -c 100000 /dev/zero | tr '\0' 'a' | sed 's/.\{9\}/& /g')"
run_step_bare "${REPO_B6}" 1260 feat "prevent overlong refs ${HUGE}"
LONG_NAME="$(json_field "${RUN_JSON}" branch_name)"
if [[ "${RUN_RC}" -eq 0 && -n "${LONG_NAME}" ]] && (( ${#LONG_NAME} <= 72 )); then
    pass "B6-huge-input" "a 100 KB task description still yields a ${#LONG_NAME}-character ref: ${LONG_NAME}"
else
    fail "B6-huge-input" "rc=${RUN_RC} 100 KB task produced '${LONG_NAME}' (${#LONG_NAME} chars)\nstderr:\n${RUN_ERR}"
fi

# --- B7: nothing usable to say -> a stable hash, never an empty tail --------
REPO_B7="$(build_repo b7)"
run_step_bare "${REPO_B7}" 9 feat "@@@ /// ###"
EMPTY_NAME="$(json_field "${RUN_JSON}" branch_name)"
if [[ "${RUN_RC}" -eq 0 && "${EMPTY_NAME}" == feat/issue-9-* ]] && (( ${#EMPTY_NAME} <= 72 )); then
    pass "B7-no-words" "an all-symbol task still yields an issue-keyed ref: ${EMPTY_NAME}"
else
    fail "B7-no-words" "rc=${RUN_RC} all-symbol task produced '${EMPTY_NAME}'"
fi

# ===========================================================================
# Part C — the REAL step-04 body against REAL repositories.
# ===========================================================================

# --- C1: an explicitly pinned branch that already exists is REUSED ----------
REPO_A="$(build_repo a)"
git -C "${REPO_A}" branch -q "${PINNED_BRANCH}" main
run_step "${REPO_A}" 142 "${INCIDENT_TASK}"
C1_BRANCH="$(json_field "${RUN_JSON}" branch_name)"
C1_WT="$(json_field "${RUN_JSON}" worktree_path)"

if [[ "${RUN_RC}" -eq 0 && "${C1_BRANCH}" == "${PINNED_BRANCH}" ]]; then
    pass "C1-explicit-wins" "step-04 used the branch the task pinned: ${C1_BRANCH}"
else
    fail "C1-explicit-wins" "rc=${RUN_RC} branch='${C1_BRANCH}' (expected '${PINNED_BRANCH}')\nstderr:\n${RUN_ERR}"
fi

COMPETING="$(git -C "${REPO_A}" for-each-ref --format='%(refname:short)' 'refs/heads/feat/*')"
if [[ -z "${COMPETING}" ]]; then
    pass "C2-no-competitor" "no second branch was derived alongside the pinned one"
else
    fail "C2-no-competitor" "a competing branch was created:\n${COMPETING}"
fi

if [[ "${C1_WT}" == *"/worktrees/${PINNED_BRANCH}" ]]; then
    pass "C3-worktree-path" "worktree directory is named for the pinned branch: ${C1_WT}"
else
    fail "C3-worktree-path" "worktree path '${C1_WT}' is not named for '${PINNED_BRANCH}'"
fi

# --- C4: an explicitly named branch that does NOT exist is created verbatim --
REPO_B="$(build_repo b)"
run_step "${REPO_B}" 143 "Repository: /Users/ryan/src/mistt-qa/ws/143/jamestown (GitHub mistt-repo/jamestown).

Branch: fix/143-brand-new-branch"
C4_BRANCH="$(json_field "${RUN_JSON}" branch_name)"
if [[ "${RUN_RC}" -eq 0 && "${C4_BRANCH}" == "fix/143-brand-new-branch" ]]; then
    pass "C4-explicit-created" "a named branch that did not exist was created verbatim: ${C4_BRANCH}"
else
    fail "C4-explicit-created" "rc=${RUN_RC} branch='${C4_BRANCH}'\nstderr:\n${RUN_ERR}"
fi

# --- C5: with no branch named, the derived name is bounded and issue-based ---
REPO_C="$(build_repo c)"
run_step "${REPO_C}" 142 "${INCIDENT_PREAMBLE}"
C5_BRANCH="$(json_field "${RUN_JSON}" branch_name)"
C5_WT="$(json_field "${RUN_JSON}" worktree_path)"

if [[ "${RUN_RC}" -eq 0 && "${C5_BRANCH}" == feat/issue-142-* ]]; then
    pass "C5-issue-keyed" "derived branch is keyed to the issue: ${C5_BRANCH}"
else
    fail "C5-issue-keyed" "rc=${RUN_RC} branch='${C5_BRANCH}'\nstderr:\n${RUN_ERR}"
fi

if [[ "${C5_BRANCH}" != *usersryan* && "${C5_BRANCH}" != *jamestown* ]]; then
    pass "C6-no-path-leak" "no filesystem path leaked into the derived branch"
else
    fail "C6-no-path-leak" "derived branch carries the task's path: ${C5_BRANCH}"
fi

C5_TAIL="${C5_BRANCH#feat/issue-142-}"
if (( ${#C5_BRANCH} <= 72 )) && (( ${#C5_TAIL} <= 24 )); then
    pass "C7-bounded" "derived branch is ${#C5_BRANCH} characters with a ${#C5_TAIL}-character tail (bounds: 72 / 24)"
else
    fail "C7-bounded" "derived branch is ${#C5_BRANCH} characters with a ${#C5_TAIL}-character tail: ${C5_BRANCH}"
fi

if [[ -d "${C5_WT}" ]]; then
    pass "C8-ondisk" "derived worktree exists on disk: ${C5_WT}"
else
    fail "C8-ondisk" "derived worktree '${C5_WT}' is not on disk"
fi

# ===========================================================================
# Part D — the prose-slug pipeline is gone from the recipe for good.
# ===========================================================================
if grep -q 'cut -c1-50' "${WORKTREE_YAML}"; then
    fail "D1-no-prose-slug" "workflow-worktree.yaml still cuts 50 characters of task prose"
else
    pass "D1-no-prose-slug" "the 50-character prose slug pipeline is gone from workflow-worktree.yaml"
fi

if grep -q 'workflow_branch_name.sh' "${WORKTREE_YAML}"; then
    pass "D2-uses-helper" "workflow-worktree.yaml scans for an explicit branch via the extracted helper"
else
    fail "D2-uses-helper" "workflow-worktree.yaml does not call tools/workflow_branch_name.sh"
fi

# D3 (#1121): the DERIVED name must never come from a bundle helper. Part B proves
# the behaviour; this pins the structural reason, so the regression cannot return
# as "the helper computes it and the recipe falls back".
if grep -qE 'BRANCH_HELPER" derive|workflow_branch_name.sh" derive' "${WORKTREE_YAML}"; then
    fail "D3-derive-inline" "the derived branch name is delegated to a bundle helper — it is load-bearing and must be computed inline (#1121)"
else
    pass "D3-derive-inline" "the derived branch name is computed inline, not behind a bundle helper"
fi

# D4 (#1426 CI): the derivation must contain no pipeline stage that stops reading
# before its producer is done. `head -c` there left `printf` writing into a closed
# pipe; under `pipefail` that emptied the branch name and killed step-04 with exit 1
# on a 100 KB task — green on bash 5.3 locally, red on the runner's 5.2.21. B6 covers
# the behaviour; this covers the shape, deterministically, on every machine.
DERIVATION="$(sed -n '/^      # BRANCH NAMING/,/^      BRANCH_NAME=/p' "${WORKTREE_YAML}" | grep -v '^ *#')"
if printf '%s' "${DERIVATION}" | grep -qE '\|[[:space:]]*(head|tail)( |$)|grep -m'; then
    fail "D4-no-early-exit" "the branch-name derivation pipes into an early-exit stage (head/tail/grep -m); under pipefail that empties the name"
else
    pass "D4-no-early-exit" "the derivation has no early-exit pipeline stage (pipefail-safe)"
fi

echo ""
echo "=== ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[[ "${FAIL_COUNT}" -eq 0 ]] || exit 1
exit 0
