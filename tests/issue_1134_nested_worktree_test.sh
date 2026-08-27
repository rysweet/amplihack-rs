#!/usr/bin/env bash
# issue_1134_nested_worktree_test.sh — regression spec for issue #1134.
#
# BUG: `step-04-setup-worktree` (amplifier-bundle/recipes/workflow-worktree.yaml)
# anchored its worktree base to the recipe run's own working directory:
#
#     cd "$REPO_PATH"                       # repo_path is typically "."
#     REPO_PATH="$(pwd -P)"                 # -> the CALLER's work tree
#     WORKTREE_PATH="${REPO_PATH}/worktrees/${BRANCH_NAME}"
#
# Nesting recipe RUNNERS is legitimate and expected, and a nested run's cwd is
# normally a LINKED worktree. `pwd -P` (like `git rev-parse --show-toplevel`)
# answers "which work tree am I standing in?", so the new worktree was created
# INSIDE the parent's worktree — repeatedly, three levels deep in the incident.
#
# FIX: anchor at the MAIN repository. A linked worktree's `.git` is a FILE
# ("gitdir: <main>/.git/worktrees/<name>"), and `git rev-parse --git-common-dir`
# follows it back to the main repo's `.git`, identical for every worktree of the
# repo. tools/workflow_worktree_root.sh turns that into the main work tree and
# also enforces the invariant "never create a worktree inside another worktree".
#
# This test builds REAL git repositories and REAL worktrees in a temp dir and
# executes the REAL extracted step-04 body. It fails on the pre-fix recipe.
#
# Usage: bash tests/issue_1134_nested_worktree_test.sh
# Exit codes: 0 = pass, 1 = fail, 2 = harness error.

set -euo pipefail

# --- Hermetic isolation -----------------------------------------------------
# Never let an ambient git environment (or the real $HOME) be touched: this
# repository has had tests escape into $HOME before.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE || true

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
WORKTREE_YAML="${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml"
ROOT_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_root.sh"

[[ -f "${WORKTREE_YAML}" ]] || { echo "HARNESS-ERROR: missing ${WORKTREE_YAML}" >&2; exit 2; }

TEST_TMP="$(mktemp -d)"
export HOME="${TEST_TMP}/home"
mkdir -p "${HOME}"

# Remove every worktree this test registered, then the whole scratch tree. All
# repos live under TEST_TMP, so nothing outside it can be affected.
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

# --- Extract the REAL step-04 bash body from the recipe ---------------------
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

# --- Fixture ----------------------------------------------------------------
# build_repo <name> -> path to a fresh clone with an origin and a main commit.
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

# run_step <cwd> <issue> <task> -> RUN_RC / RUN_JSON / RUN_ERR.
# `env -i` plus the explicit unsets above guarantee no ambient GIT_* leaks in.
run_step() {
    local cwd="$1" issue="$2" task="$3"
    shift 3
    local extra=("$@")
    set +e
    RUN_JSON="$(
        cd "${cwd}" && env -i \
            PATH="${PATH}" \
            HOME="${HOME}" \
            AMPLIHACK_HOME="${REPO_ROOT}" \
            AMPLIHACK_RUNTIME_ROOT="${TEST_TMP}/runtime" \
            REPO_PATH="." \
            TASK_DESCRIPTION="${task}" \
            BRANCH_PREFIX="fix" \
            ISSUE_NUMBER="${issue}" \
            "${extra[@]}" \
            bash "${STEP_SCRIPT}" 2>"${TEST_TMP}/step.err"
    )"
    RUN_RC=$?
    set -e
    RUN_ERR="$(cat "${TEST_TMP}/step.err")"
}

json_field() { printf '%s' "$1" | sed -n "s/.*\"$2\": *\"\([^\"]*\)\".*/\1/p"; }

# nested_worktrees <repo> -> prints every registered worktree that is a strict
# descendant of another LINKED worktree. The MAIN work tree (always the first
# porcelain entry) is excluded as a container: it legitimately owns worktrees/.
# Empty output == the #1134 invariant holds.
nested_worktrees() {
    git -C "$1" worktree list --porcelain 2>/dev/null | awk '$1=="worktree"{print $2}' \
      | awk '
        NR == 1 { main = $0; next }
        { paths[++n] = $0 }
        END {
            for (i = 1; i <= n; i++)
                for (j = 1; j <= n; j++)
                    if (i != j && index(paths[i], paths[j] "/") == 1)
                        print paths[i] " nested-in " paths[j]
        }'
}

