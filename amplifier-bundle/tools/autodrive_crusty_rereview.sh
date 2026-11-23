#!/usr/bin/env bash
# autodrive_crusty_rereview.sh: a one-release alias (#3049, #3129).
#
# It ran a Crusty re-review before each merge-ready round.
# The shared goal loop replaced it: autodrive_pr_goal.sh act review, which the loop's agent chooses when the head needs a review.
# This file stays for one release so that a run with an older pinned bundle,
# or a caller that still names it, keeps working. It prints one plain line
# naming the replacement and does nothing else: it brings back no stop, no
# budget and no gate. Sourced or run, it ends with status 0.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.

echo "autodrive_crusty_rereview.sh is retired (#3129) and does nothing here; its replacement is autodrive_pr_goal.sh act review." >&2
return 0 2>/dev/null || exit 0
