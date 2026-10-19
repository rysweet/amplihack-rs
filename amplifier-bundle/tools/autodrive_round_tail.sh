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
# Executed, never sourced. It reads; it writes nothing. Always exits 0.

set -uo pipefail
export LC_ALL=C

LOG="${1:-}"
[ -f "$LOG" ] || exit 0
if [ "$(wc -c < "$LOG" 2>/dev/null || echo 0)" -gt 20000 ]; then
  tail -c 20000 -- "$LOG" 2>/dev/null | sed '1d'
else
  cat -- "$LOG" 2>/dev/null
fi
exit 0
