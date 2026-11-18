#!/usr/bin/env bash
# autodrive_round_evidence.sh — the measured, deterministic steps of one merge
# round of auto-drive-to-merge (issue #1517, D4 and D5).
#
# These were step bodies of autodrive-merge-round.yaml until that recipe reached
# its brick budget (PR #1520 review); they moved here unchanged. The recipe's
# step-00-tools-dir finds this file once per round, with the tools beside it.
# crusty-range left the round in the next review: its caller now runs a crusty
# loop when it answers "true", and that loop must not start inside a round.
#
#   autodrive_round_evidence.sh crusty-range
#       Run by autodrive_crusty_rereview.sh before each merge round, from the
#       merge-ready loop's shell, never by a round step: does crusty re-review
#       the current head before the next round assesses anything? One JSON line:
#       {"rereview":"true|false","range":"...","first_unreviewed_sha":"...","base_sha":"..."}
#
#   autodrive_round_evidence.sh qa-evidence-sha <file>
#       step-00d-qa-evidence-hash. The hash of the qa evidence file as
#       autodrive_qa_evidence_sha computes it, or nothing when the file is not
#       a regular file.
#
#   autodrive_round_evidence.sh qa-evidence-hash <file>
#       step-00d-qa-evidence-hash itself: qa-evidence-sha as the step's one JSON
#       line, {"qa_evidence_sha":"..."}, empty unless whole 40- or 64-digit
#       lowercase hex. Moved here from the step at its brick budget (#1561).
#
#   autodrive_round_evidence.sh crusty-covers <sha>
#       Run by the step-01b agent for a Crusty verdict comment at an earlier
#       head (#2628): does that verdict still cover HEAD? Walks the commits
#       from <sha> to HEAD with the re-review rules (autodrive_crusty_range)
#       and prints one token: `ok`, `crusty-unreviewed-commits:<sha>` or
#       `crusty-range-unreadable`. Reads git only, never a state file. A
#       <sha> that is not 40 or 64 lowercase hex prints
#       crusty-range-unreadable and exits 2. It replaces the retired
#       crusty-evidence subcommand: criterion 3 is no longer read from
#       crusty-records.tsv.
#
#   The record-only subcommands measured-downgrade, merge-ready-verdict,
#   findings and round-record were retired with the parsed merge-ready record
#   (#2575): the round now writes its one-field round file through
#   autodrive_round_word.sh.
#
# Step outputs are read only as recipe-runner's RECIPE_VAR_<key> export:
# "${RECIPE_VAR_qa_evidence:-}" and the like. A bare QA_EVIDENCE, CI_EVIDENCE
# or MERGE_READY_VERDICT inherited from whatever launched the runner is never
# read (#1561; docs/reference/auto-drive-to-merge.md, "Step outputs ignore
# inherited bare variables").
# AUTODRIVE_STATE_DIR, AUTODRIVE_QA_EVIDENCE, AUTODRIVE_ROUND_RECORD,
# AUTODRIVE_ROUND_LABEL, PR_NUMBER and REPO_PATH come from there too.
# crusty-range, qa-evidence-sha and crusty-covers print only fixed tokens and
# hex SHAs, so nothing from the branch under review reaches step-04's prompt through
# them as prose.
#
# Executed, never sourced. autodrive_state.sh and autodrive_trust.sh are sourced
# from beside this file and from nowhere else. When they are missing, each
# subcommand reports what the step reported when it could not find them: no
# re-review, an empty hash, crusty-range-unreadable, or qa_evidence=modified. None of
# those can merge.
#
# Exit 0 with the output above; exit 2 for an unknown subcommand.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
HELPERS="false"
if [ -n "$HERE" ] && [ -f "$HERE/autodrive_state.sh" ] && [ -f "$HERE/autodrive_trust.sh" ]; then
  # shellcheck source=/dev/null
  . "$HERE/autodrive_state.sh" && . "$HERE/autodrive_trust.sh" && HELPERS="true"
fi

# The PR number from the environment, digits only, or nothing; `gh pr view`
# with no number reads the current branch's PR.
pr_base_ref() {
  [ -z "${AUTODRIVE_BASE_BRANCH:-}" ] || { printf '%s\n' "$AUTODRIVE_BASE_BRANCH"; return 0; }
  local pr="${PR_NUMBER:-}"
  case "$pr" in *[!0-9]*) pr="" ;; esac
  gh pr view ${pr:+"$pr"} --json baseRefName --jq .baseRefName 2>/dev/null || printf ''
}

