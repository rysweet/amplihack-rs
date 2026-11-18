#!/usr/bin/env bash
# Publish final cleanup and audit work from this run's recorded worktree.
set -euo pipefail
umask 022
export GIT_PAGER=cat PAGER=cat LESS=FRX
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
wt="$(bash "$SCRIPT_DIR/worktree_setup_path.sh" step-20b-push-cleanup)"
cd -P "$wt"
# Keep the existing dedicated linked-worktree requirement for staging.
gd="$(git rev-parse --absolute-git-dir)"
common="$(git rev-parse --path-format=absolute --git-common-dir)"
if [ "$gd" = "$common" ]; then
  echo "ERROR: publication requires the run's linked PR worktree" >&2
  exit 1
fi
current_branch="$(git branch --show-current)"
remote_branch="$(bash "$SCRIPT_DIR/workflow_remote_branch.sh" .)"
if [ -z "$current_branch" ] || [ -z "$remote_branch" ]; then
  echo "ERROR: publication requires an attached PR branch" >&2
  exit 1
fi
git check-ref-format "refs/heads/$remote_branch"
echo "=== Pushing Cleanup Changes ==="
# Synchronize before creating cleanup commits; never rewrite existing history.
bash "$SCRIPT_DIR/workflow_pr_head_sync.sh" --strict
. "$SCRIPT_DIR/workflow_runtime_artifacts.sh"
preflight_known_workflow_runtime_artifacts .
amplihack hygiene artifact-guard --repo . --mode pre-publish
git add -A
amplihack hygiene artifact-guard --repo . --mode pre-publish
STAGED="$(git diff --cached --name-only)"
if [ -n "$STAGED" ]; then
  . "$SCRIPT_DIR/git-identity.sh"
  commit_with_pre_commit_guard() {
    local pre_commit_hook
    amplihack_prepare_git_commit_identity || return "$?"
    pre_commit_hook="$(git rev-parse --git-path hooks/pre-commit)" || return "$?"
    if [ -f "$pre_commit_hook" ] && [ ! -f .pre-commit-config.yaml ]; then
      PRE_COMMIT_ALLOW_NO_CONFIG=1 git commit "$@"
    elif git commit "$@"; then
      return 0
    else
      return "$?"
    fi
  }
  commit_output_file=$(mktemp -t step20b-cleanup-commit-XXXXXX)
  if commit_with_pre_commit_guard -m "final cleanup pass" >"$commit_output_file" 2>&1; then
    cat "$commit_output_file"
    rm -f "$commit_output_file"
    echo "Cleanup commit created with $(wc -l <<< "$STAGED") file(s)"
  else
    commit_rc=$?
    echo "ERROR: git commit failed during step-20b-push-cleanup (exit $commit_rc)" >&2
    cat "$commit_output_file" >&2
    rm -f "$commit_output_file"
    git --no-pager diff --cached --name-status >&2
    git --no-pager status --short >&2
    exit "$commit_rc"
  fi
else
  echo "No staged changes from cleanup"
fi
# Push even when the audit committed all its edits already. Use the established
# remote mapping, regardless of push.default or a local alias branch name.
push_output_file="$(mktemp -t step20b-cleanup-push-XXXXXX)"
trap 'rm -f -- "$push_output_file"' EXIT
if git push -u origin "HEAD:refs/heads/$remote_branch" >"$push_output_file" 2>&1; then
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$push_output_file"
else
  push_rc=$?
  echo "ERROR: git push failed during step-20b-push-cleanup (exit $push_rc)" >&2
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$push_output_file" >&2
  exit "$push_rc"
fi
echo "=== Cleanup Complete ==="
