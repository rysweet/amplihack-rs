#!/usr/bin/env bash
# autodrive_merge.sh: the auto-drive merge request, under GitHub's own
# conditions only (#2575). docs/reference/autodrive-merge-ready-round.md.
#
# The caller is autodrive-finalize's `merge` step, after it has checked the
# captured authorisation and that the PR worktree is still at the head the
# merge-ready round assessed. No agent-written text is read here. The merge
# needs the PR to be open, its checks green or absent, and GitHub to accept
# `gh pr merge --match-head-commit <expected sha>`, which also enforces
# required reviews, branch protection and a head that moved on the server.
#
# This file only DEFINES a function; sourcing it has no other effect.
#
#   autodrive_merge <pr> <expected_sha>
#       Calls gh once for each, in order: PR state, title, body and
#       mergeStateStatus (one read), `checks --required` (falling back to all
#       checks only when gh reports that no required checks exist), the merge,
#       then either the PR state after the merge or, after a refusal, the
#       mergeStateStatus once more.
#       No retries and no waits: a pending check ends this call with
#       checks-pending, and the next drive run reads the checks again. A PR
#       with no checks reported at all goes on to GitHub's own decision.
#       Prints one JSON line {"result":...,"pr":N,"expected_head":"...","reason":"..."}.
#         0  merged: GitHub reads the PR MERGED at the expected head, whether
#            before the request, after it, or after gh reported an error
#            (#2663: GitHub's merge state decides, never a later comparison)
#         2  bad arguments, or the merge-message helper failed its path checks
#         3  checks-not-green: a check failed or could not be read; no merge attempted
#         4  refused: GitHub declined the merge, or did not report it merged
#         5  not-open: the PR was not open before the merge
#         6  message-unavailable: the merge-message helper refused the title or body
#         7  checks-pending: a check is still running; no merge attempted
#         8  base-behind: DIRTY before the merge, or BEHIND/DIRTY read back
#            after GitHub declined it; the merge loop merges the base in
#         79 a gh call exited 79 (terminal refusal); passed through at once
#
# The squash subject and body come from the trusted launch-bundle copy of
# autodrive_merge_message.sh (#2056), sourced only from the absolute
# AMPLIHACK_HOME and never through a symbolic link. The body reaches gh on
# standard input.

