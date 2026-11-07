#!/usr/bin/env bash
set -euo pipefail
. "${BASH_SOURCE[0]%/*}/workflow_publish_checkpoint.sh"
workflow_checkpoint_python selectors || exit $?
CHECKPOINT_CAPTURE=false
if [ "$#" -eq 2 ] && [ "$1" = --scope-capture ] && [ "$2" = cleanup ]; then
  CHECKPOINT_CAPTURE=true
elif [ "$#" -ne 0 ]; then
  echo 'ERROR: invalid cleanup checkpoint invocation' >&2
  exit 2
fi
set -euo pipefail
# Issue #1528: stage, commit and push only at the top of the linked task worktree, never in REPO_PATH or the cwd.
skip() { echo "WARNING: step-20b ${1:-worktree_setup_path.sh not found}; skipping cleanup commit/push — never commits in REPO_PATH (#1528)" >&2; exit 0; }
workflow_cleanup_skip() {
  if [ "$CHECKPOINT_CAPTURE" = true ]; then
    python3 -I - "${1:-worktree_setup_path.sh not found}" <<'CLEANUP_SKIP_JSON'
import json, sys
print(json.dumps(dict(skipped=True, admitted=False, reason=sys.argv[1])))
CLEANUP_SKIP_JSON
  fi
  skip "$@"
}
WT_TOOL=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ] && { WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"; break; }; done
wt="$(bash "${WT_TOOL:?worktree_setup_path.sh not found}" step-20b-push-cleanup)" || workflow_cleanup_skip "${WT_TOOL:+worktree path gone}"; cd -P "$wt"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || workflow_cleanup_skip "not a git repo at $(pwd -P) (#537)"
{ gd=$(git rev-parse --absolute-git-dir) && cd_=$(git rev-parse --path-format=absolute --git-common-dir) && [ "$gd" != "$cd_" ] && [ "$(git rev-parse --show-toplevel)" = "$(pwd -P)" ]; } 2>/dev/null || workflow_cleanup_skip "worktree path is not a linked worktree"
if [ "$CHECKPOINT_CAPTURE" = true ]; then
  workflow_checkpoint_python capture cleanup
  exit $?
fi
workflow_checkpoint_enter cleanup || exit $?
export GIT_PAGER=cat PAGER=cat LESS=FRX
redact_sensitive_output() {
  sed -E 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g'
}
echo "=== Pushing Cleanup Changes ==="
current_branch=$(git branch --show-current); remote_branch="$(bash "$(dirname "$WT_TOOL")/workflow_remote_branch.sh" .)" || exit 2; [ "$remote_branch" != "$current_branch" ] || remote_branch=""  # #1762: an alias pushes HEAD to <B>
if [ -z "$current_branch" ]; then
  echo "ERROR: current branch is empty; cannot push cleanup changes from detached HEAD" >&2
  exit 1
fi
RUNTIME_ARTIFACT_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="$(pwd)/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
workflow_checkpoint_check || exit $?
if [ -f "$RUNTIME_ARTIFACT_HELPER" ]; then . "$RUNTIME_ARTIFACT_HELPER"; preflight_known_workflow_runtime_artifacts .; else echo "WARNING: workflow runtime artifact helper not found; skipping artifact preflight enrichment and continuing in degraded mode (searched AMPLIHACK_HOME, REPO_PATH, worktree, ~/.copilot, ~/.amplihack): $RUNTIME_ARTIFACT_HELPER; cwd=$(pwd)" >&2; fi
amplihack hygiene artifact-guard --repo . --mode pre-publish
workflow_checkpoint_stage; amplihack hygiene artifact-guard --repo . --mode pre-publish
STAGED=$(git diff --cached --name-only)
if [ -n "$STAGED" ]; then
  GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }; . "$GIT_IDENTITY_HELPER"
  commit_with_pre_commit_guard() {
    local pre_commit_hook
    amplihack_prepare_git_commit_identity
    pre_commit_hook="$(git rev-parse --git-path hooks/pre-commit)"
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
    redact_sensitive_output < "$commit_output_file"
    rm -f "$commit_output_file"
    echo "Cleanup commit created with $(wc -l <<< "$STAGED") file(s)"
  else
    commit_rc=$?
    echo "ERROR: git commit failed during step-20b-push-cleanup (exit $commit_rc)" >&2
    redact_sensitive_output < "$commit_output_file" >&2
    rm -f "$commit_output_file"
    git --no-pager diff --cached --name-status >&2
    git --no-pager status --short >&2
    exit "$commit_rc"
  fi
else
  echo "No staged changes from cleanup"
fi
# #1714: fetch origin/<branch> and set a missing upstream, so the pull below runs
# whenever the branch is on origin; a bare `push -u` to a remote ahead is rejected.
workflow_checkpoint_sync_body() {
bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh" --strict-fetch
}
sync_output_file=$(mktemp -t step20b-cleanup-sync-XXXXXX)
if workflow_checkpoint_operation sync workflow_checkpoint_sync_body >"$sync_output_file" 2>&1; then
  redact_sensitive_output < "$sync_output_file"
  rm -f -- "$sync_output_file"
else
  sync_rc=$?
  redact_sensitive_output < "$sync_output_file" >&2
  rm -f -- "$sync_output_file"
  exit "$sync_rc"
fi
push_cmd=(git push -u origin "$current_branch"${remote_branch:+":$remote_branch"})
if git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
  push_cmd=(git push ${remote_branch:+origin "HEAD:$remote_branch"})
  pull_output_file=$(mktemp -t step20b-cleanup-pull-XXXXXX)
  if git --no-pager pull --rebase >"$pull_output_file" 2>&1; then
    redact_sensitive_output < "$pull_output_file"
    rm -f "$pull_output_file"
  else
    pull_rc=$?
    echo "ERROR: git pull --rebase failed during step-20b-push-cleanup (exit $pull_rc)" >&2
    redact_sensitive_output < "$pull_output_file" >&2
    rm -f "$pull_output_file"
    git --no-pager status --short >&2
    exit "$pull_rc"
  fi
fi
push_output_file=$(mktemp -t step20b-cleanup-push-XXXXXX)
if "${push_cmd[@]}" >"$push_output_file" 2>&1; then
  redact_sensitive_output < "$push_output_file"
  rm -f "$push_output_file"
else
  push_rc=$?
  echo "ERROR: git push failed during step-20b-push-cleanup (exit $push_rc)" >&2
  redact_sensitive_output < "$push_output_file" >&2
  rm -f "$push_output_file"
  git --no-pager status --short >&2
  exit "$push_rc"
fi
echo "=== Cleanup Complete ==="
