#!/usr/bin/env bash
# The three-failure termination from issue #3286 was replaced by the
# pairwise owner wait in issue #3424. Keep this entry point for runners that
# still invoke the older regression name.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
exec bash "$ROOT/tests/issue_3424_goal_loop_recovery.sh"
