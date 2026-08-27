#!/usr/bin/env bash
# autodrive_state.sh — durable, resumable state for the auto-drive-to-merge
# workflow.
#
# A run that dies partway must be re-runnable WITHOUT redoing merged work and
# WITHOUT reopening concerns a previous run already resolved and had confirmed
# clean. Exactly ONE store records what this workflow did:
#
#   Local — ${AMPLIHACK_STATE_DIR:-$HOME/.amplihack/state}/auto-drive/<key>/
#   Fast, always available, survives a crashed run on the same host.
#
# The authoritative answer to "is this already merged?" is not that store: it
# is the platform (`gh pr view --json state`). State files record what THIS
# workflow did; they never assert a merge that GitHub does not confirm.
#
# NO PR-COMMENT LEDGER. An earlier revision mirrored this store into a marked
# pull-request comment and rehydrated an empty local store from it. That made
# an ATTACKER-WRITABLE input — anyone who can comment on the PR — decide
# control flow: a forged `phases:` block containing `crusty-loop` skipped the
# entire crusty review, and a forged `resolved-concerns` list told the reviewer
# not to re-raise them. It fired precisely on a fresh host, where the local
# store is empty, which is the normal case for a fleet. Local state plus
# platform truth already cover everything the ledger was for, minus a
# fresh-host optimisation that is not worth an unauthenticated input into an
# automated merge authority. Do not reintroduce it.
#
# This file only DEFINES functions; sourcing it has no side effects.
#
# Policy note: nothing in this file, or anywhere in the auto-drive-to-merge
# workflow, may pass a hook-skipping commit flag or a branch-protection bypass
# to git or gh. Those two are NEVER used; see
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions.

# --- paths -----------------------------------------------------------------

# autodrive_state_root -> the auto-drive state root directory.
autodrive_state_root() {
  printf '%s/auto-drive\n' "${AMPLIHACK_STATE_DIR:-${HOME:-/tmp}/.amplihack/state}"
}

# autodrive_state_key <repo_path> <branch_or_pr> -> a filesystem-safe key.
autodrive_state_key() {
  local repo="${1:-.}" ident="${2:-}" slug
  slug="$(git -C "$repo" remote get-url origin 2>/dev/null || printf 'local')"
  slug="${slug%.git}"
  slug="$(printf '%s/%s' "$slug" "$ident" | tr -c 'A-Za-z0-9._-' '_')"
  printf '%s\n' "$slug"
}

# autodrive_state_dir <repo_path> <branch_or_pr> -> the state dir, created.
autodrive_state_dir() {
  local dir
  dir="$(autodrive_state_root)/$(autodrive_state_key "${1:-.}" "${2:-}")"
  mkdir -p "$dir" || return 1
  printf '%s\n' "$dir"
}

# --- phase completion ------------------------------------------------------
#
# A phase is recorded as done only after its own gate passed in a real run.
# `autodrive_phase_done` is therefore a resume optimisation, never evidence:
# the merge gate re-verifies every criterion in the run that merges.

autodrive_mark_phase_done() {
  local dir="${1:?state dir}" phase="${2:?phase}"
  printf '%s\t%s\n' "$phase" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$dir/phases.tsv"
}

autodrive_phase_done() {
  local dir="${1:?state dir}" phase="${2:?phase}"
  [ -f "$dir/phases.tsv" ] || return 1
  grep -qF "$(printf '%s\t' "$phase")" "$dir/phases.tsv"
}

# --- resolved crusty concerns ---------------------------------------------
#
# Concern identifiers that a previous run addressed AND had confirmed clean by
# a later crusty round. Passed back into crusty so a resumed run does not
# reopen settled ground. Crusty may still re-raise one with NEW evidence — the
# record informs the reviewer, it does not gag it. It is written ONLY by this
# host, from rounds this host actually ran; nothing off-host can seed it.

autodrive_record_resolved() {
  local dir="${1:?state dir}"
  shift
  local id
  for id in "$@"; do
    [ -n "$id" ] || continue
    printf '%s\n' "$id" >> "$dir/resolved-concerns.txt"
  done
  if [ -f "$dir/resolved-concerns.txt" ]; then
    sort -u "$dir/resolved-concerns.txt" -o "$dir/resolved-concerns.txt"
  fi
}

autodrive_resolved_concerns() {
  local dir="${1:?state dir}"
  [ -f "$dir/resolved-concerns.txt" ] && cat "$dir/resolved-concerns.txt"
  return 0
}

# --- platform truth --------------------------------------------------------

# autodrive_pr_state <pr> -> MERGED | OPEN | CLOSED | UNKNOWN
#
# UNKNOWN is a FAILURE signal for every caller: an unreadable platform state is
# never treated as "not merged" and never as "safe to merge".
autodrive_pr_state() {
  local pr="${1:-}" out
  case "$pr" in ''|*[!0-9]*) printf 'UNKNOWN\n'; return 0 ;; esac
  out="$(gh pr view "$pr" --json state,mergedAt 2>/dev/null)" || { printf 'UNKNOWN\n'; return 0; }
  local state merged
  state="$(printf '%s' "$out" | amplihack orch helper extract-field --field state --default '')"
  merged="$(printf '%s' "$out" | amplihack orch helper extract-field --field mergedAt --default '')"
  if [ "$state" = "MERGED" ] || { [ -n "$merged" ] && [ "$merged" != "null" ]; }; then
    printf 'MERGED\n'
  elif [ "$state" = "OPEN" ] || [ "$state" = "CLOSED" ]; then
    printf '%s\n' "$state"
  else
    printf 'UNKNOWN\n'
  fi
}
