#!/usr/bin/env bash
# Issue #1269 — a push that publishes the wrong commit must fail, loudly.
#
# The incident: a run committed nine commits, then `git checkout` moved HEAD to
# a branch literally named `feature`, leaving the worktree on plain `main` with
# 4535 uncommitted files. The branch it then published to origin pointed at
# `main` and contained none of the work. The pipeline reported a push problem,
# so the investigation went after credentials and protected branches, while the
# actual defect was only visible in `git reflog`.
#
# These tests build real repositories and drive the real script. The central
# case reproduces the incident's shape exactly: a remote branch that exists and
# was pushed successfully, but points at a commit that is not the one under test.

set -uo pipefail

SCRIPT="$(cd "$(dirname "$0")/.." && pwd)/amplifier-bundle/tools/verify_pushed_ref.sh"
[ -x "$SCRIPT" ] || { echo "missing or non-executable $SCRIPT"; exit 1; }

fails=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; fails=$((fails + 1)); }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/issue1269.XXXXXX")
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

# Keep git from reading the caller's identity, hooks or the ambient repo.
export GIT_CONFIG_NOSYSTEM=1
export HOME="$WORK/fakehome"
mkdir -p "$HOME"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@e
export GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@e
# The hook environment leaks GIT_DIR into child processes; make sure it cannot
# reach the repositories these tests build.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

git init -q --bare "$WORK/origin.git"
git init -q -b main "$WORK/repo"
cd "$WORK/repo"
git remote add origin "$WORK/origin.git"

echo base > f.txt && git add f.txt && git commit -q -m base
BASE_SHA=$(git rev-parse HEAD)
git push -q origin main

git checkout -q -b work
echo work > f.txt && git commit -q -am "the work that must not be lost"
WORK_SHA=$(git rev-parse HEAD)

# --- 1. the incident: remote branch exists but holds the wrong commit --------
# Publish 'work' pointing at the base commit, exactly as the run published a
# branch pointing at plain main.
git push -q origin "$BASE_SHA:refs/heads/work"
out=$("$SCRIPT" origin work "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 1 ] && grep -q 'published the wrong commit' <<<"$out"; then
  pass "wrong published commit is caught (exit 1)"
else
  fail "wrong published commit: expected exit 1, got $rc: ${out:0:160}"
fi
# The message must name both SHAs, or the investigator is back to guessing.
if grep -q "$WORK_SHA" <<<"$out" && grep -q "$BASE_SHA" <<<"$out"; then
  pass "error names both the expected and the actual commit"
else
  fail "error does not name both SHAs"
fi

# --- 2. the correct case must pass ------------------------------------------
git push -q -f origin work
out=$("$SCRIPT" origin work "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 0 ]; then
  pass "a correctly published branch verifies (exit 0)"
else
  fail "correct push rejected, exit $rc: ${out:0:160}"
fi

# --- 3. remote ref missing entirely -----------------------------------------
out=$("$SCRIPT" origin never-pushed "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 1 ] && grep -q 'does not exist' <<<"$out"; then
  pass "a missing remote ref is caught"
else
  fail "missing ref: expected exit 1, got $rc: ${out:0:160}"
fi

# --- 4. placeholder branch names WARN but do not block ----------------------
# 'feature' is the literal name the incident checked out, so it is worth
# flagging. It is not worth refusing: an unusual branch name is not an illegal
# one, and a repository may legitimately have a branch called `feature` --
# `step15_fast_forwards_ahead_branch_without_rebase` has a fixture that does.
# The fault in #1269 was the published commit, not the name, and that is what
# the SHA comparison gates on.
git push -q -f origin work:refs/heads/feature
out=$("$SCRIPT" origin feature "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 0 ]; then
  pass "a branch named 'feature' still verifies when the commit is right"
else
  fail "'feature' was blocked despite a correct commit, exit $rc: ${out:0:160}"
fi
grep -qi 'placeholder' <<<"$out" \
  && pass "'feature' is still flagged as a likely unsubstituted placeholder" \
  || fail "no warning emitted for a placeholder-looking branch name"

# And the real check still bites on that same branch when the commit is wrong.
git push -q -f origin "$BASE_SHA:refs/heads/feature"
out=$("$SCRIPT" origin feature "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 1 ] && grep -q 'published the wrong commit' <<<"$out"; then
  pass "a wrong commit on 'feature' is still caught (the name never gates)"
else
  fail "wrong commit on 'feature' not caught: exit $rc: ${out:0:160}"
fi

# A real issue-derived name verifies and is not flagged.
git push -q -f origin work:refs/heads/fix/1269-real-name
out=$("$SCRIPT" origin fix/1269-real-name "$WORK_SHA" 2>&1); rc=$?
if [ $rc -eq 0 ] && ! grep -qi 'placeholder' <<<"$out"; then
  pass "a real issue-derived branch name verifies with no warning"
else
  fail "real branch name mishandled, exit $rc: ${out:0:160}"
fi

# --- 5. usage errors are distinguishable from verification failures ---------
# Exit 2 vs exit 1 matters: a caller must not report "wrong commit published"
# when it simply passed bad arguments.
out=$("$SCRIPT" origin work 2>&1); rc=$?
[ $rc -eq 2 ] && pass "missing argument exits 2, not 1" || fail "missing arg exit $rc, expected 2"
out=$("$SCRIPT" origin work not-a-sha 2>&1); rc=$?
[ $rc -eq 2 ] && pass "a malformed SHA exits 2, not 1" || fail "bad sha exit $rc, expected 2"

echo
if [ "$fails" -gt 0 ]; then
  echo "issue-1269: $fails check(s) failed"
  exit 1
fi
echo "issue-1269: all checks passed"
