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
# classify_gh_error <stderr_file> [explicit] -> auth | rate_limit | transient | other
# The goal reader supplies explicit when a bare 403 is insufficient evidence.
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
  if [ "${2:-}" != explicit ] && grep -Eiq 'HTTP 403|(^|[^0-9])403([^0-9]|$)' "$f"; then
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

# --- goal-loop reads and CI waits (#1918) ---------------------------------

# Empty uses the REST wait's default. Use the setting shared with #3378.
gh_poll_interval_secs() {
  local value="${RECIPE_VAR_checks_watch_interval:-}"
  if [ -z "$value" ]; then
    printf '120\n'
  elif [[ "$value" =~ ^0*([1-9][0-9]*)$ ]]; then
    printf '%s\n' "${BASH_REMATCH[1]}"
  else
    printf '%s\n' 'WARNING: checks_watch_interval must be a positive whole number of seconds. Using 120.' >&2
    printf '120\n'
  fi
}

# Call in the shell that owns the operation, with an EXIT trap that calls
# gh_goal_cancel. Bash can handle a signal immediately while in builtin wait.
gh_goal_command() {
  local rc=0
  "$@" &
  GH_GOAL_PID=$!
  wait "$GH_GOAL_PID" || rc=$?
  GH_GOAL_PID=""
  return "$rc"
}

gh_goal_cancel() {
  [ -n "${GH_GOAL_PID:-}" ] || return 0
  kill -TERM "$GH_GOAL_PID" 2>/dev/null || :
  wait "$GH_GOAL_PID" 2>/dev/null || :
}

_gh_header() {
  LC_ALL=C awk -v name="$2" '
    BEGIN { name = tolower(name) ":" }
    index(tolower($0), name) == 1 {
      value = substr($0, length(name) + 1)
      sub(/^[ \t]+/, "", value)
      sub(/[ \t\r]+$/, "", value)
      print value
      exit
    }' "$1"
}

# The reading can supply gh_rate_limit_guidance to wait for a new entry.
# A CI wait is already under guidance wait. With no deadline, block until
# the enclosing guidance wake or a cancellation interrupts the shell.
_gh_goal_wait_for_guidance() {
  local rc=0
  if declare -F gh_rate_limit_guidance >/dev/null; then
    gh_rate_limit_guidance || rc=$?
    case "$rc" in 0|75|79) return "$rc" ;; esac
  fi
  mkfifo "$GH_READ_DIR/rate-limit.wait" || return 1
  read -r <> "$GH_READ_DIR/rate-limit.wait"
}

