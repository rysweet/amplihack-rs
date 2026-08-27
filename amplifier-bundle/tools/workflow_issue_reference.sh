#!/usr/bin/env bash
# workflow_issue_reference.sh — does this change earn the "Closes" keyword?
# (issue #1361)
#
# The near-miss: PR #1283's body said `Closes #1277` while its entire diff was
# one added file, `docs/issue-1277-step-5d-exploration-plan.md`. Its own summary
# said "Documentation only. No ... implementation ... was performed." Had it
# merged, a genuine bug report would have been auto-closed by an artifact
# describing what somebody intended to do about it. Four sibling PRs opened the
# same hour carried the same body.
#
# `Closes #N` is a promise that merging this change resolves the issue. A change
# that only writes down a plan has not resolved anything, so it gets `Refs #N`
# instead: the pull request is still linked to the issue and still shows up in
# its timeline, but merging it leaves the issue open for the work itself.
#
# WHAT IS A PLANNING ARTIFACT, AND WHAT IS NOT
#
# Not "documentation". A docs-only change is very often the actual fix for a
# docs issue, and demoting those would be a new bug. The narrow thing being
# recognised here is a document whose *name* says it records intent rather than
# a change — plan, exploration, proposal, investigation, roadmap, scratch,
# brainstorm — or a file under a directory that exists only to hold such
# working notes.
#
# Version-derived files (Cargo.lock, package.json, Cargo.toml) are neither: the
# publish phase bumps and syncs them on every run, so counting them as real
# content would make every planning-only change look substantive. They are
# ignored, exactly as workflow_publish_pr.sh already ignores lockfiles when it
# derives a PR title scope (#929).
#
# So: `Closes` when at least one changed path is substantive; `Refs` otherwise,
# including when the change is empty. The conservative answer is the default in
# every uncertain case — under-linking costs a manual issue close, over-linking
# costs a silently closed bug report.
#
# Usage:
#   git diff --cached --name-only | workflow_issue_reference.sh --keyword
#   workflow_issue_reference.sh --keyword --files-from FILE
#   workflow_issue_reference.sh --classify < paths      (one verdict per path)
#
# stdout is the single word `Closes` or `Refs` (or, with --classify, one
# `<class> <path>` line per input path). Exit code is 0 in all normal cases.

set -uo pipefail

MODE="keyword"
FILES_FROM=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --keyword) MODE="keyword"; shift ;;
    --classify) MODE="classify"; shift ;;
    --files-from) FILES_FROM="${2:-}"; shift 2 ;;
    -h|--help) sed -n '1,45p' "$0" >&2; exit 0 ;;
    *) shift ;;
  esac
done

# classify_path <path> -> "planning" | "generated" | "substantive"
#
# Paths are treated strictly as data: the basename is matched by a static case,
# never expanded, never eval'd.
classify_path() {
  local path="$1" base lower
  base="$(basename -- "$path")"
  lower="$(printf '%s' "$path" | tr '[:upper:]' '[:lower:]')"

  case "$(printf '%s' "$base" | tr '[:upper:]' '[:lower:]')" in
    *.lock|package-lock.json|npm-shrinkwrap.json|pnpm-lock.yaml|go.sum|cargo.toml|package.json)
      printf 'generated\n'; return 0 ;;
  esac

  # Directories that exist only to hold working notes. Anything inside one is
  # a planning artifact regardless of its name.
  case "$lower" in
    ai_working/*|*/ai_working/*|specs/*|*/specs/*|plans/*|*/plans/*|.claude/runtime/*)
      printf 'planning\n'; return 0 ;;
  esac

  # Elsewhere, only a document whose name says "this records intent".
  case "$lower" in
    *.md|*.json|*.txt|*.yaml|*.yml|*.rst)
      case "$lower" in
        *plan*|*exploration*|*proposal*|*investigation*|*roadmap*|*scratch*|*brainstorm*)
          printf 'planning\n'; return 0 ;;
      esac ;;
  esac

  printf 'substantive\n'
}

read_paths() {
  if [ -n "$FILES_FROM" ]; then
    cat -- "$FILES_FROM" 2>/dev/null
  else
    cat
  fi
}

substantive=0
while IFS= read -r p; do
  [ -n "$p" ] || continue
  verdict="$(classify_path "$p")"
  [ "$MODE" = "classify" ] && printf '%s %s\n' "$verdict" "$p"
  [ "$verdict" = "substantive" ] && substantive=$((substantive + 1))
done < <(read_paths)

if [ "$MODE" = "keyword" ]; then
  if [ "$substantive" -gt 0 ]; then
    printf 'Closes\n'
  else
    # No substantive path — a planning artifact, a version bump, or nothing at
    # all. None of those resolve an issue (#1361).
    printf 'Refs\n'
  fi
fi
exit 0
