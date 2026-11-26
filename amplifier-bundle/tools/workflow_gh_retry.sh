#!/usr/bin/env bash
# workflow_gh_retry.sh — shared GitHub/az rate-limit-aware retry helpers.
#
# Sourced by the default-workflow helper scripts (workflow_publish_pr.sh,
# workflow_pr_scope.sh, workflow_final_status.sh, workflow_pr_ready.sh) so a
# temporarily-exhausted GitHub API quota does not fail the workflow closed.
#
# Motivation (observed in production): when the GraphQL quota is exhausted
# (0/5000, reset up to ~1h later) the previous helpers classified "rate limit"
# as a generic transient error and retried ~3x with an immediate/short backoff.
# All retries hit the still-exhausted quota and the step failed closed. This
# library distinguishes three error classes and handles them differently:
#
#   * auth        -> permanent (401 / Bad credentials): NEVER retried.
#   * rate_limit  -> wait for the AUTHORITATIVE reset (adaptive, bounded by the
#                    observed reset window, not an arbitrary fixed cap) and/or
#                    fall back to a REST read on the core budget.
#   * transient   -> generic 5xx/network: existing short-backoff, 3 attempts.
#
# Error-handling convention (see amplifier-bundle/context/PHILOSOPHY.md):
# no silent fallbacks. Every fallback is logged (WARNING) and the caller still
# fails closed when both GraphQL and REST are exhausted.
#
# This file only DEFINES functions; sourcing it has no side effects.

# GH_RETRY_LAST_CLASS is a deliberate cross-script global read by sourcing
# helpers (e.g. to branch on rate_limit_rest); shellcheck cannot see that use.
# shellcheck disable=SC2034

# --- stderr sanitiser (redacts embedded credentials) -----------------------
gh_sanitize_stderr() (
  # Explicit pipefail even when the caller uses a conditional function call.
  set -o pipefail
  [ -n "${1:-}" ] && [ -s "$1" ] || return 0
  # Re-scan AWS matches: their trailing boundary can also begin the next key.
  LC_ALL=C tr '\r\n\t' '   ' <"$1" | LC_ALL=C tr -d '\000-\037\177' |
    LC_ALL=C sed -E \
      -e $'s/\302[\200-\237]//g' \
      -e $'s/\330\234//g' \
      -e $'s/\342\200[\213-\217\252-\256]//g' \
      -e $'s/\342\201[\240-\257]//g' \
      -e $'s/\357\273\277//g' \
      -e 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' \
      -e 's#gh[pousr]_[A-Za-z0-9]{16,}#REDACTED_TOKEN#g' \
      -e 's#github_pat_[A-Za-z0-9_]{16,}#REDACTED_TOKEN#g' \
      -e 's#(^|[^A-Za-z0-9_-])sk-[A-Za-z0-9_-]{20,}#\1REDACTED_TOKEN#g' \
      -e ':aws' \
      -e 's#(^|[^A-Za-z0-9])(AKIA|ASIA)[0-9A-Z]{16}([^0-9A-Za-z]|$)#\1REDACTED_TOKEN\3#g' \
      -e 't aws' |
    # Consume all input: truncating with head can SIGPIPE the redactor.
    LC_ALL=C awk 'length(s) < 500 {s = s substr($0, 1, 500 - length(s))} END {printf "%s", s}'
)

# Never release a formatter's partial output (it may contain credentials).
gh_safe_stderr() {
  local diagnostic
  if diagnostic="$(gh_sanitize_stderr "$1" 2>/dev/null)"; then
    printf '%s' "${diagnostic:-empty stderr (no error message)}"
  else
    printf '%s' 'stderr sanitization unavailable'
  fi
}

gh_remove_capture() {
  # The warning is a courtesy: with stderr unwritable it must not end a set -e
  # caller before the remaining captures are removed or its traps have run.
  rm -f -- "$1" 2>/dev/null || printf '%s\n' 'WARNING: gh capture cleanup failed' >&2 || :
  return 0
}

# Run owned work in a separate process group. Waiting with Bash's wait builtin
# lets catchable signals run immediately, even when a child is blocked. Restore
# the caller's monitor option before returning; no trap is installed by sourcing.
gh_start_owned() {
  local monitor=false
  case $- in *m*) monitor=true ;; esac
  set -m
  if declare -F "$1" >/dev/null; then
    # A function declared with () has its own subshell. Keep its group leader
    # alive until that nested owner has finished its signal cleanup.
    (trap 'wait; exit 143' TERM; trap 'wait; exit 130' INT; trap 'wait; exit 129' HUP
      "$@"
    ) <&0 &
  else
    "$@" <&0 &
  fi
  GH_OWNED_PID=$!
  $monitor || set +m
}

