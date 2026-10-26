#!/usr/bin/env bash
# worktree_setup_path.sh - print this run's worktree path, checked before any
# git command uses it (issue #1561).
#
# Usage (executed, never sourced):
#   worktree_setup_path.sh <step-label>
#
# Reads only `worktree_path` from the JSON object in RECIPE_VAR_worktree_setup,
# the output of workflow-worktree (step-04). It never reads the bare
# WORKTREE_SETUP_WORKTREE_PATH or RECIPE_VAR_worktree_setup__worktree_path: the
# runner writes those only for fields the current object has, so either can
# hold a parent run's path.
#
# The path is accepted only when it is one line, absolute, an existing
# directory, not a symlink, equal to its own realpath, and equal to
# `git -C <path> rev-parse --show-toplevel`. workflow-worktree outputs the
# resolved path, so an unresolved one (macOS /tmp, say) came from somewhere else.
#
# Exit codes. On success the path is printed on stdout. On failure stdout is
# empty and one line naming <step-label> and the failure goes to stderr:
#   0  accepted
#   2  worktree-setup-no-jq      jq is not on PATH
#   3  worktree-setup-unset      RECIPE_VAR_worktree_setup is unset or empty,
#                                or its object has no worktree_path
#   4  worktree-setup-not-json   RECIPE_VAR_worktree_setup is not one JSON object
#   5  worktree-setup-rejected   the path fails one of the checks above
#   64 usage error
# A caller that cannot find this tool fails too (bash exits 127, or the
# caller's `${WT:?}` stops the substitution) and takes the same outcome.
#
# What a step does on failure is its own choice, written at its call site:
#   stop       workflow-publish step-15, workflow-pr-review step-18c,
#              workflow-tdd checkpoint-after-implementation and
#              workflow-refactor-review checkpoint-after-review-feedback: they
#              commit or push, so they run only in the run's worktree (#1528).
#   repo_path  workflow-publish step-14b, step-14c, step-14g and step-16: exit 3
#              falls back to repo_path (they stage nothing, #684); any other
#              failure stops.
#   skip       workflow-finalize step-20a and step-20b: never run in REPO_PATH
#              (#1528).
#   REPO_PATH  workflow-finalize step-21 and step-22b, workflow-pr-review
#              step-19c: read-only status and scans. Final status in REPO_PATH
#              is safe because the #1596 branch lookup in
#              workflow_final_status.sh refuses REPO_PATH's branch.
#   ignore     autodrive-build step-03: skips the run-branch PR lookup (#1676).
#   invalid    workflow-terminal-state probe: exit 3 uses repo_path, any other
#              failure is FAILED_INVALID_INPUT.

set -u

label="${1:-}"
if [ "$#" -ne 1 ] || [ -z "$label" ]; then
  echo "usage: worktree_setup_path.sh <step-label>" >&2
  exit 64
fi

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: worktree-setup-no-jq: ${label}: jq is required to read worktree_setup (#1561)" >&2
  exit 2
fi

raw="${RECIPE_VAR_worktree_setup:-}"
if [ -z "$raw" ]; then
  echo "WARNING: worktree-setup-unset: ${label}: worktree_setup is not set" >&2
  exit 3
fi

if ! path="$(printf '%s' "$raw" | jq -r -s '
    if length == 1 and (.[0] | type) == "object" then .[0].worktree_path // empty
    else error("not one object") end' 2>/dev/null)"; then
  echo "WARNING: worktree-setup-not-json: ${label}: worktree_setup is not one JSON object (#1561)" >&2
  exit 4
fi

if [ -z "$path" ]; then
  echo "WARNING: worktree-setup-unset: ${label}: worktree_setup has no worktree_path" >&2
  exit 3
fi

reject() {
  echo "WARNING: worktree-setup-rejected: ${label}: worktree_setup.worktree_path $1 (#1561)" >&2
  exit 5
}

case "$path" in
  *$'\n'*) reject "spans more than one line" ;;
  /*) ;;
  *) reject "is not absolute" ;;
esac
[ -d "$path" ] || reject "is not a directory"
[ ! -L "$path" ] || reject "is a symlink"
[ "$(realpath -- "$path" 2>/dev/null)" = "$path" ] || reject "is not a resolved path"
[ "$(git -C "$path" rev-parse --show-toplevel 2>/dev/null)" = "$path" ] || reject "is not a worktree top level"

printf '%s\n' "$path"
