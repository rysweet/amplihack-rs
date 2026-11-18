#!/usr/bin/env bash
# Publish final cleanup and audit work from this run's recorded worktree.
set -euo pipefail
umask 022
export GIT_PAGER=cat PAGER=cat LESS=FRX
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# Buffer sanitizer output too: on failure never release partial/raw output.
emit_captured_output() {
  local safe
  if safe="$(amplihack orch helper sanitize-output <"$1" 2>/dev/null)"; then
    printf '%s\n' "$safe"
  else
    echo "ERROR: captured publication output sanitization unavailable" >&2
    return 1
  fi
}
commit_output_file=""
push_output_file=""
command_output_file=""
trap 'rm -f -- "$commit_output_file" "$push_output_file" "$command_output_file"' EXIT
# Synchronization, artifact checks and staging can also print untrusted paths.
run_captured_command() {
  local command_rc=0 sanitize_rc=0
  command_output_file="$(mktemp -t step20b-cleanup-command-XXXXXX)"
  "$@" >"$command_output_file" 2>&1 || command_rc=$?
  if [ "$command_rc" -eq 0 ]; then
    emit_captured_output "$command_output_file" || sanitize_rc=$?
  else
    emit_captured_output "$command_output_file" >&2 || sanitize_rc=$?
  fi
  rm -f -- "$command_output_file"
  [ "$command_rc" -eq 0 ] || return "$command_rc"
  return "$sanitize_rc"
}
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
run_captured_command bash "$SCRIPT_DIR/workflow_pr_head_sync.sh" --strict
# Expanded by the child shell, retaining errexit inside the sourced helper.
# shellcheck disable=SC2016
run_captured_command env SCRIPT_DIR="$SCRIPT_DIR" bash -c 'set -euo pipefail; . "$1"; preflight_known_workflow_runtime_artifacts .' \
  publication-preflight "$SCRIPT_DIR/workflow_runtime_artifacts.sh"
run_captured_command amplihack hygiene artifact-guard --repo . --mode pre-publish
run_captured_command git add -A
run_captured_command amplihack hygiene artifact-guard --repo . --mode pre-publish
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
    emit_captured_output "$commit_output_file"
    rm -f "$commit_output_file"
    echo "Cleanup commit created with $(wc -l <<< "$STAGED") file(s)"
  else
    commit_rc=$?
    echo "ERROR: git commit failed during step-20b-push-cleanup (exit $commit_rc)" >&2
    git --no-pager diff --cached --name-status >>"$commit_output_file" 2>&1 || true
    git --no-pager status --short >>"$commit_output_file" 2>&1 || true
    emit_captured_output "$commit_output_file" >&2 || true
    rm -f "$commit_output_file"
    exit "$commit_rc"
  fi
else
  echo "No staged changes from cleanup"
fi
# Push even when the audit committed all its edits already. Use the established
# remote mapping, regardless of push.default or a local alias branch name.
push_output_file="$(mktemp -t step20b-cleanup-push-XXXXXX)"
if git push -u origin "HEAD:refs/heads/$remote_branch" >"$push_output_file" 2>&1; then
  emit_captured_output "$push_output_file"
else
  push_rc=$?
  echo "ERROR: git push failed during step-20b-push-cleanup (exit $push_rc)" >&2
  emit_captured_output "$push_output_file" >&2 || true
  exit "$push_rc"
fi
echo "=== Cleanup Complete ==="