_gh_goal_wait_for_reset() {
  local resource="$1" headers="$2" reported_at="$3"
  local retry remaining reset deadline=0 rc=0 now
  retry="$(_gh_header "$headers" retry-after)"
  remaining="$(_gh_header "$headers" x-ratelimit-remaining)"
  reset="$(_gh_header "$headers" x-ratelimit-reset)"
  # This lookup is outside the primary allowance. Check the declined
  # resource even when the response already supplies a retry deadline.
  gh_goal_command gh api rate_limit > "$GH_READ_DIR/rate-limit.json" 2> "$GH_READ_DIR/rate-limit.err" || rc=$?
  case "$rc" in 75|79) return "$rc" ;; esac
  if [ -z "$remaining" ] || { [ "$remaining" = 0 ] && ! [[ "$reset" =~ ^[0-9]{1,12}$ ]]; }; then
    reset=""
    if [ "$rc" -eq 0 ]; then
      reset="$(jq -r --arg resource "$resource" '
        .resources[$resource] | select(.remaining == 0) | .reset // empty
      ' "$GH_READ_DIR/rate-limit.json" 2>/dev/null)" || reset=""
    else
      printf 'WARNING: GitHub rate-limit lookup failed: %s\n' "$(gh_safe_stderr "$GH_READ_DIR/rate-limit.err")" >&2
    fi
  elif [ "$remaining" != 0 ]; then
    reset=""
  fi
  if [[ "$reset" =~ ^[0-9]{1,12}$ ]]; then deadline=$((10#$reset)); fi
  if [[ "$retry" =~ ^[0-9]{1,9}$ ]] && [ "$((reported_at + 10#$retry))" -gt "$deadline" ]; then
    deadline=$((reported_at + 10#$retry))
  fi
  if [ "$deadline" -le "$reported_at" ]; then
    printf 'GitHub declined a %s read and supplied no future retry deadline. The read is pending.\n' "$resource" >&2
    _gh_goal_wait_for_guidance
    return $?
  fi
  now="$(date +%s)" || return 1
  if [ "$deadline" -gt "$now" ]; then
    printf 'GitHub declined a %s read. Waiting until its retry deadline.\n' "$resource" >&2
    gh_goal_command sleep "$((deadline - now))" || return $?
  fi
}

# gh_goal_read <core|graphql> <include_headers true|false> <gh args...>
# GH_READ_DIR is the caller's private scratch. GH_READ_BODY points to a file
# valid until the next read. Only a rate-limit response retries the command.
gh_goal_read() {
  local resource="$1" include_headers="$2" rc headers reported_at
  shift 2
  local raw="$GH_READ_DIR/gh.raw" err="$GH_READ_DIR/gh.err"
  GH_READ_BODY="$raw"
  GH_READ_STATUS=""
  GH_READ_ERROR=""
  while :; do
    rc=0
    gh_goal_command gh "$@" > "$raw" 2> "$err" || rc=$?
    case "$rc" in 75|79) return "$rc" ;; esac
    headers="$err"
    if [ "$include_headers" = true ]; then
      headers="$GH_READ_DIR/gh.headers"
      LC_ALL=C awk '{ sub(/\r$/, "") } !length { exit } { print }' "$raw" > "$headers" || return 1
      GH_READ_STATUS="$(LC_ALL=C awk 'NR == 1 && /^HTTP\// { print $2 }' "$headers")"
      if [ "$GH_READ_STATUS" = 304 ]; then return 0; fi
      if [ "$GH_READ_STATUS" = 200 ] && [ "$rc" -eq 0 ]; then
        GH_READ_BODY="$GH_READ_DIR/gh.body"
        LC_ALL=C awk 'body { print; next } /^\r?$/ { body = 1 }' "$raw" > "$GH_READ_BODY"
        return $?
      fi
    elif [ "$rc" -eq 0 ]; then
      return 0
    fi
    if [ "$GH_READ_STATUS" = 429 ] ||
      { [ "$GH_READ_STATUS" = 403 ] &&
        { [ "$(_gh_header "$headers" x-ratelimit-remaining)" = 0 ] ||
          [ -n "$(_gh_header "$headers" retry-after)" ]; }; } ||
      [ "$(classify_gh_error "$err" explicit)" = rate_limit ]; then
      reported_at="$(date +%s)" || return 1
      _gh_goal_wait_for_reset "$resource" "$headers" "$reported_at" || return $?
    else
      GH_READ_ERROR="$(gh_safe_stderr "$err")"
      [ "$rc" -ne 0 ] || rc=1
      return "$rc"
    fi
  done
}

# gh_rest_get <endpoint> <cache_dir> <jq validation> [<cache scope>]
# The caller creates cache_dir in the run's private state folder, on the same
# filesystem as GH_READ_DIR.
# Read a validated 200 or reuse the same entry on a 304. Snapshot the old
# entry before the request so a concurrent writer cannot pair its ETag with
# a different body. Publish the replacement with an atomic rename.
gh_rest_get() {
  local endpoint="$1" cache="$2" validate="$3" scope="${4:-}" key entry etag=""
  local snapshot="$GH_READ_DIR/cache.previous" staged="$GH_READ_DIR/cache.next"
  local -a headers
  GH_READ_ERROR=""
  GH_REST_LINK=""
  key="$(printf '%s\n%s' "$scope" "$endpoint" | git hash-object --stdin)" || return 1
  entry="$cache/$key.json"
  headers=()
  if [ -f "$entry" ] && [ ! -L "$entry" ] && cp -- "$entry" "$snapshot" &&
      etag="$(jq -er '.etag | strings | select(length > 0)' "$snapshot" 2>/dev/null)"; then
    headers=(-H "If-None-Match: $etag")
  else
    etag=""
  fi
  gh_goal_read core true api --include "${headers[@]}" "$endpoint" || return $?
  if [ "$GH_READ_STATUS" = 304 ]; then
    if [ -z "$etag" ]; then
      GH_READ_ERROR="GitHub returned 304 without a matching cached response"
      return 1
    fi
    GH_READ_BODY="$GH_READ_DIR/gh.body"
    GH_REST_LINK="$(jq -r '.link' "$snapshot")" &&
      jq -c '.body' "$snapshot" > "$GH_READ_BODY"
    return $?
  fi
  etag="$(_gh_header "$GH_READ_DIR/gh.headers" etag)"
  GH_REST_LINK="$(_gh_header "$GH_READ_DIR/gh.headers" link)"
  if ! jq -s -c -e --arg etag "$etag" --arg link "$GH_REST_LINK" '
    if length == 1 and (.[0] | ('"$validate"')) then {etag:$etag, link:$link, body:.[0]}
    else error("unexpected response") end
  ' "$GH_READ_BODY" > "$staged" 2>/dev/null; then
    GH_READ_ERROR="GitHub returned an unreadable response for ${endpoint%%\?*}"
    return 1
  fi
  if ! mv -f -- "$staged" "$entry"; then
    GH_READ_ERROR="the conditional REST cache could not be updated"
    return 1
  fi
}

# Each page has its own cache entry, including its Link header. Use only
# the next page number from that header and keep the request on the same
# endpoint and host. No request is made for a page GitHub did not report.
_gh_check_pages() {
  local endpoint="$1" cache="$2" validate="$3" select_checks="$4" page=1
  local next_link='<[^>]*[?&]page=([0-9]+)(&[^>]*)?>;[[:space:]]*rel="next"'
  while :; do
    gh_rest_get "${endpoint}?per_page=100&page=$page" "$cache" "$validate" || return $?
    jq -c "$select_checks" "$GH_READ_BODY" || return 1
    if [[ "$GH_REST_LINK" != *'rel="next"'* ]]; then return 0; fi
    page=$((page + 1))
    if ! [[ "$GH_REST_LINK" =~ $next_link && "${BASH_REMATCH[1]}" = "$page" ]]; then
      GH_READ_ERROR="GitHub returned an invalid next page for $endpoint"
      return 1
    fi
  done
}

# gh_head_checks <owner/repo> <sha> <cache_dir> <required_names_json>
# Pass [] to consider all checks. A missing required name is pending, as is
# a head with no reported checks. The per-turn display is capped separately
# from the counts and completion state.
gh_head_checks() {
  local commit="repos/$1/commits/$2" cache="$3" required="$4"
  local rows="$GH_READ_DIR/checks.jsonl"
  _gh_check_pages "$commit/check-runs" "$cache" '
    (.check_runs | type == "array") and all(.check_runs[];
      (.name | type == "string" and length > 0) and (.status | type == "string") and
      (.conclusion | . == null or type == "string"))
  ' '
    .check_runs[] | {name, link:(.html_url // .details_url // ""), state:(
      if .status != "completed" or .conclusion == null then "pending"
      elif .conclusion == "success" then "pass"
      elif .conclusion == "skipped" or .conclusion == "neutral" then "skipping"
      elif .conclusion == "cancelled" then "cancel"
      else "fail" end
    )}
  ' > "$rows" || return $?
  _gh_check_pages "$commit/status" "$cache" '
    (.statuses | type == "array") and all(.statuses[];
      (.context | type == "string" and length > 0) and
      (.state | . == "success" or . == "pending" or . == "failure" or . == "error"))
  ' '
    .statuses[] | {name:.context, link:(.target_url // ""), state:(
      if .state == "success" then "pass" elif .state == "pending" then "pending" else "fail" end
    )}
  ' >> "$rows" || return $?
  jq -c -s --argjson required "$required" '
    . as $all | ($all | map(.name) | unique) as $reported
    | (if ($required | length) == 0 then $all else
        [$all[] | select(.name as $name | $required | index($name))]
        + [($required - $reported)[] | {name:., state:"pending", link:""}]
       end | map(. + {elapsed:""})) as $checks
    | ($checks | group_by(.state) | map({key:.[0].state, value:length}) | from_entries) as $n
    | {checks:$checks[:200], counts:{total:($checks | length), passed:($n.pass // 0),
        failing:(($n.fail // 0) + ($n.cancel // 0)), pending:($n.pending // 0), skipped:($n.skipping // 0)}}
    | .state = (if .counts.total == 0 or .counts.pending > 0 then "pending"
        elif .counts.failing > 0 then "failed" else "passed" end)
  ' "$rows"
}
