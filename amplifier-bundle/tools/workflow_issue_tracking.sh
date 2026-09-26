#!/usr/bin/env bash
# workflow_local_tracking.sh — local (provider-free) issue tracking metadata.
#
# Extracted verbatim from `step-03-create-issue` in
# amplifier-bundle/recipes/workflow-prep.yaml. That brick is at its 400-line
# budget (scripts/check-brick-budget.sh), and the rule's remedy for a full brick
# is extraction, never compression — so the room for the issue claim check
# (#1361) was bought by moving this self-contained block out, the same way the
# 23-step plan banner moved to workflow_plan_banner.sh.
#
# These functions are meant to be SOURCED by the step, and they read the step's
# own variables (`EXISTING_ISSUE_NUMBER`, `TASK_DESC`) from the caller's scope.
#
#   derive_local_tracking_id  — a stable `local-issue-N` / `local-ab-N` /
#                               `local-<hash>` reference for a repository with
#                               no reachable issue provider
#   emit_local_metadata       — the tracking_* key/value block step-03b parses
#   sanitize_cli_output       — redact provider CLI output before it is logged
#   issue_create_host_unsupported — true when a failed `gh issue create` means
#                               this host cannot reach GitHub issues at all

derive_local_tracking_id() {
  if [[ "$EXISTING_ISSUE_NUMBER" =~ ^#?([0-9]+)$ ]]; then
    printf 'local-issue-%s' "${BASH_REMATCH[1]}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ AB#([0-9]+)|_workitems/edit/([0-9]+) ]]; then
    printf 'local-ab-%s' "${BASH_REMATCH[1]:-${BASH_REMATCH[2]}}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ local-(issue|ab)-([0-9]+)$ ]]; then
    printf '%s' "$EXISTING_ISSUE_NUMBER"
  elif [[ "$TASK_DESC" =~ ([Ii]ssue[[:space:]#]*|#)([0-9]+) ]]; then
    printf 'local-issue-%s' "${BASH_REMATCH[2]}"
  elif [[ "$TASK_DESC" =~ AB#([0-9]+) ]]; then
    printf 'local-ab-%s' "${BASH_REMATCH[1]}"
  else
    printf 'local-%s' "$(printf '%s' "$TASK_DESC" | sha256sum | cut -c1-12)"
  fi
}

emit_local_metadata() { LOCAL_REF="$(derive_local_tracking_id)"; LOCAL_NUM=""; [[ "$LOCAL_REF" =~ local-(issue|ab)-([0-9]+)$ ]] && LOCAL_NUM="${BASH_REMATCH[2]}"; printf 'tracking_system=local\ntracking_reference=%s\ntracking_issue=%s\nissue_creation=local-tracking\n' "$LOCAL_REF" "$LOCAL_REF"; [ -n "$LOCAL_NUM" ] && printf 'issue_number=%s\n' "$LOCAL_NUM"; return 0; }

sanitize_cli_output() { printf '%s\n' "$1" | head -c 4000 | sed -E 's#https?://[^[:space:]]*@#https://<redacted>@#g; s#gh[pousr]_[A-Za-z0-9_]{8,}#<redacted-token>#g; s#github_pat_[A-Za-z0-9_]+#<redacted-token>#g; s#[Bb]earer[[:space:]]+[A-Za-z0-9._~+/=-]{20,}#Bearer <redacted-token>#g; s#[A-Za-z0-9]{52}#<redacted-token>#g'; }

# issue_find_tracker TITLE — step-03's existing-tracker lookup. No --search:
# it lists the open issues (up to 1000; on GraphQL-blocked hosts gh-compat
# pages them over REST) and prints the URL of the first one whose title is
# TITLE — the same Unicode words (runs of letters and digits) in order, case,
# spacing and punctuation aside — or nothing when none is. The comparison runs
# in gh's own --jq, so no system jq is needed. A title with no word matches
# nothing. A lookup that fails does not read as "no tracker" (that would file a
# duplicate): it is reported on stderr and returns 1, except on a host that
# cannot reach GitHub issues at all (GraphQL blocked with no fallback, gh
# missing), where step-03's create path takes over and falls back to local
# tracking.
issue_find_tracker() {
  local t out err rc=0 errf
  # The title as a jq string literal: backslash and quote escaped, control
  # characters (none survive step-03's title cleanup) dropped.
  t="$(printf '%s' "$1" | tr -d '\000-\037')"; t="${t//\\/\\\\}"; t="${t//\"/\\\"}"
  errf="$(mktemp "${TMPDIR:-/tmp}/tracker-lookup.XXXXXX")" || { echo "ERROR: tracking-issue lookup: mktemp failed" >&2; return 1; }
  out="$(timeout 60 gh issue list --state open --limit 1000 --json number,title,url --jq '
    def norm: [scan("[\\p{L}\\p{N}]+") | ascii_downcase];
    ("'"$t"'" | norm) as $want
    | if ($want | length) == 0 then "" else ([.[] | select((.title // "" | norm) == $want)][0].url // "") end' 2>"$errf")" || rc=$?
  err="$(cat "$errf")"; rm -f "$errf"
  if [ "$rc" -ne 0 ]; then
    if issue_create_host_unsupported "$rc" "$err"; then return 0; fi
    echo "ERROR: looking up an existing tracking issue failed (rc $rc); not creating one, which could duplicate it. gh reported:" >&2
    sanitize_cli_output "$err" >&2
    return 1
  fi
  case "$out" in https://*|http://*|'') printf '%s\n' "$out" ;;
    *) echo "ERROR: tracking-issue lookup returned unexpected output:" >&2; sanitize_cli_output "$out" >&2; return 1 ;;
  esac
}

# issue_create_host_unsupported RC OUTPUT — true only for the failures issue
# #1484 is about: gh is missing (rc 127 / "command not found"), or the host
# blocks GitHub GraphQL (the Claude Code on the web refusal, or amplihack's gh
# compatibility layer reporting a subcommand it cannot replay over REST). Any
# other failure (permission denied, issues disabled, a bad payload) is a real
# error on a host where gh works, and step-03 must keep failing loudly on it.
issue_create_host_unsupported() {
  [ "${1:-}" = 127 ] && return 0
  printf '%s' "${2:-}" | grep -Ei 'GraphQL is not available|GraphQL (API )?(is )?(disabled|blocked)|GraphQL, which this host blocks|gh: command not found|command not found: gh|failed to run command .gh.' >/dev/null
}

# Percent-decode one path segment of an Azure DevOps remote URL. Returns 1 (and
# an empty result) on a malformed or NUL-bearing encoding so the caller falls
# back to local tracking rather than acting on a half-decoded org/project.
_pct_decode() {
  local e="$1" d="" i=0 ch hex
  while [ "$i" -lt "${#e}" ]; do
    ch="${e:$i:1}"
    if [ "$ch" = "%" ] && [ "$((i+2))" -le "${#e}" ]; then
      hex="${e:$((i+1)):2}"
      if [[ "$hex" =~ ^[0-9a-fA-F]{2}$ ]]; then
        [ "$hex" = "00" ] && { echo "WARN: NUL byte in percent-encoding" >&2; echo ""; return 1; }
        printf -v tmp "\\x$hex"; d+="$tmp"; i=$((i+3)); continue
      fi
      echo "WARN: Invalid percent-encoding '%${hex}' — using local tracking" >&2
      echo ""; return 1
    fi
    d+="$ch"; i=$((i+1))
  done
  printf '%s' "$d"
}
