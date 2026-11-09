#!/usr/bin/env bash
# workflow_default_branch.sh - decide whether a branch is the repository's
# default branch (issues #1762, #1969).
#
# Usage (executed, never sourced):
#   workflow_default_branch.sh [--ask-origin] <dir> <branch>
#
# This is the only copy of the rule. Its callers are:
#   - workflow_caller_branch.sh (step 4): a default branch is never continued;
#   - workflow_caller_branch_alias.sh (step 4): a default branch gets no alias;
#   - workflow_remote_branch.sh (step 15 and later): an alias-shaped name whose
#     prefix is a default branch is not an alias;
#   - workflow-publish.yaml step 15, with --ask-origin: never push to it.
#
# <branch> is a default branch when any of these holds, checked in order:
#   1. it is main or master;
#   2. it is the target of refs/remotes/origin/HEAD in <dir>;
#   3. refs/remotes/origin/HEAD does not resolve to a commit, and
#      a. with --ask-origin: origin answers `git ls-remote --symref origin
#         HEAD` with a valid branch name and <branch> is that name;
#      b. without --ask-origin, or when origin gives no valid answer:
#         <branch> is develop, which step 4 can fall back to as the base when
#         origin/HEAD is unknown.
# So the two modes differ only in where they learn origin's HEAD when the
# local ref is missing: without the flag the tool never touches the network
# and guesses develop; with it, origin's answer replaces the guess.
#
# Exit status: 0 default branch; 1 not a default branch; 2 usage (wrong
# arguments or a <dir> that is not a directory). Callers treat any status
# other than 0 or 1 as a failure, never as "not a default branch".
#
# It only reads (`symbolic-ref`, `rev-parse`, `check-ref-format`, and
# `ls-remote` with --ask-origin). The ls-remote query's stderr is discarded,
# so a remote URL is never printed, and GIT_TERMINAL_PROMPT=0 keeps it from
# prompting.

set -euo pipefail
# S7: an inherited repository, work tree or index must not redirect the reads.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

usage() {
  echo "usage: workflow_default_branch.sh [--ask-origin] <dir> <branch>" >&2
  exit 2
}
ask_origin=false
if [ "${1:-}" = "--ask-origin" ]; then ask_origin=true; shift; fi
[ "$#" -eq 2 ] || usage
dir="$1" branch="$2"
[ -n "$dir" ] && [ -d "$dir" ] && [ -n "$branch" ] || usage

# 1. The conventional names.
case "$branch" in main|master) exit 0 ;; esac

# 2. The local origin/HEAD target, whether or not it resolves.
origin_head="$(git -C "$dir" symbolic-ref -q refs/remotes/origin/HEAD 2>/dev/null || true)"
origin_head="${origin_head#refs/remotes/origin/}"
[ -z "$origin_head" ] || [ "$branch" != "$origin_head" ] || exit 0

# A resolvable local origin/HEAD is trusted; nothing else is asked.
git -C "$dir" rev-parse --verify -q "refs/remotes/origin/HEAD^{commit}" >/dev/null 2>&1 && exit 1

# 3a. Ask origin.
if [ "$ask_origin" = true ]; then
  remote_head=""
  tab="$(printf '\t')"
  while IFS= read -r line; do
    case "$line" in
      "ref: refs/heads/"*"${tab}HEAD")
        remote_head="${line#ref: refs/heads/}"
        remote_head="${remote_head%"${tab}HEAD"}"
        break ;;
    esac
  done < <(GIT_TERMINAL_PROMPT=0 git -C "$dir" ls-remote --symref origin HEAD 2>/dev/null || true)
  if [ -n "$remote_head" ] \
      && [ "$(git -C "$dir" check-ref-format --branch "$remote_head" 2>/dev/null || true)" = "$remote_head" ]; then
    [ "$branch" = "$remote_head" ] && exit 0
    exit 1
  fi
fi

# 3b. No answer: develop is the fallback base, so it is treated as default.
[ "$branch" = "develop" ] && exit 0
exit 1
