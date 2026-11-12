#!/usr/bin/env bash
# workflow_worktree_deconflict.sh — non-destructive worktree/branch deconfliction.
#
# See amplifier-bundle/tools/workflow_worktree_deconflict.md for the full design.
#
# PURPOSE
#   Two concurrently-running smart-orchestrator recipes can derive the SAME
#   branch slug (same issue number + same task-description). The setup step's
#   idempotency state machine (workflow-worktree.yaml step-04-setup-worktree,
#   consensus-issue-worktree.yaml step3-setup-worktree) assumed any branch it
#   found was one of its OWN prior runs. In the State-2 path it ran
#   `git branch` deletion (or `git worktree add` without -b) which
#   FAILS — and aborts the whole recipe — when that branch is currently checked
#   out by a DIFFERENT recipe's worktree at another path.
#
#   This helper detects that FOREIGN ownership BEFORE any destructive/reuse
#   decision is made and resolves it by renaming THIS run's branch (never by
#   destroying the other run's branch or worktree).
#
#   With --base <ref> (issue #1565) it also renames THIS run's branch when the
#   branch already holds work a re-run must not destroy: a same-path worktree
#   that is dirty (status --porcelain non-empty, ignored files excluded) or has
#   commits ahead of <ref>, or a leftover branch (no worktree) with commits
#   ahead of <ref>. If that state cannot be proven (status/rev-list fails, the
#   base does not resolve, a count is non-numeric) it renames too. Without
#   --base the behaviour is exactly the pre-#1565 behaviour.
#
# CONTRACT
#   workflow_worktree_deconflict.sh resolve [--base <base_ref>] <candidate_branch> <intended_worktree_path> [repo_path]
#     --base : optional, and only accepted as the FIRST argument after resolve.
#     stdout : exactly one line — the resolved branch name (unchanged, or a new
#              deconflicted name). Safe to capture with command substitution.
#     stderr : human-readable diagnostics.
#     exit 0 : a usable branch name was resolved and printed.
#     exit 1 : bad/insufficient arguments (including --base with a missing or
#              empty value), or all bounded retries (<=5) were exhausted
#              without finding a provably-free name.
#
# NON-DESTRUCTIVE INVARIANT (hard): this helper only READS git state. It
#   performs no branch deletion, no worktree removal, no hard reset, no forced
#   checkout, no force/delete push, and no filesystem removal. Status probes on
#   another run's worktree use --no-optional-locks so they never take its
#   index lock (#1560).
#
# ENV
#   AMPLIHACK_DECONFLICT_MAX_RETRIES  upper bound on suffix retries; validated
#     against ^[0-9]+$ and clamped to a HARD ceiling of 5 (override may only
#     LOWER the bound, never weaken the DoS guarantee).

set -euo pipefail

# Hard ceiling on retries — an override may only lower this, never raise it.
readonly DECONFLICT_HARD_CEILING=5

# Overall cap on a deconflicted branch name produced by the rename path. The
# caller's slug pipeline already caps the BASE (50 chars in workflow-worktree.yaml,
# 30 in consensus-issue-worktree.yaml), but appending a uniqueness suffix on the
# rename path can exceed that budget (issue #200 review, item S4). This bounds the
# result; it is generous by design and only trims pathologically long names.
readonly DECONFLICT_MAX_BRANCH_LEN=80

log() { printf '%s\n' "$*" >&2; }

usage() {
  log "usage: workflow_worktree_deconflict.sh resolve [--base <base_ref>] <candidate_branch> <intended_worktree_path> [repo_path]"
}

# normalize_path <path>
# Resolve symlinks and '.'/'..' segments (pwd -P semantics) even when the path
# does not exist yet. Walks up to the deepest existing ancestor, canonicalizes
# it, then re-appends the missing tail. Matches the issue #858 caller-checkout
# normalization so a symlinked/non-canonical intended path cannot masquerade as
# same-path or foreign.
normalize_path() {
  local p="$1"
  if [ -d "$p" ]; then
    (cd "$p" 2>/dev/null && pwd -P) || printf '%s' "$p"
    return 0
  fi
  local dir="$p" rest="" base
  while [ ! -d "$dir" ] && [ "$dir" != "/" ] && [ "$dir" != "." ]; do
    base="$(basename -- "$dir")"
    if [ -n "$rest" ]; then
      rest="$base/$rest"
    else
      rest="$base"
    fi
    dir="$(dirname -- "$dir")"
  done
  local realdir
  realdir="$(cd "$dir" 2>/dev/null && pwd -P)" || realdir="$dir"
  if [ -n "$rest" ]; then
    printf '%s/%s' "$realdir" "$rest"
  else
    printf '%s' "$realdir"
  fi
}

