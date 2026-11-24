#!/usr/bin/env bash
# Mechanical round-record writer extracted from step-06; no decision policy.
set -uo pipefail
RECORD="${AUTODRIVE_ROUND_RECORD:-}"
[ -n "$RECORD" ] || { echo "ERROR: autodrive_round_record is required; the loop driver reads the round record from a file, never from stdout." >&2; exit 1; }
g() { printf '%s' "${1:-}" | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-json --require-field "$2" \
        | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-field --field "$2" --default "$3"; }
# Whole-value checks: grep -x matches line by line, so a value with an
# embedded newline would pass and split the one-line record.
hex() { case "${#1}" in 40|64) ;; *) return 1 ;; esac; case "$1" in *[!0123456789abcdef]*) return 1 ;; esac; }
digits() { case "$1" in ''|*[!0123456789]*) return 1 ;; esac; }
VERDICT="$(printf '%s' "${RECIPE_VAR_crusty_verdict:-}" \
  | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-json --require-field crusty_verdict \
  | "${AMPLIHACK_BIN:-amplihack}" orch helper extract-field --field crusty_verdict --default CONCERNS)"
case "$VERDICT" in CLEAN|MINOR_ONLY|CONCERNS) ;; *) VERDICT="CONCERNS" ;; esac
COUNT="$(g "${RECIPE_VAR_crusty_verdict:-}" concern_count 0)"
COMMITS="$(g "${RECIPE_VAR_crusty_fix_evidence:-}" commits 0)"
digits "$COUNT" || { echo "WARNING: concern_count '$(printf '%s' "$COUNT" | tr -cd '0-9A-Za-z' | cut -c1-32)' is not a number; recorded as 0" >&2; COUNT=0; }
digits "$COMMITS" || { echo "WARNING: commits '$(printf '%s' "$COMMITS" | tr -cd '0-9A-Za-z' | cut -c1-32)' is not a number; recorded as 0" >&2; COMMITS=0; }
# The reviewed commit is the one the loop measured for itself with
# `git rev-parse` before the round started (autodrive_loop.sh) and passed in as
# reviewed_head_sha. That measurement, never an agent's claim, is what the
# record carries (#2814). Recording it is the whole of it: nothing here
# compares it with the review's own report, and no round is refused over a
# disagreement. Asking the reviewing agent for a review and reading the result
# is what the loop does; reconciling two accounts of the same commit only added
# places for a sound round to be turned away. The merge round judges for itself
# whether the review it relies on covers the PR as it now stands
# (autodrive-merge-round.yaml, criterion 3).
# Only when no measurement reached this step does the round's own report stand
# in, and that is said out loud rather than written into the record's findings.
# head_sha is the actual resulting head, so it is
# never empty (#1517). A missing SHA fails the step: forcing CONCERNS would review the same tree forever.
MEASURED="${RECIPE_VAR_reviewed_head_sha:-}"
if hex "$MEASURED"; then
  REVIEWED="$MEASURED"
else
  REVIEWED="$(g "${RECIPE_VAR_crusty_review:-}" reviewed_head_sha "$(g "${RECIPE_VAR_crusty_round_context:-}" head_sha '')")"
  echo "WARNING: no loop-measured reviewed_head_sha reached this step (got '$(printf '%s' "$MEASURED" | tr -cd '0-9A-Za-z' | cut -c1-64)'), so the recorded head rests on the round's own report." >&2
fi
SHA="$(g "${RECIPE_VAR_crusty_fix_evidence:-}" head_sha '')"
[ -n "$SHA" ] || SHA="$(git -C "${REPO_PATH:-.}" rev-parse HEAD)" || exit 1
if ! hex "$REVIEWED" || ! hex "$SHA"; then
  echo "ERROR: crusty-head-sha-unavailable: reviewed_head_sha='$(printf '%s' "$REVIEWED" | tr -cd '0-9A-Za-z' | cut -c1-64)' head_sha='$(printf '%s' "$SHA" | tr -cd '0-9A-Za-z' | cut -c1-64)'; no round record is written, so this round cannot count as a CLEAN crusty round." >&2
  exit 1
fi
LABEL="$(printf '%s' "${AUTODRIVE_ROUND_LABEL:-round}" | tr -c 'A-Za-z0-9._-' '_')"
# Readiness trusts this record as criterion-3 evidence only when no one else could have written
# it, so it is created new under umask 077, whatever the caller's umask (autodrive_state.sh, PRIVATE STATE).
umask 077
[ -f "${RECORD}.findings" ] || : > "${RECORD}.findings" || { echo "ERROR: cannot write round findings" >&2; exit 1; }
rm -f -- "$RECORD" || exit 1
printf '{"crusty_verdict":"%s","concern_count":%s,"commits_this_round":%s,"head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s","test_signal":"","ci_signal":""}\n' \
  "$VERDICT" "$COUNT" "$COMMITS" "$SHA" "$REVIEWED" "$LABEL" > "$RECORD" || { echo "ERROR: cannot write round record" >&2; exit 1; }
echo "INFO: round record written to ${RECORD}" >&2
cat -- "$RECORD" || exit 1
# Seal driver-owned evidence before the next-action agent receives it.
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ]; then
  "${AMPLIHACK_BIN:-amplihack}" orch helper autodrive-completion-seal --private-state "$AUTODRIVE_COMPLETION_STATE" || exit 1
fi
