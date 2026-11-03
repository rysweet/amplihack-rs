# shellcheck shell=bash
# workflow_issue_create.sh — opt-in, bounded GitHub tracking-issue creation for
# `step-03-create-issue` in amplifier-bundle/recipes/workflow-prep.yaml (#1881).
#
# Step 03 sources this file after workflow_issue_tracking.sh and calls, in
# order: issue_creation_opted_in, load_issue_identity,
# find_open_issue_by_exact_title, then create_github_issue_bounded. The functions use emit_local_metadata and
# sanitize_cli_output from workflow_issue_tracking.sh.
#
# The file sets no shell options and installs no trap; it works under the
# step's `set -euo pipefail`. Diagnostics go to stderr; stdout carries only an
# issue URL or the local metadata block. Every gh call runs as
# `GH_PROMPT_DISABLED=1 timeout N gh … </dev/null`, and every user value is
# one argument in --flag=value form. Contract:
# docs/reference/workflow-tracking-issue-selection.md.
#
#   issue_creation_opted_in VALUE   — 0 when VALUE, trimmed and lower-cased, is
#                                     true, 1 or yes; 1 otherwise (fails closed)
#   load_issue_identity             — sets ISSUE_REPO_NWO (owner/repo) and
#                                     ISSUE_LOGIN (gh account) once per shell;
#                                     1 with a WARNING when either fetch fails
#   find_open_issue_by_exact_title TITLE [recent]
#                                   — 0 + URL: open issue in this repository,
#                                     opened by ISSUE_LOGIN, with exactly TITLE;
#                                     1: no match; 2: lookup failed
#                                     (WARNING on stderr). With `recent`, lists the
#                                     newest open issues without --search, so an
#                                     issue created seconds ago is found
#   classify_issue_create_error OUTPUT
#                                   — prints auth|secondary|primary|not_found|
#                                     label|other (first 64 KiB, fixed patterns)
#   create_github_issue_bounded TITLE BODY_FILE LABEL
#                                   — label create, then creates with a bounded,
#                                     jittered backoff; 0 + URL, 0 + local block,
#                                     or 1 with ERROR
#   _issue_match REGEX TEXT         — case-insensitive ERE test of TEXT (no pipe)
#   _issue_backoff_int NAME DEFAULT — AMPLIHACK_ISSUE_BACKOFF_* value or DEFAULT
#                                     (WARNING on stderr when set but invalid)
#   _issue_retry_after OUTPUT       — N from `retry-after: N` (1-6 digits)
#   _issue_jitter_delay ROUND BASE MAX_DELAY
#                                   — equal-jitter delay for a round, at least 1
#   _issue_primary_wait             — seconds until the GraphQL reset + 3, >= 5;
#                                     5 with a WARNING when the reset is unreadable
#   _issue_wait_for_class CLASS OUTPUT ROUND BASE MAX_DELAY
#                                   — the planned wait for one backoff round
#   _issue_last_url OUTPUT          — last line of OUTPUT that is an issue URL
#   _issue_gh_stderr FILE RC        — prints FILE (a gh call's stderr) through
#                                     sanitize_cli_output when RC is non-zero,
#                                     then deletes FILE

# Cleared on source so a value from the environment is never trusted.
ISSUE_REPO_NWO=""
ISSUE_LOGIN=""

issue_creation_opted_in() {
  local v="${1:-}"
  v="${v#"${v%%[![:space:]]*}"}"
  v="${v%"${v##*[![:space:]]}"}"
  v="$(printf '%s' "$v" | tr '[:upper:]' '[:lower:]')"
  case "$v" in
    true|1|yes) return 0 ;;
  esac
  return 1
}

_issue_gh_stderr() {
  local f="$1" rc="$2" text=""
  [ -n "$f" ] || return 0
  if [ "$rc" -ne 0 ] && [ -s "$f" ]; then
    text=$(head -c 65536 -- "$f")
    sanitize_cli_output "$text" >&2
  fi
  rm -f -- "$f"
  return 0
}

