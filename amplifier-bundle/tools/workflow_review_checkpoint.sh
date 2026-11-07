#!/usr/bin/env bash
set -euo pipefail
. "${BASH_SOURCE[0]%/*}/workflow_publish_checkpoint.sh"
workflow_checkpoint_enter review || exit $?
redact_sensitive_file() {
  sed -E 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g' "$1"
}
set -euo pipefail
export GIT_PAGER=cat PAGER=cat LESS=FRX
WT_TOOL=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ] && { WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"; break; }; done
WORKTREE_SETUP_WORKTREE_PATH="$(bash "${WT_TOOL:?worktree_setup_path.sh not found}" step-18c-push-feedback-changes)" || WORKTREE_SETUP_WORKTREE_PATH=""
cd "${WORKTREE_SETUP_WORKTREE_PATH:?step-18c-push-feedback-changes requires worktree_setup.worktree_path; ensure parent recipe ran worktree-setup and propagated outputs}"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: step-18c-push-feedback-changes requires a git repo at $(pwd); either \`git init\` or rerun from a checkout" >&2
  exit 1
fi
echo "=== Pushing Review Feedback Changes ==="
current_branch=$(git branch --show-current); remote_branch="$(bash "$(dirname "$WT_TOOL")/workflow_remote_branch.sh" .)" || exit 2; [ "$remote_branch" != "$current_branch" ] || remote_branch=""  # #1762: an alias pushes HEAD to <B>
if [ -z "$current_branch" ]; then
  echo "ERROR: current branch is empty; cannot push review feedback changes from detached HEAD" >&2
  exit 1
fi
RUNTIME_ARTIFACT_HELPER="${AMPLIHACK_HOME:-}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="$(git rev-parse --show-toplevel 2>/dev/null)/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="$(pwd)/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"; [ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
# Pre-publish artifact-provenance gate: runs BEFORE pushing review-feedback
# commits, so per the landed #829 contract it stays fail-visible (exit 2) if
# the helper is unresolvable. Uses the git-toplevel variant (NOT REPO_PATH)
# to honor the #684 worktree invariant; the resolution ladder (#962) still
# finds it at the install root even for a gitignored worktree bundle.
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found (searched AMPLIHACK_HOME, worktree, cwd, ~/.copilot, ~/.amplihack): $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$RUNTIME_ARTIFACT_HELPER"
workflow_checkpoint_check || exit $?
preflight_known_workflow_runtime_artifacts .
amplihack hygiene artifact-guard --repo . --mode pre-publish
workflow_checkpoint_stage; amplihack hygiene artifact-guard --repo . --mode pre-publish
STAGED=$(git diff --cached --name-only)
if [ -n "$STAGED" ]; then
  GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-$(git rev-parse --show-toplevel)}/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }; . "$GIT_IDENTITY_HELPER"
  commit_output_file=$(mktemp -t step18c-review-feedback-commit-XXXXXX)
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
  if commit_with_pre_commit_guard -m "address review feedback" -m "Implemented reviewer suggestions; fixed identified issues; updated per security review; addressed philosophy compliance items" >"$commit_output_file" 2>&1; then
    commit_rc=0
  else
    commit_rc=$?
  fi
  if [ "$commit_rc" -eq 0 ]; then
    redact_sensitive_file "$commit_output_file"
    rm -f "$commit_output_file"
    echo "Review feedback commit created with $(wc -l <<< "$STAGED") file(s)"
  else
    echo "ERROR: git commit failed during step-18c-push-feedback-changes (exit $commit_rc)" >&2
    redact_sensitive_file "$commit_output_file" >&2
    rm -f "$commit_output_file"
    git --no-pager diff --cached --name-status >&2
    git --no-pager status --short >&2
    exit "$commit_rc"
  fi
else
  echo "No staged changes from review feedback"
fi
push_output_file=$(mktemp -t step18c-review-feedback-push-XXXXXX)
git_remote_with_retry() {
  local rc attempt
  : >"$push_output_file"
  for attempt in 1 2 3; do
    "$@" >>"$push_output_file" 2>&1 && return 0
    rc=$?
    # Decision 17.116 permits this first-failure stop. Push retries are unchanged.
    if [ "$#" -eq 3 ] && [ "$1" = git ] && [ "$2" = pull ] && [ "$3" = --rebase ]; then return "$rc"; fi
    # A bounded scope refusal or terminal policy refusal cannot be retried.
    if [ "$rc" -eq 78 ] || [ "$rc" -eq 79 ]; then return "$rc"; fi
    [ "$attempt" -lt 3 ] || return "$rc"
    echo "WARNING: $* failed (exit $rc); retrying ($attempt/3)" >>"$push_output_file"
  done
}
push_rc=0
if git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
  if git_remote_with_retry git pull --rebase; then
    git_remote_with_retry git push ${remote_branch:+origin "HEAD:$remote_branch"} || push_rc=$?
  else
    push_rc=$?
  fi
else
  git_remote_with_retry git push -u origin "$current_branch"${remote_branch:+":$remote_branch"} || push_rc=$?
fi
if [ "$push_rc" -eq 0 ]; then
  redact_sensitive_file "$push_output_file"
  rm -f "$push_output_file"
else
  echo "ERROR: git push failed during step-18c-push-feedback-changes (exit $push_rc); git pull --rebase may also have failed" >&2
  redact_sensitive_file "$push_output_file" >&2
  rm -f "$push_output_file"
  git --no-pager status --short >&2
  exit "$push_rc"
fi
echo "=== Changes Pushed ==="
