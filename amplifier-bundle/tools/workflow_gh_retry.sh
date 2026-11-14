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
  rm -f -- "$1" 2>/dev/null || printf '%s\n' 'WARNING: gh capture cleanup failed' >&2
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
#   GH_RETRY_DIAGNOSTIC_FILE private file owned by the caller (optional):
#                           emptied when each attempt starts and overwritten
#                           with each failed attempt's sanitized, bounded
#                           diagnostic (or its temp-creation failure);
#                           independent of retry warnings. Empty after failure
#                           means no attempt's failure was recorded (e.g.
#                           cancelled before the next attempt).
#                           Read only after failure; caller removes the file.
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

# Private-state contract: call these helpers only within _gh_retry_core.
# It owns local stderr_file/stdout_file, GH_OWNED_PID, retry_status,
# retry_interrupted_status, retry_int/term/hup and retry_shell_pid. Attempts
# replace captures and child PID; cleanup cancels/reaps the child, unlinks
# captures and clears those variables. Interrupt sets retry_interrupted_status;
# restore uses saved traps/PID to preserve the caller's signal disposition.
# The core cleans up and restores traps on return. GH_RETRY_LAST_CLASS is the
# caller-visible result; GH_RETRY_DIAGNOSTIC_FILE belongs to the caller, is
# overwritten only with safe diagnostics, and is never removed by this owner.
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
    # Clear the previous attempt's diagnostic first, so a failure before this
    # attempt runs gh never reports an earlier attempt's text. Temp creation
    # failure records its own diagnostic below; a cancellation during a
    # backoff or reset wait leaves the capture empty, and the caller falls
    # back to its ordinary stderr capture.
    if [ -n "${GH_RETRY_DIAGNOSTIC_FILE:-}" ]; then
      if : >"$GH_RETRY_DIAGNOSTIC_FILE"; then :; else
        printf 'WARNING: gh terminal diagnostic capture reset failed (exit %s)\n' "$?" >&2
      fi
    fi
    if stderr_file="$(umask 077; mktemp "${GH_RETRY_TMPDIR:-${TMPDIR:-/tmp}}/.workflow-gh-retry.XXXXXX" 2>/dev/null)"; then
      :
    else
      status=$?
      diagnostic="private stderr temp creation failed (exit $status)"
      printf 'ERROR: gh %s: %s\n' "$label" "$diagnostic" >&2
      # This is the last attempt's failure; record it so the caller does not
      # have to find it behind earlier retry warnings in ordinary stderr.
      if [ -n "${GH_RETRY_DIAGNOSTIC_FILE:-}" ]; then
        printf '%s' "$diagnostic" >"$GH_RETRY_DIAGNOSTIC_FILE" 2>/dev/null || :
      fi
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
    if [ -n "${GH_RETRY_DIAGNOSTIC_FILE:-}" ]; then
      if printf '%s' "$diagnostic" >"$GH_RETRY_DIAGNOSTIC_FILE"; then :; else
        printf 'WARNING: gh terminal diagnostic capture failed (exit %s)\n' "$?" >&2
      fi
    fi
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