load_issue_identity() {
  local errf out rc=0
  if [ -z "${ISSUE_REPO_NWO:-}" ]; then
    errf=$(mktemp) || errf=""
    out=$(GH_PROMPT_DISABLED=1 timeout 30 gh repo view --json nameWithOwner --jq .nameWithOwner </dev/null 2>"${errf:-/dev/null}") || rc=$?
    if [ "$rc" -ne 0 ] || ! [[ "$out" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]]; then
      echo "WARNING: could not determine this repository (repo view exited ${rc})" >&2
      _issue_gh_stderr "$errf" "$rc"
      return 1
    fi
    _issue_gh_stderr "$errf" 0
    ISSUE_REPO_NWO="$out"
  fi
  if [ -z "${ISSUE_LOGIN:-}" ]; then
    rc=0
    errf=$(mktemp) || errf=""
    out=$(GH_PROMPT_DISABLED=1 timeout 30 gh api user --jq .login </dev/null 2>"${errf:-/dev/null}") || rc=$?
    if [ "$rc" -ne 0 ] || ! [[ "$out" =~ ^[A-Za-z0-9][A-Za-z0-9-]{0,38}$ ]]; then
      echo "WARNING: could not determine the gh login (api user exited ${rc})" >&2
      _issue_gh_stderr "$errf" "$rc"
      return 1
    fi
    _issue_gh_stderr "$errf" 0
    ISSUE_LOGIN="$out"
  fi
  return 0
}

