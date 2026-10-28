# autodrive_layout.sh — the one definition of the auto-drive state layout.
#
# Sourced by autodrive_state.sh and autodrive_loop.sh, and compiled into
# `amplihack workstreams status` (issue #1710), which reads the state those
# scripts write. Only plain NAME=value lines: no expansions, quotes or
# commands, because Rust parses this file too and a test enforces the grammar.

# The state root is ${AMPLIHACK_STATE_DIR:-$HOME/.amplihack/state}/<subdir>.
AUTODRIVE_STATE_SUBDIR=auto-drive
# Completed phases, one "<phase><TAB><UTC time>" row each.
AUTODRIVE_PHASES_FILE=phases.tsv
AUTODRIVE_PHASE_BUILD=build
AUTODRIVE_PHASE_CRUSTY_LOOP=crusty-loop
AUTODRIVE_PHASE_MERGE_READY_LOOP=merge-ready-loop
AUTODRIVE_PHASE_MERGED=merged
# Loop names: files are <loop>-round-<N>.json, <loop>-latest.json and
# <loop>-records.tsv.
AUTODRIVE_LOOP_CRUSTY=crusty
AUTODRIVE_LOOP_MERGE_READY=merge-ready
AUTODRIVE_LATEST_SUFFIX=-latest.json
AUTODRIVE_RECORDS_SUFFIX=-records.tsv
AUTODRIVE_ROUND_INFIX=-round-
