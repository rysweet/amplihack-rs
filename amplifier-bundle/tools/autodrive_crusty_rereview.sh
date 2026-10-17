#!/usr/bin/env bash
# autodrive_crusty_rereview.sh — sends commits made after the clean crusty
# round back to crusty, between merge-ready rounds (issue #1517 D4).
#
# The merge-ready loop runs this as its before-round step:
#
#   autodrive_loop.sh --loop-name merge-ready ... \
#     --before-round <tools>/autodrive_crusty_rereview.sh
#
# and autodrive_loop.sh calls it, before every merge round, as
#
#   bash autodrive_crusty_rereview.sh --repo <repo> --state-dir <dir> -c <key=value>...
#
# with the context the merge round gets. Only pr_number, pr_url and
# task_description are read from it.
#
# WHY HERE, AND NOT INSIDE THE MERGE ROUND. Every `amplihack recipe run` starts
# a runner one session level deeper, and every step of that runner, bash or
# agent, runs one level deeper again. Started at depth 0, auto-drive's runner
# is at 1, the loop drivers' bash steps at 2, a round's runner at 3 and the
# round's steps at 4. The recursion guard refuses a `recipe run` issued at
# depth >= AMPLIHACK_MAX_DEPTH, 3 by default, with exit 79, and the
# environment may only lower that limit. A crusty loop started from a merge
# round step would issue its crusty round's `recipe run` at depth 4 and be
# refused (PR #1520 review). This script runs in the merge-ready loop's own
# shell, at depth 2, so the crusty loop it starts runs exactly where phase 2's
# crusty loop runs. It never starts a recipe runner itself.
#
# What it does:
#   1. `autodrive_round_evidence.sh crusty-range` in <repo>: when the crusty
#      loop ended DONE and CLEAN and a commit after the reviewed head is code,
#      it removes the `crusty-loop` marker and answers rereview "true". A
#      base merge or a description or evidence change needs no re-review, and
#      an unreadable range is left to step-01b and the merge gate, which block.
#   2. On "true": the crusty loop, `autodrive_loop.sh --loop-name crusty` on
#      the same state dir. When it ends DONE, autodrive_record_crusty_loop_done
#      writes the resolved concern ids and the marker back.
#
# Prints one JSON line on stdout:
#   {"before_round_result":"<result>","range":"<token>","crusty_loop_result":"<token>"}
# where <result> is one of
#   crusty-rereview-not-needed   no unreviewed code commit; exit 0
#   crusty-rereview-done         the crusty loop ended DONE; exit 0
#   crusty-rereview-not-done     the crusty loop ended STUCK, failed, or was not
#                                recorded; the marker stays absent; exit 1
#   crusty-rereview-refused      the crusty loop returned the terminal policy
#                                refusal; exit 79, never retried
#   crusty-rereview-unavailable  the range could not be measured, or a tool is
#                                missing; exit 1
# Exit 2 is a usage error.
#
# Executed, never sourced. Every tool it uses is taken from beside this file
# and from nowhere else. It writes into the state dir only through
# autodrive_rereview_decision and autodrive_record_crusty_loop_done, both
# under umask 077. Policy: nothing here passes a hook-skipping commit flag or a
# branch-protection bypass; both are NEVER used (see
# docs/reference/auto-drive-to-merge.md#two-absolute-prohibitions).

set -uo pipefail
export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

REPO=""; DIR=""; PR=""; PR_URL_V=""; TASK=""
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="${2:-}"; shift 2 ;;
    --state-dir) DIR="${2:-}"; shift 2 ;;
    -c)
      case "${2:-}" in
        pr_number=*) PR="${2#*=}" ;;
        pr_url=*) PR_URL_V="${2#*=}" ;;
        task_description=*) TASK="${2#*=}" ;;
      esac
      shift 2 ;;
    *) echo "ERROR: autodrive_crusty_rereview.sh: unknown argument '$1'" >&2; exit 2 ;;
  esac