autodrive_merge() {
  local -x LC_ALL=C
  local pr="${1:-}" sha="${2:-}" mm state rc vrc out errf json subject ms
  _autodrive_merge_emit() { # <result> <reason>
    jq -cn --arg r "$1" --argjson pr "$pr" --arg sha "$sha" --arg why "$2" \
      '{result:$r,pr:$pr,expected_head:$sha,reason:$why}'
  }
  # _autodrive_merge_wait <gh pr checks output>: checks-pending, naming the
  # pending checks with control characters removed and cut at 300 bytes.
  _autodrive_merge_wait() {
    local names
    names="$(printf '%s\n' "$1" | awk -F '\t' '$2 == "pending" { print $1 }' \
      | tr '\n' ',' | tr -d '\000-\037\177' | sed 's/,$//; s/,/, /g' | head -c 300)"
    _autodrive_merge_emit checks-pending "waiting for ${names:-required checks} at ${sha}"
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

  # _autodrive_merge_at_head <gh pr view JSON>: true only when GitHub reads
  # the PR MERGED and its head is the expected head (#2663).
  _autodrive_merge_at_head() {
    [ "$(printf '%s' "$1" | jq -r 'select(.state == "MERGED") | .headRefOid // empty | strings' 2>/dev/null)" = "$sha" ]
  }

  json="$(gh pr view "$pr" --json state,title,body,mergeStateStatus,headRefOid)"; rc=$?
  [ "$rc" -eq 79 ] && return 79
  state=""
  [ "$rc" -eq 0 ] && state="$(printf '%s' "$json" | jq -r '.state // empty' 2>/dev/null)"
  if [ "$rc" -eq 0 ] && _autodrive_merge_at_head "$json"; then
    _autodrive_merge_emit merged "GitHub already reads the PR merged at the expected head; no merge request made"
    return 0
  fi
  if [ "$rc" -ne 0 ] || [ "$state" != OPEN ]; then
    _autodrive_merge_emit not-open "the PR state read before the merge was '${state:-unreadable}' (gh exit ${rc})"
    return 5
  fi

  # Checks are read once; a pending check is a wait for the next wake, never
  # a poll here (#2575 B2). A PR with no checks at all is not refused for
  # that reason: GitHub's branch protection decides (#2575 minor 3).
  # gh's "no checks" message is read from stderr only, and only when stdout
  # lists no checks: check names come from the PR's own workflows, so a
  # failing check named "no checks reported" must stay a failing check.
  errf="$(mktemp)" || { _autodrive_merge_emit checks-not-green "the checks could not be read (no temporary file)"; return 3; }
  out="$(gh pr checks "$pr" --required 2>"$errf")"; rc=$?
  [ "$rc" -eq 79 ] && { rm -f "$errf"; return 79; }
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 8 ] && [ -z "$out" ] && grep -qF 'no required checks reported' "$errf"; then
    out="$(gh pr checks "$pr" 2>"$errf")"; rc=$?
    [ "$rc" -eq 79 ] && { rm -f "$errf"; return 79; }
    if [ "$rc" -ne 0 ] && [ "$rc" -ne 8 ] && [ -z "$out" ] && grep -qF 'no checks reported' "$errf"; then
      rc=0
    fi
  fi
  rm -f "$errf"
  case "$rc" in
    0) ;;
    8) _autodrive_merge_wait "$out"; return 7 ;;
    *) _autodrive_merge_emit checks-not-green "checks are failing or unreadable (gh exit ${rc})"; return 3 ;;
  esac

  ms="$(printf '%s' "$json" | jq -r '.mergeStateStatus // empty | strings' 2>/dev/null)"
  if [ "$ms" = DIRTY ]; then
    _autodrive_merge_emit base-behind "the branch conflicts with its base (mergeStateStatus DIRTY); the base must be merged in first"
    return 8
  fi

  if ! subject="$(autodrive_merge_subject "$json" "$pr")" || ! autodrive_merge_body "$json"; then
    _autodrive_merge_emit message-unavailable "the PR title or body could not be read into a squash message (#2056)"
    return 6
  fi

  printf '%s' "$MERGE_BODY" | gh pr merge "$pr" --squash --match-head-commit "$sha" --subject "$subject" --body-file -
  rc="${PIPESTATUS[1]}"
  [ "$rc" -eq 79 ] && return 79
  if [ "$rc" -ne 0 ]; then
    # Read the PR once more. GitHub may have merged it even though gh exited
    # non-zero; merged at the expected head is merged, whatever the base now
    # reads (#2663). Otherwise an out-of-date or conflicting branch is
    # base-behind (the merge loop syncs the base); anything else is refused.
    json="$(gh pr view "$pr" --json state,headRefOid,mergeStateStatus)"; vrc=$?
    [ "$vrc" -eq 79 ] && return 79
    ms=""
    if [ "$vrc" -eq 0 ]; then
      if _autodrive_merge_at_head "$json"; then
        _autodrive_merge_emit merged "GitHub reads the PR merged at the expected head although gh exited ${rc}; report cut from the squash body: ${MERGE_REPORT_CUT:-no}"
        return 0
      fi
      ms="$(printf '%s' "$json" | jq -r '.mergeStateStatus // empty | strings' 2>/dev/null)"
    fi
    case "$ms" in
      BEHIND|DIRTY)
        _autodrive_merge_emit base-behind "GitHub declined the merge and the branch is out of date with its base (mergeStateStatus ${ms})"
        return 8 ;;
      *)
        _autodrive_merge_emit refused "GitHub declined the merge at the expected head (gh exit ${rc})"
        return 4 ;;
    esac
  fi

  json="$(gh pr view "$pr" --json state,headRefOid)"; rc=$?
  [ "$rc" -eq 79 ] && return 79
  if [ "$rc" -ne 0 ] || ! _autodrive_merge_at_head "$json"; then
    state="$(printf '%s' "$json" | jq -r '.state // empty | strings' 2>/dev/null)"
    _autodrive_merge_emit refused "the merge request returned but the PR does not read MERGED at the expected head (state '${state:-unreadable}')"
    return 4
  fi
  _autodrive_merge_emit merged "merged at the expected head; report cut from the squash body: ${MERGE_REPORT_CUT:-no}"
  return 0
}
