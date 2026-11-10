#!/usr/bin/env bash
# test-issue-1808-worktree-base.sh — issue #1808 worktree base and owned paths.
#
# workflow_worktree_root.sh gains a read-only `base` subcommand. With
# AMPLIHACK_WORK_ROOT_EXPLICIT=1 and an absolute AMPLIHACK_WORK_ROOT, worktrees
# go to <work root>/worktrees/<sanitised repo name>-<8 hex hash of main path>;
# otherwise they stay at <main>/worktrees (issue #1134 anchoring). A linked
# worktree resolves to the same base as its main checkout.
#
# amplihack_run_owned.sh record <kind> <abs path> [branch] appends
# "kind<TAB>path<TAB>branch" to $AMPLIHACK_RUN_DIR/owned-paths (mode 0600),
# and refuses relative paths, control characters and a symlinked file.
# `setup-worktree` is the recipes' single call (record, or adopt on reuse).
# Three tools exist to keep recipes under the brick limit without compressing
# them: workflow_fetch_origin.sh (fetch with retry, from
# consensus-issue-worktree.yaml), and workflow_pr_branch.sh (pr_number to head
# branch) and workflow_runtime_root.sh (per-branch runtime folders), both from
# workflow-worktree.yaml. M1-M4 run step-04 with an older AMPLIHACK_HOME
# bundle that lacks the new tools: it must still set up the worktree, and must
# prefer a checkout that holds every helper.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1808-worktree-base.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

# Fixture repository selection must not inherit an enclosing Git hook (#1937).
unset GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CEILING_DIRECTORIES GIT_COMMON_DIR \
    GIT_DIR GIT_DISCOVERY_ACROSS_FILESYSTEM GIT_INDEX_FILE GIT_INDEX_VERSION \
    GIT_NAMESPACE GIT_OBJECT_DIRECTORY GIT_PREFIX GIT_QUARANTINE_PATH GIT_WORK_TREE \
    GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT
while IFS= read -r git_variable; do
    case "${git_variable}" in GIT_CONFIG_KEY_*|GIT_CONFIG_VALUE_*) unset "${git_variable}" ;; esac
done < <(compgen -e)
unset git_variable

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
ROOT_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_root.sh"
RECORDER="${REPO_ROOT}/amplifier-bundle/tools/amplihack_run_owned.sh"

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2"; }
check() { local id="$1" msg="$2"; shift 2; if "$@"; then pass "$id" "$msg"; else fail "$id" "$msg"; fi; }

if [[ ! -f "$ROOT_HELPER" ]]; then
    echo "HARNESS-ERROR: required helper not found: $ROOT_HELPER" >&2
    exit 2
fi