done
[ -n "$REPO" ] && [ -n "$DIR" ] || { echo "ERROR: autodrive_crusty_rereview.sh: --repo and --state-dir are required" >&2; exit 2; }
case "$PR" in *[!0-9]*) PR="" ;; esac

result() { # result <before_round_result> <range> <crusty_loop_result> <exit>
  printf '{"before_round_result":"%s","range":"%s","crusty_loop_result":"%s"}\n' "$1" "$2" "$3"
  exit "$4"
}

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd -P)"
for t in autodrive_round_evidence.sh autodrive_loop.sh autodrive_state.sh autodrive_trust.sh; do
  [ -n "$HERE" ] && [ -f "$HERE/$t" ] && continue
  echo "ERROR: autodrive-tools-not-found: ${t} is not beside autodrive_crusty_rereview.sh (${HERE:-<unknown>})" >&2
  result crusty-rereview-unavailable not-checked "" 1
done
# shellcheck source=/dev/null
. "$HERE/autodrive_state.sh" || result crusty-rereview-unavailable not-checked "" 1

# 1. The range, measured in the repository; the marker is cleared there.
DECISION="$(cd -- "$REPO" 2>/dev/null && AUTODRIVE_STATE_DIR="$DIR" PR_NUMBER="$PR" \
  bash "$HERE/autodrive_round_evidence.sh" crusty-range)" || DECISION=""
case "$DECISION" in
  '{"rereview":"'*) ;;
  *) echo "WARNING: the crusty range gave no decision; crusty is not re-run, and step-01b reports criterion 3." >&2
     result crusty-rereview-unavailable not-checked "" 1 ;;
esac
RANGE="$(printf '%s' "$DECISION" | sed -n 's/.*"range":"\([A-Za-z0-9_-]*\)".*/\1/p')"
case "$DECISION" in
  *'"rereview":"true"'*) ;;
  *) result crusty-rereview-not-needed "${RANGE:-not-checked}" "" 0 ;;
esac

# 2. The crusty loop, in this shell, on the same state dir.
echo "INFO: commits after the clean crusty round need review; running the crusty loop at session depth ${AMPLIHACK_SESSION_DEPTH:-0}, the merge-ready loop's own." >&2
OUT="$(bash "$HERE/autodrive_loop.sh" \
  --loop-name "crusty" \
  --round-recipe "autodrive-crusty-round" \
  --clean-token "CLEAN" \
  --verdict-field "crusty_verdict" \
  --repo "$REPO" \
  --state-dir "$DIR" \
  --context "repo_path=${REPO}" \
  --context "pr_number=${PR}" \
  --context "pr_url=${PR_URL_V}" \
  --context "task_description=${TASK}")"
RC=$?
LOOP_RESULT="$(printf '%s' "$OUT" | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-json --require-field loop_result \
  | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-field --field loop_result --default '' | LC_ALL=C tr -cd 'A-Z_')"
if [ "$RC" -eq 79 ]; then
  echo "ERROR: exit 79 terminal policy refusal in the crusty re-review (#1327/#1332). Final; never retried." >&2
  result crusty-rereview-refused "$RANGE" "${LOOP_RESULT:-TERMINAL_POLICY_REFUSAL}" 79
fi
if [ "$RC" -ne 0 ] || [ "$LOOP_RESULT" != "DONE" ]; then
  echo "ERROR: the crusty re-review ended '${LOOP_RESULT:-unknown}' (rc=${RC}); the crusty-loop marker stays absent and criterion 3 is not met." >&2
  result crusty-rereview-not-done "$RANGE" "$LOOP_RESULT" 1
fi
autodrive_record_crusty_loop_done "$DIR" \
  || { echo "ERROR: could not record the crusty re-review in the state dir." >&2; result crusty-rereview-not-done "$RANGE" DONE 1; }
echo "INFO: the crusty re-review ended DONE; the crusty-loop marker is written back." >&2
result crusty-rereview-done "$RANGE" DONE 0
