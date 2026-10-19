# shellcheck shell=bash
# autodrive_refusal.sh — is a child's failure a terminal policy refusal?
#
# Sourced by autodrive_loop.sh from its own directory, never executed. It
# defines functions only: no top-level commands, no shell-option changes.
#
# Issue #1437: the loop used to grep a child's transcript for the guard's
# refusal text. A transcript holds every file the round's agents read and every
# refusal a descendant already handled inline, so an agent whose API session
# died after it had pushed its work was reported as refused, and the run
# stopped for good. Two things decide now, and transcript text is neither:
#
#   1. the child's exit status, compared as the string 79 (#1327/#1332);
#   2. "structural_refusal": true on the LAST line of the log that begins with
#      the runner's marker prefix `amplihack.recipe.failure_class `. The runner
#      writes its marker after all step output, so a nested run's marker or a
#      forged one printed earlier is overridden by the outermost runner's.
#
# Every marker value is cleaned before use (SR-5), so a hostile log cannot add
# a history row, write escape sequences to stderr or break the escalation JSON.
# Fields are read with the same `orch helper` the loop uses for every
# structured verdict; this file adds no dependency.
#
#   classify_child <log>               prints the last marker line; sets the
#                                      cleaned CHILD_FAILURE_* and
#                                      CHILD_STRUCTURAL_REFUSAL. Always 0.
#   terminal_refusal <rc> <log>        0 when terminal, 1 otherwise (always 1
#                                      for rc 0 or a non-integer rc); prints
#                                      nothing; sets TERMINAL_WHY.
#   child_failure_summary <rc> <log>   prints `rc=<n> class=<c> retryable=<r>`
#                                      plus ` signal=<s>` when there is one.
#
# Docs: docs/reference/auto-drive-to-merge.md#autodrive_refusalsh

# autodrive_marker_field <json> <name>: one field of the marker, or nothing.
autodrive_marker_field() {
  printf '%s' "${1:-}" \
    | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-json --require-field "$2" 2>/dev/null \
    | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-field --field "$2" --default "" 2>/dev/null
  return 0
}

classify_child() { # classify_child <log>
  CHILD_FAILURE_CLASS="unknown"; CHILD_FAILURE_RETRYABLE="unknown"
  CHILD_FAILURE_SIGNAL=""; CHILD_STRUCTURAL_REFUSAL=""
  local log="${1:-}" line json value
  [ -n "$log" ] && [ -f "$log" ] && [ -r "$log" ] || return 0
  # The prefix is FAILURE_CLASS_MARKER_PREFIX in
  # crates/amplihack-cli/src/commands/recipe/run/failure_class.rs. Anchored at
  # the start of the line with the dots escaped, so an indented or quoted copy
  # of a marker is not one. -a reads a log with binary bytes in it; only the
  # last line is kept, and the log is never read into a variable.
  line="$(LC_ALL=C grep -a '^amplihack\.recipe\.failure_class ' "$log" 2>/dev/null | tail -n 1)"
  [ -n "$line" ] || return 0
  printf '%s\n' "$line"
  json="${line#amplihack.recipe.failure_class }"

  value="$(autodrive_marker_field "$json" class)"
  if printf '%s' "$value" | LC_ALL=C grep -qxE '[a-z_]{1,32}'; then CHILD_FAILURE_CLASS="$value"; fi
  value="$(autodrive_marker_field "$json" retryable)"
  case "$value" in true|false) CHILD_FAILURE_RETRYABLE="$value" ;; esac
  value="$(autodrive_marker_field "$json" structural_refusal)"
  case "$value" in true|false) CHILD_STRUCTURAL_REFUSAL="$value" ;; esac
  value="$(autodrive_marker_field "$json" signal | LC_ALL=C tr -d '\000-\037\177' | head -c 120)"
  [ "$value" = "null" ] || CHILD_FAILURE_SIGNAL="$value"
  return 0
}

# shellcheck disable=SC2034 # TERMINAL_WHY is read by the caller.
terminal_refusal() { # terminal_refusal <rc> <log>
  TERMINAL_WHY=""
  if [ "${1:-}" = "79" ]; then
    TERMINAL_WHY="exit 79"
    return 0
  fi
  # A child that exited 0 was not refused. The runner writes its final marker
  # only when a run fails, so a successful child's last marker line is a nested
  # run's or a forged one. A missing or non-numeric status never reads it.
  case "${1:-}" in ''|0|*[!0-9]*) return 1 ;; esac
  classify_child "${2:-}" >/dev/null
  if [ "$CHILD_STRUCTURAL_REFUSAL" = "true" ]; then
    TERMINAL_WHY="structural_refusal in failure_class marker"
    return 0
  fi
  return 1
}

child_failure_summary() { # child_failure_summary <rc> <log>
  local rc="unknown"
  case "${1:-}" in ''|*[!0-9]*) ;; *) rc="$1" ;; esac
  classify_child "${2:-}" >/dev/null
  printf 'rc=%s class=%s retryable=%s%s\n' "$rc" "$CHILD_FAILURE_CLASS" "$CHILD_FAILURE_RETRYABLE" \
    "${CHILD_FAILURE_SIGNAL:+ signal=${CHILD_FAILURE_SIGNAL}}"
}
