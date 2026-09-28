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
# it lists the open issues (the newest 1000; on GraphQL-blocked hosts gh-compat
# pages them over REST) and prints the URL of the first one whose title is
# TITLE — the same words (runs of Unicode letters and digits) in the same
# order, compared case-insensitively (simple one-to-one Unicode case, É = é;
# multi-character folds such as ß/SS depend on the jq engine and are not
# promised), with spacing, punctuation and control characters aside. TITLE
# reaches jq through the environment (env.ISSUE_TITLE_LOOKUP), so no
# character needs escaping or stripping and the lookup sees exactly what
# `gh issue create --title` was given. The comparison runs in gh's own --jq,
# so no system jq is needed. A title with no word matches nothing.
# When the list holds exactly 1000 issues the scan was cut at the newest
# 1000 (real gh and gh-compat both cap there): a WARNING says so, because a
# tracker older than that would be missed and a duplicate filed.
# A lookup that fails does not read as "no tracker" (that would file a
# duplicate): it is reported on stderr and returns 1, except on a host that
# cannot reach GitHub issues at all (GraphQL blocked with no fallback, gh
# missing), where it says so on stderr and returns 0 so step-03's create path
# takes over and falls back to local tracking. A missing jq is a local defect,
# not such a host: it is reported and stops the step.
issue_find_tracker() {
  local out err rc=0 errf cnt url
  errf="$(mktemp "${TMPDIR:-/tmp}/tracker-lookup.XXXXXX")" || { echo "ERROR: tracking-issue lookup: mktemp failed" >&2; return 1; }
  # Two output lines: how many issues were listed, then the URL (or nothing).
  out="$(ISSUE_TITLE_LOOKUP="$1" timeout 60 gh issue list --state open --limit 1000 --json number,title,url --jq '
    def norm: [scan("[\\p{L}\\p{N}]+")] | join(" ");
    (env.ISSUE_TITLE_LOOKUP | norm) as $want
    | (length | tostring),
      (if $want == "" then "" else
         ([.[] | select((.title // "") | norm | test("^" + $want + "$"; "i"))][0].url // "") end)' 2>"$errf")" || rc=$?
  err="$(cat "$errf")"; rm -f "$errf"
  if [ "$rc" -ne 0 ]; then
    if issue_create_host_unsupported "$rc" "$err"; then
      echo "WARNING: the tracking-issue lookup cannot reach GitHub issues on this host; the create step decides what follows. gh reported:" >&2
      sanitize_cli_output "$err" >&2
      return 0
    fi
    echo "ERROR: looking up an existing tracking issue failed (rc $rc); not creating one, which could duplicate it. gh reported:" >&2
    sanitize_cli_output "$err" >&2
    return 1
  fi
  case "$out" in *$'\n'*) cnt="${out%%$'\n'*}"; url="${out#*$'\n'}" ;; *) cnt="$out"; url="" ;; esac
  case "$cnt" in ''|*[!0-9]*) echo "ERROR: tracking-issue lookup returned unexpected output:" >&2; sanitize_cli_output "$out" >&2; return 1 ;; esac
  [ "$cnt" -ge 1000 ] && echo "WARNING: the tracker scan read only the newest 1000 open issues; an older tracker with this title would be missed and a duplicate filed." >&2
  case "$url" in
    https://*|http://*|'') printf '%s\n' "$url" ;;
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
  # A missing jq is a fixable local defect (gh-compat says "needs jq"), not a
  # host that cannot reach GitHub: it must stay loud, never degrade quietly.
  printf '%s' "${2:-}" | grep -Eiq 'needs jq' && return 1
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
