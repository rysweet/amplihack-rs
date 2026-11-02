#!/usr/bin/env bash
# autodrive_escalate.sh: sourced by autodrive_loop.sh. How the loop stops:
# escalate writes the stop to stderr and its JSON record to stdout, and
# usage_limit_stop tells a child that failed on a Claude usage limit from any
# other failure, so the loop stops as USAGE_LIMIT and never as STUCK (#1801).
# docs/reference/auto-drive-to-merge.md. Defines functions only; the caller
# sets LOOP_NAME, ROUND_LABEL and RECORD.

# log_body <log_file>: the log without input echoes, as terminal_refusal reads
# it. workflow_hook_repair.sh keeps a copy, which a test holds equal to this.
log_body() {
  awk 'NR > 1 { if (prev ~ /^Recipe: / && $0 ~ /^(Run ID|Status): /) { cut = 1; exit } print prev } { prev = $0 } END { if (NR > 0 && !cut) print prev }' "$1" 2>/dev/null \
    | grep -vE '^[[:space:]]*(\[[0-9]{2}:[0-9]{2}:[0-9]{2}\] \[amplihack:|amplihack\.recipe\.log_pointer \{)'
}

# usage_limit_failure <rc> <log>: a non-zero exit and a column-0 failure_class
# marker in log_body whose top-level class is usage_limit (#1801). jq when
# installed, else anchored to the runner's alphabetical key order, so a class
# nested in evidence cannot match; docs/reference/auto-drive-to-merge.md.
usage_limit_failure() {
  local m=""
  [ "${1:-0}" != "0" ] && [ -f "${2:-}" ] || return 1
  m="$(log_body "$2" | grep -E '^amplihack\.recipe\.failure_class \{')"
  [ -n "$m" ] || return 1
  if command -v jq >/dev/null 2>&1; then
    m="$(printf '%s\n' "$m" | jq -R -r 'ltrimstr("amplihack.recipe.failure_class ") | (fromjson? // null) | select(type == "object" and .class == "usage_limit") | .class' 2>/dev/null)"
  else
    m="$(printf '%s\n' "$m" | grep -E '^amplihack\.recipe\.failure_class \{"action":"[a-z_]+","attempt":[0-9]+,"class":"usage_limit"')"
  fi
  [ -n "$m" ]
}
# usage_limit_stop <rc> <log> <reason>: escalate USAGE_LIMIT and exit 1 on one.
usage_limit_stop() { usage_limit_failure "$1" "$2" || return 0; escalate "USAGE_LIMIT" "$3" usage_limit; exit 1; }

escalate() { # escalate <verdict> <why> [usage_limit]
  echo "AUTO_DRIVE_LOOP: ${1} — loop '${LOOP_NAME}' stops at round label '${ROUND_LABEL:-?}'." >&2
  echo "  Reason: ${2}" >&2
  echo "  Round record: ${RECORD:-<none>}" >&2
  if [ "${3:-}" = "usage_limit" ]; then echo "  This is a usage limit, not a judgement about progress. Run again when a login has capacity." >&2
  else echo "  This is a judgement about absence of progress, not an attempt count." >&2; fi
  local f=() v # JSON-escaped; control characters become spaces (#1801)
  for v in "$LOOP_NAME" "$1" "${ROUND_LABEL:-?}" "$2"; do f+=("$(printf '%s' "$v" | tr '\001-\037' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g')"); done
  printf '{"loop":"%s","loop_result":"%s","round_label":"%s","reason":"%s"}\n' "${f[@]}"
}
