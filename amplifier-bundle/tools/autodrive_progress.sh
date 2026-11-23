#!/usr/bin/env bash
# autodrive_progress.sh: a one-release alias (#3049, #3129).
#
# It recorded each round and stopped a loop whose rounds made no progress.
# The shared goal loop replaced it: autodrive_goal_loop.sh; whether a review converges is the agent's judgement, recorded in the turn records.
# This file stays for one release so that a run with an older pinned bundle,
# or a caller that still names it, keeps working. It prints one plain line
# naming the replacement and does nothing else: it brings back no stop, no
# budget and no gate. Sourced or run, it ends with status 0.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.

echo "autodrive_progress.sh is retired (#3129) and does nothing here; its replacement is autodrive_goal_loop.sh and its turn records." >&2
return 0 2>/dev/null || exit 0
