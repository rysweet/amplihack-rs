#!/usr/bin/env bash
# autodrive_round_word.sh: the merge-ready round's outcome word and round
# files (#2575). docs/reference/autodrive-merge-ready-round.md.
#
# Merge readiness is an agentic step. The merge-ready agent writes up the 13
# criteria for people; a second agent answers, in one word, whether that
# write-up concluded ready, not ready or hand-off. Nothing here reads the
# write-up itself: it is stored as it is, whatever its shape.
#
# This file only DEFINES functions; sourcing it has no other effect. The
# caller sources autodrive_state.sh first (autodrive_private_dir).
#
#   autodrive_round_word <answer>
#       Lower-cases <answer>, keeps only a-z and - (LC_ALL=C) and compares the
#       result exactly. Prints one JSON line:
#         ready      {"word":"ready","merge_ready_verdict":"MERGE_READY"}
#         not-ready  {"word":"not-ready","merge_ready_verdict":"NOT_MERGE_READY"}
#         hand-off   {"word":"hand-off","merge_ready_verdict":"HANDOFF"}
#       Anything else, including an empty answer, is not-ready: an unclear
#       answer costs one more round and never merges or ends the drive.
#       Returns 0, or 1 when jq cannot build the line.
#
#   autodrive_round_file <state_dir> <round_label> <verdict> <assessed_head>
#       Writes, privately, into <state_dir>:
#         merge-ready-<label>.json     {"merge_ready_verdict":"<verdict>"}
#         merge-ready-<label>.writeup  standard input, cut at 256 KiB
#         merge-ready-<label>.head     <assessed_head>
#       The write-up comes from standard input, never from an argument. Each
#       file goes to a temporary file in the same directory under umask 077
#       and is moved into place with mv -f, so a symbolic link planted at the
#       name is replaced, never written through. Returns 0 when written, 2 for
#       a bad argument or a state directory that is not private, 1 when a
#       write fails.

AUTODRIVE_WRITEUP_MAX_BYTES=262144

autodrive_round_word() {
  local -x LC_ALL=C
  local word verdict
  word="$(printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]' | tr -cd 'a-z-')"
  case "$word" in
    ready) verdict=MERGE_READY ;;
    not-ready) verdict=NOT_MERGE_READY ;;
    hand-off) verdict=HANDOFF ;;
    *) word=not-ready; verdict=NOT_MERGE_READY ;;
  esac
  jq -cn --arg w "$word" --arg v "$verdict" '{word:$w,merge_ready_verdict:$v}'
}

# _autodrive_round_put <dir> <name>: standard input to <dir>/<name> through a
# private temporary file and mv -f.
_autodrive_round_put() {
  local tmp
  tmp="$(umask 077 && mktemp "${1}/.round-file.XXXXXX")" || return 1
  if cat > "$tmp" && mv -f -- "$tmp" "${1}/${2}"; then
    return 0
  fi
  rm -f -- "$tmp"
  return 1
}

autodrive_round_file() {
  local -x LC_ALL=C
  local dir="${1:-}" label="${2:-}" verdict="${3:-}" head="${4:-}" base raw size
  if [ "$#" -ne 4 ]; then
    echo "ERROR: autodrive_round_file: expected <state_dir> <round_label> <verdict> <assessed_head>; the write-up is read from standard input" >&2
    return 2
  fi
  if ! [[ "$label" =~ ^[A-Za-z0-9._-]{1,64}$ ]] || [ "$label" = . ] || [ "$label" = .. ]; then
    echo "ERROR: autodrive_round_file: the round label is not 1 to 64 characters of A-Za-z0-9._- (or is . or ..)" >&2
    return 2
  fi
  case "$verdict" in
    MERGE_READY|NOT_MERGE_READY|HANDOFF) ;;
    *) echo "ERROR: autodrive_round_file: the verdict is not MERGE_READY, NOT_MERGE_READY or HANDOFF" >&2; return 2 ;;
  esac
  if ! [[ "$head" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]; then
    echo "ERROR: autodrive_round_file: the assessed head is not 40 or 64 lowercase hex characters" >&2
    return 2
  fi
  if ! declare -F autodrive_private_dir >/dev/null; then
    echo "ERROR: autodrive_round_file: autodrive_state.sh is not sourced, so the state directory cannot be checked" >&2
    return 2
  fi
  autodrive_private_dir "$dir" || return 2
  base="merge-ready-${label}"

  raw="$(umask 077 && mktemp "${dir}/.round-writeup.XXXXXX")" || return 1
  if ! cat > "$raw"; then
    rm -f -- "$raw"
    echo "ERROR: autodrive_round_file: could not read the write-up from standard input" >&2
    return 1
  fi
  size="$(wc -c < "$raw" | tr -cd '0-9')"
  if [ "${size:-0}" -gt "$AUTODRIVE_WRITEUP_MAX_BYTES" ]; then
    { head -c "$AUTODRIVE_WRITEUP_MAX_BYTES" "$raw"
      printf '\n[write-up truncated at 256 KiB by autodrive_round_file; %s bytes were received]\n' "$size"
    } | _autodrive_round_put "$dir" "${base}.writeup"
  else
    _autodrive_round_put "$dir" "${base}.writeup" < "$raw"
  fi || { rm -f -- "$raw"; echo "ERROR: autodrive_round_file: could not write ${base}.writeup" >&2; return 1; }
  rm -f -- "$raw"

  printf '%s\n' "$head" | _autodrive_round_put "$dir" "${base}.head" \
    || { echo "ERROR: autodrive_round_file: could not write ${base}.head" >&2; return 1; }
  jq -cn --arg v "$verdict" '{merge_ready_verdict:$v}' | _autodrive_round_put "$dir" "${base}.json" \
    || { echo "ERROR: autodrive_round_file: could not write ${base}.json" >&2; return 1; }
  return 0
}
