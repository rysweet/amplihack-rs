#!/usr/bin/env bash
# autodrive_merge.sh: the auto-drive merge request, under GitHub's own
# conditions only (#2575). docs/reference/autodrive-merge-ready-round.md.
#
# The caller is autodrive-finalize's `merge` step, after it has checked the
# captured authorisation and that the PR worktree is still at the head the
# merge-ready round assessed. No agent-written text is read here. The merge
# needs the PR to be open, its checks green, and GitHub to accept
# `gh pr merge --match-head-commit <expected sha>`, which also enforces
# required reviews, branch protection and a head that moved on the server.
#
# This file only DEFINES a function; sourcing it has no other effect.
#
#   autodrive_merge <pr> <expected_sha>
#       Calls gh once for each, in order: PR state, title and body (one
#       read), `checks --required` (falling back to all checks only when gh
#       reports that no required checks exist), the merge, PR state after the
#       merge.
#       No retries and no waits: pending checks are not green. Prints one JSON
#       line {"result":...,"pr":N,"expected_head":"...","reason":"..."}.
#         0  merged (the read-back state is MERGED)
#         2  bad arguments, or the merge-message helper failed its path checks
#         3  checks-not-green; no merge attempted
#         4  refused: GitHub declined the merge, or did not report it merged
#         5  not-open: the PR was not open before the merge
#         6  message-unavailable: the merge-message helper refused the title or body
#         79 a gh call exited 79 (terminal refusal); passed through at once
#
# The squash subject and body come from the trusted launch-bundle copy of
# autodrive_merge_message.sh (#2056), sourced only from the absolute
# AMPLIHACK_HOME and never through a symbolic link. The body reaches gh on
# standard input.

autodrive_merge() {
  local -x LC_ALL=C
  local pr="${1:-}" sha="${2:-}" mm state rc err json subject
  _autodrive_merge_emit() { # <result> <reason>
    jq -cn --arg r "$1" --argjson pr "$pr" --arg sha "$sha" --arg why "$2" \
      '{result:$r,pr:$pr,expected_head:$sha,reason:$why}'
  }
  if [ "$#" -ne 2 ] || ! [[ "$pr" =~ ^[1-9][0-9]{0,9}$ ]] || ! [[ "$sha" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
    echo "ERROR: autodrive_merge: expected <pr> (a positive integer) and <expected_sha> (40 or 64 lowercase hex characters)" >&2
    return 2
  fi
  case "${AMPLIHACK_HOME:-}" in
    /*) ;;
    *) echo "ERROR: autodrive_merge: AMPLIHACK_HOME is not the absolute launch-bundle home (#2056)" >&2; return 2 ;;
  esac
  mm="${AMPLIHACK_HOME}/amplifier-bundle/tools/autodrive_merge_message.sh"
  if [ ! -f "$mm" ] || [ -L "$mm" ]; then
    echo "ERROR: autodrive_merge: the trusted autodrive_merge_message.sh is missing or a symbolic link (#2056)" >&2
    return 2
  fi
  # shellcheck source=autodrive_merge_message.sh
  . "$mm" || { echo "ERROR: autodrive_merge: cannot load autodrive_merge_message.sh (#2056)" >&2; return 2; }

  json="$(gh pr view "$pr" --json state,title,body)"; rc=$?
  [ "$rc" -eq 79 ] && return 79
  state=""
  [ "$rc" -eq 0 ] && state="$(printf '%s' "$json" | jq -r '.state // empty' 2>/dev/null)"
  if [ "$rc" -ne 0 ] || [ "$state" != OPEN ]; then
    _autodrive_merge_emit not-open "the PR state read before the merge was '${state:-unreadable}' (gh exit ${rc})"
    return 5
  fi

  err="$( { gh pr checks "$pr" --required >/dev/null; } 2>&1 )"; rc=$?
  [ "$rc" -eq 79 ] && return 79
  if [ "$rc" -ne 0 ] && [[ "$err" == *"no required checks reported"* ]]; then
    gh pr checks "$pr" >/dev/null 2>&1; rc=$?
    [ "$rc" -eq 79 ] && return 79
    if [ "$rc" -ne 0 ]; then
      _autodrive_merge_emit checks-not-green "no required checks exist and the PR's checks are failing, pending or absent (gh exit ${rc})"
      return 3
    fi
  elif [ "$rc" -ne 0 ]; then
    _autodrive_merge_emit checks-not-green "required checks are failing or pending (gh exit ${rc})"
    return 3
  fi

  if ! subject="$(autodrive_merge_subject "$json" "$pr")" || ! autodrive_merge_body "$json"; then
    _autodrive_merge_emit message-unavailable "the PR title or body could not be read into a squash message (#2056)"
    return 6
  fi

  printf '%s' "$MERGE_BODY" | gh pr merge "$pr" --squash --match-head-commit "$sha" --subject "$subject" --body-file -
  rc="${PIPESTATUS[1]}"
  [ "$rc" -eq 79 ] && return 79
  if [ "$rc" -ne 0 ]; then
    _autodrive_merge_emit refused "GitHub declined the merge at the expected head (gh exit ${rc})"
    return 4
  fi

  state="$(gh pr view "$pr" --json state --jq .state)"; rc=$?
  [ "$rc" -eq 79 ] && return 79
  if [ "$rc" -ne 0 ] || [ "$state" != MERGED ]; then
    _autodrive_merge_emit refused "the merge request returned but the PR reads '${state:-unreadable}', not MERGED"
    return 4
  fi
  _autodrive_merge_emit merged "merged at the expected head; report cut from the squash body: ${MERGE_REPORT_CUT:-no}"
  return 0
}
