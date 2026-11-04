#!/usr/bin/env bash
# test-issue-1930-setup-no-push-ahead.sh: TDD spec for issue #1930.
#
# Issue #1930: the worktree setup step pushed the branch on a best-effort
# basis. On an existing branch whose local head is ahead of origin, that push
# ran the repository's pre-push hook (in this repository, the full workspace
# test suite) before any work had been done, and published unreviewed commits.
#
# REQUIRED BEHAVIOUR (workflow-worktree.yaml step-04 and its MIRROR,
# consensus-issue-worktree.yaml step3-setup-worktree):
#   - refs/remotes/origin/<B> exists: set the upstream to origin/<B> on a
#     best-effort basis and never push.
#   - origin/<B> is absent, new-branch path: keep the push. The head is then
#     the base commit (KEPT_AHEAD == 0), so later publish steps find @{u}.
#   - Existing-branch path (#342) with no origin/<B>: never push; print a
#     WARNING to stderr saying setup does not push it (#1930).
#   - Hooks are never bypassed: no --no-verify, core.hooksPath, HUSKY= or SKIP=.
#
# Every case gives the scratch clone a pre-push hook that writes a marker
# file. A marker means setup ran a push.
#
#   A   workflow step-04, EXISTING_BRANCH, local head one commit ahead of
#       origin/<B>, hook fails     -> exit 0, created=false, no marker, @{u} set
#   A2  as A, hook passes          -> origin/<B> unchanged (nothing published)
#   B   workflow step-04, new-branch path, stale origin/<B> at an older main,
#       hook fails                 -> exit 0, no marker, @{u} set
#   C   consensus step3, same fixture as B -> exit 0, setup_complete, no marker
#   D   control: workflow step-04, fresh <B>, no origin/<B>, hook passes
#                                  -> marker present, origin/<B> created
#   E   workflow step-04, EXISTING_BRANCH that is local-only, hook fails
#                                  -> exit 0, no marker, WARNING on stderr
#   S   static: both recipes at or under 400 lines, no hook bypass.
#
# RECIPES_DIR may point at another checkout's amplifier-bundle/recipes (for
# example a scratch worktree of origin/main) to show the test fails before the
# fix; the bundle tools are then taken from that checkout too.
#
# Exit status: 0 all pass, 1 any case fails, 2 harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECIPES_DIR="${RECIPES_DIR:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
RECIPES_DIR="$(cd "${RECIPES_DIR}" && pwd -P)"
BUNDLE_HOME="$(cd "${RECIPES_DIR}/../.." && pwd -P)"
WORKFLOW_YAML="${RECIPES_DIR}/workflow-worktree.yaml"
CONSENSUS_YAML="${RECIPES_DIR}/consensus-issue-worktree.yaml"

harness() { echo "HARNESS-ERROR: $*" >&2; exit 2; }

for f in "${WORKFLOW_YAML}" "${CONSENSUS_YAML}"; do
    [[ -f "$f" && ! -L "$f" ]] || harness "recipe must be a regular file: $f"
done
[[ -f "${BUNDLE_HOME}/amplifier-bundle/tools/workflow_worktree_deconflict.sh" ]] \
    || harness "bundle tools not found under ${BUNDLE_HOME}"
command -v python3 >/dev/null 2>&1 || harness "python3 is required"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

# --- Hermetic scratch ------------------------------------------------------
SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/i1930.XXXXXX")"
cleanup() {
    if [ -n "${SCRATCH:-}" ] && [ -d "${SCRATCH}" ]; then
        case "${SCRATCH}" in
            */i1930.*) rm -rf -- "${SCRATCH}" ;;
        esac
    fi
}
trap cleanup EXIT
mkdir -p "${SCRATCH}/home" "${SCRATCH}/runtime" "${SCRATCH}/tmp"

