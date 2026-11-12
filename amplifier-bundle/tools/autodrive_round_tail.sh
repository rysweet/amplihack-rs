#!/usr/bin/env bash
# autodrive_round_tail.sh <log-file> — print the last 20000 bytes of a round
# log, for the loop-health evaluator's loop_last_round_output.
#
# A byte tail of a longer log starts inside a line. When the log is longer
# than 20000 bytes, the partial first line is dropped; every line printed is
# whole. A shorter log is printed as it is. A missing log prints nothing.
#
# The collector's refusal check (#1437, autodrive_refusal.sh) reads only the
# LAST line that begins `amplihack.recipe.failure_class `. The runner writes
# that line last, so it is always inside the tail, and whole lines keep a cut
# fragment from beginning at column 0 like one. Text no longer decides a
# refusal, so the agent-line filter and summary cut #1510 and PR #1632 needed
# for it are gone from the collector.
#
# When the log has a run summary header (`Recipe: <name>` followed by
# `Run ID:` or `Status:`) and the printed tail does not, the header's two
# lines are still printed first, so a reader of the tail can tell the rest is
# summary.
#
# Executed, never sourced. It reads; it writes nothing. Always exits 0.

set -uo pipefail
export LC_ALL=C

LOG="${1:-}"
[ -f "$LOG" ] || exit 0
if [ "$(wc -c < "$LOG" 2>/dev/null || echo 0)" -le 20000 ]; then
  cat -- "$LOG" 2>/dev/null
  exit 0
fi

summary_header() {
  awk 'NR > 1 && prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): / { print prev; print; exit } { prev = $0 }'
}

TAIL="$(tail -c 20000 -- "$LOG" 2>/dev/null | sed '1d')"
HEADER="$(summary_header < "$LOG" 2>/dev/null)"
if [ -n "$HEADER" ] && [ -z "$(printf '%s\n' "$TAIL" | summary_header)" ]; then
  printf '%s\n' "$HEADER"
fi
printf '%s\n' "$TAIL"
exit 0
