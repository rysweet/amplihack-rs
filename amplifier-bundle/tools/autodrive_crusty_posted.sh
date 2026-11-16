#!/usr/bin/env bash
# autodrive_crusty_posted.sh: record that this Crusty round's verdict is on the
# PR (#2575 B1). Step-06 of autodrive-crusty-round.yaml runs it after the round
# record is written. The merge round judges criterion 3 from the PR's verdict
# comment; autodrive_rereview_decision treats a CLEAN or MINOR_ONLY record with
# no matching line here as no record, so a failed post, or a state folder
# written before this change, sends the next merge round to Crusty.
#
# No tool reads PR comments (#2628): the agent step that posts the verdict
# (step-03c) ends with {"verdict_posted":"true|false","head_sha":"<sha>"}. When
# it says true for the head this round reviewed, `<WORD> <reviewed sha>` is
# appended to crusty-verdict-posted.txt next to the round record. Its word
# decides only whether Crusty runs again, never a merge: a wrong "true" leaves
# criterion 3 unmet, as before. A dry run posts nothing; it records the line,
# as a dry run cannot meet criterion 3 from the PR and Crusty would only
# repeat itself. Always exits 0.
set -uo pipefail
RECORD="${AUTODRIVE_ROUND_RECORD:-}"
if [ -z "$RECORD" ] || [ ! -f "$RECORD" ] || [ -L "$RECORD" ]; then
  echo "WARNING: no round record; the posted verdict is not recorded [autodrive_crusty_posted]" >&2
  exit 0
fi
DIR="$(dirname -- "$RECORD")"
{ [ -d "$DIR" ] && [ ! -L "$DIR" ]; } || exit 0
OUT="${DIR%/}/crusty-verdict-posted.txt"
[ ! -L "$OUT" ] || { echo "WARNING: $OUT is a symbolic link; nothing recorded [autodrive_crusty_posted]" >&2; exit 0; }
REC="$(jq -R -c 'fromjson? | select(type == "object")' < "$RECORD" 2>/dev/null | tail -n 1)"
VERDICT="$(printf '%s' "$REC" | jq -r '.crusty_verdict // empty | strings' 2>/dev/null)"
SHA="$(printf '%s' "$REC" | jq -r '.reviewed_head_sha // empty | strings' 2>/dev/null)"
[[ "$SHA" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]] || { echo "WARNING: the round record has no reviewed head; nothing recorded [autodrive_crusty_posted]" >&2; exit 0; }
case "$VERDICT" in CLEAN|MINOR_ONLY) WORD="$VERDICT" ;; *) WORD=BLOCKING ;; esac
record() { ( umask 077; printf '%s %s\n' "$WORD" "$SHA" >> "$OUT" ) || echo "WARNING: could not write $OUT [autodrive_crusty_posted]" >&2; }
if [ "${RECIPE_VAR_autodrive_dry_run:-false}" = true ]; then
  record; exit 0
fi
POSTED="$(printf '%s\n' "${RECIPE_VAR_crusty_verdict_posted:-}" \
  | jq -R -c 'fromjson? | select(type == "object" and has("verdict_posted"))' 2>/dev/null | tail -n 1)"
SAID="$(printf '%s' "$POSTED" | jq -r '.verdict_posted // empty | strings' 2>/dev/null)"
AT="$(printf '%s' "$POSTED" | jq -r '.head_sha // empty | strings' 2>/dev/null)"
if [ "$SAID" = true ] && [ "$AT" = "$SHA" ]; then
  record
else
  echo "WARNING: the verdict post for ${SHA} was not reported done; the next merge round runs Crusty [autodrive_crusty_posted]" >&2
fi
exit 0