# branch_worktree_path <branch>
# Print the (raw) worktree path that has <branch> checked out, or nothing.
# Parses the stable machine-readable porcelain form, never the human output.
branch_worktree_path() {
  local branch="$1"
  git worktree list --porcelain 2>/dev/null | awk -v b="refs/heads/${branch}" '
    $1 == "worktree" { wt = substr($0, 10) }
    $1 == "branch" && $2 == b { print wt; exit }
  '
}

# branch_is_checked_out_anywhere <branch> -> 0 if any worktree owns it.
branch_is_checked_out_anywhere() {
  local branch="$1"
  git worktree list --porcelain 2>/dev/null \
    | grep -qxF "branch refs/heads/${branch}"
}

# name_is_free <branch> -> 0 when the name has no local ref, no remote ref, and
# is not checked out by ANY worktree.
name_is_free() {
  local branch="$1"
  if git show-ref --verify --quiet "refs/heads/${branch}"; then
    return 1
  fi
  if git show-ref --verify --quiet "refs/remotes/origin/${branch}"; then
    return 1
  fi
  if branch_is_checked_out_anywhere "$branch"; then
    return 1
  fi
  return 0
}

# resolve_max_retries -> echo the effective, clamped retry bound.
resolve_max_retries() {
  local max="$DECONFLICT_HARD_CEILING"
  local override="${AMPLIHACK_DECONFLICT_MAX_RETRIES:-}"
  if [ -n "$override" ] && printf '%s' "$override" | grep -qE '^[0-9]+$'; then
    if [ "$override" -lt "$max" ]; then
      max="$override"
    fi
  fi
  printf '%s' "$max"
}

