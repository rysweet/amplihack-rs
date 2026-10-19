#!/usr/bin/env bash
# Build the step-15 commit subject and body from the staged index (#1575).
#
# Step 15 used the task description as the commit subject, which put prompt
# text into git history and said nothing about what was committed. The subject
# and body now describe the staged change itself. This helper never reads the
# task description.
#
#   subject <prefix>  '<prefix>update <path>' for one staged file, otherwise
#                     '<prefix>update <N> files in <d1>[, <d2>[, <d3>]][, …]'
#                     (top-level directories, '.' for the root). Control
#                     characters removed, cut to 72 characters.
#   body              the staged `--stat`, at most 30 lines, then
#                     '… (stat truncated)' when it was longer.
#
# Prints nothing for an empty index or outside a git repository, and always
# exits 0, so callers under `set -euo pipefail` decide the fallback.

set -u
# Character (not byte) semantics for the 72-character cut. An unknown locale
# only falls back to byte semantics, so its warning is not worth printing.
{ export LC_ALL=C.UTF-8; } 2>/dev/null

MAX_SUBJECT=72
MAX_STAT_LINES=30

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || exit 0

strip_controls() {
  local s
  s="$(printf '%s' "$1" | LC_ALL=C tr -d '\000-\037\177')"
  printf '%s' "${s//[[:cntrl:]]/}"
}

subject() {
  local prefix="${1:-}" files=() dirs=() f d s list
  mapfile -d '' -t files < <(git -c core.quotePath=false diff --cached --name-only -z --no-renames 2>/dev/null)
  [ ${#files[@]} -gt 0 ] || return 0
  if [ ${#files[@]} -eq 1 ]; then
    s="${prefix}update ${files[0]}"
  else
    for f in "${files[@]}"; do
      case "$f" in */*) d="${f%%/*}" ;; *) d="." ;; esac
      strip_controls "$d"
      printf '\n'
    done | LC_ALL=C sort -u >"$scratch"
    mapfile -t dirs <"$scratch"
    list="${dirs[0]}"
    [ ${#dirs[@]} -gt 1 ] && list+=", ${dirs[1]}"
    [ ${#dirs[@]} -gt 2 ] && list+=", ${dirs[2]}"
    [ ${#dirs[@]} -gt 3 ] && list+=", …"
    s="${prefix}update ${#files[@]} files in $list"
  fi
  s="$(strip_controls "$s")"
  printf '%s\n' "${s:0:MAX_SUBJECT}"
}

body() {
  local stat lines
  stat="$(git -c core.quotePath=true diff --cached --stat=72 --no-color --no-ext-diff 2>/dev/null)" || return 0
  [ -n "$stat" ] || return 0
  lines="$(printf '%s\n' "$stat" | wc -l)"
  printf '%s\n' "$stat" | head -n "$MAX_STAT_LINES"
  [ "$lines" -gt "$MAX_STAT_LINES" ] && printf '%s\n' "… (stat truncated)"
  return 0
}

scratch="$(mktemp -t commit-message-XXXXXX 2>/dev/null)" || exit 0
trap 'rm -f "$scratch"' EXIT
case "${1:-}" in
  subject) subject "${2:-}" ;;
  body) body ;;
  *) echo "usage: $(basename "$0") subject <prefix> | body" >&2 ;;
esac
exit 0
