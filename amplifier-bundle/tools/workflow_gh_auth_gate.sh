# shellcheck shell=bash
# gh authentication and push-permission gates for step-21-pr-ready (#1575).
#
# Sourced by workflow_pr_ready.sh. Step 21 used one `gh auth status` check and
# failed the whole run at its last step when it missed. This host shares
# ~/.config/gh/hosts.yml across concurrent workstreams, so the active account
# can drop or flip for a few seconds and come back. These gates retry with
# capped exponential backoff instead.
#
#   gh_auth_gate                     0 once `gh auth status` succeeds; 1 with
#                                    the contract ERROR when every attempt fails.
#   gh_push_permission_gate <o/r>    0 when .permissions.push is true, unknown,
#                                    or the identity is invalid; 1 when it is
#                                    false on every attempt.
#
# Settings (each must be 1-3 digits and in range, else WARNING + default):
#   WORKFLOW_GH_AUTH_ATTEMPTS   default 5,  1-10
#   WORKFLOW_GH_AUTH_BASE_DELAY default 4,  0-300 seconds
#   WORKFLOW_GH_AUTH_MAX_DELAY  default 60, 0-600 seconds
# The wait before retry i is min(BASE * 2^(i-1), MAX).
#
# Read-only: never switches, logs in, refreshes or prints credentials. Uses only
# local variables and never changes the caller's shell options or traps.

# Print a validated setting, or warn and print its default.
_gh_gate_setting() {
  local name="$1" default="$2" min="$3" max="$4" raw value
  raw="${!name:-}"
  if [ -z "$raw" ]; then
    printf '%s\n' "$default"
    return 0
  fi
  if [[ "$raw" =~ ^[0-9]{1,3}$ ]]; then
    value=$((10#$raw))
    if [ "$value" -ge "$min" ] && [ "$value" -le "$max" ]; then
      printf '%s\n' "$value"
      return 0
    fi
  fi
  echo "WARNING: $name must be a whole number from $min to $max; using the default $default" >&2
  printf '%s\n' "$default"
}

# Print min(base * 2^(attempt-1), max) without overflowing.
_gh_gate_delay() {
  local delay="$1" max="$2" attempt="$3" k=1
  while [ "$k" -lt "$attempt" ] && [ "$delay" -lt "$max" ]; do
    delay=$((delay * 2))
    k=$((k + 1))
  done
  if [ "$delay" -gt "$max" ]; then delay="$max"; fi
  printf '%s\n' "$delay"
}

# Load the three settings into the caller's locals `attempts base max`.
_gh_gate_settings() {
  attempts="$(_gh_gate_setting WORKFLOW_GH_AUTH_ATTEMPTS 5 1 10)"
  base="$(_gh_gate_setting WORKFLOW_GH_AUTH_BASE_DELAY 4 0 300)"
  max="$(_gh_gate_setting WORKFLOW_GH_AUTH_MAX_DELAY 60 0 600)"
}

_gh_gate_wait() {
  if [ "$1" -gt 0 ]; then sleep "$1"; fi
}

gh_auth_gate() {
  local attempts base max attempt=1 delay
  _gh_gate_settings
  while :; do
    if timeout 30 gh auth status >/dev/null 2>&1; then
      return 0
    fi
    if [ "$attempt" -ge "$attempts" ]; then
      break
    fi
    delay="$(_gh_gate_delay "$base" "$max" "$attempt")"
    echo "WARNING: gh auth status failed (attempt $attempt/$attempts); retrying in ${delay}s" >&2
    _gh_gate_wait "$delay"
    attempt=$((attempt + 1))
  done
  echo "ERROR: gh CLI is unavailable or unauthenticated — cannot verify or mutate GitHub PR readiness" >&2
  return 1
}

gh_push_permission_gate() {
  local repo_id="${1:-}" attempts base max attempt=1 delay answer login
  if ! [[ "$repo_id" =~ ^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$ ]] || [[ "${repo_id#*/}" == .* ]]; then
    echo "WARNING: repo identity is not a valid GitHub owner/repo; skipping push-permission check" >&2
    return 0
  fi
  _gh_gate_settings
  while :; do
    if ! answer="$(timeout 30 gh api "repos/$repo_id" --jq .permissions.push 2>/dev/null)"; then
      answer="unavailable"
    fi
    case "$answer" in
      true) return 0 ;;
      false) ;;
      *)
        echo "WARNING: could not determine push permission on $repo_id; continuing (GitHub enforces permissions on the mutation)" >&2
        return 0
        ;;
    esac
    if [ "$attempt" -ge "$attempts" ]; then
      break
    fi
    delay="$(_gh_gate_delay "$base" "$max" "$attempt")"
    echo "WARNING: gh reports push=false on $repo_id (attempt $attempt/$attempts); retrying in ${delay}s" >&2
    _gh_gate_wait "$delay"
    attempt=$((attempt + 1))
  done
  if ! login="$(timeout 30 gh api user --jq .login 2>/dev/null)" || ! [[ "$login" =~ ^[A-Za-z0-9-]{1,39}$ ]]; then
    login="unknown"
  fi
  echo "ERROR: active gh account '$login' lacks push permission on $repo_id" >&2
  return 1
}
