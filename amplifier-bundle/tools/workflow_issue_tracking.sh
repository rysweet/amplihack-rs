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

# issue_search_query TITLE — step-03's tracker lookup query, the same on hosts
# with real /search and on GraphQL-blocked ones: whole words only (at most 100
# characters, cut at a word boundary: a cut word matches nothing), and never
# anything gh-compat's search grammar refuses. Double quotes and parentheses
# become spaces (parse(x) stays "parse x", not "parsex"); a leading '-' is
# stripped; bare OR/NOT/AND are lowercased; ':' outside URLs becomes a space;
# a '#' that does not start a clean #N (#12's, #12/#13, #1.5, #1st) becomes a
# space. A title with no letter or digit at all gives an empty query (step-03
# then searches nothing and creates its issue). A single over-long first
# token is cut to 100 characters, never mid-way through a multi-byte character.
issue_search_query() {
  local t out="" w cand toks=() plain=""
  t="$(printf '%s' "$1" | tr '"()' '   ')"
  # Pass 1: split off the characters that could form syntax.
  read -r -a toks <<<"$t"
  for w in "${toks[@]}"; do
    case "$w" in
      http://*|https://*) ;;
      *) w="${w//:/ }"
         case "$w" in \#*) [[ "$w" =~ ^\#[0-9]+$ ]] || w="${w//#/ }" ;; esac ;;
    esac
    plain="$plain $w"
  done
  # Pass 2: per resulting token, drop negation and operators, then cut.
  read -r -a toks <<<"$plain"
  for w in "${toks[@]}"; do
    while [ "${w#-}" != "$w" ]; do w="${w#-}"; done
    case "$w" in OR|NOT|AND) w="$(printf '%s' "$w" | tr 'A-Z' 'a-z')" ;; esac
    [ -n "$w" ] || continue
    cand="${out:+$out }$w"
    if [ "${#cand}" -gt 100 ]; then
      [ -n "$out" ] || out="${w:0:100}"
      break
    fi
    out="$cand"
  done
  # ${w:0:100} counts bytes outside a UTF-8 locale: drop a cut character.
  if command -v iconv >/dev/null 2>&1; then out="$(printf '%s' "$out" | iconv -c -f UTF-8 -t UTF-8 2>/dev/null)"; fi
  # Words are what gh-compat's search counts: runs of Unicode letters/digits.
  [ "$(issue_title_words "$out")" = "" ] && out=""
  printf '%s\n' "$out"
}

# issue_title_words TEXT — TEXT's words, lowercased, one line: the runs of
# Unicode letters and digits, exactly gh-compat's search `uwords`.
issue_title_words() {
  jq -rn --arg t "$1" '[$t | scan("[\\p{L}\\p{N}]+") | ascii_downcase] | join(" ")' 2>/dev/null
}

# issue_pick_tracker TITLE — read `gh issue list --json number,title,url`
# output on stdin and print the URL of step-03's existing tracker: the first
# issue whose title equals TITLE (the same Unicode words in order, #N and URLs
# included; case, spacing and punctuation aside, as gh-compat's exact tier
# compares), else the first result, else nothing. The search query is cut at
# 100 characters, so search ranking alone cannot tell a long title's own
# tracker from a newer near-duplicate; comparing against the full title can,
# the same way on hosts with real /search and on GraphQL-blocked ones.
issue_pick_tracker() {
  jq -r --arg t "$1" '
    def norm: [scan("[\\p{L}\\p{N}]+") | ascii_downcase];
    ($t | norm) as $want
    | if type != "array" or ($want | length) == 0 then "" else
        ((map(select(((.title // "") | norm) == $want)) + .)[0].url // "")
      end' 2>/dev/null || true
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
