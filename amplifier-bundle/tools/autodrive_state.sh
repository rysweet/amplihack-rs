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
# For most phases `autodrive_phase_done` only lets a resumed run skip work; the
# merge gate re-verifies those criteria in the run that merges. The exception
# is `crusty-loop`: the merge gate reads that marker, together with the final
# verdict in crusty-latest.json, as evidence for merge-ready criterion 3 (issue
# #1517), and only from a state dir private to this user. The only permitted
# writers are autodrive-crusty-loop.yaml (the marker, after the loop reports
# DONE) and autodrive_loop.sh (crusty-latest.json, a copy of the last round
# record, and crusty-records.tsv, the manifest of the round records the loop
# wrote, one row per round with the record's git blob hash). No agent step may
# create, edit or delete these files; autodrive_crusty_final below trusts a
# record only when the manifest names it and its hash still matches.

autodrive_mark_phase_done() {
  local dir="${1:?state dir}" phase="${2:?phase}"
  printf '%s\t%s\n' "$phase" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$dir/phases.tsv"
}

autodrive_phase_done() {
  local dir="${1:?state dir}" phase="${2:?phase}"
  [ -f "$dir/phases.tsv" ] || return 1
  grep -qF "$(printf '%s\t' "$phase")" "$dir/phases.tsv"
}

# --- criterion 3: the crusty loop's last loop-written record ---------------

# autodrive_blob_hash <dir> < file -> the git blob hash of stdin. git runs with
# <dir> as its working directory, so the config of whatever checkout the caller
# is in cannot change the object format or apply filters. The loop computes
# the manifest hash the same way.
autodrive_blob_hash() {
  ( cd -- "${1:-.}" && env -u GIT_DIR -u GIT_WORK_TREE git hash-object --no-filters --stdin ) 2>/dev/null
}

# autodrive_crusty_manifest_row <dir> -> "<file> <hash>" from the last
# non-blank row of crusty-records.tsv (CRLF tolerated), printed only when that
# row has exactly three tab-separated fields: a label, a file name matching
# ^crusty-[A-Za-z0-9._-]+\.json$, and a 40- or 64-character hex hash. Returns
# 1 and prints nothing otherwise, including for an absent or symlinked file.
autodrive_crusty_manifest_row() {
  local m="${1:-}/crusty-records.tsv"
  { [ -n "${1:-}" ] && [ -f "$m" ] && [ ! -L "$m" ]; } || return 1
  LC_ALL=C tr -d '\r' < "$m" | LC_ALL=C awk -F '\t' '
    /[^[:space:]]/ { last = $0 }
    END {
      n = split(last, f, "\t")
      ok = (n == 3 && f[1] ~ /^[A-Za-z0-9._-]+$/ && f[2] ~ /^crusty-[A-Za-z0-9._-]+\.json$/)
      ok = ok && f[3] ~ /^[0-9a-f]+$/ && (length(f[3]) == 40 || length(f[3]) == 64)
      if (!ok) exit 1
      print f[2], f[3]
    }'
}

# autodrive_crusty_final <state dir> -> criterion 3 under auto-drive (#1517).
#
# Prints only the reviewed head SHA and returns 0 when the crusty loop ended
# DONE and its last loop-written round record is CLEAN. Otherwise prints one
# token and returns 1. The checks run in this order:
#
#   crusty-loop-not-done     no `crusty-loop` marker in phases.tsv
#   crusty-manifest-missing  crusty-records.tsv absent, a symlink, or its last row malformed
#   crusty-record-missing    the record that row names is absent or a symlink
#   crusty-record-modified   its hash differs from the manifest or from crusty-latest.json,
#                            or it is not one line that starts with crusty_verdict and
#                            carries exactly one reviewed_head_sha
#   crusty-not-clean         its verdict is not CLEAN
#   crusty-head-sha-empty    reviewed_head_sha is not a 40- or 64-character hex SHA
#
# The record is copied once into a private temporary file, and that copy is
# both hashed and parsed, so the file cannot change between the two. File
# contents are never printed. Records the manifest does not name are ignored.
# Needs only bash, git and coreutils, and is safe under `set -u`.
autodrive_crusty_final() {
  local -x LC_ALL=C
  local dir="${1:-}" row="" file="" want="" copy="" tok="" sha="" latest=""
  if [ -z "$dir" ] || ! autodrive_phase_done "$dir" "crusty-loop"; then
    printf 'crusty-loop-not-done\n'; return 1
  fi
  row="$(autodrive_crusty_manifest_row "$dir")" || { printf 'crusty-manifest-missing\n'; return 1; }
  file="${row%% *}"; want="${row#* }"
  if [ ! -f "$dir/$file" ] || [ -L "$dir/$file" ]; then
    printf 'crusty-record-missing\n'; return 1
  fi
  copy="$(mktemp "${TMPDIR:-/tmp}/autodrive-crusty.XXXXXX" 2>/dev/null)" \
    || { echo "ERROR: cannot create temporary copy" >&2; printf 'crusty-record-modified\n'; return 1; }
  latest="$dir/crusty-latest.json"
  if ! cat -- "$dir/$file" > "$copy" 2>/dev/null \
     || [ "$(autodrive_blob_hash "$dir" < "$copy")" != "$want" ] \
     || [ ! -f "$latest" ] || [ -L "$latest" ] \
     || [ "$(autodrive_blob_hash "$dir" < "$latest")" != "$want" ] \
     || [ "$(awk 'END { print NR }' "$copy")" != "1" ] \
     || ! grep -Eq '^\{"crusty_verdict":"(CLEAN|CONCERNS)",' "$copy" \
     || [ "$(grep -o '"reviewed_head_sha":' "$copy" | wc -l | tr -d ' ')" != "1" ]; then
    tok="crusty-record-modified"
  elif ! grep -Eq '^\{"crusty_verdict":"CLEAN",' "$copy"; then
    tok="crusty-not-clean"
  else
    sha="$(sed -n 's/.*"reviewed_head_sha":"\([0-9a-f]*\)".*/\1/p' "$copy")"
    case "${#sha}" in 40|64) ;; *) tok="crusty-head-sha-empty" ;; esac
  fi
  rm -f -- "$copy"
  if [ -n "$tok" ]; then printf '%s\n' "$tok"; return 1; fi
  printf '%s\n' "$sha"
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
