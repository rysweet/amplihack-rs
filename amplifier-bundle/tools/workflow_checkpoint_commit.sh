#!/usr/bin/env bash
# Scoped checkpoint commit; real pre-commit hooks remain authoritative.
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