# field <json> <name> <default>: the canonical structured read
# (docs/reference/structured-verdict-parsing.md), the last JSON object that
# carries the field.
field() {
  printf '%s' "${1:-}" | amplihack orch helper extract-json --require-field "$2" \
    | amplihack orch helper extract-field --field "$2" --default "$3"
}

# token <value>: the value with every byte outside [A-Za-z0-9_.:/-] removed.
token() { printf '%s' "${1:-}" | LC_ALL=C tr -cd 'A-Za-z0-9_.:/-'; }

# hex_or_empty <value>: the value when it is 40 or 64 lowercase hex, else nothing.
hex_or_empty() {
  case "${#1}:${1}" in
    40:*[!0123456789abcdef]* | 64:*[!0123456789abcdef]*) ;;
    40:* | 64:*) printf '%s' "$1" ;;
  esac
}

crusty_range() {
  local no='{"rereview":"false","range":"not-checked","first_unreviewed_sha":"","base_sha":""}'
  local dir="${AUTODRIVE_STATE_DIR:-}" out
  if [ -z "$dir" ]; then
    echo "WARNING: no autodrive_state_dir was passed to this round; the range is not checked here." >&2
    printf '%s\n' "$no"; return 0
  fi
  if [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_state.sh and autodrive_trust.sh not found together beside autodrive_round_evidence.sh; the range is not checked here and step-01b reports it." >&2
    printf '%s\n' "$no"; return 0
  fi
  out="$(autodrive_rereview_decision "$dir" "$PWD" "$(pr_base_ref)")" || out=""
  case "$out" in
    '{"rereview":"'*) ;;
    *) echo "WARNING: autodrive_rereview_decision gave no decision; crusty is not re-run here." >&2; out="$no" ;;
  esac
  echo "INFO: crusty range at round start: $(printf '%s' "$out" | tr -cd 'A-Za-z0-9_:,{}"-')" >&2
  printf '%s\n' "$out"
}

