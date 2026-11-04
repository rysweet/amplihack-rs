#!/usr/bin/env bash
# test-issue-1930-setup-does-not-push-ahead-branch.sh — TDD spec for issue #1930.
#
# Issue #1930: workflow-worktree.yaml step-04-setup-worktree, on the
# existing-branch path (#342: existing_branch or pr_number), ran a best-effort
# `git push origin <branch>` with its output discarded. When the local branch
# was ahead of origin, that push ran the pre-push hook on unreviewed local
# commits, and a hook failure was hidden by `|| true`.
#
# REQUIRED BEHAVIOUR: the existing-branch path only fetches, attaches the
# worktree and sets the upstream. It never pushes, so no pre-push hook runs.
# Neither the local nor the remote ref moves.
#
# Part A: static checks on the existing-branch blocks of both recipes.
# Part S: the real step-04 body run against scratch repos.
#   S1: EXISTING_BRANCH=feat/x
#   S2: PR_NUMBER=7, resolved to feat/x by a strict stub `gh`
#
# Override: WORKTREE_YAML=<regular file> runs the test against another copy of
# the recipe, e.g. origin/main's, to show the test fails there:
#   git show origin/main:amplifier-bundle/recipes/workflow-worktree.yaml \
#     > "$TMPDIR/wt-main.yaml"
#   WORKTREE_YAML="$TMPDIR/wt-main.yaml" bash <this script>   # must exit 1
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1930-setup-does-not-push-ahead-branch.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WORKTREE_YAML="${WORKTREE_YAML:-${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml}"
CONSENSUS_YAML="${CONSENSUS_YAML:-${REPO_ROOT}/amplifier-bundle/recipes/consensus-issue-worktree.yaml}"

TARGET_BRANCH="feat/x"
STUB_PR="7"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

for f in "${WORKTREE_YAML}" "${CONSENSUS_YAML}"; do
    if [[ ! -f "${f}" || -L "${f}" ]]; then
        echo "HARNESS-ERROR: recipe must be a regular file: ${f}" >&2
        exit 2
    fi
done

# --- Hermetic scratch (never /tmp): everything lives under $TMPDIR. ---------
: "${TMPDIR:?TMPDIR must be set (scratch must not land on /tmp)}"
SCRATCH="$(mktemp -d "${TMPDIR%/}/t1930.XXXXXX")"
# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() {
    case "${SCRATCH}" in
        "${TMPDIR%/}"/t1930.*) rm -rf -- "${SCRATCH}" ;;
    esac
}
trap cleanup EXIT
mkdir -p "${SCRATCH}/home" "${SCRATCH}/runtime" "${SCRATCH}/tmp"

# Isolate git from the developer's config and identity. No core.hooksPath is
# set anywhere: the hook under test lives in the clone's own .git/hooks.
export HOME="${SCRATCH}/home"
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="t@example.com"
export GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="t@example.com"

# ---------------------------------------------------------------------------
# Step extraction (same idiom as test-issue-1565 / test-issue-1121).
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

# extract_step_body <file> <step_id> [key] -> the block scalar under <key>
# (default `command`; consensus step3 is an agent step, so its script is in `prompt`).
extract_step_body() {
    extract_step "$1" "$2" | awk -v key="${3:-command}" '
        $0 ~ "^    " key ":[[:space:]]*\\|" { grab = 1; next }
        grab && /^    [A-Za-z_-]+:/  { grab = 0 }
        grab { line = $0; sub(/^      /, "", line); print line }
    '
}

# existing_branch_block <body> -> the lines of the existing-branch path, from
# the `if [ -n "$EXISTING_BRANCH" ] || [ -n "$PR_NUMBER" ]` guard to that
# path's `exit 0`.
existing_branch_block() {
    awk '
        /if \[ -n "\$EXISTING_BRANCH" \] \|\| \[ -n "\$PR_NUMBER" \]/ { grab = 1 }
        grab { print }
        grab && /^[[:space:]]*exit 0([[:space:]]|$)/ { exit }
    ' <<<"$1"
}