gh_cancel_owned() {
  [ -n "${1:-}" ] || return 0
  # timeout may lead its own group; all other children inherit the owned group.
  kill -TERM -- "-$1" 2>/dev/null || kill -TERM "$1" 2>/dev/null || :
  wait "$1" 2>/dev/null || :
}

# --- classifier ------------------------------------------------------------
# classify_gh_error <stderr_file> -> echoes: auth | rate_limit | transient | other
#
# Order matters: permanent auth errors are checked first (never retried), then
# rate-limit signals (wait-for-reset), then generic transient signals.
classify_gh_error() {
  local f="${1:-}"
  [ -n "$f" ] && [ -s "$f" ] || { printf 'other\n'; return 0; }

  # Permanent authentication/authorisation failures — must NOT be retried.
  if grep -Eiq 'bad credentials|HTTP 401|(^|[^0-9])401([^0-9]|$)|unauthorized|authentication failed|requires authentication|gh auth login|not logged in|token .*(expired|invalid)|invalid .*token' "$f"; then
    printf 'auth\n'; return 0
  fi

  # Explicit rate-limit signals (primary + secondary + abuse detection).
  if grep -Eiq 'api rate limit exceeded|rate limit already exceeded|secondary rate limit|abuse detection|retry-after|x-ratelimit-reset|rate limit|HTTP 429|(^|[^0-9])429([^0-9]|$)' "$f"; then
    printf 'rate_limit\n'; return 0
  fi

  # A bare 403 is treated as a (secondary) rate-limit signal UNLESS it clearly
  # reads as a permission error, in which case it is not retryable.
  if grep -Eiq 'HTTP 403|(^|[^0-9])403([^0-9]|$)' "$f"; then
    if grep -Eiq 'resource not accessible|must have admin|not authorized|insufficient|permission' "$f"; then
      printf 'other\n'; return 0
    fi
    printf 'rate_limit\n'; return 0
  fi

  # Generic transient server/network errors — short-backoff retry.
  if grep -Eiq 'HTTP 5[0-9][0-9]|(^|[^0-9])(500|502|503|504)([^0-9]|$)|timed out|timeout|temporar|connection reset|connection refused|TLS handshake|network|server error|i/o timeout|no such host|EOF' "$f"; then
    printf 'transient\n'; return 0
  fi

  printf 'other\n'
}

# --- authoritative reset lookup -------------------------------------------
# gh_rate_limit_reset_epoch <resource> -> echoes epoch seconds, or returns 1.
#
# Uses the `gh api rate_limit` endpoint, which does NOT itself consume the
# core/graphql budget.
gh_rate_limit_reset_epoch() {
  local resource="${1:-graphql}" json reset
  command -v gh >/dev/null 2>&1 || return 1
  command -v jq >/dev/null 2>&1 || return 1
  json="$(gh api rate_limit 2>/dev/null)" || return 1
  [ -n "$json" ] || return 1
  reset="$(printf '%s' "$json" | jq -r --arg r "$resource" '.resources[$r].reset // empty' 2>/dev/null)" || return 1
  case "$reset" in ''|*[!0-9]*) return 1 ;; esac
  printf '%s\n' "$reset"
}

# gh_rate_limit_remaining <resource> -> echoes remaining count, or returns 1.
gh_rate_limit_remaining() {
  local resource="${1:-core}" json remaining
  command -v gh >/dev/null 2>&1 || return 1
  command -v jq >/dev/null 2>&1 || return 1
  json="$(gh api rate_limit 2>/dev/null)" || return 1
  [ -n "$json" ] || return 1
  remaining="$(printf '%s' "$json" | jq -r --arg r "$resource" '.resources[$r].remaining // empty' 2>/dev/null)" || return 1
  case "$remaining" in ''|*[!0-9]*) return 1 ;; esac
  printf '%s\n' "$remaining"
}

# _gh_core_has_budget -> returns 0 when the REST/core budget still has requests.
_gh_core_has_budget() {
  local remaining
  remaining="$(gh_rate_limit_remaining core 2>/dev/null)" || return 1
  [ "$remaining" -gt 0 ]
}

