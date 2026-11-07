# shellcheck shell=bash
# autodrive_followups_check.sh - the `check` and `gate` commands of
# autodrive_followups.sh (issue #1979). Sourced by autodrive_followups.sh after
# its arguments are parsed and its helpers are defined; never run on its own.
# It uses STATE, PR, TSV, SET_FILE, ERR_FILE, PARENT_FILE, JQ_MISSING and the
# helpers state_ok, safe_file, norm_set, rows_ok and filed_ids from there.
#
# check  No network. Prints `followups-ok <count>` (exit 0) or one failing
#        token (exit 1): followup-check-unavailable (no jq),
#        followup-state-unsafe, followup-error-recorded, followup-set-invalid,
#        followup-row-malformed, followup-set-empty (the crusty verdict is
#        MINOR_ONLY and the set is empty), followup-missing:<id>, or
#        followup-parent-uncommented (the task is an issue and no comment
#        listing the follow-ups was recorded on it in followups.parent).
# gate   The merge gate's call: check, then one sanitised evidence line
#        (`followups-ok <n>` and ` <id>=#<issue>` per followups.tsv row,
#        exit 0), or `ERROR: followups-unfiled: <token>` on stderr and the
#        gate's blocker sentence on stdout (exit 1).

cmd_check() {
  local n fset filed missing
  say() { printf '%s\n' "$1"; exit "${2:-1}"; }
  [ -z "${JQ_MISSING:-}" ] || say followup-check-unavailable
  state_ok || say followup-state-unsafe
  for n in followups.jsonl followups.tsv followups.error followups.parent crusty-latest.json; do
    safe_file "$STATE/$n" || say followup-state-unsafe
  done
  [ ! -e "$ERR_FILE" ] || say followup-error-recorded
  fset="$(norm_set "$SET_FILE")" || say followup-set-invalid
  rows_ok || say followup-row-malformed
  n="$(printf '%s' "$fset" | jq length)"
  if [ "$n" = 0 ]; then
    if [ -f "$STATE/crusty-latest.json" ] \
       && grep -q '^{"crusty_verdict":"MINOR_ONLY",' "$STATE/crusty-latest.json"; then
      say followup-set-empty
    fi
    say "followups-ok 0" 0
  fi
  filed="$(filed_ids)"
  missing="$(printf '%s' "$fset" | jq -r --argjson d "$filed" \
    'first(.[] | select(.id as $i | $d | index([$i]) | not) | .id) // empty')"
  [ -z "$missing" ] || say "followup-missing:${missing}"
  parent_commented || say followup-parent-uncommented
  say "followups-ok ${n}" 0
}

# gate: cmd_check runs in a subshell, so its exit ends only that subshell.
cmd_gate() {
  local out rc rows
  out="$(cmd_check)"; rc=$?
  out="$(printf '%s' "$out" | head -n 1 | LC_ALL=C tr -cd 'A-Za-z0-9:._ -' | cut -c 1-200)"
  if [ "$rc" -eq 0 ] && [ -n "$out" ]; then
    rows=""
    if [ -e "$TSV" ]; then
      rows="$(LC_ALL=C awk -F '\t' 'NF { printf " %s=#%s", $1, $2 }' "$TSV" \
        | LC_ALL=C tr -cd 'A-Za-z0-9._=# -' | cut -c 1-2000)"
    fi
    printf '%s%s\n' "$out" "$rows"
    exit 0
  fi
  out="${out:-followup-check-failed}"
  echo "ERROR: followups-unfiled: ${out}" >&2
  printf 'follow-up issues not filed for every minor finding (%s); the drive is not READY (#1979)\n' "$out"
  exit 1
}
