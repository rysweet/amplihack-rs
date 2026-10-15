#!/usr/bin/env bash
# test-issue-1494-1560-worktree-safety.sh — regression tests for #1494 and #1560.
#
# Issue #1494: cleanup_known_workflow_runtime_artifacts ran `rm -rf <repo>/worktrees`
# when a run started from the main checkout, destroying every concurrent run's task
# worktree and its uncommitted work. On the main checkout `worktrees/` must be left
# alone (and excluded from git, so clean-status gates still pass); on a linked task
# worktree its nested scratch worktrees are still cleaned.
#
# Issue #1560: workflow_worktree_sweep.sh removed worktrees whose branch had no
# commits ahead of the base, without looking at `git status`, so uncommitted and
# untracked work was deleted. `sweep` and `cleanup_on_failure` must keep any
# worktree with uncommitted or untracked files, and still prune clean ones.
#
# Keeping worktrees must not create new failures either: the sweep must not take
# a live worktree's index lock, and a kept worktree whose PR was squash-merged
# must be pruned rather than pushed back to origin as an "unmerged" branch.
#
# Usage: bash amplifier-bundle/recipes/tests/test-issue-1494-1560-worktree-safety.sh
# Exit codes: 0 = pass, 1 = fail, 2 = test harness error.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
TOOLS="${REPO_ROOT}/amplifier-bundle/tools"
ARTIFACTS="${TOOLS}/workflow_runtime_artifacts.sh"
SWEEP="${TOOLS}/workflow_worktree_sweep.sh"
FINAL_STATUS="${TOOLS}/workflow_final_status.sh"

for f in "$ARTIFACTS" "$SWEEP" "$FINAL_STATUS"; do
    if [[ ! -f "$f" ]]; then
        echo "HARNESS-ERROR: required helper not found: $f" >&2
        exit 2
    fi
done

