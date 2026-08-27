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
