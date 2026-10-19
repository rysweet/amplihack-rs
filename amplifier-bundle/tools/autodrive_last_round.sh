#!/usr/bin/env bash
# autodrive_last_round.sh <state-dir> <loop-name> — print the highest round
# number <loop-name> has already used in <state-dir>, or 0.
#
# autodrive_loop.sh labels its rounds from one past this number. The state dir
# may already hold this loop's rounds (phase 2's crusty loop before a
# re-review, or a resumed run), and relabelling from round-1 would rewrite an
# earlier record that a manifest row still names (PR #1520 review). The number
# is the highest N in a <loop>-round-N... entry or in a round-N row of
# <loop>-records.tsv. The loop runs this after autodrive_private_dir, so
# untrusted entries are not counted. It reads; it writes nothing.
#
# Executed, never sourced. Exit 2 is a usage error.

set -uo pipefail
export LC_ALL=C

STATE_DIR="${1:-}"; LOOP_NAME="${2:-}"
if [ -z "$STATE_DIR" ] || [ -z "$LOOP_NAME" ]; then
  echo "usage: autodrive_last_round.sh <state-dir> <loop-name>" >&2
  exit 2
fi

top=0; re='^[1-9][0-9]{0,8}$'
for f in "${STATE_DIR}/${LOOP_NAME}-round-"*; do
  [ -e "$f" ] || [ -L "$f" ] || continue
  n="${f#"${STATE_DIR}/${LOOP_NAME}-round-"}"; n="${n%%[!0-9]*}"
  [[ "$n" =~ $re ]] && [ "$n" -gt "$top" ] && top="$n"
done
if [ -f "${STATE_DIR}/${LOOP_NAME}-records.tsv" ] && [ ! -L "${STATE_DIR}/${LOOP_NAME}-records.tsv" ]; then
  while IFS=$'\t' read -r label _; do
    n="${label#round-}"
    [ "$n" != "$label" ] && [[ "$n" =~ $re ]] && [ "$n" -gt "$top" ] && top="$n"
  done < <(tr -d '\r' < "${STATE_DIR}/${LOOP_NAME}-records.tsv")
fi
printf '%s\n' "$top"
