#!/usr/bin/env bash
# QA for #2335, reproducing communication #851: a run's worktree whose only
# untracked files are the code-graph indexer's .amplihack/graph_db and
# .amplihack/indexing.pid must reach the PR stage of step 16. A product file
# left uncommitted must still fail step 16, and the message must name it.
#
# Flow: main checkout and bare origin -> step-04 worktree sweep (creation-time
# exclusion) -> linked task worktree with one pushed commit -> indexer files
# written -> step 16 (workflow_publish_pr.sh) with a gh stand-in on PATH.
# Everything lives under a 0700 scratch root in TMPDIR; no real home, login,
# remote or gh is used.
set -euo pipefail
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
tools="$repo/amplifier-bundle/tools"
root=$(mktemp -d "${TMPDIR:-/tmp}/amplihack-2335-qa.XXXXXX")
chmod 0700 "$root"
trap 'rm -rf -- "$root"' EXIT
export HOME="$root/home" TMPDIR="$root/tmp"
mkdir -p "$HOME" "$TMPDIR" "$root/bin"; chmod 0700 "$HOME"
export GIT_CONFIG_GLOBAL="$root/gitconfig" GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0
git config --global user.email qa@example.com
git config --global user.name "QA"
git config --global init.defaultBranch main

git init -q --bare "$root/origin.git"
git init -q "$root/main"
cd "$root/main"
echo base > README.md; git add README.md; git commit -qm base
git remote add origin "$root/origin.git"
git push -q origin main
git fetch -q origin

# Step 04: the sweep runs against the main checkout before the worktree exists.
bash "$tools/workflow_worktree_sweep.sh" sweep "$root/main"
wt="$root/main/worktrees/feat/issue-851"
git worktree add -q -b feat/issue-851 "$wt" origin/main
cd "$wt"
echo fix > fix.txt; git add fix.txt; git commit -qm "fix: the change"
git push -q origin feat/issue-851
# Step 16 derives the GitHub repository from origin's URL; the work is already
# pushed to the local bare origin, so point origin at a GitHub-shaped URL now.
git remote set-url origin https://github.com/owner/repo.git

# The indexer writes its state into the worktree, as on 22 Nov.
mkdir -p .amplihack; : > .amplihack/graph_db; echo 4242 > .amplihack/indexing.pid

printf '#!/usr/bin/env bash\necho "$*" >> %q\nexit 77\n' "$root/gh.log" > "$root/bin/gh"
chmod 0755 "$root/bin/gh"
step16() {
  PATH="$root/bin:$PATH" REMOTE_HOST_TYPE=github ISSUE_NUMBER=851 \
    WORKFLOW_RUNTIME_ARTIFACT_HELPER="$tools/workflow_runtime_artifacts.sh" \
    bash "$tools/workflow_publish_pr.sh" >"$root/out.json" 2>"$root/err.txt"
}

fail() { echo "FAIL: $*" >&2; echo "--- stdout"; cat "$root/out.json"; echo "--- stderr"; cat "$root/err.txt"; exit 1; }

[ -z "$(git status --porcelain)" ] || fail "indexer files show in git status after the sweep"
step16 || true
grep -q FAILED_DIRTY_WORKTREE "$root/out.json" && fail "runtime-only files failed step 16 as dirty"
[ -s "$root/gh.log" ] || fail "step 16 did not reach gh"
echo "PASS: indexer files are excluded and step 16 reaches gh"

: > "$root/gh.log"
echo stray > forgotten.rs
rc=0; step16 || rc=$?
[ "$rc" = 1 ] || fail "an uncommitted product file must fail step 16 (exit $rc)"
msg=$(jq -r .message "$root/out.json")
case "$msg" in *"?? forgotten.rs"*) ;; *) fail "message does not name forgotten.rs" ;; esac
case "$msg" in *.amplihack*) fail "message names runtime files" ;; esac
case "$msg" in *"rerun step 16"*) ;; *) fail "message gives no recovery step" ;; esac
[ ! -s "$root/gh.log" ] || fail "a dirty worktree must not reach gh"
echo "PASS: an uncommitted product file fails step 16 and is named"