echo "=== Issue #1134: worktrees must never be created inside worktrees ==="

# ===========================================================================
# Part A — the helper's root resolution (unit level, real worktrees).
# ===========================================================================
MAIN="$(build_repo a)"
git -C "${MAIN}" worktree add -q "${MAIN}/worktrees/feat/parent" -b feat/parent main
LINKED="${MAIN}/worktrees/feat/parent"

# Sanity: a linked worktree's .git is a FILE, and --show-toplevel points at the
# linked worktree — the very resolution that caused the bug.
if [[ -f "${LINKED}/.git" && ! -d "${LINKED}/.git" ]]; then
    pass "A0-gitfile" "linked worktree .git is a file, not a directory"
else
    fail "A0-gitfile" "expected ${LINKED}/.git to be a file"
fi

TOPLEVEL="$(cd "${LINKED}" && git rev-parse --show-toplevel)"
if [[ "${TOPLEVEL}" == "$(cd "${LINKED}" && pwd -P)" ]]; then
    pass "A1-toplevel" "--show-toplevel resolves to the LINKED worktree (the wrong anchor)"
else
    fail "A1-toplevel" "unexpected --show-toplevel: ${TOPLEVEL}"
fi

if [[ -f "${ROOT_HELPER}" ]]; then
    HELPER_ROOT="$(bash "${ROOT_HELPER}" root "${LINKED}")"
    if [[ "${HELPER_ROOT}" == "$(cd "${MAIN}" && pwd -P)" ]]; then
        pass "A2-helper-root" "helper resolves the MAIN repo (${HELPER_ROOT}) from a linked worktree"
    else
        fail "A2-helper-root" "helper returned '${HELPER_ROOT}', expected '$(cd "${MAIN}" && pwd -P)'"
    fi

    if (cd "${MAIN}" && bash "${ROOT_HELPER}" assert-not-nested "${MAIN}/worktrees/fix/ok" "${MAIN}") >/dev/null 2>&1; then
        pass "A3-invariant-ok" "assert-not-nested accepts a path anchored at the main repo"
    else
        fail "A3-invariant-ok" "assert-not-nested wrongly rejected a main-anchored path"
    fi

    if (cd "${MAIN}" && bash "${ROOT_HELPER}" assert-not-nested "${LINKED}/worktrees/fix/bad" "${MAIN}") >/dev/null 2>&1; then
        fail "A4-invariant-reject" "assert-not-nested accepted a path nested inside a linked worktree"
    else
        pass "A4-invariant-reject" "assert-not-nested refuses a path nested inside a linked worktree"
    fi
else
    fail "A2-helper-root" "tools/workflow_worktree_root.sh is missing"
    fail "A3-invariant-ok" "tools/workflow_worktree_root.sh is missing"
    fail "A4-invariant-reject" "tools/workflow_worktree_root.sh is missing"
fi

# ===========================================================================
# Part B — the REAL step-04 body, executed from inside a linked worktree.
# This is the faithful reproduction of the incident.
# ===========================================================================
run_step "${LINKED}" 1134 "child task"
CHILD_WT="$(json_field "${RUN_JSON}" worktree_path)"

if [[ "${RUN_RC}" -eq 0 ]]; then
    pass "B1-exit" "step-04 succeeded when run from inside a linked worktree"
else
    fail "B1-exit" "step-04 exited ${RUN_RC} from a linked worktree; stderr:\n${RUN_ERR}"
fi

MAIN_REAL="$(cd "${MAIN}" && pwd -P)"
LINKED_REAL="$(cd "${LINKED}" && pwd -P)"

# Exact path, not a prefix: a nested path such as
# <main>/worktrees/feat/parent/worktrees/fix/... also starts with
# "${MAIN_REAL}/worktrees/", so a prefix test would pass on the buggy recipe.
EXPECT_CHILD="${MAIN_REAL}/worktrees/fix/issue-1134-child-task"
if [[ "${CHILD_WT}" == "${EXPECT_CHILD}" ]]; then
    pass "B2-anchored" "child worktree anchored at the MAIN repo: ${CHILD_WT}"
else
    fail "B2-anchored" "child worktree '${CHILD_WT}' != expected '${EXPECT_CHILD}'"
fi

if [[ -n "${CHILD_WT}" && "${CHILD_WT}" == "${LINKED_REAL}/"* ]]; then
    fail "B3-not-nested" "child worktree '${CHILD_WT}' was created INSIDE the parent worktree (#1134)"
