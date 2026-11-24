#!/usr/bin/env bash
# autodrive_round_word.sh: a one-release alias (#3049, #3129).
#
# It read the merge-ready round's outcome word, a parsed-record readiness gate.
# The shared goal loop replaced it: autodrive_goal_loop.sh; readiness is the agent's judgement from the live reading.
# This file stays for one release so that a path that still names it finds a
# file. It keeps the name, not the old interface. It prints one plain line
# naming the replacement and does nothing else: it brings back no stop, no
# budget and no gate. Sourced or run, it ends with status 0.
# A run started on 1.2.212 never reaches it: auto-drive runs every tool from
# the launch copy of the bundle made when the run starts (#2959).
# A 1.2.212 merge-round step that did source it would find no
# autodrive_round_word or autodrive_round_file and stop with status 127.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.

echo "autodrive_round_word.sh is retired (#3129) and does nothing here; its replacement is autodrive_goal_loop.sh; readiness is the agent's judgement." >&2
return 0 2>/dev/null || exit 0
