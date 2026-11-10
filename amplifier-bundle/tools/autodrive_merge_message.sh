#!/usr/bin/env bash
# autodrive_merge_message.sh — the squash commit message for the merge gate.
#
# `main` squashes with `squash_merge_commit_message: COMMIT_MESSAGES`, so a
# merge that supplies no message gets every commit on the branch concatenated
# into one. That drags in each commit's trailers, a `Fixes #N` or `Task:` line
# written for a branch this one is stacked on, and anything else that was only
# ever true of a single commit. A wrong trailer on a pushed commit cannot be
# corrected afterwards: correcting it needs a force-push, and this workflow
# does not rewrite pushed commits. So the gate supplies the message itself —
# what reviewers read on the pull request is what lands on the base branch.
#
# This file only DEFINES functions; sourcing it has no side effects, so the
# gate may source it twice. It is sourced by autodrive_merge_gate.sh from
# GATE_HOME and so runs inside the process the #1551 `gh` shim trusts to
# merge: it is a GATE_FILES entry, it reads nothing a pull request could
# write, and it runs no command that could merge. Sourcing adds no process
# boundary, so the shim still sees the gate as the parent of the merge.
#
# It exists as a second file for the reason autodrive_followups_check.sh does:
# the gate stands at exactly the 400-line brick budget and no tool may exceed
# it (issue #2056). What stays in the gate is what has to — the argv, its
# self-check, the body read and the blocker calls.

# autodrive_merge_sha12 <head-sha>: the head SHA's first 12 characters,
# printed only when they are 12 lowercase hex digits.
#
# The gate builds a file name from this, so a value that is not a SHA must
# never become a path. `-` alone is stdin to `gh`, and `..` would leave the
# private state directory. Section 0's own check on HEAD_SHA is `7 hex
# followed by anything`, which is weaker than what a path needs.
autodrive_merge_sha12() {
  local sha12="${1:0:12}"
  case "$sha12" in
    [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
    *) return 1 ;;
  esac
  printf '%s' "$sha12"
}

# autodrive_merge_subject <pr-view-json> <pr>: the squash commit subject,
# `<title> (#N)`. Returns non-zero — which the gate turns into a BLOCKER —
# when the title is absent, JSON null, not a string, blank once control
# characters are removed, or longer than 1024 characters.
#
# Read with jq, NEVER with the gate's field() helper. field() resolves to
# `extract-json --require-field`, which deliberately selects the LAST JSON
# object carrying the field (#1337, PR #1347). A pull request body is free
# text and in this repository routinely quotes recipe and evidence JSON, so
# that helper is exactly the wrong reader for this input. jq parses the one
# object `gh` returned and nothing quoted inside it. Choosing jq here is a
# security control, not a style.
#
# The 1024-character bound, as `${#title}` counts them, is a sanity bound well
# above GitHub's own 256-character title limit: anything past it is a read
# gone wrong, not a subject.
#
# Control characters (\000-\037 and \177) are removed rather than only CR and
# LF. The subject reaches an operator's terminal through the gate's own
# messages and is written into the evidence bundle, so ESC is an escape
# sequence delivered to a terminal and NUL silently truncates a bash array
# element. Removing them is what stops a title forging a line of that bundle.
#
# The ` (#N)` suffix is GitHub's own convention, and on this repository it is
# literally what the platform would have produced (`squash_merge_commit_title`
# is COMMIT_OR_PR_TITLE). A title already ending with exactly this pull
# request's suffix keeps it, so the suffix is never doubled; a title ending
# with ANOTHER pull request's number still gets its own.
autodrive_merge_subject() {
  local json="${1:-}" pr="${2:-}" title subject
  title="$(printf '%s' "$json" \
    | jq -r 'if (has("title") and (.title | type == "string")) then .title else error end' 2>/dev/null)" \
    || return 1
  [ "${#title}" -le 1024 ] || return 1
  subject="$(printf '%s' "$title" | tr -d '\000-\037\177')"
  case "$subject" in
    *[![:space:]]*) ;;
    *) return 1 ;;
  esac
  case "$subject" in
    *" (#${pr})") printf '%s' "$subject" ;;
    *) printf '%s (#%s)' "$subject" "$pr" ;;
  esac
}