# fresh_name <candidate> -> print a new, provably-free '<candidate>-<suffix>'
# name, or return 1 after the bounded retries are exhausted.
fresh_name() {
  local candidate="$1"
  local max attempt suffix candidate_new base_max base_trunc
  max="$(resolve_max_retries)"
  attempt=0
  while [ "$attempt" -lt "$max" ]; do
    attempt=$((attempt + 1))
    # Short unique suffix. Sanitized through tr -cd 'a-z0-9-' so it can never
    # introduce shell or ref metacharacters. Attempt + RANDOM disambiguate the
    # astronomically-rare same-second collision.
    suffix="$(printf '%s' "$(date +%s)-${attempt}-$$-${RANDOM:-0}" | tr -cd 'a-z0-9-')"
    # Re-apply an overall length cap on the rename path (review item S4). Truncate
    # only the BASE so the uniqueness-bearing suffix is preserved intact; strip any
    # resulting trailing hyphen so we never emit 'base--suffix' or a trailing '-'.
    base_max=$(( DECONFLICT_MAX_BRANCH_LEN - ${#suffix} - 1 ))
    [ "$base_max" -lt 1 ] && base_max=1
    base_trunc="${candidate:0:base_max}"
    base_trunc="${base_trunc%-}"
    candidate_new="${base_trunc}-${suffix}"
    if ! git check-ref-format --branch "$candidate_new" >/dev/null 2>&1; then
      continue
    fi
    if name_is_free "$candidate_new"; then
      printf '%s\n' "$candidate_new"
      return 0
    fi
  done

  log "ERROR: exhausted ${max} deconfliction attempts without a free branch name for '${candidate}'."
  return 1
}

# commits_ahead <base> <branch> -> print the number of commits on <branch> that
# are not on <base>, or "unknown" when that cannot be proven (#1565).
commits_ahead() {
  local base="$1" branch="$2" count
  if ! git rev-parse --verify --quiet "${base}^{commit}" >/dev/null 2>&1; then
    printf 'unknown'
    return 0
  fi
  if ! count="$(git rev-list --count "${base}..refs/heads/${branch}" 2>/dev/null)"; then
    printf 'unknown'
    return 0
  fi
  if printf '%s' "$count" | grep -qE '^[0-9]+$'; then
    printf '%s' "$count"
  else
    printf 'unknown'
  fi
}

# worktree_dirty <path> -> print yes/no, or "unknown" when status fails (a
# missing or prunable directory). Ignored files never count. Only yes/no is
# printed, so no porcelain file name ever reaches a diagnostic.
worktree_dirty() {
  local path="$1" porcelain
  if ! porcelain="$(git --no-optional-locks -C "$path" status --porcelain 2>/dev/null)"; then
    printf 'unknown'
  elif [ -n "$porcelain" ]; then
    printf 'yes'
  else
    printf 'no'
  fi
}

# keep_work <candidate> <path> <dirty> <ahead> <base>
# Print the candidate unchanged when it provably holds no work; otherwise print
# a fresh name and WARN which branch was kept and how to continue it (#1565).
keep_work() {
  local candidate="$1" path="$2" dirty="$3" ahead="$4" base="$5" new
  if [ "$dirty" = "no" ] && [ "$ahead" = "0" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi
  new="$(fresh_name "$candidate")" || return 1
  log "WARNING: keeping existing_branch=${candidate} at ${path} (uncommitted/untracked changes: ${dirty}, commits ahead of ${base}: ${ahead}); this run uses ${new}. To continue the kept branch, re-run with existing_branch=${candidate}."
  printf '%s\n' "$new"
}

cmd_resolve() {
  local base="" has_base=0
  if [ "${1:-}" = "--base" ]; then
    has_base=1
    base="${2:-}"
    if [ -z "$base" ]; then
      log "ERROR: --base requires a non-empty <base_ref>."
      usage
      return 1
    fi
    shift 2
  fi
  local candidate="${1:-}"
  local intended="${2:-}"
  local repo="${3:-$(pwd)}"

  if [ -z "$candidate" ] || [ -z "$intended" ]; then
    log "ERROR: resolve requires <candidate_branch> and <intended_worktree_path>."
    usage
    return 1
  fi

  if ! cd "$repo" 2>/dev/null; then
    log "ERROR: repo_path '$repo' is not accessible."
    return 1
  fi
  if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    # Not a git repo — nothing to introspect. Return the candidate untouched so
    # the caller proceeds with its legacy behaviour (graceful, non-fatal).
    printf '%s\n' "$candidate"
    return 0
  fi

  local owner
  owner="$(branch_worktree_path "$candidate" || true)"

  if [ -z "$owner" ]; then
    # Branch not checked out by any worktree: absent, or a leftover branch this
    # recipe may safely manage itself (normal State-2). Return unchanged, unless
    # --base shows the leftover branch holds commits a re-run must keep.
    if [ "$has_base" -eq 1 ] && git show-ref --verify --quiet "refs/heads/${candidate}"; then
      keep_work "$candidate" "(no worktree)" "no" "$(commits_ahead "$base" "$candidate")" "$base"
      return
    fi
    printf '%s\n' "$candidate"
    return 0
  fi

  local owner_norm intended_norm
  owner_norm="$(normalize_path "$owner")"
  intended_norm="$(normalize_path "$intended")"

  if [ "$owner_norm" = "$intended_norm" ]; then
    # Same-path resume (State-1): this is our own worktree. Return unchanged,
    # unless --base shows it holds work a re-run must keep (#1565).
    if [ "$has_base" -eq 1 ]; then
      keep_work "$candidate" "$owner_norm" "$(worktree_dirty "$owner")" \
        "$(commits_ahead "$base" "$candidate")" "$base"
      return
    fi
    printf '%s\n' "$candidate"
    return 0
  fi

  # Foreign ownership: a DIFFERENT worktree holds this branch. Do NOT touch it.
  # Rename THIS run's branch to a fresh, provably-free name.
  log "INFO: branch '${candidate}' is owned by a foreign worktree at '${owner_norm}'"
  log "INFO: (intended path is '${intended_norm}') — deconflicting to a new branch."

  fresh_name "$candidate"
}

main() {
  local op="${1:-}"
  case "$op" in
    resolve)
      shift
      cmd_resolve "$@"
      ;;
    ""|-h|--help|help)
      usage
      [ -z "$op" ] && return 1 || return 0
      ;;
    *)
      log "ERROR: unknown operation '$op'."
      usage
      return 1
      ;;
  esac
}

main "$@"
