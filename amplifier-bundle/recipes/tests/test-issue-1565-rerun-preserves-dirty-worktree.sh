#!/usr/bin/env bash
# test-issue-1565-rerun-preserves-dirty-worktree.sh — TDD spec for issue #1565.
#
# Issue #1565: workflow-worktree.yaml step-04-setup-worktree, on a re-run of a
# task whose worktree already exists (State 1), ran `checkout -- .`, `clean -fd`
# and `reset --hard "$BASE_REF"` on that worktree, deleting uncommitted and
# untracked work and orphaning unpushed commits. State 2 (branch without a
# worktree) ran `git branch -D` on a branch with unmerged commits.
#
# REQUIRED BEHAVIOUR (option 1 of the issue):
#   - A re-run never destroys work it did not create. A dirty or ahead branch
#     and its worktree are KEPT; this run gets a fresh branch through the
#     deconflict helper's new `resolve --base <BASE_REF>` mode.
#   - A clean, not-ahead State 1 is reused unchanged (created=false).
#   - If the helper is missing and the state machine meets work, step-04 fails
#     loud ("refusing to reset") instead of resetting.
#   - The helper's stdout is validated with `git check-ref-format --branch`.
#
# Part H: unit tests of tools/workflow_worktree_deconflict.sh `resolve --base`.
# Part A: static checks on step-04.
# Part S: the real step-04 body run against scratch repos (S1..S6).
#
# Override: WORKTREE_YAML=<regular file> runs Part A/S against another copy of
# the recipe, e.g. origin/main's, to show the test fails there:
#   git show origin/main:amplifier-bundle/recipes/workflow-worktree.yaml \
#     > "$TMPDIR/wt-main.yaml"
#   WORKTREE_YAML="$TMPDIR/wt-main.yaml" bash <this script>   # must exit 1
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1565-rerun-preserves-dirty-worktree.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
WORKTREE_YAML="${WORKTREE_YAML:-${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml}"
HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"

# Derived by step-04 from these inputs (#1426 slug rules):
#   fix + 1565 + "rerun keeps work" -> fix/issue-1565-rerun-keeps-work
STEP_TASK_DESC="rerun keeps work"
STEP_BRANCH_PREFIX="fix"
STEP_ISSUE_NUMBER="1565"
TARGET_BRANCH="fix/issue-1565-rerun-keeps-work"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }

if [[ ! -f "${WORKTREE_YAML}" || -L "${WORKTREE_YAML}" ]]; then
    echo "HARNESS-ERROR: WORKTREE_YAML must be a regular file: ${WORKTREE_YAML}" >&2
    exit 2
fi
if [[ ! -f "${HELPER}" ]]; then
    echo "HARNESS-ERROR: deconflict helper not found: ${HELPER}" >&2
    exit 2
fi

# --- Hermetic scratch (never /tmp): everything lives under $TMPDIR. ---------
: "${TMPDIR:?TMPDIR must be set (scratch must not land on /tmp)}"
SCRATCH="$(mktemp -d "${TMPDIR%/}/i1565.XXXXXX")"
cleanup() {
    case "${SCRATCH}" in
        "${TMPDIR%/}"/i1565.*) rm -rf -- "${SCRATCH}" ;;
    esac
}
trap cleanup EXIT
mkdir -p "${SCRATCH}/home" "${SCRATCH}/runtime" "${SCRATCH}/tmp"

# Isolate git from the developer's config and identity.
export HOME="${SCRATCH}/home"
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_NOSYSTEM=1
export GIT_TERMINAL_PROMPT=0
export GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="t@example.com"
export GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="t@example.com"

# ---------------------------------------------------------------------------
# Step extraction (same idiom as test-issue-1121 / test-issue-840).
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

extract_step_body() {
    extract_step "$1" "$2" | awk '
        /^    command:[[:space:]]*\|/ { grab = 1; next }
        grab && /^    [A-Za-z_-]+:/  { grab = 0 }
        grab { line = $0; sub(/^      /, "", line); print line }
    '
}

