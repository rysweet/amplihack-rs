#!/usr/bin/env bash
# autodrive_progress.sh: the no-progress rule of auto-drive (#1743), decided in
# code. docs/reference/auto-drive-to-merge.md#the-no-progress-rule.
#
#   record <state> <loop> <label> <sha> <clean 0|1|2> <findings> [<ci_signal>]
#       appends one row to <state>/<loop>-progress.tsv:
#       round_label, head_sha, clean (1 clean, 0 not clean, 2 failed: the
#       round exited non-zero or left no readable verdict), open items joined by ',' (cut for
#       display), ci_signal, sha256 of the full sorted open-item set
#   check <state> <loop> <label> <sha> <findings> [<ci_signal>]
#       compares the current round (not clean) with the last row. Exits 10,
#       prints the reason and writes <state>/<loop>-stuck-reason.txt only when
#       the last row is 0 (not clean and not failed), both SHAs are non-empty and equal, the open
#       items are equal (by the sha256 of the full set; by the cut text for a
#       row with no hash), the CI signals are equal and neither says pending
#       (CI still running is not a stop). Every other case exits 0, so the evaluator decides; an
#       untrusted progress file is not read (WARNING, exit 0: fails open).
#   last <state> <loop>
#       prints the label of the last row, or nothing; exit 3 when the
#       progress file is untrusted.
#
# Exit 2 is a usage error. Executed, never sourced. Nothing here reads state
# as code: rows are split with cut and compared as strings.

set -uo pipefail
umask 077
export LC_ALL=C

usage() { echo "ERROR: autodrive_progress.sh: ${1:-usage}. Usage: record <state> <loop> <label> <sha> <clean> <findings> [<ci_signal>] | check <state> <loop> <label> <sha> <findings> [<ci_signal>] | last <state> <loop>" >&2; exit 2; }

CMD="${1:-}"
[ $# -gt 0 ] && shift
case "$CMD" in
  record) [ $# -eq 6 ] || [ $# -eq 7 ] || usage "record takes 6 or 7 arguments" ;;
  check) [ $# -eq 5 ] || [ $# -eq 6 ] || usage "check takes 5 or 6 arguments" ;;
  last) [ $# -eq 2 ] || usage "last takes 2 arguments" ;;
  *) usage "unknown command '${CMD}'" ;;
esac
STATE="$1"; LOOP="$(printf '%s' "$2" | tr -c 'A-Za-z0-9._-' '_')"
[ -d "$STATE" ] && [ ! -L "$STATE" ] || usage "state dir '${STATE}' is not a directory"
[ -n "$LOOP" ] || usage "empty loop name"
FILE="${STATE}/${LOOP}-progress.tsv"

safe_label() { printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_'; }
safe_ci() { printf '%s' "$1" | tr -c 'A-Za-z0-9:/._-' '_' | cut -b 1-64; }
safe_sha() { # a 40- or 64-digit lowercase hex SHA, or nothing
  case "$1" in *[!0-9a-f]*|'') return 0 ;; esac
  case "${#1}" in 40|64) printf '%s' "$1" ;; esac
}

# trusted <file>: absent, or a regular file of ours with no group/other access.
trusted() {
  [ -e "$1" ] || [ -L "$1" ] || return 0
  [ -f "$1" ] && [ ! -L "$1" ] && [ -O "$1" ] || return 1
  local out # a find error is untrusted, never "no match" (fails closed)
  out="$(find "$1" -maxdepth 0 \( -perm -0040 -o -perm -0020 -o -perm -0004 -o -perm -0002 \) -print)" || return 1
  [ -z "$out" ]
}

# full_items <findings>: trimmed, non-blank, separators and control characters
# as '_', each cut to 200 bytes, sorted unique, joined by ','.
full_items() {
  [ -f "$1" ] && [ -r "$1" ] || return 0
  tr -d '\r' < "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//' | grep -v '^$' \
    | tr ',\t\001-\010\013-\037\177' '_' | cut -b 1-200 | sort -u | paste -sd, -
}
# items <joined>: the display copy, at most 3,700 bytes, so a whole row (label,
# SHA, clean flag, items, CI signal, hash) stays within 4 KiB.
items() {
  if [ "$(printf '%s' "$1" | wc -c)" -gt 3700 ]; then printf '%s…(truncated)' "$(printf '%s' "$1" | head -c 3684)"; else printf '%s' "$1"; fi
}
# items_hash <joined>: sha256 of the full set, or nothing when no tool exists.
items_hash() {
  local h
  if command -v sha256sum >/dev/null 2>&1; then h="$(printf '%s' "$1" | sha256sum)"
  elif command -v shasum >/dev/null 2>&1; then h="$(printf '%s' "$1" | shasum -a 256)"
  else echo "WARNING: no sha256sum or shasum; open items are compared by their shortened text." >&2; return 0; fi
  safe_sha "${h%% *}"
}

# A failed `git rev-parse HEAD` in the loop gives an empty SHA; say so.
no_sha_warning() {
  [ -n "$SHA" ] || echo "WARNING: no valid HEAD SHA for ${LABEL}; the no-progress rule is off for this round." >&2
}

last_row() { [ -s "$FILE" ] && tail -n 1 -- "$FILE"; }

if ! trusted "$FILE"; then
  echo "WARNING: ${FILE} is a symlink, not a regular file of this user, or readable by others; the no-progress rule does not read or write it, and the evaluator decides." >&2
  case "$CMD" in record) exit 1 ;; last) exit 3 ;; esac
  exit 0
fi

case "$CMD" in
  last)
    ROW="$(last_row)" || exit 0
    L="$(printf '%s' "$ROW" | cut -f1)"
    [ "$L" = "$(safe_label "$L")" ] && printf '%s\n' "$L"
    exit 0 ;;
  record)
    LABEL="$(safe_label "$3")"; SHA="$(safe_sha "$4")"
    case "$5" in 0|1|2) ;; *) usage "clean must be 0, 1 or 2, not '$5'" ;; esac
    no_sha_warning
    ALL="$(full_items "$6")"
    ROW="$(printf '%s\t%s\t%s\t%s\t%s\t%s' "$LABEL" "$SHA" "$5" "$(items "$ALL")" "$(safe_ci "${7:-}")" "$(items_hash "$ALL")")"
    # One write on an O_APPEND descriptor, under flock where it exists.
    if { if command -v flock >/dev/null 2>&1; then flock -w 10 9 || echo "WARNING: lock on ${FILE} not taken in 10 s; appending without it." >&2; fi; printf '%s\n' "$ROW" >&9; } 9>>"$FILE"; then
      exit 0
    fi
    echo "WARNING: could not append the ${LABEL} row to ${FILE}." >&2
    exit 1 ;;
esac

# Compatibility operation: observations never decide convergence (#2333).
exit 0