find_open_issue_by_exact_title() {
  local t="${1:-}" mode="${2:-}" phrase out url errf rc=0
  if ! load_issue_identity; then
    echo "WARNING: exact-title lookup failed; could not determine this repository or the gh login" >&2
    return 2
  fi
  errf=$(mktemp) || errf=""
  if [ "$mode" = "recent" ]; then
    # Search indexes new issues after a delay; a plain listing does not.
    out=$(GH_PROMPT_DISABLED=1 timeout 30 gh issue list --state open --limit 100 --json number,title,url,author </dev/null 2>"${errf:-/dev/null}") || rc=$?
  else
    phrase="${t//\"/}"
    phrase="${phrase//\\/ }"
    out=$(GH_PROMPT_DISABLED=1 timeout 30 gh issue list --state open --limit 100 --search="\"${phrase}\" in:title" --json number,title,url,author </dev/null 2>"${errf:-/dev/null}") || rc=$?
  fi
  if [ "$rc" -ne 0 ]; then
    echo "WARNING: exact-title lookup failed; issue list exited ${rc}" >&2
    _issue_gh_stderr "$errf" "$rc"
    return 2
  fi
  _issue_gh_stderr "$errf" 0
  rc=0
  url=$(printf '%s' "$out" | jq -r --arg t "$t" --arg r "$ISSUE_REPO_NWO" --arg u "$ISSUE_LOGIN" '
    if type != "array" then error("issue list output is not an array") else
    [ .[]
      | select(type == "object" and .title == $t and (.url | type) == "string")
      | select((.author | type) == "object" and (.author.login | type) == "string"
          and (.author.login | ascii_downcase) == ($u | ascii_downcase))
      | select((.url | split("/")) as $p
          | ($p | length) == 7 and $p[0] == "https:" and $p[1] == ""
            and ($p[2] | test("\\A[A-Za-z0-9.-]+\\z")) and $p[5] == "issues"
            and ($p[6] | test("\\A[0-9]+\\z"))
            and (($p[3] + "/" + $p[4]) | ascii_downcase) == ($r | ascii_downcase))
    ][0].url // empty end' 2>/dev/null) || rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "WARNING: exact-title lookup failed; issue list did not return a JSON array" >&2
    return 2
  fi
  [ -n "$url" ] || return 1
  printf '%s\n' "$url"
  return 0
}

_issue_match() {
  grep -Eiq -- "$1" <<<"$2"
}

classify_issue_create_error() {
  local LC_ALL=C s
  s="${1:0:65536}"
  if _issue_match 'HTTP 401|(^|[^0-9])401([^0-9]|$)|bad credentials|requires authentication|not logged in' "$s"; then
    echo auth
  elif _issue_match 'secondary rate limit|abuse detection|submitted too quickly' "$s"; then
    echo secondary
  elif _issue_match 'api rate limit|rate limit already exceeded|rate limit exceeded|(^|[^0-9])429([^0-9]|$)' "$s"; then
    echo primary
  elif _issue_match 'could not resolve to a repository|repository not found|repository.*inaccessible|HTTP 404' "$s"; then
    echo not_found
  elif _issue_match 'could not add label|label.*(not found|does not exist)' "$s"; then
    echo label
  else
    echo other
  fi
}

_issue_backoff_int() {
  local v="${!1:-}"
  if [[ "$v" =~ ^[1-9][0-9]{0,5}$ ]]; then printf '%s\n' "$v"; return 0; fi
  if [ -n "$v" ]; then
    # Only the length is printed, in case a secret was put here by mistake.
    printf 'WARNING: ignoring invalid %s (%s characters); using default %s\n' "$1" "${#v}" "$2" >&2
  fi
  printf '%s\n' "$2"
}

_issue_retry_after() {
  local LC_ALL=C s re='retry-after:[[:space:]]*([0-9]{1,6})([^0-9]|$)'
  s="$(printf '%s' "${1:0:65536}" | tr '[:upper:]' '[:lower:]')"
  [[ "$s" =~ $re ]] || return 1
  printf '%s\n' "$((10#${BASH_REMATCH[1]}))"
}

_issue_jitter_delay() {
  local round="$1" base="$2" cap="$3" exp d half w
  exp="$round"; [ "$exp" -gt 30 ] && exp=30
  d=$((base * (1 << exp)))
  [ "$d" -gt "$cap" ] && d="$cap"
  half=$((d / 2))
  w=$((half + RANDOM % (half + 1)))
  [ "$w" -lt 1 ] && w=1
  printf '%s\n' "$w"
}

_issue_primary_wait() {
  local epoch now w errf rc=0
  errf=$(mktemp) || errf=""
  epoch=$(GH_PROMPT_DISABLED=1 timeout 30 gh api rate_limit --jq .resources.graphql.reset </dev/null 2>"${errf:-/dev/null}") || rc=$?
  if [ "$rc" -ne 0 ] || ! [[ "$epoch" =~ ^[0-9]{1,12}$ ]]; then
    echo "WARNING: could not read GitHub rate-limit reset; waiting 5s" >&2
    _issue_gh_stderr "$errf" "$rc"
    printf '5\n'
    return 0
  fi
  _issue_gh_stderr "$errf" 0
  now=$(date +%s)
  w=$((10#$epoch - now + 3))
  [ "$w" -lt 5 ] && w=5
  printf '%s\n' "$w"
}

_issue_wait_for_class() {
  local class="$1" out="$2" round="$3" base="$4" cap="$5" ra=""
  ra=$(_issue_retry_after "$out") || ra=""
  if [ -n "$ra" ]; then
    printf '%s\n' "$ra"
  elif [ "$class" = "secondary" ]; then
    _issue_jitter_delay "$round" "$base" "$cap"
  else
    _issue_primary_wait
  fi
}

_issue_last_url() {
  local line last=""
  while IFS= read -r line; do
    [[ "$line" =~ ^https://[A-Za-z0-9.-]+/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+/issues/[0-9]+$ ]] && last="$line"
  done <<<"$1"
  [ -n "$last" ] || return 1
  printf '%s\n' "$last"
}

create_github_issue_bounded() {
  local title="$1" body_file="$2" label="$3"
  local base cap max used=0 round=0 w rc out class url with_label=1 lrc found pass attempt errf
  base=$(_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_BASE_SECS 5)
  cap=$(_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_MAX_DELAY_SECS 60)
  max=$(_issue_backoff_int AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS 300)
  # Fetched once here so every lookup during backoff reuses them; a failure
  # is retried by the lookup itself.
  load_issue_identity || true
  rc=0
  errf=$(mktemp) || errf=""
  GH_PROMPT_DISABLED=1 timeout 30 gh label create "$label" --color=0366d6 --description="Created by default-workflow recipe" </dev/null >/dev/null 2>"${errf:-/dev/null}" || rc=$?
  [ "$rc" -eq 0 ] || echo "INFO: gh label create ${label} exited ${rc} (the label may already exist); continuing" >&2
  _issue_gh_stderr "$errf" "$rc"
  # Each pass of this bounded loop is one create; it ends by return on every
  # class, and a rate-limited pass only continues while used + wait <= max,
  # with every wait >= 1 — so at most max + 2 passes run.
  for ((pass = 0; pass <= max + 2; pass++)); do
    rc=0
    if [ "$with_label" -eq 1 ]; then
      out=$(GH_PROMPT_DISABLED=1 timeout 60 gh issue create --title="$title" --body-file="$body_file" --label="$label" </dev/null 2>&1) || rc=$?
    else
      out=$(GH_PROMPT_DISABLED=1 timeout 60 gh issue create --title="$title" --body-file="$body_file" </dev/null 2>&1) || rc=$?
    fi
    if [ "$rc" -eq 0 ]; then
      url=$(_issue_last_url "$out") && { printf '%s\n' "$url"; return 0; }
      echo "ERROR: GitHub issue creation failed. gh issue create succeeded but printed no issue URL:" >&2
      sanitize_cli_output "$out" >&2
      return 1
    fi
    if [ "$rc" -eq 124 ]; then
      # timeout killed the create; GitHub may still have made the issue.
      lrc=0
      found=$(find_open_issue_by_exact_title "$title" recent) || lrc=$?
      if [ "$lrc" -eq 0 ]; then
        echo "INFO: gh issue create timed out but the issue exists; reusing ${found}" >&2
        printf '%s\n' "$found"
        return 0
      fi
      echo "ERROR: GitHub issue creation failed. gh issue create timed out after 60s and no open issue with this title was found." >&2
      sanitize_cli_output "$out" >&2
      return 1
    fi
    class=$(classify_issue_create_error "$out")
    case "$class" in
      label)
        if [ "$with_label" -eq 1 ]; then
          with_label=0
          echo "WARNING: gh issue create failed on label ${label}; creating it again without --label (one fallback per run)" >&2
          continue
        fi
        echo "ERROR: GitHub issue creation failed." >&2
        sanitize_cli_output "$out" >&2
        return 1 ;;
      not_found)
        echo "WARNING: GitHub issue creation failed because the repository could not be resolved or accessed; using local tracking metadata instead." >&2
        emit_local_metadata
        return 0 ;;
      secondary|primary) ;;
      *)
        echo "ERROR: GitHub issue creation failed." >&2
        sanitize_cli_output "$out" >&2
        return 1 ;;
    esac
    # Rate limited: wait, look up, and only create again after a lookup that
    # succeeded and found nothing. A failed lookup costs another wait. The
    # lookup lists recent issues without search, because the issue this
    # create may have made is not yet in the search index.
    for ((attempt = 0; attempt <= max; attempt++)); do
      w=$(_issue_wait_for_class "$class" "$out" "$round" "$base" "$cap")
      [ "$w" -lt 1 ] && w=1
      if [ $((used + w)) -gt "$max" ]; then
        echo "WARNING: GitHub issue creation gave up after ${used}s (AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=${max}); falling back to local tracking." >&2
        emit_local_metadata
        return 0
      fi
      if [ "$class" = "secondary" ]; then
        echo "WARNING: gh issue create hit a GitHub secondary rate limit; backing off ${w}s (round ${round}, ${used}/${max}s used)" >&2
        round=$((round + 1))
      else
        echo "WARNING: gh issue create hit the GitHub API rate limit; waiting ${w}s for reset (${used}/${max}s used)" >&2
      fi
      sleep "$w"
      used=$((used + w))
      lrc=0
      found=$(find_open_issue_by_exact_title "$title" recent) || lrc=$?
      if [ "$lrc" -eq 0 ]; then
        echo "INFO: reusing open issue with identical title: ${found}" >&2
        printf '%s\n' "$found"
        return 0
      fi
      [ "$lrc" -eq 1 ] && break
    done
  done
  echo "WARNING: GitHub issue creation gave up after ${used}s (AMPLIHACK_ISSUE_BACKOFF_MAX_TOTAL_SECS=${max}); falling back to local tracking." >&2
  emit_local_metadata
  return 0
}