else
    pass "B3-not-nested" "child worktree is not inside the parent worktree"
fi

if [[ -d "${CHILD_WT}" ]]; then
    pass "B4-ondisk" "child worktree exists on disk"
else
    fail "B4-ondisk" "child worktree '${CHILD_WT}' does not exist on disk"
fi

# ===========================================================================
# Part C — three levels deep (4991 -> 4993 -> 4994 in the incident).
# ===========================================================================
if [[ -d "${CHILD_WT}" ]]; then
    run_step "${CHILD_WT}" 1135 "grandchild task"
    GRAND_WT="$(json_field "${RUN_JSON}" worktree_path)"
    EXPECT_GRAND="${MAIN_REAL}/worktrees/fix/issue-1135-grandchild-task"
    if [[ "${RUN_RC}" -eq 0 && "${GRAND_WT}" == "${EXPECT_GRAND}" ]]; then
        pass "C1-grandchild" "grandchild worktree also anchored at the MAIN repo: ${GRAND_WT}"
    else
        fail "C1-grandchild" "rc=${RUN_RC} grandchild='${GRAND_WT}'; stderr:\n${RUN_ERR}"
    fi
else
    fail "C1-grandchild" "no child worktree to recurse from"
fi

NESTED="$(nested_worktrees "${MAIN}")"
if [[ -z "${NESTED}" ]]; then
    pass "C2-zero-nested" "git worktree list reports ZERO worktrees nested inside another worktree"
else
    fail "C2-zero-nested" "nested worktrees present:\n${NESTED}"
fi

# ===========================================================================
# Part D — no regressions in related behaviour.
# ===========================================================================

# D1: run from the MAIN checkout — unchanged layout (MAIN/worktrees/<branch>).
MAIN2="$(build_repo d)"
run_step "${MAIN2}" 1136 "plain task"
PLAIN_WT="$(json_field "${RUN_JSON}" worktree_path)"
MAIN2_REAL="$(cd "${MAIN2}" && pwd -P)"
if [[ "${RUN_RC}" -eq 0 && "${PLAIN_WT}" == "${MAIN2_REAL}/worktrees/fix/issue-1136-plain-task" ]]; then
    pass "D1-baseline" "non-nested run keeps the canonical <repo>/worktrees/<branch> layout"
else
    fail "D1-baseline" "rc=${RUN_RC} worktree='${PLAIN_WT}'; stderr:\n${RUN_ERR}"
fi

# D2 (#858): the caller checkout must still never be reused as a task worktree.
run_step "${MAIN2}" 1137 "existing branch task" EXISTING_BRANCH="main"
if [[ "${RUN_RC}" -ne 0 ]] && printf '%s' "${RUN_ERR}" | grep -q 'refusing to use the caller checkout'; then
    pass "D2-858" "#858 caller-checkout refusal still fires"
else
    fail "D2-858" "expected the #858 refusal; rc=${RUN_RC}; stderr:\n${RUN_ERR}"
fi

# D3 (#858/#342): targeting an existing branch from a LINKED worktree still
# anchors the new worktree at the main repo.
git -C "${MAIN2}" branch -q feat/preexisting main
run_step "${MAIN2}/worktrees/fix/issue-1136-plain-task" 1138 "reuse branch" EXISTING_BRANCH="feat/preexisting"
REUSE_WT="$(json_field "${RUN_JSON}" worktree_path)"
if [[ "${RUN_RC}" -eq 0 && "${REUSE_WT}" == "${MAIN2_REAL}/worktrees/feat/preexisting" ]]; then
    pass "D3-existing-branch" "existing-branch path anchors at the MAIN repo: ${REUSE_WT}"
else
    fail "D3-existing-branch" "rc=${RUN_RC} worktree='${REUSE_WT}'; stderr:\n${RUN_ERR}"
fi

NESTED2="$(nested_worktrees "${MAIN2}")"
if [[ -z "${NESTED2}" ]]; then
    pass "D4-zero-nested" "second fixture also reports ZERO nested worktrees"
else
    fail "D4-zero-nested" "nested worktrees present:\n${NESTED2}"
fi

# ===========================================================================
echo ""
echo "=== ${PASS_COUNT} passed, ${FAIL_COUNT} failed ==="
[[ "${FAIL_COUNT}" -eq 0 ]] || exit 1
exit 0
