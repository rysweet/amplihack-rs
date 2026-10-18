#!/usr/bin/env bash
# workflow_body_file.sh - write text from stdin to a file for
# `gh ... --body-file` or `git commit -F` (issue #1570).
#
# Usage (sourced or run directly):
#   write_body_file <OUT> [<SOURCE>] < input
#
# Input of at most WORKFLOW_BODY_MAX_BYTES (default 60000) bytes is written
# byte for byte. Larger input keeps its first WORKFLOW_BODY_MAX_BYTES bytes,
# cut back to a UTF-8 character boundary, followed by
#   \n\n[... truncated N bytes; full text in <SOURCE> ...]
# <SOURCE> defaults to "the workflow step output".
#
# Exit codes: 0 written, 1 read or write failure, 2 usage error.
# Needs only POSIX tools (head, od, tail, wc). No eval; the text never
# reaches argv.

# The body runs in a subshell so umask, traps and errexit never leak into a
# caller that sourced this file.
write_body_file() (
  set -u
  if [ "$#" -lt 1 ] || [ "$#" -gt 2 ] || [ -z "${1:-}" ]; then
    printf '%s\n' "usage: write_body_file <OUT> [<SOURCE>] < input" >&2
    exit 2
  fi
  out=$1
  source_name=${2:-the workflow step output}
  max=${WORKFLOW_BODY_MAX_BYTES:-60000}
  case "$max" in
    '' | *[!0-9]*)
      printf '%s\n' "write_body_file: WORKFLOW_BODY_MAX_BYTES must be a non-negative integer" >&2
      exit 2
      ;;
  esac
  max=$((10#$max))

  umask 077
  raw=
  tmp=
  trap 'rm -f -- "$raw" "$tmp"' EXIT
  raw=$(mktemp) || exit 1
  tmp=$(mktemp) || exit 1

  cat > "$raw" || {
    printf '%s\n' "write_body_file: failed to read stdin" >&2
    exit 1
  }
  size=$(wc -c < "$raw" | tr -d ' ') || exit 1

  if [ "$size" -le "$max" ]; then
    cat -- "$raw" > "$tmp" || exit 1
  else
    keep=$max
    # The cut at `keep` lies on a character boundary unless the byte at
    # offset `keep` (the first dropped byte) is a UTF-8 continuation byte
    # (0x80-0xBF). A character is at most four bytes, so at most three
    # steps back are needed.
    steps=0
    while [ "$keep" -gt 0 ] && [ "$steps" -lt 4 ]; do
      # head reads the file itself, so no writer in this pipeline can be
      # killed by SIGPIPE (which pipefail would report as a failure).
      byte=$(head -c "$((keep + 1))" -- "$raw" | tail -c 1 | od -An -tu1 | tr -d ' \n') || exit 1
      if [ -z "$byte" ] || [ "$byte" -lt 128 ] || [ "$byte" -ge 192 ]; then
        break
      fi
      keep=$((keep - 1))
      steps=$((steps + 1))
    done
    dropped=$((size - keep))
    {
      head -c "$keep" -- "$raw" &&
        printf '%s' "

[... truncated ${dropped} bytes; full text in ${source_name} ...]"
    } > "$tmp" || exit 1
  fi

  mv -f -- "$tmp" "$out" || {
    printf '%s\n' "write_body_file: cannot write $out" >&2
    exit 1
  }
  tmp=
  exit 0
)

# Run directly: behave like the function.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  write_body_file "$@"
  exit $?
fi