SCRATCH_PARENT="${TMPDIR:-/tmp}"
[[ "${SCRATCH_PARENT}" == /* && -d "${SCRATCH_PARENT}" ]] || { echo "HARNESS-ERROR: invalid scratch parent" >&2; exit 2; }
SCRATCH_PARENT="$(cd -- "${SCRATCH_PARENT:?}" && pwd -P)" || exit 2
[[ "${SCRATCH_PARENT}" == /* && "${SCRATCH_PARENT}" != / && -d "${SCRATCH_PARENT}" ]] || { echo "HARNESS-ERROR: invalid canonical scratch parent" >&2; exit 2; }
WORK="$(mktemp -d "${SCRATCH_PARENT:?}/issue-1808.XXXXXX")" || exit 2
: "${WORK:?mktemp returned an empty scratch path}"
case "${WORK}" in "${SCRATCH_PARENT:?}/issue-1808."*) ;; *) echo "HARNESS-ERROR: invalid scratch allocation" >&2; exit 2 ;; esac
[[ -d "${WORK}" && ! -L "${WORK}" && -O "${WORK}" ]] || { echo "HARNESS-ERROR: scratch is not an owned directory" >&2; exit 2; }
WORK_PHYSICAL="$(cd -- "${WORK:?}" && pwd -P)" || exit 2
[[ -n "${WORK_PHYSICAL}" && "${WORK_PHYSICAL}" == "${WORK}" && "${WORK%/*}" == "${SCRATCH_PARENT}" ]] || { echo "HARNESS-ERROR: scratch escaped its parent" >&2; exit 2; }
trap 'rm -rf -- "${WORK:?}"' EXIT
export HOME="${WORK:?}/home" XDG_CONFIG_HOME="${WORK:?}/home/.config"
mkdir -p "${HOME:?}"

export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID

new_repo() {
    : "${1:?repository fixture path must be nonempty}"
    git init -q -b main "$1"
    echo base > "$1/file.txt"
    git -C "$1" add file.txt
    git -C "$1" commit -q -m base
}

MAIN="$WORK/src/repo"
OTHER="$WORK/elsewhere/repo"
ODD="$WORK/src/my repo;x"
mkdir -p "$WORK/src" "$WORK/elsewhere"
new_repo "$MAIN"
new_repo "$OTHER"
new_repo "$ODD"
git -C "$MAIN" worktree add -q -b linked "$MAIN/worktrees/linked"
ROOT="$WORK/work-root"

base() { bash "$ROOT_HELPER" base "$@"; }

echo "workflow_worktree_root.sh base"

out="$(base "$MAIN" 2>/dev/null)"; rc=$?
check B1 "unset explicit flag keeps <main>/worktrees" test "$rc" = 0 -a "$out" = "$MAIN/worktrees"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=0 base "$MAIN" 2>/dev/null)"
check B2 "EXPLICIT=0 keeps <main>/worktrees even with a work root" test "$out" = "$MAIN/worktrees"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$MAIN" 2>/dev/null)"; rc=$?
check B3 "EXPLICIT=1 moves the base under <work root>/worktrees" \
    bash -c '[[ "$1" = 0 && "$2" =~ ^"$3"/worktrees/repo-[0-9a-f]{8}$ ]]' _ "$rc" "$out" "$ROOT"
B3_OUT="$out"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$MAIN" 2>/dev/null)"
check B4 "the base is stable" test -n "$out" -a "$out" = "$B3_OUT"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$MAIN/worktrees/linked" 2>/dev/null)"
check B5 "a linked worktree resolves to its main checkout's base" test -n "$out" -a "$out" = "$B3_OUT"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$OTHER" 2>/dev/null)"
check B6 "two checkouts with the same name get different bases" \
    bash -c '[[ -n "$1" && "$1" != "$2" && "$1" == "$3"/worktrees/repo-* ]]' _ "$out" "$B3_OUT" "$ROOT"

out="$(AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$ODD" 2>/dev/null)"
check B7 "the repository name is sanitised to [A-Za-z0-9._-]" \
    bash -c '[[ "$(basename -- "$1")" =~ ^[A-Za-z0-9._-]+-[0-9a-f]{8}$ ]]' _ "$out"

AMPLIHACK_WORK_ROOT="relative/root" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$MAIN" >/dev/null 2>&1; rc=$?
check B8 "a relative work root is refused" test "$rc" != 0

check B9 "base is read-only (creates nothing)" test ! -e "$ROOT"

CAND="$B3_OUT/feat/x"
AMPLIHACK_WORK_ROOT="$ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 \
    bash "$ROOT_HELPER" assert-not-nested "$CAND" "$MAIN" >/dev/null 2>&1; rc=$?
check B10 "assert-not-nested accepts the external base" test "$rc" = 0

echo "workflow_branch_name.sh under an explicit work root"

BRANCH_HELPER="${REPO_ROOT}/amplifier-bundle/tools/workflow_branch_name.sh"
REUSE_ROOT="$WORK/reuse-root"
mkdir -p "$REUSE_ROOT"
REUSE_BASE="$(AMPLIHACK_WORK_ROOT="$REUSE_ROOT" AMPLIHACK_WORK_ROOT_EXPLICIT=1 base "$MAIN" 2>/dev/null)"
mkdir -p "$REUSE_BASE/feat"
git -C "$MAIN" worktree add -q -b feat/reuse-1808 "$REUSE_BASE/feat/reuse-1808"
branch_explicit() {
    TASK_DESCRIPTION="Branch: feat/reuse-1808" AMPLIHACK_WORK_ROOT="$REUSE_ROOT" \
        AMPLIHACK_WORK_ROOT_EXPLICIT="$1" \
        bash "$BRANCH_HELPER" explicit --repo-path "$MAIN" --main-repo "$MAIN"
}
out="$(branch_explicit 1 2>/dev/null)"; rc=$?
check N1 "a re-run under an explicit work root reuses its own worktree" \
    test "$rc" = 0 -a "$out" = "feat/reuse-1808"
branch_explicit 0 >/dev/null 2>&1; rc=$?
check N2 "the same worktree is another session's without the explicit root" test "$rc" = 3

echo "amplihack_run_owned.sh record"

if [[ ! -f "$RECORDER" ]]; then
    fail R0 "recorder not found: $RECORDER"
else
    RUN_DIR="$WORK/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3"
    mkdir -p "$RUN_DIR"
    OWNED="$RUN_DIR/owned-paths"
    rec() { AMPLIHACK_RUN_DIR="$RUN_DIR" bash "$RECORDER" record "$@"; }

    rec worktree "$MAIN/worktrees/linked" linked; rc=$?
    check R1 "records a worktree" test "$rc" = 0
    check R2 "worktree line is kind<TAB>path<TAB>branch" \
        grep -qxF "$(printf 'worktree\t%s\tlinked' "$MAIN/worktrees/linked")" "$OWNED"
    check R3 "owned-paths has mode 0600" test "$(stat -c %a "$OWNED" 2>/dev/null)" = 600

    rec target "$ROOT/targets/t1"; rc=$?
    check R4 "a target line has an empty branch and a trailing tab" \
        bash -c '[[ "$1" = 0 ]] && grep -qxF "$(printf "target\t%s\t" "$2")" "$3"' _ "$rc" "$ROOT/targets/t1" "$OWNED"

    lines_before="$(wc -l < "$OWNED")"
    rec target "relative/path" >/dev/null 2>&1; rc=$?
    check R5 "relative paths are refused" test "$rc" != 0 -a "$(wc -l < "$OWNED")" = "$lines_before"

    rec target "$(printf '%s/a\nworktree\t/etc' "$ROOT")" >/dev/null 2>&1; rc=$?
    check R6 "control characters are refused" test "$rc" != 0 -a "$(wc -l < "$OWNED")" = "$lines_before"

    rec bogus "$ROOT/x" >/dev/null 2>&1; rc=$?
    check R7 "unknown kinds are refused" test "$rc" != 0 -a "$(wc -l < "$OWNED")" = "$lines_before"

    LINK_RUN="$WORK/runs/link-run"
    mkdir -p "$LINK_RUN"
    echo "precious" > "$WORK/precious"
    ln -s "$WORK/precious" "$LINK_RUN/owned-paths"
    AMPLIHACK_RUN_DIR="$LINK_RUN" bash "$RECORDER" record target "$ROOT/targets/t2" >/dev/null 2>&1; rc=$?
    check R8 "a symlinked owned-paths file is refused" \
        test "$rc" != 0 -a "$(cat "$WORK/precious")" = "precious"

    (unset AMPLIHACK_RUN_DIR; bash "$RECORDER" record target "$ROOT/targets/t3" >/dev/null 2>&1)
    check R9 "outside a run nothing is written" test ! -e "$WORK/owned-paths"

    NEXT_RUN="$WORK/runs/7a1d2e3f-0000-4b8a-9f57-2c41d0e7a6b3"
    mkdir -p "$NEXT_RUN"
    adopt() { AMPLIHACK_RUN_DIR="$NEXT_RUN" bash "$RECORDER" adopt worktree "$@"; }
    adopt "$MAIN/worktrees/linked" linked; rc=$?
    check A1 "adopt records a reused worktree an earlier run recorded" \
        bash -c '[[ "$1" = 0 ]] && grep -qxF "$(printf "worktree\t%s\tlinked" "$2")" "$3"' \
        _ "$rc" "$MAIN/worktrees/linked" "$NEXT_RUN/owned-paths"
    adopt "$MAIN/worktrees/by-hand" by-hand; rc=$?
    check A2 "adopt leaves a worktree no run recorded unrecorded" \
        bash -c '[[ "$1" = 0 ]] && ! grep -qF "by-hand" "$2"' _ "$rc" "$NEXT_RUN/owned-paths"
    adopt "$MAIN/worktrees/link" >/dev/null 2>&1; rc=$?
    check A3 "adopt does not match a path that is only a prefix" \
        bash -c '! grep -qF "$(printf "worktree\t%s\t" "$1")" "$2"' _ "$MAIN/worktrees/link" "$NEXT_RUN/owned-paths"

    # setup-worktree: the recipe's single call (consensus-issue-worktree.yaml).
    SETUP_RUN="$WORK/runs/9b2e3f4a-0000-4b8a-9f57-2c41d0e7a6b3"
    mkdir -p "$SETUP_RUN" "$WORK/alias"
    ln -s "$MAIN/worktrees/linked" "$WORK/alias/linked-link"
    setup() { AMPLIHACK_RUN_DIR="$SETUP_RUN" bash "$RECORDER" setup-worktree "$@"; }
    setup create-from-base "$WORK/alias/linked-link" linked; rc=$?
    check S1 "setup-worktree records the physical path for a new worktree" \
        bash -c '[[ "$1" = 0 ]] && grep -qxF "$(printf "worktree\t%s\tlinked" "$2")" "$3"' \
        _ "$rc" "$MAIN/worktrees/linked" "$SETUP_RUN/owned-paths"
    REUSE_RUN="$WORK/runs/9b2e3f4a-1111-4b8a-9f57-2c41d0e7a6b3"
    mkdir -p "$REUSE_RUN" "$MAIN/worktrees/by-hand"
    AMPLIHACK_RUN_DIR="$REUSE_RUN" bash "$RECORDER" setup-worktree reuse-clean-worktree "$MAIN/worktrees/by-hand" by-hand; rc=$?
    check S2 "setup-worktree adopts on reuse, so a worktree no run recorded stays unrecorded" \
        bash -c '[[ "$1" = 0 ]] && ! grep -qsF "by-hand" "$2"' _ "$rc" "$REUSE_RUN/owned-paths"
    out="$(setup create-from-base "$WORK/missing-worktree" x 2>&1)"; rc=$?
    check S3 "setup-worktree warns and exits 1 when the path cannot be resolved" \
        bash -c '[[ "$1" = 1 ]] && [[ "$2" = *"WARNING: could not record"*"for cleanup (issue #1808)"* ]]' _ "$rc" "$out"
    (unset AMPLIHACK_RUN_DIR; bash "$RECORDER" setup-worktree create-from-base "$WORK/missing-worktree" x); rc=$?
    check S4 "setup-worktree does nothing outside a run" test "$rc" = 0
    bash "$RECORDER" setup-worktree create-from-base >/dev/null 2>&1; rc=$?
    check S5 "setup-worktree without a path is a usage error" test "$rc" = 2
fi

echo "workflow_fetch_origin.sh"
FETCHER="${REPO_ROOT}/amplifier-bundle/tools/workflow_fetch_origin.sh"
NO_REMOTE="$WORK/no-remote"
new_repo "$NO_REMOTE"
out="$(cd "$NO_REMOTE" && bash "$FETCHER" 2>&1)"; rc=$?
check F1 "a fetch that keeps failing retries three times and still exits 0" \
    bash -c '[[ "$1" = 0 ]] && [[ "$2" = *"retrying (2/3)"* ]] && [[ "$2" = *"after 3 attempts"* ]]' _ "$rc" "$out"
git clone -q "$MAIN" "$WORK/cloned"
(cd "$WORK/cloned" && bash "$FETCHER" >/dev/null 2>&1); rc=$?
check F2 "a working fetch exits 0" test "$rc" = 0

echo "workflow_pr_branch.sh"
# A BASH_ENV file may define a gh function that would hide the shim.
unset BASH_ENV
PR_BRANCH="${REPO_ROOT}/amplifier-bundle/tools/workflow_pr_branch.sh"
GH_DIR="$WORK/gh-shim"
mkdir -p "$GH_DIR"
printf '#!/usr/bin/env bash\n[ "$1 $2 $3" = "pr view 342" ] && printf feat/from-pr && exit 0\nexit 1\n' > "$GH_DIR/gh"
chmod +x "$GH_DIR/gh"
out="$(PATH="$GH_DIR:$PATH" bash "$PR_BRANCH" 342 2>/dev/null)"; rc=$?
check P1 "a PR number resolves to its head branch" bash -c '[[ "$1" = 0 && "$2" = feat/from-pr ]]' _ "$rc" "$out"
for bad in "342 --repo evil/x" "-1" "0" '$(id)' ""; do
    out="$(PATH="$GH_DIR:$PATH" bash "$PR_BRANCH" "$bad" 2>&1)"; rc=$?
    check P2 "pr_number '$bad' is refused before gh runs" \
        bash -c '[[ "$1" = 1 && "$2" = *"arg-injection guard"* ]]' _ "$rc" "$out"
done
out="$(PATH="$GH_DIR:$PATH" bash "$PR_BRANCH" 7 2>&1)"; rc=$?
check P3 "a PR gh cannot find is an error" bash -c '[[ "$1" = 1 && "$2" = *"returned no branch name"* ]]' _ "$rc" "$out"
out="$(PATH=/nonexistent /bin/bash "$PR_BRANCH" 342 2>&1)"; rc=$?
check P4 "no gh on PATH is an error" bash -c '[[ "$1" = 1 && "$2" = *"not on PATH"* ]]' _ "$rc" "$out"

echo "workflow_runtime_root.sh"
RT_ROOT="${REPO_ROOT}/amplifier-bundle/tools/workflow_runtime_root.sh"
rt_dirs_exist() { local d; for d in locks reflection logs metrics provenance; do [[ -d "$1/$d" ]] || return 1; done; }
out="$(env -u AMPLIHACK_RUNTIME_ROOT XDG_RUNTIME_DIR="$WORK/xdg" bash "$RT_ROOT" feat/x 2>/dev/null)"; rc=$?
check T1 "XDG_RUNTIME_DIR gives <xdg>/amplihack-runtime/<branch> with its folders" \
    bash -c '[[ "$1" = 0 && "$2" = "$3/xdg/amplihack-runtime/feat/x" ]]' _ "$rc" "$out" "$WORK"
check T1 "the five runtime folders exist" rt_dirs_exist "$WORK/xdg/amplihack-runtime/feat/x"
out="$(AMPLIHACK_RUNTIME_ROOT="$WORK/given" XDG_RUNTIME_DIR="$WORK/xdg" bash "$RT_ROOT" feat/x 2>/dev/null)"; rc=$?
check T2 "a set AMPLIHACK_RUNTIME_ROOT is kept" bash -c '[[ "$1" = 0 && "$2" = "$3/given" ]]' _ "$rc" "$out" "$WORK"
check T2 "the five runtime folders exist under it" rt_dirs_exist "$WORK/given"
# T3 keeps the real helper's fallback selection and checks all five mkdir
# destinations, but maps its filesystem writes into an owned fixture.
RT_TMP_BRANCH="issue-1808-test-$$-${RANDOM}"
RT_TMP_FIXTURE="$(mktemp -d "${WORK:?}/runtime-fallback.XXXXXX")" || exit 2
: "${RT_TMP_FIXTURE:?}"
rt_private_fallback() (
    export RT_FALLBACK_BRANCH="${RT_TMP_BRANCH:?}" RT_FALLBACK_FIXTURE="${RT_TMP_FIXTURE:?}"
    mkdir() {
        [[ "${1:-}" == -p ]] || return 2
        shift
        local path suffix
        for path in "$@"; do
            case "${path}" in
                "/tmp/amplihack-runtime/${RT_FALLBACK_BRANCH:?}/"*) ;;
                *) echo "unexpected runtime fallback mkdir destination: ${path}" >&2; return 2 ;;
            esac
            suffix="${path#"/tmp/amplihack-runtime/${RT_FALLBACK_BRANCH:?}/"}"
            case "${suffix}" in locks|reflection|logs|metrics|provenance) ;; *) return 2 ;; esac
            command mkdir -p "${RT_FALLBACK_FIXTURE:?}/runtime/${suffix:?}" || return $?
        done
    }
    export -f mkdir
    env -u AMPLIHACK_RUNTIME_ROOT -u XDG_RUNTIME_DIR bash "${RT_ROOT:?}" "${RT_TMP_BRANCH:?}"
)
out="$(rt_private_fallback 2>/dev/null)"; rc=$?
check T3 "without XDG_RUNTIME_DIR the root is /tmp/amplihack-runtime/<branch>" \
    bash -c '[[ "$1" = 0 && "$2" = "/tmp/amplihack-runtime/$3" ]]' _ "$rc" "$out" "$RT_TMP_BRANCH"
check T3 "the five fallback folders exist inside the owned fixture" rt_dirs_exist "${RT_TMP_FIXTURE:?}/runtime"
out="$(bash "$RT_ROOT" 2>&1)"; rc=$?
check T4 "no branch name is an error" bash -c '[[ "$1" = 1 && "$2" = *usage* ]]' _ "$rc" "$out"
: > "$WORK/blocker"
out="$(AMPLIHACK_RUNTIME_ROOT="$WORK/blocker" bash "$RT_ROOT" feat/x 2>/dev/null)"; rc=$?
check T5 "folders that cannot be created are an error with nothing on stdout" \
    bash -c '[[ "$1" = 1 && -z "$2" ]]' _ "$rc" "$out"

echo "workflow-worktree.yaml calls the tools"
RECIPE="${REPO_ROOT}/amplifier-bundle/recipes/workflow-worktree.yaml"
check R1 "ownership goes through amplihack_run_owned.sh setup-worktree" \
    grep -qF 'amplihack_run_owned.sh" setup-worktree "$@"' "$RECIPE"
check R2 "pr_number goes through workflow_pr_branch.sh" grep -qF 'workflow_pr_branch.sh' "$RECIPE"
check R3 "no inline gh pr view is left in the recipe" bash -c '! grep -qF "gh pr view" "$1"' _ "$RECIPE"
check R4 "the runtime root goes through workflow_runtime_root.sh" grep -qF 'workflow_runtime_root.sh' "$RECIPE"
check R5 "wt_own is not a one-line || chain" bash -c '! grep -qE "^ *wt_own\(\) \{.*\}" "$1"' _ "$RECIPE"

echo "step-04 with an older AMPLIHACK_HOME bundle (recipe and tools from different versions)"
# The stale bundle lacks workflow_runtime_root.sh and workflow_pr_branch.sh, and
# its workflow_worktree_root.sh has no `base`, like an install made before #1808.
STEP04="$WORK/step-04.sh"
awk '/^  - id: "step-04-setup-worktree"/ { inblk = 1; next }
     inblk && /^  - id:/ { exit }
     inblk && /^    command:[[:space:]]*\|/ { grab = 1; next }
     grab && /^    [A-Za-z_-]+:/ { grab = 0 }
     grab { sub(/^      /, ""); print }' "$RECIPE" > "$STEP04"
STALE="$WORK/stale-home"
mkdir -p "$STALE/amplifier-bundle/tools"
cp "${REPO_ROOT}"/amplifier-bundle/tools/*.sh "$STALE/amplifier-bundle/tools/"
rm -f "$STALE/amplifier-bundle/tools/workflow_runtime_root.sh" "$STALE/amplifier-bundle/tools/workflow_pr_branch.sh"
printf '#!/usr/bin/env bash\n[ "${1:-}" != base ] || { echo "ERROR: unknown operation base." >&2; exit 1; }\nexec bash %q "$@"\n' \
    "${REPO_ROOT}/amplifier-bundle/tools/workflow_worktree_root.sh" > "$STALE/amplifier-bundle/tools/workflow_worktree_root.sh"
# new_origin_repo <dir>: a repository with an origin holding main and feat/from-pr.
new_origin_repo() {
    : "${1:?repository fixture path must be nonempty}"
    git init -q --bare -b main "$1.git"
    git clone -q "$1.git" "$1" 2>/dev/null
    echo base > "$1/file.txt"; git -C "$1" add file.txt; git -C "$1" commit -q -m base
    git -C "$1" push -q origin main 2>/dev/null; git -C "$1" push -q origin main:feat/from-pr 2>/dev/null
    git -C "$1" remote set-head origin main
}
run_step04() {  # run_step04 <repo> [pr_number]; sets out, err, rc
    out="$(cd "$1" && env -u AMPLIHACK_RUNTIME_ROOT HOME="$WORK/home" AMPLIHACK_HOME="$STALE" \
        XDG_RUNTIME_DIR="$WORK/xdg-step" PATH="$GH_DIR:$PATH" REPO_PATH="$1" \
        TASK_DESCRIPTION="stale bundle check" ISSUE_NUMBER=1808 BRANCH_PREFIX=feat PR_NUMBER="${2:-}" \
        bash "$STEP04" 2>"$WORK/step04.err")"; rc=$?; err="$(cat "$WORK/step04.err")"
}
PLAIN="$WORK/plain"
new_origin_repo "$PLAIN"
run_step04 "$PLAIN"
check M1 "a TOOLS_DIR without workflow_runtime_root.sh still sets up the worktree" \
    bash -c '[[ "$1" = 0 && "$2" = *"\"worktree_path\": \"$3/worktrees/feat/issue-1808-"* ]]' _ "$rc" "$out" "$PLAIN"
check M1 "the runtime root and its five folders are made inline" \
    bash -c '[[ "$1" = *"\"runtime_root\": \"$2/xdg-step/amplihack-runtime/feat/issue-1808-"* ]] && ls -d "$2"/xdg-step/amplihack-runtime/feat/issue-1808-*/provenance >/dev/null' _ "$out" "$WORK"
check M2 "a root helper without \`base\` is not asked for it" bash -c '[[ "$1" != *"unknown operation base"* ]]' _ "$err"
run_step04 "$PLAIN" 342
check M3 "pr_number with no tier holding workflow_pr_branch.sh fails with its name" \
    bash -c '[[ "$1" = 1 && "$2" = *"workflow_pr_branch.sh not found"* ]]' _ "$rc" "$err"
CHECKOUT="$WORK/checkout"
new_origin_repo "$CHECKOUT"
mkdir -p "$CHECKOUT/amplifier-bundle/tools"
cp "${REPO_ROOT}"/amplifier-bundle/tools/*.sh "$CHECKOUT/amplifier-bundle/tools/"
run_step04 "$CHECKOUT" 342
check M4 "a checkout holding every helper is used in place of the stale AMPLIHACK_HOME" \
    bash -c '[[ "$1" = 0 && "$2" = *"\"branch_name\": \"feat/from-pr\""* ]]' _ "$rc" "$out"

echo
echo "issue-1808 worktree base: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ "$FAIL_COUNT" -eq 0 ]]