qa_evidence_sha() {
  local sha=""
  if [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_trust.sh not found beside autodrive_round_evidence.sh; the qa evidence cannot be hashed." >&2
    return 0
  fi
  sha="$(autodrive_qa_evidence_sha "${1:-}")" || sha=""
  [ -z "$sha" ] || printf '%s\n' "$sha"
}

qa_evidence_hash() {
  local q
  q="$(hex_or_empty "$(qa_evidence_sha "${1:-}")")"
  [ -n "$q" ] || echo "WARNING: the qa evidence is not a regular file; qa_evidence_sha is empty, so this round cannot reach MERGE_READY." >&2
  printf '{"qa_evidence_sha":"%s"}\n' "$q"
}

# crusty_covers <sha>: does a Crusty verdict at <sha> still cover HEAD? Read
# only: git and the base lookup, never a state file (#2628).
crusty_covers() {
  local sha="${1:-}" head="" base="" res=""
  if ! [[ "$sha" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
    echo "ERROR: crusty-covers takes one 40- or 64-character lowercase hex SHA" >&2
    printf 'crusty-range-unreadable\n'; exit 2
  fi
  if [ "$HELPERS" != "true" ]; then
    echo "WARNING: autodrive_state.sh and autodrive_trust.sh not found beside autodrive_round_evidence.sh; the range cannot be checked." >&2
    printf 'crusty-range-unreadable\n'; return 0
  fi
  head="$(autodrive_trust_git "$PWD" rev-parse --verify --quiet "HEAD^{commit}" 2>/dev/null)" || head=""
  if ! autodrive_is_sha "$head"; then
    printf 'crusty-range-unreadable\n'; return 0
  fi
  # An empty base SHA (no base name or a failed fetch) excludes nothing, so a
  # base merge counts as code and Crusty runs again: never a pass.
  base="$(autodrive_base_sha "$PWD" "$(pr_base_ref)")" || base=""
  res="$(autodrive_crusty_range "$PWD" "$sha" "$head" "$base")"
  case "$res" in
    ok|crusty-unreviewed-commits:*) printf '%s\n' "$res" ;;
    *) printf 'crusty-range-unreadable\n' ;;
  esac
}

# Ordinary workflow evidence, not a checkout, manifest or mutation gate. The
# recipe checkpoint retains this output across an interrupted reviewer resume.
# The directory holds the contents of the worktree's untracked files and may
# hold secrets, so nothing outside the round reads it and autodrive_loop.sh
# removes every pre-review.* in the state directory once the round returns
# (#2808 SR-10).
#
# Evidence only, and deliberately so. Collection failure is non-fatal: the
# function writes 'incomplete' to collection-status, warns on stderr and still
# returns the directory and exit 0, leaving the assessor to preserve uncertain
# work. Do not add a hash comparison against this baseline, and do not exit
# non-zero when the worktree differs from it. Either change turns the round's
# observation back into the must-match refusal that #2808 removed.
review_baseline() (
  umask 077
  export GIT_OPTIONAL_LOCKS=0
  . "$HERE/git-repo-env.sh"
  amplihack_clear_git_repo_env
  cd -- "${REPO_PATH:-.}" || return 1
  local dir index shared
  dir="$(mktemp -d "${AUTODRIVE_STATE_DIR:-${AUTODRIVE_ROUND_RECORD%/*}}/pre-review.XXXXXX")" || return 1
  collect() {
    git rev-parse --show-toplevel --absolute-git-dir --git-common-dir > "$dir/repository" || return 1
    git rev-parse HEAD > "$dir/head" || return 1
    git symbolic-ref -q HEAD > "$dir/branch"
    local branch_rc=$?
    # A detached HEAD has no symbolic branch and is a valid observation.
    [ "$branch_rc" -eq 0 ] || [ "$branch_rc" -eq 1 ] || return "$branch_rc"
    index="$(git rev-parse --path-format=absolute --git-path index)" || return 1
    if [ -f "$index" ]; then cp -- "$index" "$dir/index" || return 1; fi
    shared="$(git rev-parse --path-format=absolute --shared-index-path)" || return 1
    if [ -n "$shared" ]; then cp -- "$shared" "$dir/$(basename -- "$shared")" || return 1; fi
    git -c core.fsmonitor=false status --porcelain=v1 -z --untracked-files=all > "$dir/status" &&
    git ls-files --stage -z > "$dir/entries" &&
    git -c core.fsmonitor=false diff --binary --no-ext-diff --no-textconv --ignore-submodules=all --cached > "$dir/staged.diff" &&
    git -c core.fsmonitor=false diff --binary --no-ext-diff --no-textconv --ignore-submodules=all > "$dir/unstaged.diff" &&
    git ls-files --cached --others --exclude-standard -z > "$dir/paths" || return 1
    # Use NUL names, retain symlink targets and modes, never follow links. Save
    # only paths still present: deleted files remain represented by Git evidence.
    local path parent safe incomplete=0
    while IFS= read -r -d '' path; do
      parent="$path"; safe=1
      while [[ "$parent" == */* ]]; do
        parent="${parent%/*}"
        if [ -L "$parent" ]; then safe=0; incomplete=1; break; fi
      done
      if [ "$safe" = 1 ] && { [ -e "$path" ] || [ -L "$path" ]; }; then printf '%s\0' "$path" || return 1; fi
    done < "$dir/paths" > "$dir/present-paths"
    tar --create --file "$dir/contents.tar" --no-recursion --null --files-from "$dir/present-paths" || return 1
    [ "$incomplete" = 0 ] || { echo "A path has a symlink parent; external contents were not collected" >&2; return 1; }
  }
  if collect 2> "$dir/collection-errors"; then
    printf 'complete\n' > "$dir/collection-status"
  else
    printf 'incomplete; inspect collection-errors and preserve existing work\n' > "$dir/collection-status"
    echo "WARNING: pre-review evidence is incomplete; the assessor must preserve uncertain work" >&2
  fi
  printf '%s\n' "$dir"
)

case "${1:-}" in
  review-baseline) review_baseline ;;
  crusty-range) crusty_range ;;
  qa-evidence-sha) qa_evidence_sha "${2:-}" ;;
  qa-evidence-hash) qa_evidence_hash "${2:-}" ;;
  crusty-covers) crusty_covers "${2:-}" ;;
  *) echo "ERROR: autodrive_round_evidence.sh: unknown subcommand '${1:-}' (crusty-range," \
       "qa-evidence-sha <file>, qa-evidence-hash <file>, crusty-covers <sha>)" >&2
     exit 2 ;;
esac