# --- reset epoch from response headers (fallback) --------------------------
# gh_reset_epoch_from_stderr <stderr_file> -> echoes epoch seconds, or returns 1.
# Honors a `Retry-After: <seconds>` or `X-RateLimit-Reset: <epoch>` header when
# `gh api rate_limit` is unavailable.
gh_reset_epoch_from_stderr() {
  local f="${1:-}" now retry reset
  [ -n "$f" ] && [ -s "$f" ] || return 1
  now="$(date +%s)"
  retry="$(grep -Eio 'retry-after:[[:space:]]*[0-9]+' "$f" | grep -Eo '[0-9]+' | head -n1 || true)"
  if [ -n "$retry" ]; then
    printf '%s\n' "$((now + retry))"; return 0
  fi
  reset="$(grep -Eio 'x-ratelimit-reset:[[:space:]]*[0-9]+' "$f" | grep -Eo '[0-9]+' | head -n1 || true)"
  if [ -n "$reset" ]; then
    printf '%s\n' "$reset"; return 0
  fi
  return 1
}

# --- adaptive wait ---------------------------------------------------------
# gh_wait_for_rate_limit <resource> <stderr_file>
#
# Sleeps until just after the authoritative reset for <resource>. There is NO
# arbitrary fixed cap: the wait is bounded by the observed reset window. A small
# floor (5s) and buffer (3s) guard against negative/absent values and clock
# skew. NO-TIMEOUT policy: this is a liveness-bounded sleep, not a wall clock.
gh_wait_for_rate_limit() {
  local resource="${1:-graphql}" stderr_file="${2:-}"
  local now epoch wait floor=5 buffer=3
  now="$(date +%s)"
  epoch="$(gh_rate_limit_reset_epoch "$resource" 2>/dev/null || true)"
  if [ -z "$epoch" ] && [ -n "$stderr_file" ]; then
    epoch="$(gh_reset_epoch_from_stderr "$stderr_file" 2>/dev/null || true)"
  fi
  if [ -z "$epoch" ]; then
    wait="$floor"
    echo "WARNING: GitHub ${resource} rate limit hit; reset time unknown, waiting ${wait}s (floor) before re-probing." >&2
  else
    wait=$(( epoch - now + buffer ))
    [ "$wait" -lt "$floor" ] && wait="$floor"
    echo "WARNING: GitHub ${resource} rate limit hit; waiting ${wait}s for authoritative reset (adaptive, bounded by reset window)." >&2
  fi
  sleep "$wait"
}

