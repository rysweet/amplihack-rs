#!/usr/bin/env bash
# autodrive_done_override.sh <raw-verdict> <verdict> <prev-overridden>
#   <prev-verdict> <findings> <prev-findings> <baseline> <head>
#
# What autodrive_loop.sh does with a loop-health DONE over a round whose own
# verdict is not the clean token (issue #1597). Such a DONE never advances:
# the findings may have been addressed where no review has seen it yet (a PR
# description edit, say), so the loop runs one more round to re-review.
#
#   Exit 0 — run another round to re-review.
#   Exit 1 — STUCK. stdout carries the reason clause, or nothing when the
#            round wrote no record (<raw-verdict> MISSING).
#   Exit 2 — usage error; the loop reads any non-zero exit as STUCK.
#
# STUCK when the round record is missing, or when the previous round's DONE
# was overridden too (<prev-overridden> 1) and this round moved nothing: the
# same <verdict>, the same findings once whitespace is collapsed and trimmed,
# and <head> equal to the round's <baseline>. An empty <baseline> or <head>
# (git could not read HEAD) counts as moved. There is no counter here; the
# stop comes from the absence of progress. <verdict> and <prev-verdict> are
# the loop's A-Za-z0-9_ copies; the raw verdict is only compared to MISSING.
#
# Executed, never sourced. It reads its arguments and writes nothing.

set -uo pipefail
export LC_ALL=C

if [ $# -ne 8 ]; then
  echo "usage: autodrive_done_override.sh <raw-verdict> <verdict> <prev-overridden> <prev-verdict> <findings> <prev-findings> <baseline> <head>" >&2
  exit 2
fi
RAW_VERDICT="$1"; VERDICT="$2"; PREV_OVERRIDDEN="$3"; PREV_VERDICT="$4"
FINDINGS="$5"; PREV_FINDINGS="$6"; BASELINE="$7"; HEAD="$8"

[ "$RAW_VERDICT" = "MISSING" ] && exit 1

trim_ws() { tr -s '[:space:]' ' ' | sed 's/^ //; s/ $//'; }

if [ "$PREV_OVERRIDDEN" = "1" ] && [ "$VERDICT" = "$PREV_VERDICT" ] \
   && [ "$(printf '%s' "$FINDINGS" | trim_ws)" = "$(printf '%s' "$PREV_FINDINGS" | trim_ws)" ] \
   && [ -n "$BASELINE" ] && [ "$HEAD" = "$BASELINE" ]; then
  printf "repeated DONE over non-clean '%s' with no progress (same verdict, same findings, HEAD unchanged)\n" "$VERDICT"
  exit 1
fi
exit 0
