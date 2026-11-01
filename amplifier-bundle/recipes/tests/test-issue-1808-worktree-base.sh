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
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1808-worktree-base.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -uo pipefail

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

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1808.XXXXXX")"
WORK="$(cd "$WORK" && pwd -P)"
trap 'rm -rf "$WORK"' EXIT

export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
unset AMPLIHACK_WORK_ROOT AMPLIHACK_WORK_ROOT_EXPLICIT AMPLIHACK_RUN_DIR AMPLIHACK_RUN_ID

new_repo() {
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
fi

echo
echo "issue-1808 worktree base: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[[ "$FAIL_COUNT" -eq 0 ]]