# --- REST existence fallback ----------------------------------------------
# gh_pr_exists_rest <OWNER/REPO> <branch> -> echoes a JSON array shaped like
# `gh pr list --json ...` output so callers can consume it identically.
#
# Uses the REST /pulls endpoint (core budget) so a GraphQL-only outage does not
# block PR-existence checks. Returns 1 when the REST call itself fails.
gh_pr_exists_rest() {
  local repo="${1:-}" branch="${2:-}" owner rest
  [ -n "$repo" ] && [ -n "$branch" ] || return 1
  command -v gh >/dev/null 2>&1 || return 1
  command -v jq >/dev/null 2>&1 || return 1
  owner="${repo%%/*}"
  rest="$(gh api "repos/${repo}/pulls?head=${owner}:${branch}&state=all&per_page=100" 2>/dev/null)" || return 1
  [ -n "$rest" ] || return 1
  printf '%s' "$rest" | jq -c '
    (if type == "array" then . else [] end)
    | map({
        number: .number,
        title: (.title // ""),
        body: (.body // ""),
        state: (if (.merged_at // null) != null then "MERGED"
                elif ((.state // "") | ascii_downcase) == "open" then "OPEN"
                else "CLOSED" end),
        createdAt: (.created_at // ""),
        mergedAt: (.merged_at // ""),
        url: (.html_url // ""),
        headRefName: (.head.ref // ""),
        baseRefName: (.base.ref // ""),
        headRefOid: (.head.sha // ""),
        headRepositoryOwner: { login: (.head.repo.owner.login // "") },
        headRepository: { name: (.head.repo.name // "") },
        isCrossRepository: (((.head.repo.full_name // "") != (.base.repo.full_name // "")))
      })' 2>/dev/null || return 1
}

# gh_pr_view_rest <OWNER/REPO> <pr_number> -> echoes a single JSON object shaped
# like `gh pr view --json ...` output (core budget). Returns 1 on failure.
gh_pr_view_rest() {
  local repo="${1:-}" number="${2:-}" rest
  [ -n "$repo" ] || return 1
  case "$number" in ''|*[!0-9]*) return 1 ;; esac
  command -v gh >/dev/null 2>&1 || return 1
  command -v jq >/dev/null 2>&1 || return 1
  rest="$(gh api "repos/${repo}/pulls/${number}" 2>/dev/null)" || return 1
  [ -n "$rest" ] || return 1
  printf '%s' "$rest" | jq -c '{
      number: .number,
      title: (.title // ""),
      body: (.body // ""),
      state: (if (.merged_at // null) != null then "MERGED"
              elif ((.state // "") | ascii_downcase) == "open" then "OPEN"
              else "CLOSED" end),
      createdAt: (.created_at // ""),
      mergedAt: (.merged_at // ""),
      url: (.html_url // ""),
      headRefName: (.head.ref // ""),
      baseRefName: (.base.ref // ""),
      headRefOid: (.head.sha // ""),
      headRepositoryOwner: { login: (.head.repo.owner.login // "") },
      headRepository: { name: (.head.repo.name // "") },
      isCrossRepository: (((.head.repo.full_name // "") != (.base.repo.full_name // "")))
    }' 2>/dev/null || return 1
}

# gh_pr_number_from_url <url> -> echoes the trailing pull-request number, or 1.
gh_pr_number_from_url() {
  local url="${1:-}" num
  num="${url##*/pull/}"
  num="${num%%[!0-9]*}"
  case "$num" in ''|*[!0-9]*) return 1 ;; esac
  printf '%s\n' "$num"
}

# --- retry driver ----------------------------------------------------------
# _gh_retry_core <label> <gh-arg>...
#
#   stdout : command output on success (or REST fallback output).
#   stderr : retry WARNINGs first, then the terminal diagnostic as the LAST
#            line. Callers that report a single line (workflow_publish_pr.sh)
#            read that final line, so every failing exit path, cancellation
#            included, ends with its own terminal line. The one exception is
#            a capture that cannot be removed: gh_remove_capture then prints a
#            WARNING after the terminal line. The return status is unaffected.
#   return : 0 on success; otherwise the last gh exit status.
#
# Behaviour is tuned via environment variables (all optional):
#   GH_RETRY_RESOURCE       graphql|core       (default: graphql) — resource to
#                           consult for reset epoch on a rate-limit.
#   GH_RETRY_MAX_TRANSIENT  integer            (default: 3) — attempts for the
#                           generic transient path (preserves legacy behaviour).
#   GH_RETRY_MAX_RL_WINDOWS integer            (default: unset) — optional
#                           safety valve for callers that explicitly want to
#                           cap reset windows. Unset means no arbitrary cap:
#                           keep honoring the authoritative reset window.
#   GH_RETRY_REST_FALLBACK  function name      (default: unset) — when set, a
#                           READ path: on rate-limit the driver serves the
#                           request via this REST fallback (core budget) instead
#                           of blocking, logging a WARNING. Leave UNSET for write
#                           paths so they wait-for-reset with no REST fallback.
#
# Sets GH_RETRY_LAST_CLASS to the class of the final failure (or
# `success`/`rate_limit_rest`) for callers that need to branch on it.
_gh_retry_core() {
  local stderr_file="" stdout_file="" GH_OWNED_PID="" retry_status=0 retry_interrupted_status=0
  local retry_int retry_term retry_hup retry_shell_pid
  retry_int=$(trap -p INT); retry_term=$(trap -p TERM); retry_hup=$(trap -p HUP)
  # $$ names the original shell even inside command substitution on Bash 3.2.
  read -r retry_shell_pid < <(exec sh -c 'echo "$PPID"')
  trap '_gh_retry_interrupt INT 130; return 130' INT
  trap '_gh_retry_interrupt TERM 143; return 143' TERM
  trap '_gh_retry_interrupt HUP 129; return 129' HUP
  if stdout_file=$(umask 077; mktemp "${GH_RETRY_TMPDIR:-${TMPDIR:-/tmp}}/.workflow-gh-output.XXXXXX"); then
    if _gh_retry_attempts "$@"; then retry_status=0; else retry_status=$?; fi
  else
    retry_status=$?
    printf 'ERROR: gh %s: private stdout temp creation failed (exit %s)\n' "${1:-gh}" "$retry_status" >&2
  fi
  _gh_retry_cleanup
  _gh_retry_restore_traps
  [ "$retry_interrupted_status" -eq 0 ] || retry_status=$retry_interrupted_status
  return "$retry_status"
}

_gh_retry_cleanup() {
  gh_cancel_owned "$GH_OWNED_PID"
  GH_OWNED_PID=""
  [ -z "$stderr_file" ] || gh_remove_capture "$stderr_file"
  stderr_file=""
  [ -z "$stdout_file" ] || gh_remove_capture "$stdout_file"
  stdout_file=""
}

_gh_retry_restore_traps() {
  trap - INT TERM HUP
  # These strings come only from Bash's trap -p, never provider data.
  eval "$retry_int"
  eval "$retry_term"
  eval "$retry_hup"
  return "${1:-0}"
}

_gh_retry_interrupt() {
  retry_interrupted_status=$2
  _gh_retry_cleanup
  # Terminal line for the caller: it must be the last stderr line, as with a
  # rejection, so a retry warning before the signal is not reported instead.
  # A failed write (closed or full stderr) must not replace the cancellation
  # status or skip the caller's trap, which `set -e` would otherwise do.
  printf 'ERROR: gh interrupted by %s (exit %s)\n' "$1" "$2" >&2 || :
  local previous_trap
  case "$1" in INT) previous_trap=$retry_int ;; TERM) previous_trap=$retry_term ;; HUP) previous_trap=$retry_hup ;; esac
  _gh_retry_restore_traps
  # An untrapped cancellation exits with the conventional signal status so the
  # caller's EXIT trap observes it, rather than kill's successful status (0).
  [ -n "$previous_trap" ] || exit "$2"
  # Re-deliver with the caller's original disposition (including ignored or
  # returning traps). EXIT and its original cancellation status remain intact.
  if _gh_retry_restore_traps "$2"; then :; else
    kill -s "$1" "$retry_shell_pid"
  fi
}

_gh_retry_attempts() {
  local label="${1:-gh}"; shift
  local resource="${GH_RETRY_RESOURCE:-graphql}"
  local max_transient="${GH_RETRY_MAX_TRANSIENT:-3}"
  local max_windows="${GH_RETRY_MAX_RL_WINDOWS:-}"
  local rest_fallback="${GH_RETRY_REST_FALLBACK:-}"
  local attempt=0 rl_windows=0 delay=1
  local output status class fb_out diagnostic
  GH_RETRY_LAST_CLASS=""

  while :; do
    attempt=$((attempt + 1))
    if stderr_file="$(umask 077; mktemp "${GH_RETRY_TMPDIR:-${TMPDIR:-/tmp}}/.workflow-gh-retry.XXXXXX" 2>/dev/null)"; then
      :
    else
      status=$?
      printf 'ERROR: gh %s: private stderr temp creation failed (exit %s)\n' "$label" "$status" >&2
      return "$status"
    fi
    gh_start_owned timeout 60 gh "$@" >"$stdout_file" 2>"$stderr_file"
    if wait "$GH_OWNED_PID"; then
      GH_OWNED_PID=""
      output=$(<"$stdout_file")
      gh_remove_capture "$stderr_file"
      stderr_file=""
      printf '%s\n' "$output"
      GH_RETRY_LAST_CLASS="success"
      return 0
    else
      status=$?
    fi
    GH_OWNED_PID=""
    diagnostic="$(gh_safe_stderr "$stderr_file")"
    class="$(classify_gh_error "$stderr_file")" || class=other
    GH_RETRY_LAST_CLASS="$class"

    case "$class" in
      auth)
        echo "ERROR: gh ${label} failed with an authentication error (exit ${status}); not retrying: $diagnostic" >&2
        gh_remove_capture "$stderr_file"
        stderr_file=""
        return "$status"
        ;;
      rate_limit)
        # READ path: prefer an immediate REST fallback when the core budget is
        # still available, rather than blocking for the GraphQL reset.
        if [ -n "$rest_fallback" ] && _gh_core_has_budget; then
          if fb_out="$("$rest_fallback")"; then
            echo "WARNING: gh ${label}: GraphQL rate-limited; served via REST /pulls fallback on the core budget (explicit fallback)." >&2
            gh_remove_capture "$stderr_file"
            stderr_file=""
            printf '%s\n' "$fb_out"
            GH_RETRY_LAST_CLASS="rate_limit_rest"
            return 0
          fi
          echo "WARNING: gh ${label}: REST fallback attempt failed; will wait for the authoritative reset instead." >&2
        fi
        # Write path, or core also exhausted: wait for the authoritative reset
        # (bounded by the observed reset window) and retry.
        if [ -z "$max_windows" ] || [ "$rl_windows" -lt "$max_windows" ]; then
          rl_windows=$((rl_windows + 1))
          if [ -n "$max_windows" ]; then
            echo "WARNING: gh ${label} hit a GitHub rate limit (exit ${status}); waiting for reset (window ${rl_windows}/${max_windows}): $diagnostic" >&2
          else
            echo "WARNING: gh ${label} hit a GitHub rate limit (exit ${status}); waiting for reset (window ${rl_windows}, no arbitrary cap): $diagnostic" >&2
          fi
          gh_start_owned gh_wait_for_rate_limit "$resource" "$stderr_file"
          wait "$GH_OWNED_PID"
          GH_OWNED_PID=""
          gh_remove_capture "$stderr_file"
          stderr_file=""
          continue
        fi
        # Windows exhausted: last-ditch REST for reads before failing closed.
        if [ -n "$rest_fallback" ] && _gh_core_has_budget; then
          if fb_out="$("$rest_fallback")"; then
            echo "WARNING: gh ${label}: GraphQL still exhausted after ${rl_windows} reset window(s); served via REST fallback." >&2
            gh_remove_capture "$stderr_file"
            stderr_file=""
            printf '%s\n' "$fb_out"
            GH_RETRY_LAST_CLASS="rate_limit_rest"
            return 0
          fi
        fi
        printf 'ERROR: gh %s failed (exit %s): %s\n' "$label" "$status" "$diagnostic" >&2
        gh_remove_capture "$stderr_file"
        stderr_file=""
        return "$status"
        ;;
      transient)
        if [ "$attempt" -lt "$max_transient" ]; then
          echo "WARNING: gh ${label} failed transiently (exit ${status}); retrying (${attempt}/${max_transient}): $diagnostic" >&2
          gh_remove_capture "$stderr_file"
          stderr_file=""
          gh_start_owned sleep "$delay"
          wait "$GH_OWNED_PID"
          GH_OWNED_PID=""
          delay=$((delay * 2))
          continue
        fi
        printf 'ERROR: gh %s failed (exit %s): %s\n' "$label" "$status" "$diagnostic" >&2
        gh_remove_capture "$stderr_file"
        stderr_file=""
        return "$status"
        ;;
      *)
        printf 'ERROR: gh %s failed (exit %s): %s\n' "$label" "$status" "$diagnostic" >&2
        gh_remove_capture "$stderr_file"
        stderr_file=""
        return "$status"
        ;;
    esac
  done
}

# --- the goal loop's REST read of a head's checks (#1918) -------------------
# One setting, one conditional REST read and one wait for a declined call,
# shared by the auto-drive reading (autodrive_pr_reading.sh) and its check
# wait (autodrive_pr_goal.sh ci-wait). docs/reference/autodrive-goal-loop.md.

# gh_poll_interval_secs: the seconds between two reads of a check wait, from
# RECIPE_VAR_autodrive_github_poll_secs (#3378). Empty means 120. A positive
# whole number of at most 9 digits is used as given. Anything else means 120,
# with one line on stderr.
gh_poll_interval_secs() {
  local v="${RECIPE_VAR_autodrive_github_poll_secs:-}"
  v="${v#"${v%%[![:space:]]*}"}"; v="${v%"${v##*[![:space:]]}"}"
  if [ -z "$v" ]; then printf '120\n'; return 0; fi
  if [[ "$v" =~ ^[0-9]{1,9}$ ]] && [ "$((10#$v))" -gt 0 ]; then printf '%s\n' "$((10#$v))"; return 0; fi
  printf "WARNING: autodrive_github_poll_secs must be a positive whole number of seconds (got '%s'); 120 applies\n" \
    "$(printf '%s' "$v" | LC_ALL=C tr -cd '[:print:]' | head -c 40)" >&2
  printf '120\n'
}

# gh_sleep_owned <seconds>: sleeps in a child whose PID is GH_SLEEP_PID while
# it runs, so a caller's TERM trap can stop that one process.
gh_sleep_owned() {
  sleep "$1" &
  GH_SLEEP_PID=$!
  wait "$GH_SLEEP_PID"
  GH_SLEEP_PID=""
}

# _gh_header <headers_file> <name>: the value of the first header <name>
# (case-insensitive, at the start of a line), without a carriage return.
_gh_header() {
  [ -f "$1" ] || return 1
  LC_ALL=C awk -v n="$2" 'BEGIN { n = tolower(n) ":" }
    { l = $0; sub(/\r$/, "", l) }
    index(tolower(l), n) == 1 { v = substr(l, length(n) + 1); sub(/^[ \t]+/, "", v); print v; exit }' "$1"
}

# gh_rest_get <endpoint> <cache_dir> <body_out>: one conditional GET.
# The ETag and body of the last 200 for <endpoint> are kept in <cache_dir>,
# and the next GET sends If-None-Match, so an unchanged answer is a 304 that
# GitHub does not count against the limit; the kept body is then used. A 304
# is read from the status line whatever gh's exit code is.
# Sets GH_REST_STATUS (the HTTP status, or empty), GH_REST_HEADERS (a file of
# the response headers) and GH_REST_ERR (one cleaned line on failure).
# Returns 0 with the body in <body_out>; 2 when GitHub declined the call for a
# rate limit (primary or secondary); 79 or 75 from gh unchanged; 1 otherwise.
gh_rest_get() {
  local ep="$1" cache="$2" out="$3" key etag raw err rc st
  GH_REST_STATUS=""; GH_REST_ERR=""
  key="$(printf '%s' "$ep" | cksum | tr -c '0-9\n' _ | head -n 1)"
  key="${key%_}"
  raw="$cache/$key.raw"; err="$cache/$key.err"; GH_REST_HEADERS="$cache/$key.headers"
  etag=""
  if [ -s "$cache/$key.etag" ] && [ -s "$cache/$key.body" ]; then etag="$(head -n 1 "$cache/$key.etag")"; fi
  if [ -n "$etag" ]; then
    gh api -i -H "If-None-Match: $etag" "$ep" > "$raw" 2> "$err"; rc=$?
  else
    gh api -i "$ep" > "$raw" 2> "$err"; rc=$?
  fi
  case "$rc" in 79|75) return "$rc" ;; esac
  # Headers end at the first empty line; the body follows.
  LC_ALL=C awk '{ l = $0; sub(/\r$/, "", l) } l == "" { exit } { print l }' "$raw" > "$GH_REST_HEADERS"
  st="$(head -n 1 "$GH_REST_HEADERS" | LC_ALL=C awk '$1 ~ /^HTTP\// { print $2 }')"
  GH_REST_STATUS="$st"
  if [ "$st" = 304 ] && [ -n "$etag" ]; then
    cp -- "$cache/$key.body" "$out"; return 0
  fi
  if [ "$rc" -eq 0 ] && [ "$st" = 200 ]; then
    LC_ALL=C awk 'b { print; next } { l = $0; sub(/\r$/, "", l) } l == "" { b = 1 }' "$raw" > "$out"
    etag="$(_gh_header "$GH_REST_HEADERS" etag)"
    if [ -n "$etag" ]; then
      cp -- "$out" "$cache/$key.body.tmp" && mv -f -- "$cache/$key.body.tmp" "$cache/$key.body"
      printf '%s\n' "$etag" > "$cache/$key.etag.tmp" && mv -f -- "$cache/$key.etag.tmp" "$cache/$key.etag"
    fi
    return 0
  fi
  GH_REST_ERR="$(gh_safe_stderr "$err")"
  if [ "$st" = 429 ] || [ -n "$(_gh_header "$GH_REST_HEADERS" retry-after)" ] \
    || { [ "$st" = 403 ] && [ "$(_gh_header "$GH_REST_HEADERS" x-ratelimit-remaining)" = 0 ]; } \
    || [ "$(classify_gh_error "$err")" = rate_limit ]; then
    return 2
  fi
  return 1
}

# gh_decline_reset_epoch <resource> <headers_file>: when a declined call may
# be made again, in epoch seconds. Retry-After (a secondary limit) first; then
# `gh api rate_limit`, which GitHub does not count, when <resource> is spent;
# then the response's own X-RateLimit-Reset when its remaining count is 0.
# Returns 1 when none of them says.
gh_decline_reset_epoch() {
  local resource="$1" hdrs="${2:-}" ra json reset now
  now="$(date +%s)"
  ra="$(_gh_header "$hdrs" retry-after 2>/dev/null)"
  if [[ "$ra" =~ ^[0-9]{1,9}$ ]]; then printf '%s\n' "$((now + 10#$ra))"; return 0; fi
  if json="$(gh api rate_limit 2>/dev/null)"; then
    reset="$(printf '%s' "$json" | jq -r --arg r "$resource" \
      '.resources[$r] | select(.remaining == 0) | .reset // empty' 2>/dev/null)"
    if [[ "$reset" =~ ^[0-9]{1,12}$ ]]; then printf '%s\n' "$reset"; return 0; fi
  fi
  if [ "$(_gh_header "$hdrs" x-ratelimit-remaining 2>/dev/null)" = 0 ]; then
    reset="$(_gh_header "$hdrs" x-ratelimit-reset 2>/dev/null)"
    if [[ "$reset" =~ ^[0-9]{1,12}$ ]]; then printf '%s\n' "$reset"; return 0; fi
  fi
  return 1
}

# gh_wait_out_decline <resource> <headers_file> <fallback_secs>: waits once
# until GitHub's reset for a declined call (gh_decline_reset_epoch), plus one
# second. When the reset is unknown or already past, it waits <fallback_secs>,
# the check wait's own interval. Prints one line on stderr. The caller makes
# the call again once; nothing here counts or retries.
gh_wait_out_decline() {
  local resource="$1" hdrs="${2:-}" fallback="${3:-120}" epoch now w
  now="$(date +%s)"
  if epoch="$(gh_decline_reset_epoch "$resource" "$hdrs")" && [ "$epoch" -gt "$now" ]; then
    w=$((epoch - now + 1))
    echo "GitHub declined a ${resource} call for a rate limit; waiting ${w}s, until the reset GitHub gives" >&2
  else
    w="$fallback"
    echo "GitHub declined a ${resource} call for a rate limit and gave no reset still ahead; waiting ${w}s, the check wait's interval" >&2
  fi
  gh_sleep_owned "$w"
}

# gh_head_checks <owner/repo> <sha> <cache_dir> <required_names_json> <required_only>
# The checks at <sha>, from REST: commits/<sha>/check-runs and
# commits/<sha>/status, each read with gh_rest_get. Prints one JSON object:
#   {checks:[{name, state, link}], counts:{total, passed, failing, pending,
#    skipped}, state}
# state words: pass, fail, cancel, pending, skipping. A check run that has not
# completed is pending. With <required_only> true and a non-empty name list,
# only those checks are listed, and a required name with no run or status yet
# is pending. `state` is passed, failed or pending; no checks at all is
# pending. Returns as gh_rest_get does; on 2 the declined read's headers are in
# GH_REST_HEADERS.
gh_head_checks() {
  local repo="$1" sha="$2" cache="$3" req="${4:-[]}" only="${5:-false}" page=1 rc total
  local runs="$cache/check-runs.$$" st="$cache/status.$$"
  : > "$runs"
  while :; do
    gh_rest_get "repos/$repo/commits/$sha/check-runs?per_page=100&page=$page" "$cache" "$cache/page.$$"; rc=$?
    [ "$rc" -eq 0 ] || { rm -f -- "$runs" "$cache/page.$$"; return "$rc"; }
    jq -c '.check_runs[]? | {name, status, conclusion, link:(.html_url // .details_url // "")}' "$cache/page.$$" >> "$runs" 2>/dev/null \
      || { GH_REST_ERR="the check-runs answer is not JSON"; rm -f -- "$runs" "$cache/page.$$"; return 1; }
    total="$(jq -r '.total_count // 0' "$cache/page.$$" 2>/dev/null)"
    [[ "$total" =~ ^[0-9]+$ ]] && [ "$total" -gt $((page * 100)) ] && [ "$page" -lt 10 ] || break
    page=$((page + 1))
  done
  rm -f -- "$cache/page.$$"
  gh_rest_get "repos/$repo/commits/$sha/status" "$cache" "$st"; rc=$?
  [ "$rc" -eq 0 ] || { rm -f -- "$runs" "$st"; return "$rc"; }
  jq -n -c --slurpfile runs "$runs" --slurpfile st "$st" --argjson req "$req" --argjson only "$only" '
    def run_state: if .status != "completed" then "pending"
      elif .conclusion == "success" then "pass"
      elif .conclusion == "skipped" or .conclusion == "neutral" then "skipping"
      elif .conclusion == "cancelled" then "cancel"
      else "fail" end;
    def ctx_state: if .state == "success" then "pass" elif .state == "pending" then "pending" else "fail" end;
    ([$runs[] | {name, state:run_state, link}]
      + [($st[0].statuses // [])[] | {name:.context, state:ctx_state, link:(.target_url // "")}]) as $all
    | (if $only and ($req | length) > 0
        then [$all[] | select(.name as $n | $req | index($n))]
          + [$req[] | select(. as $n | [$all[].name] | index($n) | not) | {name:., state:"pending", link:""}]
        else $all end) as $c
    | {checks:($c | .[:200]),
       counts:{total:($c | length), passed:([$c[] | select(.state == "pass")] | length),
         failing:([$c[] | select(.state == "fail" or .state == "cancel")] | length),
         pending:([$c[] | select(.state == "pending")] | length),
         skipped:([$c[] | select(.state == "skipping")] | length)}}
    | .state = (if .counts.total == 0 or .counts.pending > 0 then "pending"
        elif .counts.failing > 0 then "failed" else "passed" end)'
  rc=$?
  rm -f -- "$runs" "$st"
  [ "$rc" -eq 0 ] || { GH_REST_ERR="the checks answer could not be read"; return 1; }
}