# build_repo <name> -> path of a fresh clone of a local bare origin with main.
build_repo() {
    local name="$1"
    local remote="${SCRATCH}/${name}-remote.git"
    local work="${SCRATCH}/${name}"
    git init --quiet --bare "${remote}"
    git clone --quiet "${remote}" "${work}" 2>/dev/null
    git -C "${work}" checkout -q -b main 2>/dev/null || git -C "${work}" checkout -q main
    echo "base" > "${work}/README.md"
    git -C "${work}" add README.md
    git -C "${work}" commit -q -m "base commit"
    git -C "${work}" push -q -u origin main 2>/dev/null
    git -C "${work}" remote set-head origin main 2>/dev/null || true
    # T1: origin must be the local bare repo inside SCRATCH, never a network URL.
    local url
    url="$(git -C "${work}" remote get-url origin)"
    case "${url}" in
        "${SCRATCH}"/*) ;;
        *) echo "HARNESS-ERROR: origin '${url}' is outside scratch" >&2; exit 2 ;;
    esac
    printf '%s\n' "${work}"
}

# add_worktree <repo> <branch> -> registers MAIN/worktrees/<branch> on a new branch.
add_worktree() {
    git -C "$1" worktree add -q "$1/worktrees/$2" -b "$2" origin/main
}

STEP_SCRIPT="${SCRATCH}/step-04-body.sh"

# run_step <repo> [amplihack_home]
# Runs the real step-04 body. Sets RUN_RC, RUN_STDOUT, RUN_STDERR, RUN_JSON.
run_step() {
    local repo="$1" ahome="${2:-${REPO_ROOT}}"
    local out="${SCRATCH}/run.out" err="${SCRATCH}/run.err"
    RUN_RC=0
    (
        cd "${repo}" && env -i \
            PATH="${PATH}" \
            HOME="${HOME}" \
            TMPDIR="${SCRATCH}/tmp" \
            GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 \
            GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=t@example.com \
            GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=t@example.com \
            AMPLIHACK_HOME="${ahome}" \
            XDG_RUNTIME_DIR="${SCRATCH}/runtime" \
            AMPLIHACK_RUNTIME_ROOT="${SCRATCH}/runtime/root" \
            AMPLIHACK_WORKTREE_STALE_SECS=999999999 \
            REPO_PATH="${repo}" \
            TASK_DESCRIPTION="${STEP_TASK_DESC}" \
            BRANCH_PREFIX="${STEP_BRANCH_PREFIX}" \
            ISSUE_NUMBER="${STEP_ISSUE_NUMBER}" \
            bash "${STEP_SCRIPT}"
    ) >"${out}" 2>"${err}" || RUN_RC=$?
    RUN_STDOUT="$(cat "${out}")"
    RUN_STDERR="$(cat "${err}")"
    # Last JSON-looking line. A single awk reads all its input, so there is no
    # early-exit stage for pipefail to turn into a failure (#1434).
    RUN_JSON="$(awk '/^\{/{l=$0} END{print l}' <<<"${RUN_STDOUT}")"
}

# last_lines <n> <text> -> the last <n> lines of <text>. Kept in one awk that
# reads everything (a ring of n slots), not `| tail`, so pipefail cannot turn
# an early-closing reader into a failed or empty diagnostic (#1434).
last_lines() {
    awk -v n="$1" '{ring[NR % n] = $0} END {for (i = NR - n + 1; i <= NR; i++) if (i > 0) print ring[i % n]}' <<<"$2"
}

# json_field <key> -> value of <key> in RUN_JSON ("" if absent/unparseable).
json_field() {
    printf '%s' "${RUN_JSON}" | python3 -I -c '
import json, sys
try:
    v = json.load(sys.stdin).get(sys.argv[1], "")
except Exception:
    v = ""
print(str(v).lower() if isinstance(v, bool) else v)
' "$1"
}

# run_helper <repo> <args...> -> HELPER_RC, HELPER_OUT (stdout), HELPER_ERR.
run_helper() {
    local repo="$1"; shift
    local out="${SCRATCH}/helper.out" err="${SCRATCH}/helper.err"
    HELPER_RC=0
    (cd "${repo}" && bash "${HELPER}" "$@") >"${out}" 2>"${err}" || HELPER_RC=$?
    HELPER_OUT="$(cat "${out}")"
    HELPER_ERR="$(cat "${err}")"
}

# git_state <repo> -> fingerprint of refs + every worktree's status (read-only check).
git_state() {
    local repo="$1" wt
    git -C "${repo}" for-each-ref --format='%(refname) %(objectname)'
    git -C "${repo}" worktree list --porcelain
    git -C "${repo}" worktree list --porcelain | awk '$1=="worktree"{print substr($0,10)}' \
        | while IFS= read -r wt; do
            echo "== ${wt}"
            git --no-optional-locks -C "${wt}" status --porcelain 2>/dev/null || echo "(status failed)"
        done
}

one_line() { [[ -n "$1" && "$(printf '%s\n' "$1" | wc -l)" -eq 1 ]]; }

# ===========================================================================
# Part H — helper unit tests: `resolve --base <ref>`.
# ===========================================================================
echo "=== Part H: deconflict helper 'resolve --base' ==="

# H1: back-compat — without --base, a dirty same-path worktree is unchanged.
R="$(build_repo h1)"; add_worktree "${R}" "${TARGET_BRANCH}"
echo "wip" > "${R}/worktrees/${TARGET_BRANCH}/untracked.txt"
run_helper "${R}" resolve "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && "${HELPER_OUT}" == "${TARGET_BRANCH}" ]]; then
    pass "H1-no-base-unchanged" "without --base, same-path dirty branch is returned unchanged"
else
    fail "H1-no-base-unchanged" "without --base expected '${TARGET_BRANCH}' rc=0, got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H2: --base + same-path worktree with only an untracked file -> fresh name.
#     Also H10 (read-only) and H11 (no file names leak into diagnostics).
R="$(build_repo h2)"; add_worktree "${R}" "${TARGET_BRANCH}"
echo "wip" > "${R}/worktrees/${TARGET_BRANCH}/secret-untracked-name.txt"
BEFORE="$(git_state "${R}")"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
AFTER="$(git_state "${R}")"
if [[ ${HELPER_RC} -eq 0 ]] && one_line "${HELPER_OUT}" && [[ "${HELPER_OUT}" != "${TARGET_BRANCH}" ]] \
   && [[ "${HELPER_OUT}" == "${TARGET_BRANCH}-"* ]] \
   && git check-ref-format --branch "${HELPER_OUT}" >/dev/null 2>&1 \
   && ! git -C "${R}" show-ref --verify --quiet "refs/heads/${HELPER_OUT}"; then
    pass "H2-dirty-renamed" "--base renames a dirty same-path branch to a free '${TARGET_BRANCH}-<suffix>'"
else
    fail "H2-dirty-renamed" "--base on dirty same-path: expected one fresh free name, got '${HELPER_OUT}' rc=${HELPER_RC}"
fi
if [[ -n "${HELPER_OUT}" && ${#HELPER_OUT} -le 80 ]]; then
    pass "H2-len-cap" "renamed branch is <= 80 characters"
else
    fail "H2-len-cap" "renamed branch is empty or ${#HELPER_OUT} characters (> 80)"
fi
if grep -q 'WARNING' <<<"${HELPER_ERR}" \
   && grep -qF "existing_branch=${TARGET_BRANCH}" <<<"${HELPER_ERR}"; then
    pass "H2-warning" "helper stderr WARNING names existing_branch=${TARGET_BRANCH}"
else
    fail "H2-warning" "helper stderr lacks a WARNING naming existing_branch=${TARGET_BRANCH}: ${HELPER_ERR}"
fi
if [[ "${BEFORE}" == "${AFTER}" ]]; then
    pass "H10-read-only" "helper left refs, worktrees and status untouched"
else
    fail "H10-read-only" "helper changed git state (refs/worktrees/status differ)"
fi
if grep -qF "secret-untracked-name" <<<"${HELPER_ERR}${HELPER_OUT}"; then
    fail "H11-no-filenames" "helper output leaks a porcelain file name"
else
    pass "H11-no-filenames" "helper diagnostics contain no file names"
fi

# H3: --base + clean same-path worktree with a commit ahead -> fresh name.
R="$(build_repo h3)"; add_worktree "${R}" "${TARGET_BRANCH}"
W="${R}/worktrees/${TARGET_BRANCH}"
echo "c" > "${W}/c.txt"; git -C "${W}" add c.txt; git -C "${W}" commit -q -m "ahead"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${W}" "${R}"
if [[ ${HELPER_RC} -eq 0 && -n "${HELPER_OUT}" && "${HELPER_OUT}" != "${TARGET_BRANCH}" ]]; then
    pass "H3-ahead-renamed" "--base renames a clean-but-ahead same-path branch"
else
    fail "H3-ahead-renamed" "--base on ahead same-path: expected a new name, got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H4: --base + clean, not-ahead same-path -> unchanged (State 1 reuse).
R="$(build_repo h4)"; add_worktree "${R}" "${TARGET_BRANCH}"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && "${HELPER_OUT}" == "${TARGET_BRANCH}" ]]; then
    pass "H4-clean-unchanged" "--base keeps a clean, not-ahead same-path branch"
else
    fail "H4-clean-unchanged" "--base on clean same-path: expected '${TARGET_BRANCH}', got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H12: ignored files do not count as dirty (clean -fd never touched them).
R="$(build_repo h12)"; add_worktree "${R}" "${TARGET_BRANCH}"
echo '*.log' >> "${R}/.git/info/exclude"
echo "noise" > "${R}/worktrees/${TARGET_BRANCH}/build.log"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && "${HELPER_OUT}" == "${TARGET_BRANCH}" ]]; then
    pass "H12-ignored-not-dirty" "ignored files alone do not trigger a rename"
else
    fail "H12-ignored-not-dirty" "ignored file triggered a rename: '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H5: --base + leftover branch (no worktree) ahead of base -> fresh name.
R="$(build_repo h5)"
git -C "${R}" branch -q "${TARGET_BRANCH}" origin/main
git -C "${R}" worktree add -q "${SCRATCH}/h5-tmpwt" "${TARGET_BRANCH}"
echo "c" > "${SCRATCH}/h5-tmpwt/c.txt"
git -C "${SCRATCH}/h5-tmpwt" add c.txt; git -C "${SCRATCH}/h5-tmpwt" commit -q -m "ahead"
git -C "${R}" worktree remove "${SCRATCH}/h5-tmpwt"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && -n "${HELPER_OUT}" && "${HELPER_OUT}" != "${TARGET_BRANCH}" ]]; then
    pass "H5-leftover-ahead-renamed" "--base renames a leftover branch with commits ahead"
else
    fail "H5-leftover-ahead-renamed" "--base on leftover ahead: expected a new name, got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H9: --base with an unresolvable ref on a leftover branch -> rename (fail toward keeping work).
run_helper "${R}" resolve --base origin/no-such-base "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && -n "${HELPER_OUT}" && "${HELPER_OUT}" != "${TARGET_BRANCH}" ]]; then
    pass "H9-unresolvable-base-renames" "unresolvable --base is treated as 'cannot prove clean' -> rename"
else
    fail "H9-unresolvable-base-renames" "unresolvable --base: expected a new name, got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H6: --base + leftover branch NOT ahead -> unchanged.
R="$(build_repo h6)"
git -C "${R}" branch -q "${TARGET_BRANCH}" origin/main
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && "${HELPER_OUT}" == "${TARGET_BRANCH}" ]]; then
    pass "H6-leftover-clean-unchanged" "--base keeps a leftover branch with nothing ahead"
else
    fail "H6-leftover-clean-unchanged" "--base on leftover clean: expected '${TARGET_BRANCH}', got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H7: --base + absent branch -> unchanged.
R="$(build_repo h7)"
run_helper "${R}" resolve --base origin/main "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -eq 0 && "${HELPER_OUT}" == "${TARGET_BRANCH}" ]]; then
    pass "H7-absent-unchanged" "--base keeps an absent branch name"
else
    fail "H7-absent-unchanged" "--base on absent: expected '${TARGET_BRANCH}', got '${HELPER_OUT}' rc=${HELPER_RC}"
fi

# H8: --base without a value, or with an empty value -> exit 1, nothing on stdout.
run_helper "${R}" resolve --base
if [[ ${HELPER_RC} -ne 0 && -z "${HELPER_OUT}" ]]; then
    pass "H8-base-missing-value" "'resolve --base' with no value exits non-zero"
else
    fail "H8-base-missing-value" "'resolve --base' with no value: rc=${HELPER_RC} stdout='${HELPER_OUT}'"
fi
run_helper "${R}" resolve --base "" "${TARGET_BRANCH}" "${R}/worktrees/${TARGET_BRANCH}" "${R}"
if [[ ${HELPER_RC} -ne 0 && -z "${HELPER_OUT}" ]]; then
    pass "H8-base-empty-value" "'resolve --base \"\"' exits non-zero"
else
    fail "H8-base-empty-value" "'resolve --base \"\"': rc=${HELPER_RC} stdout='${HELPER_OUT}'"
fi

# ===========================================================================
# Part A — static checks on step-04.
# ===========================================================================
echo "=== Part A: step-04 static checks (${WORKTREE_YAML}) ==="

STEP04="$(extract_step "${WORKTREE_YAML}" "step-04-setup-worktree")"
extract_step_body "${WORKTREE_YAML}" "step-04-setup-worktree" > "${STEP_SCRIPT}"
if [[ -z "${STEP04}" || ! -s "${STEP_SCRIPT}" ]]; then
    echo "HARNESS-ERROR: could not extract step-04-setup-worktree body" >&2
    exit 2
fi

# A1: no destructive command remains anywhere in step-04 (comments excluded).
STEP04_CODE="$(printf '%s\n' "${STEP04}" | sed -e 's/^[[:space:]]*#.*$//')"
DESTRUCTIVE=""
grep -qE 'checkout[[:space:]]+--[[:space:]]+\.' <<<"${STEP04_CODE}" && DESTRUCTIVE+=" 'checkout -- .'"
grep -qE 'clean[[:space:]]+-[a-zA-Z]*f' <<<"${STEP04_CODE}" && DESTRUCTIVE+=" 'clean -f'"
grep -qE 'reset[[:space:]]+--hard' <<<"${STEP04_CODE}" && DESTRUCTIVE+=" 'reset --hard'"
grep -qE 'branch[[:space:]]+-D' <<<"${STEP04_CODE}" && DESTRUCTIVE+=" 'branch -D'"
if [[ -z "${DESTRUCTIVE}" ]]; then
    pass "A1-no-destructive" "step-04 contains no checkout -- . / clean -f / reset --hard / branch -D"
else
    fail "A1-no-destructive" "step-04 still contains:${DESTRUCTIVE}"
fi

# A2: step-04 passes --base "$BASE_REF" to the deconflict helper.
if grep -qE 'resolve[[:space:]]+--base[[:space:]]+"\$\{?BASE_REF\}?"' <<<"${STEP04_CODE}"; then
    pass "A2-passes-base" "step-04 calls the helper with resolve --base \"\$BASE_REF\""
else
    fail "A2-passes-base" "step-04 does not call 'resolve --base \"\$BASE_REF\"'"
fi

# A3: helper output validated with check-ref-format before use (V1).
if grep -qE 'check-ref-format[[:space:]]+--branch[[:space:]]+"\$\{?RESOLVED_BRANCH\}?"' <<<"${STEP04_CODE}"; then
    pass "A3-validates-resolved" "step-04 validates RESOLVED_BRANCH with check-ref-format --branch"
else
    fail "A3-validates-resolved" "step-04 does not validate RESOLVED_BRANCH with check-ref-format --branch"
fi

# A4: brick limit — workflow-worktree.yaml strictly < 400 lines.
YAML_LINES=$(wc -l < "${WORKTREE_YAML}")
if [[ "${YAML_LINES}" -lt 400 ]]; then
    pass "A4-400" "workflow-worktree.yaml is ${YAML_LINES} lines (< 400)"
else
    fail "A4-400" "workflow-worktree.yaml is ${YAML_LINES} lines (>= 400)"
fi

# ===========================================================================
# Part S — the real step-04 body against scratch repos.
# ===========================================================================
echo "=== Part S: step-04 end-to-end ==="

# --- S1: dirty State 1 — untracked file, modified tracked file, unpushed commit.
R="$(build_repo s1)"; add_worktree "${R}" "${TARGET_BRANCH}"
W="${R}/worktrees/${TARGET_BRANCH}"
echo "committed work" > "${W}/committed.txt"
git -C "${W}" add committed.txt; git -C "${W}" commit -q -m "unpushed work"
KEPT_SHA="$(git -C "${R}" rev-parse "refs/heads/${TARGET_BRANCH}")"
echo "modified tracked content" > "${W}/README.md"
echo "untracked content" > "${W}/untracked.txt"
BASE_SHA="$(git -C "${R}" rev-parse origin/main)"
run_step "${R}"

if [[ ${RUN_RC} -eq 0 ]]; then
    pass "S1-exit0" "step-04 exits 0 on a dirty State 1 re-run"
else
    fail "S1-exit0" "step-04 exited ${RUN_RC} on a dirty State 1 re-run: $(last_lines 5 "${RUN_STDERR}")"
fi
if [[ -f "${W}/untracked.txt" && "$(cat "${W}/untracked.txt")" == "untracked content" ]]; then
    pass "S1-untracked-survives" "untracked file survives"
else
    fail "S1-untracked-survives" "untracked file was deleted or changed (#1565)"
fi
if [[ "$(cat "${W}/README.md" 2>/dev/null)" == "modified tracked content" ]]; then
    pass "S1-modified-survives" "uncommitted modification survives"
else
    fail "S1-modified-survives" "uncommitted modification was reverted (#1565)"
fi
if [[ "$(git -C "${R}" rev-parse "refs/heads/${TARGET_BRANCH}" 2>/dev/null)" == "${KEPT_SHA}" \
      && "$(git -C "${W}" rev-parse HEAD 2>/dev/null)" == "${KEPT_SHA}" \
      && -f "${W}/committed.txt" ]]; then
    pass "S1-commit-survives" "unpushed commit ${KEPT_SHA:0:12} is still the branch and worktree HEAD"
else
    fail "S1-commit-survives" "unpushed commit was orphaned (branch/worktree HEAD moved) (#1565)"
fi
NEW_BRANCH="$(json_field branch_name)"
NEW_WT="$(json_field worktree_path)"
if [[ -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "${TARGET_BRANCH}" && "${NEW_BRANCH}" == "${TARGET_BRANCH}-"* ]]; then
    pass "S1-new-branch" "run moved to its own branch '${NEW_BRANCH}'"
else
    fail "S1-new-branch" "expected a fresh '${TARGET_BRANCH}-<suffix>' branch, got '${NEW_BRANCH}'"
fi
if [[ -n "${NEW_WT}" && "${NEW_WT}" == "${R}/worktrees/"* && "${NEW_WT}" != "${W}" && -d "${NEW_WT}" ]] \
   && [[ "$(git -C "${NEW_WT}" rev-parse HEAD 2>/dev/null)" == "${BASE_SHA}" ]] \
   && [[ "$(git -C "${NEW_WT}" symbolic-ref --short HEAD 2>/dev/null)" == "${NEW_BRANCH}" ]]; then
    pass "S1-new-worktree" "new worktree exists under worktrees/, on the new branch, at BASE_REF"
else
    fail "S1-new-worktree" "new worktree '${NEW_WT}' is missing, reuses the kept path, or is not at BASE_REF"
fi
if [[ "$(json_field created)" == "true" ]]; then
    pass "S1-created" "created=true for the new branch"
else
    fail "S1-created" "expected created=true, got '$(json_field created)'"
fi
if awk -v want="existing_branch=${TARGET_BRANCH}" '/WARNING/ && index($0, want) {f=1} END {exit !f}' <<<"${RUN_STDERR}"; then
    pass "S1-warns" "stderr WARNING names the kept branch (existing_branch=${TARGET_BRANCH})"
else
    fail "S1-warns" "stderr does not WARN about the kept branch '${TARGET_BRANCH}'"
fi
if [[ -z "$(git -C "${R}" ls-remote origin "refs/heads/${TARGET_BRANCH}")" ]]; then
    pass "S1-kept-not-pushed" "kept branch was not pushed to origin"
else
    fail "S1-kept-not-pushed" "kept branch '${TARGET_BRANCH}' was pushed to origin"
fi

# --- S2: clean State 1 — reused unchanged.
R="$(build_repo s2)"; add_worktree "${R}" "${TARGET_BRANCH}"
run_step "${R}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${TARGET_BRANCH}" \
      && "$(json_field worktree_path)" == "${R}/worktrees/${TARGET_BRANCH}" \
      && "$(json_field created)" == "false" ]]; then
    pass "S2-clean-reuse" "clean State 1 reused: same branch, same path, created=false"
else
    fail "S2-clean-reuse" "clean State 1 not reused: rc=${RUN_RC} json=${RUN_JSON}"
fi

# --- S3: State 2 — branch 1 commit ahead, worktree removed.
R="$(build_repo s3)"; add_worktree "${R}" "${TARGET_BRANCH}"
W="${R}/worktrees/${TARGET_BRANCH}"
echo "c" > "${W}/c.txt"; git -C "${W}" add c.txt; git -C "${W}" commit -q -m "ahead"
KEPT_SHA="$(git -C "${R}" rev-parse "refs/heads/${TARGET_BRANCH}")"
git -C "${R}" worktree remove "${W}"
run_step "${R}"
if [[ "$(git -C "${R}" rev-parse --verify --quiet "refs/heads/${TARGET_BRANCH}" 2>/dev/null)" == "${KEPT_SHA}" ]]; then
    pass "S3-branch-survives" "ahead branch still points at ${KEPT_SHA:0:12}"
else
    fail "S3-branch-survives" "ahead branch was deleted or moved (State 2 'branch -D') (#1565)"
fi
NEW_BRANCH="$(json_field branch_name)"
if [[ ${RUN_RC} -eq 0 && -n "${NEW_BRANCH}" && "${NEW_BRANCH}" != "${TARGET_BRANCH}" ]]; then
    pass "S3-new-branch" "run moved to its own branch '${NEW_BRANCH}'"
else
    fail "S3-new-branch" "expected a fresh branch, got '${NEW_BRANCH}' rc=${RUN_RC}"
fi

# --- S4: State 2 with nothing ahead — attached as before (created=true, same name).
R="$(build_repo s4)"
git -C "${R}" branch -q "${TARGET_BRANCH}" origin/main
run_step "${R}"
if [[ ${RUN_RC} -eq 0 && "$(json_field branch_name)" == "${TARGET_BRANCH}" \
      && "$(json_field created)" == "true" && -d "${R}/worktrees/${TARGET_BRANCH}" ]]; then
    pass "S4-leftover-clean-attach" "clean leftover branch is attached unchanged (created=true)"
else
    fail "S4-leftover-clean-attach" "clean leftover branch not attached as before: rc=${RUN_RC} json=${RUN_JSON}"
fi

# --- S5: helper missing everywhere + dirty State 1 -> fail loud, nothing destroyed.
NOHELPER_HOME="${SCRATCH}/ahome-nohelper"
mkdir -p "${NOHELPER_HOME}/amplifier-bundle"
cp -R "${REPO_ROOT}/amplifier-bundle/tools" "${NOHELPER_HOME}/amplifier-bundle/tools"
rm -f -- "${NOHELPER_HOME}/amplifier-bundle/tools/workflow_worktree_deconflict.sh"
R="$(build_repo s5)"; add_worktree "${R}" "${TARGET_BRANCH}"
W="${R}/worktrees/${TARGET_BRANCH}"
echo "c" > "${W}/c.txt"; git -C "${W}" add c.txt; git -C "${W}" commit -q -m "ahead"
KEPT_SHA="$(git -C "${R}" rev-parse "refs/heads/${TARGET_BRANCH}")"
echo "untracked content" > "${W}/untracked.txt"
if [[ -e "${NOHELPER_HOME}/amplifier-bundle/tools/workflow_worktree_deconflict.sh" \
      || -e "${R}/amplifier-bundle/tools/workflow_worktree_deconflict.sh" ]]; then
    echo "HARNESS-ERROR: S5 helper is not absent" >&2; exit 2
fi
run_step "${R}" "${NOHELPER_HOME}"
if [[ ${RUN_RC} -ne 0 ]] && grep -qi 'refusing to reset' <<<"${RUN_STDERR}"; then
    pass "S5-fail-loud" "missing helper + dirty State 1 fails loud ('refusing to reset')"
else
    fail "S5-fail-loud" "missing helper + dirty State 1: rc=${RUN_RC}, expected non-zero with 'refusing to reset'"
fi
if [[ -f "${W}/untracked.txt" && "$(git -C "${W}" rev-parse HEAD)" == "${KEPT_SHA}" ]]; then
    pass "S5-nothing-destroyed" "untracked file and commit survive the refusal"
else
    fail "S5-nothing-destroyed" "work was destroyed although the helper was missing"
fi

# --- S6: stale/stub helper that exits 0 but prints '--base' -> V1 rejects it.
STUB_HOME="${SCRATCH}/ahome-stub"
mkdir -p "${STUB_HOME}/amplifier-bundle"
cp -R "${REPO_ROOT}/amplifier-bundle/tools" "${STUB_HOME}/amplifier-bundle/tools"
cat > "${STUB_HOME}/amplifier-bundle/tools/workflow_worktree_deconflict.sh" <<'STUB'
#!/usr/bin/env bash
# Stub of an old helper that echoes its first argument after 'resolve'.
printf '%s\n' "$2"
exit 0
STUB
R="$(build_repo s6)"
REFS_BEFORE="$(git -C "${R}" for-each-ref --format='%(refname)' refs/heads)"
run_step "${R}" "${STUB_HOME}"
REFS_AFTER="$(git -C "${R}" for-each-ref --format='%(refname)' refs/heads)"
if [[ ${RUN_RC} -ne 0 ]] && grep -q 'ERROR' <<<"${RUN_STDERR}"; then
    pass "S6-invalid-name-rejected" "helper output '--base' is rejected (ERROR, non-zero exit)"
else
    fail "S6-invalid-name-rejected" "helper output '--base' was not rejected: rc=${RUN_RC}"
fi
if [[ "${REFS_BEFORE}" == "${REFS_AFTER}" && ! -e "${R}/worktrees" ]] || \
   [[ "${REFS_BEFORE}" == "${REFS_AFTER}" && -z "$(ls -A "${R}/worktrees" 2>/dev/null)" ]]; then
    pass "S6-no-ref-created" "no branch or worktree created from the invalid name"
else
    fail "S6-no-ref-created" "a ref or worktree was created from the invalid helper output"
fi

# ===========================================================================
echo ""
echo "--- Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed ---"
if [[ ${FAIL_COUNT} -gt 0 ]]; then
    exit 1
fi
echo "PASS: Issue #1565 — re-running step-04 never destroys an existing worktree's work."
exit 0
