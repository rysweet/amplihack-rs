# shellcheck shell=bash
# Mechanical round preservation and existing accounting inputs, sourced by
# autodrive_loop.sh after checking the child status and review directory.
# --- read the round's STRUCTURED record -----------------------------------
# A missing or unparseable record is never clean; it is evidence of a round that produced nothing.
RAW=""
[ -f "$RECORD" ] && RAW="$(cat "$RECORD")"
ROUND_VERDICT="$(field "$RAW" "$VERDICT_FIELD" "MISSING")"
case "$ROUND_VERDICT" in
  "$CLEAN_TOKEN"|"${ALSO_CLEAN:-$CLEAN_TOKEN}") ROUND_CLEAN="true" ;;
  MISSING) ROUND_CLEAN="false"
    echo "WARNING: ${ROUND_LABEL} produced no parseable '${VERDICT_FIELD}'; treated as NOT clean." >&2 ;;
  *) ROUND_CLEAN="false" ;;
esac
# Only A-Za-z0-9_ of the verdict reaches the evaluator, history and #1597 messages; a non-clean one that sanitises to the token ('CLEAN!') is tagged _NONCANONICAL so none of them reads it as clean.
VERDICT_SAFE="$(printf '%s' "$ROUND_VERDICT" | LC_ALL=C tr -cd 'A-Za-z0-9_')"; [ "$ROUND_CLEAN" = "true" ] || [ "$VERDICT_SAFE" != "$TOKEN_SAFE" ] || VERDICT_SAFE="${VERDICT_SAFE}_NONCANONICAL"
# Preserve the real verdict in history. The generic evaluator path normalizes
# clean tokens; Crusty dispatch uses the original verdict and completion decision.
EVAL_VERDICT="$VERDICT_SAFE"; [ "$ROUND_CLEAN" != "true" ] || EVAL_VERDICT="$TOKEN_SAFE"
FINDINGS=""
[ -f "${RECORD}.findings" ] && FINDINGS="$(cat "${RECORD}.findings")"
# #2575: the merge-ready round's write-up and assessed head, for the next
# round's prior_writeup, finalisation's expected head and the hand-off.
if [ "$LOOP_NAME" = merge-ready ]; then
  # A round that wrote no write-up or head leaves none: no earlier assessment
  # reaches the next round or the hand-off, and finalisation refuses with no head.
  rm -f -- "${STATE_DIR}/merge-ready-latest.writeup" "${STATE_DIR}/merge-ready-latest.head"
  copy_private "${RECORD%.json}.writeup" "${STATE_DIR}/merge-ready-latest.writeup"
  copy_private "${RECORD%.json}.head" "${STATE_DIR}/merge-ready-latest.head"
fi
# Stable path to the latest round record, so a later gate binds evidence to the round that made it.
copy_private "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"
copy_private "${RECORD}.findings" "${STATE_DIR}/${LOOP_NAME}-latest.json.findings"
# #1517: <loop>-records.tsv gets one row per round THIS loop wrote (label, record, git blob hash)
# before any later agent runs. autodrive_crusty_final trusts a record only
# when the last row names it and the hash matches (git from the state dir, GIT_DIR/GIT_WORK_TREE
# unset, as autodrive_blob_hash). A failed check: WARNING, no row.
if [ -f "$RECORD" ]; then
  MANIFEST_FILE="${RECORD##*/}"; MANIFEST_HASH=""
  case "$MANIFEST_FILE" in
    .*|*[!A-Za-z0-9._-]*|'')
      echo "WARNING: round record name '${MANIFEST_FILE}' is not a plain file name; no manifest row for ${ROUND_LABEL}." >&2 ;;
    *)
      if ! cmp -s "$RECORD" "${STATE_DIR}/${LOOP_NAME}-latest.json"; then
        echo "WARNING: ${RECORD} and its ${LOOP_NAME}-latest.json copy differ; no manifest row for ${ROUND_LABEL}." >&2
      else
        MANIFEST_HASH="$( (cd -- "$STATE_DIR" && env -u GIT_DIR -u GIT_WORK_TREE git hash-object --no-filters --stdin) < "$RECORD" 2>/dev/null)"
        case "$MANIFEST_HASH" in *[!0-9a-f]*) MANIFEST_HASH="" ;; esac
        case "${#MANIFEST_HASH}" in 40|64) ;; *) MANIFEST_HASH="" ;; esac
        [ -n "$MANIFEST_HASH" ] || echo "WARNING: git could not hash ${RECORD}; no manifest row for ${ROUND_LABEL}, so this round cannot count as loop-written evidence." >&2
      fi ;;
  esac
  if [ -n "$MANIFEST_HASH" ]; then
    ( umask 077
      printf '%s\t%s\t%s\n' "$(printf '%s' "$ROUND_LABEL" | tr -c 'A-Za-z0-9._-' '_')" "$MANIFEST_FILE" "$MANIFEST_HASH" \
        >> "${STATE_DIR}/${LOOP_NAME}-records.tsv" ) \
      || echo "WARNING: could not append the manifest row for ${ROUND_LABEL} to ${LOOP_NAME}-records.tsv." >&2
  fi
fi
