#!/usr/bin/env bash
# workflow_issue_tracking.sh — tracking-issue selection and local (provider-free)
# issue tracking metadata.
#
# The local-tracking functions were extracted from `step-03-create-issue` in
# amplifier-bundle/recipes/workflow-prep.yaml; the GitHub selection functions
# (#1595) were written here. That brick is at its 400-line budget
# (scripts/check-brick-budget.sh), and the remedy for a full brick is extraction,
# never compression; the room for the issue claim check (#1361) was bought the
# same way the 23-step plan banner moved to workflow_plan_banner.sh.
#
# These functions are meant to be SOURCED by the step, and they read the step's
# own variables (`EXISTING_ISSUE_NUMBER`, `TASK_DESC`) from the caller's scope.
#
#   derive_local_tracking_id  — a stable `local-issue-N` / `local-ab-N` /
#                               `local-<hash>` reference for a repository with
#                               no reachable issue provider
#   emit_local_metadata       — the tracking_* key/value block step-03b parses
#   sanitize_cli_output       — redact provider CLI output before it is logged
#   select_github_tracking_issue — GitHub tracking-issue choice (#1595): an
#                               explicit issue_number wins; otherwise the first
#                               task-text reference that is an open issue (not a
#                               PR) in this repository, marked ones first
#   _tracking_issue_masked_text — lower-cased task text with PR, owner/repo#N,
#                               AB#N and hex-colour references removed
#   _tracking_issue_refs      — `N kind` lines for one kind (marked or bare)
#   _tracking_issue_candidates — up to 5 `N kind` candidates, no network
#   _github_open_issue_url    — read-only check of one candidate via `gh api`
#   _is_github_issue_url      — the one URL check: https://…/issues/N, one line

derive_local_tracking_id() {
  if [[ "$EXISTING_ISSUE_NUMBER" =~ ^#?([0-9]+)$ ]]; then
    printf 'local-issue-%s' "${BASH_REMATCH[1]}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ AB#([0-9]+)|_workitems/edit/([0-9]+) ]]; then
    printf 'local-ab-%s' "${BASH_REMATCH[1]:-${BASH_REMATCH[2]}}"
  elif [[ "$EXISTING_ISSUE_NUMBER" =~ ^local-(issue|ab)-([0-9]+)$ ]]; then
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

# Masked, lower-cased task text: PR references, owner/repo#N, AB#N and #N
# followed by a word character (hex colours) are removed so they are never
# looked up. Only the first 64 KiB is read (bash substring: no SIGPIPE).
_tracking_issue_masked_text() {
  printf '%s\n' "${TASK_DESC:0:65536}" | tr '[:upper:]' '[:lower:]' \
    | sed -E 's/(^|[^a-z0-9_])(pr|pull request|pull)[[:space:]]*#[0-9]+/\1 /g; s/[a-z0-9_.-]+\/[a-z0-9_.-]+#[0-9]+/ /g; s/ab#[0-9]+/ /g; s/#[0-9]+[a-z_][a-z0-9_]*/ /g'
}

# `N kind` lines for references of one kind (`marked` or `bare`), in order of
# appearance.
_tracking_issue_refs() {
  local masked
  masked="$(_tracking_issue_masked_text)"
  if [ "$1" = "marked" ]; then
    printf '%s\n' "$masked" | grep -oE '(^|[^a-z0-9_])(issue|fix(es|ed)?|close[sd]?|resolve[sd]?)[[:space:]]*:?[[:space:]]*#[0-9]+' | sed -E 's/.*#([0-9]+)$/\1 marked/' || true
  else
    printf '%s\n' "$masked" | grep -oE '(^|[^a-z0-9_#])#[0-9]+' | sed -E 's/.*#([0-9]+)$/\1 bare/' || true
  fi
}

# Up to 5 `N kind` candidates, marked references first, each number once (its
# first kind wins). awk reads all of its input (no early exit), so pipefail
# never sees a SIGPIPE.
_tracking_issue_candidates() {
  { _tracking_issue_refs marked; _tracking_issue_refs bare; } \
    | awk '$1 ~ /^[1-9][0-9]{0,8}$/ && !seen[$1]++ && c < 5 { print; c++ }'
  return 0
}

# Return 0 only if $1 is a single-line https:// URL ending in /issues/$2.
_is_github_issue_url() { [[ "$1" =~ ^https://[^[:space:]]+/issues/${2}$ ]]; }

# Print the URL of issue N and return 0 only if it is an open issue, not a
# pull request, in this repository. Read-only; every other outcome returns 1.
_github_open_issue_url() {
  local n row state is_pr url
  n="${1:-}"
  [[ "$n" =~ ^[0-9]+$ ]] || return 1
  row="$(timeout 30 gh api "repos/{owner}/{repo}/issues/$n" --jq '[.state, (.pull_request != null), .html_url] | @tsv' </dev/null 2>/dev/null)" || return 1
  IFS=$'\t' read -r state is_pr url <<< "$row" || return 1
  [ "$state" = "open" ] && [ "$is_pr" = "false" ] && _is_github_issue_url "$url" "$n" || return 1
  printf '%s\n' "$url"
}

# GitHub tracking issue for step-03. Returns 0 with a URL (or, for an explicit
# number gh cannot resolve as an issue, local metadata) on stdout; returns 1
# with empty stdout when nothing is selected. Diagnostics go to stderr only.
select_github_tracking_issue() {
  local explicit n kind url rc=0 checked=""
  explicit="${EXISTING_ISSUE_NUMBER:-}"; explicit="${explicit#\#}"
  if [[ "$explicit" =~ ^[0-9]{1,9}$ ]]; then
    url="$(timeout 60 gh issue view "$explicit" --json url --jq '.url // ""' </dev/null 2>/dev/null)" || rc=$?
    if [ "$rc" -eq 0 ] && _is_github_issue_url "$url" "$explicit"; then
      echo "INFO: tracking issue #${explicit} selected (explicit)" >&2
      printf '%s\n' "$url"; return 0
    fi
    # gh's stderr stays discarded (it can hold tokens); the exit code is enough
    # to tell a timeout (124) or auth/network failure from a bad number.
    if [ "$rc" -ne 0 ]; then
      echo "WARN: could not verify issue #${explicit} (gh exit ${rc}); falling back to local tracking" >&2
    else
      echo "WARN: explicit issue_number #${explicit} could not be resolved as an issue in this repository — using local tracking" >&2
    fi
    emit_local_metadata; return 0
  elif [ -n "$explicit" ]; then
    echo "WARN: issue_number is not a valid issue number — ignoring it for tracking-issue selection" >&2
  fi
  while read -r n kind; do
    checked+=" $n"
    url="$(_github_open_issue_url "$n")" || continue
    echo "INFO: tracking issue #${n} selected (${kind})" >&2
    printf '%s\n' "$url"; return 0
  done < <(_tracking_issue_candidates)
  [ -z "$checked" ] || echo "INFO: no task-text candidate accepted as an open issue (checked:${checked}); falling back to title search" >&2
  return 1
}
