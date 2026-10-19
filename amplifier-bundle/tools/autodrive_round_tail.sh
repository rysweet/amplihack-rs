#!/usr/bin/env bash
# autodrive_round_tail.sh <log-file> — print the last 20000 bytes of a round
# log, for the loop-health evaluator's loop_last_round_output.
#
# A byte tail of a longer log starts inside a line. That fragment has lost its
# `[HH:MM:SS] [amplihack:...]` prefix, so an agent line cut there would escape
# the evidence collector's agent-line filter and could be read as a policy
# refusal (#1510). When the log is longer than 20000 bytes, the partial first
# line is dropped; every line printed is whole. A shorter log is printed as it
# is. A missing log prints nothing.
#
# The collector cuts the runner's run summary (`Recipe: <name>` followed by
# `Run ID:` or `Status:`) before its refusal checks, because the summary
# reprints agent output unprefixed (round 3 of PR #1632). A summary longer
# than the window would lose that header and leave its body to be read as
# guard output. So when the log has a summary header and the printed tail
# does not, the header's two lines are printed first: the collector then cuts
# the whole tail, all of which is summary. The awk that finds the header is
# the one the detectors cut with.
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
