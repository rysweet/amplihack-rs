#!/usr/bin/env bash
# workflow_worktree_base_ref.sh — fetch origin and resolve the remote base ref.
#
# Extracted VERBATIM (`fetch_origin_with_retry` and `resolve_base_ref`) from
# step-04-setup-worktree in amplifier-bundle/recipes/workflow-worktree.yaml.
#
# WHY IT MOVED: that brick sits at the 400-line budget enforced by
# `every_phase_subrecipe_under_400_lines` and scripts/check-brick-budget.sh, and
# the rule's remedy for a full brick is EXTRACTION into amplifier-bundle/tools/,
# never compressing the logic until the counter is satisfied. Issue #1426 needed
# room for branch-name resolution; this is the room it bought.
#
# The recipe keeps a DEGRADED MIRROR of both ladders for the case where the
# bundle is not on disk (the phase-brick unit tests run the extracted step body
# with no bundle at all). The mirror is deliberately the same sequence with the
# retries dropped; this file is the authority, and the two must move together.
#
# Contract — the subcommands are named for the functions they were extracted
# from, so the recipe's call site still reads as the ladder it invokes:
#   fetch_origin_with_retry [REPO_PATH]
#       Three attempts, exponential backoff, never fatal. Always exits 0 — a
#       transient network blip must not abort a run, and resolve_base_ref
#       decides on its own whether the refs it needs are actually present.
#   resolve_base_ref [REPO_PATH]
#       Prints origin/HEAD (re-probed via `git remote set-head` when unset),
#       else origin/master, else origin/develop. Exits 1 with a diagnostic when
#       none resolves: a task worktree must branch off a fetched remote base
#       ref, never the caller's HEAD (issue #858).
#
# Portability: bash 3.2 (macOS system bash) — issue #1423.

set -uo pipefail

fetch_origin_with_retry() {
  local attempt
  local status
  local delay=1
  local max_attempts=3

  for attempt in 1 2 3; do
    if git fetch origin --no-tags >&2; then
      return 0
    fi
    status=$?

    if [ "$attempt" -lt "$max_attempts" ]; then
      echo "WARNING: git fetch origin failed (exit ${status}); retrying (${attempt}/${max_attempts})." >&2
      sleep "$delay"
      delay=$((delay * 2))
      continue
    fi

    echo "WARNING: git fetch origin failed after ${max_attempts} attempts (exit ${status}); continuing with local refs after repository validation succeeded." >&2
    return 0
  done
}

resolve_base_ref() {
  local candidate

  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
    printf '%s\n' "$candidate"
    return 0
  fi

  git remote set-head origin -a >/dev/null 2>&1 || true
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
    printf '%s\n' "$candidate"
    return 0
  fi

  for candidate in origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  echo "ERROR: no supported remote base ref found. Expected origin/HEAD, origin/master, or origin/develop." >&2
  return 1
}

ACTION="${1:-}"
cd "${2:-.}" 2>/dev/null || {
  echo "ERROR: base-ref helper cannot enter '${2:-.}'" >&2
  [ "$ACTION" = "fetch_origin_with_retry" ] && exit 0
  exit 1
}

case "$ACTION" in
  fetch_origin_with_retry) fetch_origin_with_retry; exit 0 ;;
  resolve_base_ref)        resolve_base_ref ;;
  *)
    echo "usage: workflow_worktree_base_ref.sh {fetch_origin_with_retry|resolve_base_ref} [REPO_PATH]" >&2
    exit 2
    ;;
esac
