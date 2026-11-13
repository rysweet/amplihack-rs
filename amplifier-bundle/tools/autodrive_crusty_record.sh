#!/usr/bin/env bash
# Mechanical round-record writer extracted from step-06; no decision policy.
set -uo pipefail
RECORD="${AUTODRIVE_ROUND_RECORD:-}"
[ -n "$RECORD" ] || { echo "ERROR: autodrive_round_record is required; the loop driver reads the round record from a file, never from stdout." >&2; exit 1; }
g() { printf '%s' "${1:-}" | amplihack orch helper extract-json --require-field "$2" \
        | amplihack orch helper extract-field --field "$2" --default "$3"; }
# Whole-value checks: grep -x matches line by line, so a value with an
# embedded newline would pass and split the one-line record.
hex() { case "${#1}" in 40|64) ;; *) return 1 ;; esac; case "$1" in *[!0123456789abcdef]*) return 1 ;; esac; }
digits() { case "$1" in ''|*[!0123456789]*) return 1 ;; esac; }
VERDICT="$(printf '%s' "${RECIPE_VAR_crusty_verdict:-}" \
  | amplihack orch helper extract-json --require-field crusty_verdict \
  | amplihack orch helper extract-field --field crusty_verdict --default CONCERNS)"
case "$VERDICT" in CLEAN|MINOR_ONLY|CONCERNS) ;; *) VERDICT="CONCERNS" ;; esac
COUNT="$(g "${RECIPE_VAR_crusty_verdict:-}" concern_count 0)"
COMMITS="$(g "${RECIPE_VAR_crusty_fix_evidence:-}" commits 0)"
digits "$COUNT" || { echo "WARNING: concern_count '$(printf '%s' "$COUNT" | tr -cd '0-9A-Za-z' | cut -c1-32)' is not a number; recorded as 0" >&2; COUNT=0; }
digits "$COMMITS" || { echo "WARNING: commits '$(printf '%s' "$COMMITS" | tr -cd '0-9A-Za-z' | cut -c1-32)' is not a number; recorded as 0" >&2; COMMITS=0; }
# The reviewed head comes from bash (step-01: the PR's headRefOid, equal to the local HEAD), never
# from an agent. head_sha is the post-fix head when step-05 ran, otherwise the reviewed head, so it is
# never empty (#1517). A missing SHA fails the step: forcing CONCERNS would review the same tree forever.
REVIEWED="$(g "${RECIPE_VAR_crusty_round_context:-}" head_sha '')"
SHA="$(g "${RECIPE_VAR_crusty_fix_evidence:-}" head_sha '')"
[ -n "$SHA" ] || SHA="$REVIEWED"
if ! hex "$REVIEWED" || ! hex "$SHA"; then
  echo "ERROR: crusty-head-sha-unavailable: reviewed_head_sha='$(printf '%s' "$REVIEWED" | tr -cd '0-9A-Za-z' | cut -c1-64)' head_sha='$(printf '%s' "$SHA" | tr -cd '0-9A-Za-z' | cut -c1-64)'; no round record is written, so this round cannot count as a CLEAN crusty round." >&2
  exit 1
fi
LABEL="$(printf '%s' "${AUTODRIVE_ROUND_LABEL:-round}" | tr -c 'A-Za-z0-9._-' '_')"
# Readiness trusts this record as criterion-3 evidence only when no one else could have written
# it, so it is created new under umask 077, whatever the caller's umask (autodrive_state.sh, PRIVATE STATE).
umask 077
[ -f "${RECORD}.findings" ] || : > "${RECORD}.findings"
rm -f -- "$RECORD"
printf '{"crusty_verdict":"%s","concern_count":%s,"commits_this_round":%s,"head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s","test_signal":"","ci_signal":""}\n' \
  "$VERDICT" "$COUNT" "$COMMITS" "$SHA" "$REVIEWED" "$LABEL" > "$RECORD"
echo "INFO: round record written to ${RECORD}" >&2
# Issue #2096: bind the record to the drive's private review generation
# (generation and reviewed SHA) and print the bound record.
amplihack internal review-bind-record "${REVIEW_DIR:-}" "$RECORD" || exit 1
# Seal driver-owned evidence before the next-action agent receives it.
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ]; then
  amplihack orch helper autodrive-completion-seal --private-state "$AUTODRIVE_COMPLETION_STATE" || exit 1
fi