# Git config holds only an identity, so no host-wide core.hooksPath can
# silently disable the test hook (control case D guards this too).
GITCFG="${SCRATCH}/gitconfig"
printf '[user]\n\tname = Test\n\temail = t@example.com\n[init]\n\tdefaultBranch = main\n' > "${GITCFG}"
export HOME="${SCRATCH}/home"
export GIT_CONFIG_GLOBAL="${GITCFG}"
export GIT_CONFIG_SYSTEM=/dev/null
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0

# --- Step extraction (same idioms as the #1565 and #1626 tests) -------------
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

extract_step_body() {
    extract_step "$1" "$2" | awk '
        /^    command:[[:space:]]*\|/ { grab = 1; next }
        grab && /^    [A-Za-z_-]+:/  { grab = 0 }
        grab { line = $0; sub(/^      /, "", line); print line }
    '
}

extract_step3_bash() {
    awk '
        /^[[:space:]]*- id: "step3-setup-worktree"[[:space:]]*$/ { found = 1; next }
        found && !inblk && /^[[:space:]]*- id: "/ { exit }
        found && !inblk && $0 == "      ```bash" { inblk = 1; next }
        inblk && /^      ```[[:space:]]*$/ { done = 1; exit }
        inblk { line = $0; sub(/^      /, "", line); print line }
        END { if (!done) exit 1 }
    ' "$1"
}

WF_SCRIPT="${SCRATCH}/step-04-body.sh"
CS_SCRIPT="${SCRATCH}/step3-body.sh"
extract_step_body "${WORKFLOW_YAML}" "step-04-setup-worktree" > "${WF_SCRIPT}"
[[ -s "${WF_SCRIPT}" ]] || harness "could not extract step-04-setup-worktree"
extract_step3_bash "${CONSENSUS_YAML}" > "${CS_SCRIPT}" \
    || harness "could not extract a closed bash block from step3-setup-worktree"
[[ -s "${CS_SCRIPT}" ]] || harness "extracted step3 block is empty"
bash -n "${WF_SCRIPT}" || harness "extracted step-04 does not parse"
bash -n "${CS_SCRIPT}" || harness "extracted step3 does not parse"

# --- Fixtures --------------------------------------------------------------
# build_repo <name> -> clone of a fresh local bare origin with main.
build_repo() {
    local name="$1"
    local remote="${SCRATCH}/${name}-origin.git" work="${SCRATCH}/${name}"
    git init --quiet --bare "${remote}"
    git clone --quiet "${remote}" "${work}" 2>/dev/null || harness "fixture clone of ${remote} failed"
    git -C "${work}" checkout -q -b main 2>/dev/null || git -C "${work}" checkout -q main
    echo "base" > "${work}/README.md"
    git -C "${work}" add README.md
    git -C "${work}" commit -q -m "base commit"
    git -C "${work}" push -q -u origin main 2>/dev/null || harness "fixture push of main failed"
    git -C "${work}" remote set-head origin main >/dev/null 2>&1 || true
    local url
    url="$(git -C "${work}" remote get-url origin)"
    case "${url}" in
        "${SCRATCH}"/*) ;;
        *) harness "origin '${url}' is outside scratch" ;;
    esac
    printf '%s\n' "${work}"
}

# commit_on <repo> <file> -> one commit on the currently checked-out branch.
commit_on() {
    echo "$2" > "$1/$2"
    git -C "$1" add -- "$2"
    git -C "$1" commit -q -m "commit $2"
}

# advance_main <repo> -> main moves on by one commit, pushed (no hook yet).
advance_main() {
    git -C "$1" checkout -q main
    commit_on "$1" "main-moved.txt"
    git -C "$1" push -q origin main 2>/dev/null || harness "fixture push of moved main failed"
}

# install_hook <repo> <rc> -> pre-push hook that touches the marker, exits <rc>.
install_hook() {
    local repo="$1" rc="$2" hook
    MARKER="${SCRATCH}/$(basename "${repo}").pre-push-ran"
    rm -f -- "${MARKER}"
    hook="$(git -C "${repo}" rev-parse --git-common-dir)/hooks/pre-push"
    case "${hook}" in /*) ;; *) hook="${repo}/${hook}" ;; esac
    mkdir -p "$(dirname "${hook}")"
    printf '#!/bin/sh\ntouch %s\nexit %s\n' "'${MARKER}'" "${rc}" > "${hook}"
    chmod 0755 "${hook}"
}

remote_sha() { git --git-dir="$1-origin.git" rev-parse --verify --quiet "refs/heads/$2" || echo absent; }
upstream_of() { git -C "$1" rev-parse --abbrev-ref --symbolic-full-name "$2@{u}" 2>/dev/null || true; }

# run_wf <repo> [VAR=value...] -> runs step-04; sets RUN_RC, RUN_STDERR, RUN_JSON.
run_wf() {
    local repo="$1"; shift
    _run "${repo}" "${WF_SCRIPT}" \
        XDG_RUNTIME_DIR="${SCRATCH}/runtime" \
        AMPLIHACK_RUNTIME_ROOT="${SCRATCH}/runtime/root-$(basename "${repo}")" \
        AMPLIHACK_WORKTREE_STALE_SECS=999999999 \
        TASK_DESCRIPTION="setup no push" BRANCH_PREFIX="fix" ISSUE_NUMBER="1930" "$@"
}

# run_cs <repo> -> runs consensus step3 with worktrees in <repo>-wts.
run_cs() {
    local repo="$1"; shift
    _run "${repo}" "${CS_SCRIPT}" WORKTREE_DIR="${repo}-wts" \
        TASK_DESCRIPTION="setup no push" "$@"
}

_run() {
    local repo="$1" script="$2"; shift 2
    local out="${SCRATCH}/run.out" err="${SCRATCH}/run.err"
    RUN_RC=0
    (
        cd "${repo}" && env -i \
            PATH="${PATH}" HOME="${HOME}" TMPDIR="${SCRATCH}/tmp" \
            GIT_CONFIG_GLOBAL="${GITCFG}" GIT_CONFIG_SYSTEM=/dev/null GIT_CONFIG_NOSYSTEM=1 \
            GIT_TERMINAL_PROMPT=0 GIT_ALLOW_PROTOCOL=file \
            AMPLIHACK_HOME="${BUNDLE_HOME}" REPO_PATH="${repo}" \
            "$@" bash "${script}"
    ) >"${out}" 2>"${err}" </dev/null || RUN_RC=$?
    RUN_STDERR="$(cat "${err}")"
    RUN_JSON="$(awk '/^\{/{l=$0} END{print l}' "${out}")"
}

json_field() {
    printf '%s' "${RUN_JSON}" | python3 -I -c '
import json, sys
try:
    v = json.load(sys.stdin).get(sys.argv[1], "")
except Exception:
    v = "<invalid-json>"
print(str(v).lower() if isinstance(v, bool) else v)
' "$1"
}

tail_err() { awk '{r[NR % 4] = $0} END {for (i = NR - 3; i <= NR; i++) if (i > 0) print r[i % 4]}' <<<"${RUN_STDERR}"; }

# check_common <id> -> exit 0 and valid JSON.
check_common() {
    if [[ "${RUN_RC}" -eq 0 ]]; then pass "$1-exit" "step exits 0"
    else fail "$1-exit" "step exited ${RUN_RC}: $(tail_err)"; fi
    if [[ -n "${RUN_JSON}" && "$(json_field branch_name)" != "<invalid-json>" ]]; then
        pass "$1-json" "stdout ends in a valid JSON object"
    else
        fail "$1-json" "no valid JSON on stdout (got '${RUN_JSON}')"
    fi
}

check_no_marker() {
    if [[ ! -e "${MARKER}" ]]; then pass "$1-no-hook" "setup did not run the pre-push hook"
    else fail "$1-no-hook" "setup ran the pre-push hook (it pushed $2)"; fi
}

# The fixture is only meaningful if the step kept the branch name we set up.
require_branch() {
    local got; got="$(json_field branch_name)"
    [[ "${RUN_RC}" -ne 0 || "${got}" == "$1" ]] \
        || harness "$2: step used branch '${got}', expected '$1' (fixture tripped deconfliction)"
}

check_upstream() {
    local up; up="$(upstream_of "$2" "$3")"
    if [[ "${up}" == "origin/$3" ]]; then pass "$1-upstream" "$3@{u} is origin/$3"
    else fail "$1-upstream" "$3@{u} is '${up:-unset}', expected origin/$3"; fi
}

WF_BRANCH="fix/issue-1930-setup-no-push"
CS_BRANCH="feat/consensus-setup-no-push"

# ===========================================================================
echo "=== Case A: existing branch ahead of origin/<B>, hook fails ==="
R="$(build_repo a)"
git -C "${R}" checkout -q -b feat/x
commit_on "${R}" "pushed.txt"
git -C "${R}" push -q -u origin feat/x 2>/dev/null || harness "fixture push of feat/x failed"
commit_on "${R}" "ahead.txt"
git -C "${R}" checkout -q main
REMOTE_BEFORE="$(remote_sha "${R}" feat/x)"
install_hook "${R}" 1
run_wf "${R}" EXISTING_BRANCH=feat/x
check_common A
if [[ "$(json_field created)" == "false" ]]; then pass "A-created" "created=false"
else fail "A-created" "created is '$(json_field created)', expected false"; fi
check_no_marker A "an existing branch whose head is ahead of origin/feat/x"
check_upstream A "${R}" feat/x
if [[ "$(remote_sha "${R}" feat/x)" == "${REMOTE_BEFORE}" ]]; then pass "A-remote" "origin feat/x unchanged"
else fail "A-remote" "origin feat/x moved"; fi

# ===========================================================================
echo "=== Case A2: as A, hook passes: nothing is published ==="
R="$(build_repo a2)"
git -C "${R}" checkout -q -b feat/x
commit_on "${R}" "pushed.txt"
git -C "${R}" push -q -u origin feat/x 2>/dev/null || harness "fixture push of feat/x failed"
commit_on "${R}" "ahead.txt"
git -C "${R}" checkout -q main
REMOTE_BEFORE="$(remote_sha "${R}" feat/x)"
LOCAL_HEAD="$(git -C "${R}" rev-parse refs/heads/feat/x)"
install_hook "${R}" 0
run_wf "${R}" EXISTING_BRANCH=feat/x
check_common A2
if [[ "$(remote_sha "${R}" feat/x)" == "${REMOTE_BEFORE}" ]]; then
    pass "A2-remote" "origin feat/x still at the pushed commit; the ahead commit stays local"
else
    fail "A2-remote" "setup published the local head ($(remote_sha "${R}" feat/x), was ${REMOTE_BEFORE})"
fi
if [[ "$(git -C "${R}" rev-parse refs/heads/feat/x)" == "${LOCAL_HEAD}" ]]; then
    pass "A2-local" "local feat/x keeps its unpushed commit"
else
    fail "A2-local" "local feat/x moved"
fi

# ===========================================================================
echo "=== Case B: workflow new-branch path, stale origin/<B>, hook fails ==="
R="$(build_repo b)"
git -C "${R}" push -q origin "main:refs/heads/${WF_BRANCH}" 2>/dev/null || harness "fixture push of ${WF_BRANCH} failed"
advance_main "${R}"
git -C "${R}" fetch -q origin
REMOTE_BEFORE="$(remote_sha "${R}" "${WF_BRANCH}")"
install_hook "${R}" 1
run_wf "${R}"
check_common B
require_branch "${WF_BRANCH}" B
check_no_marker B "'${WF_BRANCH}' although origin/${WF_BRANCH} exists"
check_upstream B "${R}" "${WF_BRANCH}"
if [[ "$(remote_sha "${R}" "${WF_BRANCH}")" == "${REMOTE_BEFORE}" ]]; then pass "B-remote" "origin ${WF_BRANCH} unchanged"
else fail "B-remote" "origin ${WF_BRANCH} moved"; fi

# ===========================================================================
echo "=== Case C: consensus step3, stale origin/<B>, hook fails ==="
R="$(build_repo c)"
git -C "${R}" push -q origin "main:refs/heads/${CS_BRANCH}" 2>/dev/null || harness "fixture push of ${CS_BRANCH} failed"
advance_main "${R}"
git -C "${R}" fetch -q origin
REMOTE_BEFORE="$(remote_sha "${R}" "${CS_BRANCH}")"
install_hook "${R}" 1
run_cs "${R}"
check_common C
require_branch "${CS_BRANCH}" C
if [[ "$(json_field setup_complete)" == "true" ]]; then pass "C-complete" "setup_complete:true"
else fail "C-complete" "setup_complete is '$(json_field setup_complete)'"; fi
check_no_marker C "'${CS_BRANCH}' although origin/${CS_BRANCH} exists"
check_upstream C "${R}" "${CS_BRANCH}"
if [[ "$(remote_sha "${R}" "${CS_BRANCH}")" == "${REMOTE_BEFORE}" ]]; then pass "C-remote" "origin ${CS_BRANCH} unchanged"
else fail "C-remote" "origin ${CS_BRANCH} moved"; fi

# ===========================================================================
echo "=== Case D (control): fresh branch, no origin/<B>: the push is kept ==="
R="$(build_repo d)"
install_hook "${R}" 0
run_wf "${R}"
check_common D
require_branch "${WF_BRANCH}" D
if [[ -e "${MARKER}" ]]; then pass "D-hook-ran" "the retained push ran the pre-push hook (hooks are live)"
else fail "D-hook-ran" "the pre-push hook did not run: the new-branch push was removed or hooks are disabled"; fi
if [[ "$(remote_sha "${R}" "${WF_BRANCH}")" != "absent" ]]; then pass "D-remote" "origin ${WF_BRANCH} created"
else fail "D-remote" "origin ${WF_BRANCH} was not created"; fi
check_upstream D "${R}" "${WF_BRANCH}"

# ===========================================================================
echo "=== Case E: existing local-only branch: not pushed, warning printed ==="
R="$(build_repo e)"
git -C "${R}" checkout -q -b feat/local
commit_on "${R}" "local.txt"
git -C "${R}" checkout -q main
install_hook "${R}" 1
run_wf "${R}" EXISTING_BRANCH=feat/local
check_common E
check_no_marker E "the local-only branch feat/local"
if [[ "$(remote_sha "${R}" feat/local)" == "absent" ]]; then pass "E-remote" "origin has no feat/local"
else fail "E-remote" "setup created origin feat/local"; fi
if grep -qF "WARNING: branch 'feat/local' has no origin/feat/local" <<<"${RUN_STDERR}" \
   && grep -qF "#1930" <<<"${RUN_STDERR}"; then
    pass "E-warning" "stderr says setup does not push the branch (#1930)"
else
    fail "E-warning" "no #1930 warning for a local-only existing branch on stderr"
fi
if ! grep -qF "has no origin/" <<<"${RUN_JSON}"; then pass "E-stdout" "the warning is not on stdout"
else fail "E-stdout" "the warning leaked into the JSON line"; fi

# ===========================================================================
echo "=== Part S: static checks ==="
for f in "${WORKFLOW_YAML}" "${CONSENSUS_YAML}"; do
    n="$(wc -l < "$f")"
    if [[ "$n" -le 400 ]]; then pass "S-lines" "$(basename "$f") has ${n} lines (<= 400)"
    else fail "S-lines" "$(basename "$f") has ${n} lines (> 400)"; fi
    if ! grep -nE -- '--no-verify|core\.hooksPath|GIT_HOOKS|HUSKY=|SKIP=|push[^|]*--force|--mirror' "$f" >/dev/null; then
        pass "S-no-bypass" "$(basename "$f") has no hook bypass or forced push"
    else
        fail "S-no-bypass" "$(basename "$f"): $(grep -nE -- '--no-verify|core\.hooksPath|GIT_HOOKS|HUSKY=|SKIP=|push[^|]*--force|--mirror' "$f" | head -1)"
    fi
done

echo ""
echo "Result: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ "${FAIL_COUNT}" -eq 0 ]]
