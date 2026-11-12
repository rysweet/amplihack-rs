#!/usr/bin/env bash
# workflow_delivery_inputs.sh — checks for two caller-supplied inputs of
# default-workflow: `base_branch`, and the opt-in `strict_delivery` contract.
# Reference: docs/reference/workflow-delivery-inputs.md.
#
# Nothing here reads RECIPE_VAR_*. Every value arrives as an argument, so the
# recipe that passes a value is the one that declares its key (#1561).
#
# Sourced for its functions by workflow_issue_tracking.sh (step-03) and
# workflow_publish_pr.sh (step-16). Executed by step-04-setup-worktree:
#
#   workflow_delivery_inputs.sh worktree BASE_BRANCH STRICT EXISTING_BRANCH FORBIDDEN
#       Fails when STRICT is `true` and EXISTING_BRANCH is empty: a strict run
#       works on the branch its caller assigned and never derives one. Prints
#       origin/BASE_BRANCH when BASE_BRANCH is set (see delivery_base_ref) and
#       nothing when it is empty, so an ordinary run keeps its own discovery.
#
# Functions:
#   delivery_base_ref BASE BRANCH FORBIDDEN
#       Prints origin/BASE. Returns 1, with the reason on stderr, when BASE is
#       not a valid branch name, is written as a ref (`refs/...`) or `@`,
#       equals BRANCH (the branch being delivered; may be empty), matches a
#       pattern in FORBIDDEN (whitespace-separated shell globs, for example
#       `release/* prod`), or origin/BASE does not resolve to a commit. It never
#       substitutes another base and never fetches: the caller has fetched.
#   same_github_repository A B
#       Returns 0 when owner/repo A and B name the same repository. GitHub
#       compares owner and repository names case-insensitively.
#   strict_issue_tag ISSUE REPOSITORY PR_REPOSITORY
#       Prints `#ISSUE` when the issue and the pull request share a repository,
#       else `REPOSITORY#ISSUE`. GitHub resolves a bare #N in the repository of
#       the text that contains it, so an issue in another repository must be
#       written in full.
#
# This file defines functions only when sourced: no set -e/-u/-o and nothing
# printed. bash 3.2 compatible (no ${x,,}, no mapfile).

delivery_base_ref() {
  local base="${1:-}" branch="${2:-}" forbidden="${3:-}" pattern
  local patterns=()
  if ! git check-ref-format --branch "$base" >/dev/null 2>&1; then
    printf 'ERROR: base_branch %q is not a valid branch name\n' "$base" >&2
    return 1
  fi
  case "$base" in
    refs/*|@)
      echo "ERROR: base_branch '$base' must be a plain branch name, not a ref" >&2
      return 1 ;;
  esac
  if [ -n "$branch" ] && [ "$base" = "$branch" ]; then
    echo "ERROR: base_branch '$base' is the branch being delivered; a pull request needs a different base" >&2
    return 1
  fi
  # read splits on whitespace and, unlike an unquoted expansion, never expands
  # a pattern against the file system.
  read -r -a patterns <<< "$forbidden" || true
  for pattern in ${patterns[@]+"${patterns[@]}"}; do
    # shellcheck disable=SC2254 # the pattern is meant to match as a glob
    case "$base" in
      $pattern)
        echo "ERROR: base_branch '$base' matches forbidden_base_branches pattern '$pattern'" >&2
        return 1 ;;
    esac
  done
  if ! git rev-parse --verify --quiet "refs/remotes/origin/${base}^{commit}" >/dev/null; then
    echo "ERROR: base_branch '$base' has no remote-tracking ref origin/$base; fetch it first (no other base is substituted)" >&2
    return 1
  fi
  printf 'origin/%s\n' "$base"
}

same_github_repository() {
  [ -n "${1:-}" ] && [ "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" = "$(printf '%s' "${2:-}" | tr '[:upper:]' '[:lower:]')" ]
}

strict_issue_tag() {
  if same_github_repository "${2:-}" "${3:-}"; then
    printf '#%s\n' "$1"
  else
    printf '%s#%s\n' "$2" "$1"
  fi
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  case "${1:-}" in
    worktree)
      base="${2:-}" strict="${3:-}" branch="${4:-}" forbidden="${5:-}"
      if [ "$strict" = true ] && [ -z "$branch" ]; then
        echo "ERROR: strict_delivery=true requires existing_branch: a strict run works on the branch its caller assigned and never derives one" >&2
        exit 1
      fi
      [ -z "$base" ] || delivery_base_ref "$base" "$branch" "$forbidden"
      ;;
    *)
      echo "usage: workflow_delivery_inputs.sh worktree BASE_BRANCH STRICT EXISTING_BRANCH FORBIDDEN" >&2
      exit 2 ;;
  esac
fi