PASS_COUNT=0
FAIL_COUNT=0
pass() { PASS_COUNT=$((PASS_COUNT + 1)); echo "  PASS[$1]: $2"; }
fail() { FAIL_COUNT=$((FAIL_COUNT + 1)); echo "  FAIL[$1]: $2" >&2; }
check() { local id="$1" msg="$2"; shift 2; if "$@"; then pass "$id" "$msg"; else fail "$id" "$msg"; fi; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/issue-1494-1560.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1

# new_repo <dir> [origin-dir] — a main checkout on branch `main` with one commit
# (file.txt, and a .gitignore that ignores target/), pushed to a new bare origin
# when [origin-dir] is given. It never ignores worktrees/.
new_repo() {
    git init -q -b main "$1"
    echo base > "$1/file.txt"
    echo 'target/' > "$1/.gitignore"
    git -C "$1" add file.txt .gitignore
    git -C "$1" commit -q -m base
    if [[ -n "${2:-}" ]]; then
        git init -q --bare -b main "$2"
        git -C "$1" remote add origin "$2"
        git -C "$1" push -q -u origin main
    fi
}

registered() { git -C "$1" worktree list --porcelain | grep -qx "worktree $2"; }
branch_exists() { git -C "$1" rev-parse --verify --quiet "refs/heads/$2" >/dev/null; }
remote_branch_exists() { [[ -n "$(git -C "$1" ls-remote --heads origin "$2")" ]]; }
status_clean() { [[ -z "$(git -C "$1" status --porcelain)" ]]; }
not() { ! "$@"; }

echo "== #1494: runtime-artifact cleanup on the main checkout =="
MAIN="$WORK/main"
new_repo "$MAIN"
git -C "$MAIN" worktree add -q -b task-a "$MAIN/worktrees/task-a"
echo wip >> "$MAIN/worktrees/task-a/file.txt"
echo new > "$MAIN/worktrees/task-a/untracked.txt"
(
    # shellcheck source=/dev/null
    . "$ARTIFACTS"
    cleanup_known_workflow_runtime_artifacts "$MAIN"
) 2>"$WORK/a.err" || true
check 1494-a "another run's worktree directory survives" test -d "$MAIN/worktrees/task-a"
check 1494-b "its uncommitted change survives" grep -q wip "$MAIN/worktrees/task-a/file.txt"
check 1494-c "its untracked file survives" test -f "$MAIN/worktrees/task-a/untracked.txt"
check 1494-d "its registration survives" registered "$MAIN" "$MAIN/worktrees/task-a"
check 1494-e "the main checkout's status is clean: worktrees/ is excluded, not untracked" status_clean "$MAIN"
check 1494-f "the skip is reported" grep -q '#1494' "$WORK/a.err"

echo "== #1494: nested scratch worktrees on a linked task worktree are still cleaned =="
TASK="$MAIN/worktrees/task-b"
git -C "$MAIN" worktree add -q -b task-b "$TASK"
git -C "$TASK" worktree add -q -b scratch "$TASK/worktrees/scratch"
(
    # shellcheck source=/dev/null
    . "$ARTIFACTS"
    cleanup_known_workflow_runtime_artifacts "$TASK"
) 2>/dev/null || true
check 1494-g "the linked worktree's worktrees/ is removed" not test -e "$TASK/worktrees"
check 1494-h "the nested worktree is deregistered" not registered "$MAIN" "$TASK/worktrees/scratch"
check 1494-i "the linked task worktree itself is kept" test -f "$TASK/file.txt"

echo "== #1494: the no-diff final-status gate passes on a main checkout with kept worktrees =="
FS="$WORK/final"
new_repo "$FS" "$WORK/final-origin.git"
git -C "$FS" worktree add -q -b sibling "$FS/worktrees/sibling"
echo wip > "$FS/worktrees/sibling/wip.txt"
fs_rc=0
(
    cd "$FS"
    env -u PR_URL -u PR_NUMBER PR_PUBLISH_RESULT_STATE=no-diff WORKFLOW_RUNTIME_ARTIFACT_HELPER="$ARTIFACTS" \
        bash "$FINAL_STATUS"
) >"$WORK/final.out" 2>&1 || fs_rc=$?
check 1494-j "the gate confirms the no-diff state (rc 0)" test "$fs_rc" -eq 0
check 1494-k "it does not report an unconfirmable state" not grep -q 'could not confirm that state' "$WORK/final.out"
check 1494-l "the sibling's uncommitted work survives" test -f "$FS/worktrees/sibling/wip.txt"

echo "== #1560: sweep keeps worktrees with uncommitted or untracked work =="
SW="$WORK/sweep"
new_repo "$SW"
for b in dirty untracked clean ignoredonly; do git -C "$SW" worktree add -q -b "$b" "$SW/worktrees/$b"; done
echo wip >> "$SW/worktrees/dirty/file.txt"
echo new > "$SW/worktrees/untracked/notes.txt"
mkdir -p "$SW/worktrees/ignoredonly/target" && echo bin > "$SW/worktrees/ignoredonly/target/out.bin"
AMPLIHACK_WORKTREE_STALE_SECS=0 AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" sweep "$SW" 2>"$WORK/sweep.err" || true
check 1560-a "a worktree with an uncommitted change is kept" grep -q wip "$SW/worktrees/dirty/file.txt"
check 1560-b "its branch is kept" branch_exists "$SW" dirty
check 1560-c "a worktree with an untracked file is kept" test -f "$SW/worktrees/untracked/notes.txt"
check 1560-d "its branch is kept" branch_exists "$SW" untracked
check 1560-e "a clean stale worktree is still pruned" not test -e "$SW/worktrees/clean"
check 1560-f "the clean worktree's branch is still deleted" not branch_exists "$SW" clean
check 1560-g "a worktree with only ignored build output is still pruned" not test -e "$SW/worktrees/ignoredonly"
check 1560-h "keeping is reported" grep -q '#1560' "$WORK/sweep.err"
check 1560-i "the sweep excludes worktrees/ so the main checkout's status is clean" status_clean "$SW"

echo "== #1560: the sweep's status probe does not rewrite a live worktree's index =="
LK="$WORK/lock"
new_repo "$LK"
git -C "$LK" worktree add -q -b live "$LK/worktrees/live"
echo new > "$LK/worktrees/live/notes.txt"
INDEX="$(git -C "$LK/worktrees/live" rev-parse --path-format=absolute --git-path index)"
sleep 1
touch "$LK/worktrees/live/file.txt"   # stale stat data: a plain `git status` would refresh and rewrite the index
before="$(stat -c %y "$INDEX")"
AMPLIHACK_WORKTREE_STALE_SECS=0 AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" sweep "$LK" 2>/dev/null || true
check 1560-j "the probed worktree's index is untouched" test "$(stat -c %y "$INDEX")" = "$before"

echo "== #1560: cleanup_on_failure keeps a dirty worktree, removes a clean one =="
CF="$WORK/cof"
new_repo "$CF"
git -C "$CF" worktree add -q -b keep "$CF/worktrees/keep"
echo new > "$CF/worktrees/keep/notes.txt"
git -C "$CF" worktree add -q -b drop "$CF/worktrees/drop"
AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" cleanup_on_failure "$CF" keep 2>/dev/null || true
AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" cleanup_on_failure "$CF" drop 2>/dev/null || true
check 1560-k "a failed run's worktree with an untracked file is kept" test -f "$CF/worktrees/keep/notes.txt"
check 1560-l "its branch is kept" branch_exists "$CF" keep
check 1560-m "a failed run's clean worktree is still removed" not test -e "$CF/worktrees/drop"
check 1560-n "its branch is still deleted" not branch_exists "$CF" drop

echo "== #1560: cleanup_on_failure keeps work it cannot attribute to a worktree =="
git -C "$CF" worktree add -q -b gone "$CF/worktrees/gone"
echo wip >> "$CF/worktrees/gone/file.txt"
git -C "$CF" update-ref -d refs/heads/gone
AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" cleanup_on_failure "$CF" gone 2>/dev/null || true
check 1560-o "a dirty worktree whose branch is gone is kept" grep -q wip "$CF/worktrees/gone/file.txt"
# A plain directory, not a worktree, under an ignored worktrees/: git would
# report the enclosing (clean) repository's status for it.
printf '/worktrees/\n' >> "$CF/.git/info/exclude"
mkdir -p "$CF/worktrees/foo" && echo wip > "$CF/worktrees/foo/wip.txt"
AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" cleanup_on_failure "$CF" foo 2>/dev/null || true
check 1560-p "files in a non-worktree directory under worktrees/ are kept" test -f "$CF/worktrees/foo/wip.txt"

echo "== #1560: a kept worktree whose PR was squash-merged is pruned, not pushed back =="
SQ="$WORK/squash"
new_repo "$SQ" "$WORK/squash-origin.git"
git -C "$SQ" worktree add -q -b feat "$SQ/worktrees/feat"
echo feature > "$SQ/worktrees/feat/feature.txt"
git -C "$SQ/worktrees/feat" add feature.txt
git -C "$SQ/worktrees/feat" commit -q -m feature
git -C "$SQ/worktrees/feat" push -q -u origin feat
git -C "$SQ" merge -q --squash feat >/dev/null
git -C "$SQ" commit -q -m "feature (#1)"
git -C "$SQ" push -q origin main
git -C "$SQ" push -q origin --delete feat
AMPLIHACK_WORKTREE_STALE_SECS=0 AMPLIHACK_WORKTREE_BASE_REF=main bash "$SWEEP" sweep "$SQ" 2>/dev/null || true
check 1560-q "the merged branch is not pushed back to origin" not remote_branch_exists "$SQ" feat
check 1560-r "the merged worktree is pruned" not test -e "$SQ/worktrees/feat"
check 1560-s "the merged local branch is deleted" not branch_exists "$SQ" feat

echo
echo "Summary: ${PASS_COUNT} passed, ${FAIL_COUNT} failed"
[ "$FAIL_COUNT" -eq 0 ] || exit 1
exit 0