# ---------------------------------------------------------------------------
# Fixture.
#   origin.git: main at C0, feat/x at A (child of C0).
#   clone:      on main; local feat/x at B (child of A), not checked out in
#               any worktree, so step-04 takes the `attach` route.
#   hook:       clone/.git/hooks/pre-push writes $SCRATCH/<name>-pre-push-ran
#               and exits 1. Any push from the clone or its worktrees runs it.
# Sets FX_REPO, FX_REMOTE, FX_A, FX_B, FX_MARKER.
# ---------------------------------------------------------------------------
build_fixture() {
    local name="$1"
    FX_REMOTE="${SCRATCH}/${name}-origin.git"
    FX_REPO="${SCRATCH}/${name}"
    FX_MARKER="${SCRATCH}/${name}-pre-push-ran"
    git init --quiet --bare "${FX_REMOTE}"
    git clone --quiet "${FX_REMOTE}" "${FX_REPO}" 2>/dev/null
    git -C "${FX_REPO}" checkout -q -b main 2>/dev/null || git -C "${FX_REPO}" checkout -q main
    echo "base" > "${FX_REPO}/README.md"
    git -C "${FX_REPO}" add README.md
    git -C "${FX_REPO}" commit -q -m "C0"
    git -C "${FX_REPO}" push -q -u origin main 2>/dev/null
    git -C "${FX_REPO}" remote set-head origin main 2>/dev/null || true

    git -C "${FX_REPO}" checkout -q -b "${TARGET_BRANCH}"
    echo "a" > "${FX_REPO}/a.txt"
    git -C "${FX_REPO}" add a.txt
    git -C "${FX_REPO}" commit -q -m "A"
    FX_A="$(git -C "${FX_REPO}" rev-parse HEAD)"
    git -C "${FX_REPO}" push -q origin "${TARGET_BRANCH}" 2>/dev/null
    echo "b" > "${FX_REPO}/b.txt"
    git -C "${FX_REPO}" add b.txt
    git -C "${FX_REPO}" commit -q -m "B (local only, unreviewed)"
    FX_B="$(git -C "${FX_REPO}" rev-parse HEAD)"
    git -C "${FX_REPO}" checkout -q main
    # No upstream yet: assertion 4 must be the step's doing.
    git -C "${FX_REPO}" branch -q --unset-upstream "${TARGET_BRANCH}" 2>/dev/null || true

    # Installed after the fixture pushes so it only sees pushes by the step.
    local hook="${FX_REPO}/.git/hooks/pre-push"
    mkdir -p "${FX_REPO}/.git/hooks"
    printf '#!/bin/sh\n: > %q\necho "pre-push hook rejected the push (#1930 fixture)" >&2\nexit 1\n' \
        "${FX_MARKER}" > "${hook}"
    chmod 755 "${hook}"

    # Sanity: the fixture is the shape the test claims, and origin is local.
    local url
    url="$(git -C "${FX_REPO}" remote get-url origin)"
    case "${url}" in
        *://*) echo "HARNESS-ERROR: origin has a URL scheme" >&2; exit 2 ;;
        "${SCRATCH}"/*) ;;
        *) echo "HARNESS-ERROR: origin is outside scratch" >&2; exit 2 ;;
    esac
    [[ "$(git -C "${FX_REPO}" rev-parse "refs/heads/${TARGET_BRANCH}")" == "${FX_B}" ]] \
        || { echo "HARNESS-ERROR: local ${TARGET_BRANCH} is not at B" >&2; exit 2; }
    [[ "$(git --git-dir="${FX_REMOTE}" rev-parse "refs/heads/${TARGET_BRANCH}")" == "${FX_A}" ]] \
        || { echo "HARNESS-ERROR: origin ${TARGET_BRANCH} is not at A" >&2; exit 2; }
    [[ "$(git -C "${FX_REPO}" rev-parse "${FX_B}^")" == "${FX_A}" ]] \
        || { echo "HARNESS-ERROR: B is not a child of A" >&2; exit 2; }
    [[ "$(git -C "${FX_REPO}" symbolic-ref --short HEAD)" == "main" ]] \
        || { echo "HARNESS-ERROR: caller checkout is not on main" >&2; exit 2; }
    [[ -z "$(git -C "${FX_REPO}" config --get core.hooksPath || true)" ]] \
        || { echo "HARNESS-ERROR: core.hooksPath is set in the fixture" >&2; exit 2; }
    [[ ! -e "${FX_MARKER}" ]] \
        || { echo "HARNESS-ERROR: hook marker exists before the step ran" >&2; exit 2; }
}

# Strict stub gh: answers exactly `pr view 7 --json headRefName -q .headRefName`.
STUB_BIN="${SCRATCH}/bin"
mkdir -p "${STUB_BIN}"
cat > "${STUB_BIN}/gh" <<EOF
#!/bin/sh
if [ "\$*" = "pr view ${STUB_PR} --json headRefName -q .headRefName" ]; then
    printf '%s\n' "${TARGET_BRANCH}"
    exit 0
fi
echo "stub gh: unexpected arguments" >&2
exit 1
EOF
chmod 700 "${STUB_BIN}/gh"

STEP_SCRIPT="${SCRATCH}/step-04-body.sh"

# run_step <repo> <extra env assignments...>
# Runs the real step-04 body under env -i. Sets RUN_RC and RUN_STDERR.
run_step() {
    local repo="$1"; shift
    local out="${SCRATCH}/run.out" err="${SCRATCH}/run.err"
    RUN_RC=0
    (
        cd "${repo}" && env -i \
            PATH="${STUB_BIN}:${PATH}" \
            HOME="${HOME}" \
            TMPDIR="${SCRATCH}/tmp" \
            GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
            GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=t@example.com \
            GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=t@example.com \
            AMPLIHACK_HOME="${REPO_ROOT}" \
            XDG_RUNTIME_DIR="${SCRATCH}/runtime" \
            AMPLIHACK_RUNTIME_ROOT="${SCRATCH}/runtime/root" \
            AMPLIHACK_WORKTREE_STALE_SECS=999999999 \
            REPO_PATH="${repo}" \
            TASK_DESCRIPTION="issue 1930 fixture" \
            BRANCH_PREFIX="feat" \
            ISSUE_NUMBER="1930" \
            "$@" \
            bash "${STEP_SCRIPT}"
    ) >"${out}" 2>"${err}" </dev/null || RUN_RC=$?
    RUN_STDERR="$(cat "${err}")"
}

# assert_case <tag> -> the six ordered assertions on the current fixture.
assert_case() {
    local tag="$1" upstream
    if [[ "${RUN_RC}" -eq 0 ]]; then
        pass "${tag}.1" "step exits 0"
    else
        fail "${tag}.1" "step exited ${RUN_RC}; stderr tail:"
        awk '{ring[NR % 15] = $0} END {for (i = NR - 14; i <= NR; i++) if (i > 0) print "    " ring[i % 15]}' \
            <<<"${RUN_STDERR}" >&2
    fi
    if grep -qF '=== Worktree Setup Complete (existing-branch path) ===' <<<"${RUN_STDERR}"; then
        pass "${tag}.2" "existing-branch completion marker on stderr"
    else
        fail "${tag}.2" "existing-branch completion marker missing from stderr"
    fi
    if [[ ! -e "${FX_MARKER}" ]]; then
        pass "${tag}.3" "pre-push hook did not run (no push from setup)"
    else
        fail "${tag}.3" "pre-push hook ran: setup pushed the local branch (#1930)"
    fi
    upstream="$(git -C "${FX_REPO}" rev-parse --abbrev-ref "${TARGET_BRANCH}@{u}" 2>/dev/null || true)"
    if [[ "${upstream}" == "origin/${TARGET_BRANCH}" ]]; then
        pass "${tag}.4" "upstream of ${TARGET_BRANCH} is origin/${TARGET_BRANCH}"
    else
        fail "${tag}.4" "upstream of ${TARGET_BRANCH} is '${upstream:-<none>}', want origin/${TARGET_BRANCH}"
    fi
    if [[ "$(git -C "${FX_REPO}" rev-parse "refs/heads/${TARGET_BRANCH}")" == "${FX_B}" ]]; then
        pass "${tag}.5" "local ${TARGET_BRANCH} still at B"
    else
        fail "${tag}.5" "local ${TARGET_BRANCH} moved from B"
    fi
    if [[ "$(git --git-dir="${FX_REMOTE}" rev-parse "refs/heads/${TARGET_BRANCH}")" == "${FX_A}" ]]; then
        pass "${tag}.6" "origin ${TARGET_BRANCH} still at A"
    else
        fail "${tag}.6" "origin ${TARGET_BRANCH} moved from A"
    fi
}

# ---------------------------------------------------------------------------
# Part A: static checks.
# ---------------------------------------------------------------------------
echo "Part A: static checks"
STEP04_BODY="$(extract_step_body "${WORKTREE_YAML}" "step-04-setup-worktree")"
if [[ -z "${STEP04_BODY}" ]]; then
    echo "HARNESS-ERROR: could not extract step-04-setup-worktree from ${WORKTREE_YAML}" >&2
    exit 2
fi
printf '%s\n' "${STEP04_BODY}" > "${STEP_SCRIPT}"

STEP04_EB="$(existing_branch_block "${STEP04_BODY}")"
if [[ -z "${STEP04_EB}" ]]; then
    echo "HARNESS-ERROR: existing-branch block not found in step-04" >&2
    exit 2
fi
if grep -Eq 'git[^#]*[[:space:]]push([[:space:]]|$)' <<<"${STEP04_EB}"; then
    fail "A1" "step-04 existing-branch path still runs git push"
else
    pass "A1" "step-04 existing-branch path runs no git push"
fi
# shellcheck disable=SC2016  # literal recipe text, not an expansion
if grep -Fq -- '--set-upstream-to="origin/${BRANCH_NAME}"' <<<"${STEP04_EB}"; then
    pass "A2" "step-04 existing-branch path still sets the upstream"
else
    fail "A2" "step-04 existing-branch path no longer sets the upstream"
fi

CONS_BODY="$(extract_step_body "${CONSENSUS_YAML}" "step3-setup-worktree" prompt)"
if [[ -z "${CONS_BODY}" ]]; then
    echo "HARNESS-ERROR: could not extract step3-setup-worktree from ${CONSENSUS_YAML}" >&2
    exit 2
fi
CONS_EB="$(existing_branch_block "${CONS_BODY}")"
if [[ -z "${CONS_EB}" ]]; then
    echo "HARNESS-ERROR: existing-branch block not found in consensus step3" >&2
    exit 2
fi
if grep -Eq 'git[^#]*[[:space:]]push([[:space:]]|$)' <<<"${CONS_EB}"; then
    fail "A3" "consensus step3 existing-branch path runs git push (MIRROR)"
else
    pass "A3" "consensus step3 existing-branch path runs no git push (MIRROR)"
fi

# No hook bypass in either recipe.
for f in "${WORKTREE_YAML}" "${CONSENSUS_YAML}"; do
    if grep -Eq -- '--no-verify|core\.hooksPath|HUSKY=0' "${f}"; then
        fail "A4" "hook bypass found in $(basename "${f}")"
    else
        pass "A4" "no hook bypass in $(basename "${f}")"
    fi
done

# ---------------------------------------------------------------------------
# Part S: the real step-04 body.
# ---------------------------------------------------------------------------
echo "Part S1: EXISTING_BRANCH=${TARGET_BRANCH}"
build_fixture s1
run_step "${FX_REPO}" EXISTING_BRANCH="${TARGET_BRANCH}"
assert_case S1

echo "Part S2: PR_NUMBER=${STUB_PR} (stub gh -> ${TARGET_BRANCH})"
build_fixture s2
run_step "${FX_REPO}" PR_NUMBER="${STUB_PR}"
if grep -qF "Targeting existing branch '${TARGET_BRANCH}'" <<<"${RUN_STDERR}"; then
    pass "S2.0" "pr_number resolved to ${TARGET_BRANCH} through gh"
else
    fail "S2.0" "pr_number did not resolve to ${TARGET_BRANCH}"
fi
assert_case S2

echo ""
echo "Result: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ "${FAIL_COUNT}" -eq 0 ]] || exit 1
exit 0
