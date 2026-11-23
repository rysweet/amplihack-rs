#!/usr/bin/env bash
# autodrive_escalate.sh: a one-release alias (#3049, #3129).
#
# It wrote how the old loop stopped (STUCK, budget and no-progress stops).
# The shared goal loop replaced it: autodrive_goal_loop.sh, which writes loop-end.json. log_body and usage_limit_failure moved to autodrive_refusal.sh; this alias sources that file so its callers keep them.
# This file stays for one release so that a run with an older pinned bundle,
# or a caller that still names it, keeps working. It prints one plain line
# naming the replacement and does nothing else: it brings back no stop, no
# budget and no gate. Sourced or run, it ends with status 0.
# docs/reference/autodrive-goal-loop.md#retired-keys-and-aliases.
case "${BASH_SOURCE[0]}" in */*) _ad_refusal="${BASH_SOURCE[0]%/*}/autodrive_refusal.sh" ;; *) _ad_refusal="autodrive_refusal.sh" ;; esac
# shellcheck source=autodrive_refusal.sh
[ -f "$_ad_refusal" ] && . "$_ad_refusal"
unset _ad_refusal
echo "autodrive_escalate.sh is retired (#3129) and does nothing here; its replacement is autodrive_goal_loop.sh; log_body and usage_limit_failure are in autodrive_refusal.sh." >&2
return 0 2>/dev/null || exit 0
