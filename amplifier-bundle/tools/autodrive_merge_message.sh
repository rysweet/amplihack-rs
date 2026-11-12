#!/usr/bin/env bash
# autodrive_merge_message.sh — the squash commit message for the merge gate.
#
# `main` squashes with `squash_merge_commit_message: COMMIT_MESSAGES`, so a
# merge that supplies no message gets every commit on the branch concatenated
# into one. That drags in each commit's trailers, a `Fixes #N` or `Task:` line
# written for a branch this one is stacked on, and anything else that was only
# ever true of a single commit. A wrong trailer on a pushed commit cannot be
# corrected afterwards: correcting it needs a force-push, and this workflow
# does not rewrite pushed commits. So the gate supplies the message itself,
# from the pull request's title and body; what GitHub adds to it is not seen here.
#
# This file only DEFINES functions; sourcing it has no side effects, so the
# gate may source it twice. It is sourced by autodrive_merge_gate.sh from
# GATE_HOME and so runs inside the process the #1551 `gh` shim trusts to
# merge: it is a GATE_FILES entry, it reads nothing a pull request could
# write, and it runs no command that could merge. Sourcing adds no process
# boundary, so the shim still sees the gate as the parent of the merge.
#
# The body reaches `gh` on standard input, from the gate's own memory: the
# merge argv ends `--body-file -`, and the gate pipes MERGE_BODY into it. It is
# never handed over as a path. A file in the state directory can be rewritten
# by a process running as this same user (#1517 D5) at any moment between the
# last check the gate makes on it and `gh` opening it, and no check made by
# path can close that window (CWE-367). Bytes held in the gate's own variable
# are not reachable that way.
#
# One function here DOES write: autodrive_merge_record writes a copy of the
# body under the state directory, as the record of what this run commits. That
# file is never read back — not by the gate, not by `gh` — so rewriting it
# changes the record and nothing else. It is still written privately, because
# it is the pull request's text and it lives at a deterministic name: see that
# function. It chooses no path and emits no blocker; the gate owns where state
# lives and what refusal means.
#
# It exists as a second file for the reason autodrive_followups_check.sh does:
# the gate stands at the 400-line brick budget and no tool may exceed it
# (issue #2056). What stays in the gate is the argv, its self-check, and the
# blocker calls — the decisions. Composing the message is this module's whole
# contract, subject and body alike.

# autodrive_merge_sha12 <head-sha>: the head SHA's first 12 characters,
# printed only when they are 12 lowercase hex digits.
#
# The gate builds the record's file name from this, so a value that is not a
# SHA must never become a path: `..` would leave the private state directory.
# Section 0's own check on HEAD_SHA is `7 hex followed by anything`, which is
# weaker than what a path needs.
autodrive_merge_sha12() {
  local sha12="${1:0:12}"
  [ "${#sha12}" -eq 12 ] || return 1
  case "$sha12" in *[!0123456789abcdef]*) return 1 ;; esac
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

# autodrive_merge_body <pr-view-json>: set MERGE_BODY to the pull request body,
# byte for byte, with NUL removed. 0 — set. 1 — the body is unreadable, and
# MERGE_BODY is left empty.
#
# It sets a variable rather than printing, because the caller would have to
# capture what it printed with `$(...)`, which strips every trailing newline.
#
# An absent `body` key is NOT an empty body: it is a read that came back
# malformed, and a criterion the gate cannot read never merges. JSON null IS
# an empty body — that is how `gh` reports a pull request with no description.
#
# The 65,536-byte bound is GitHub's own limit on a pull request body, and it
# is here for the reason the 1024-character bound is on the title: past it the
# read went wrong, and the gate should not depend on a platform cap it does
# not enforce itself. It is measured on the JSON string, before NUL removal.
#
# `jq -j`, never `jq -r`: -r appends a newline the body does not contain, so
# every squash commit would gain a spurious blank line. NUL is removed because
# neither git nor a bash variable can carry it. Nothing else is removed — the
# body is the author's text and lands verbatim, trailing newline included: the
# `x` appended inside the capture is what keeps `$(...)` from stripping it.
#
# jq's own exit status decides, under pipefail, and the `x` is only appended
# when the whole pipeline succeeded. A jq that fails part-way therefore leaves
# MERGE_BODY empty and returns 1, which the gate blocks on — it never becomes
# a short body that `gh` would commit. That is why the gate does not pipe jq
# straight into `gh`: there, a jq failure would hand `gh` an empty body.
autodrive_merge_body() {
  local json="${1:-}" body
  MERGE_BODY=""
  printf '%s' "$json" | jq -e '
      has("body") and ((.body | type) as $t | ($t == "string" or $t == "null"))
      and (((.body // "") | utf8bytelength) <= 65536)' >/dev/null 2>&1 || return 1
  body="$(set -o pipefail; printf '%s' "$json" | jq -j '.body // ""' 2>/dev/null | tr -d '\000' && printf x)" \
    || return 1
  # shellcheck disable=SC2034 # read by autodrive_merge_gate.sh, which sources this file
  MERGE_BODY="${body%x}"
}

# autodrive_merge_record <body> <path>: write <body> to <path>, byte for byte,
# as the record of what this run commits. 0 — written. 1 — not written.
#
# The record is not an input. The gate pipes the body to `gh` from its own
# variable, and neither reads this file back, so a process that rewrites it
# afterwards changes what the record says and nothing that lands on the base
# branch. The commit GitHub creates is the authoritative record; this one
# exists so that what the gate would commit can be read off disk on a dry run
# and on a run that ends NOT_MERGED.
#
# The write is `umask 077; set -C; rm -f`, and that still matters with the
# file out of the trust path. The name is deterministic and the gate runs as
# the user whose files a planted symlink would point at, so following one
# would write a pull request body over an arbitrary file of theirs: the
# unconditional rm removes a symlink or a stale record from an earlier round,
# O_EXCL (set -C) refuses anything that appears at the path after it, and
# mode 0600 whatever the caller's umask keeps the author's text private on a
# shared host.
autodrive_merge_record() {
  local body="${1-}" path="${2:-}"
  [ -n "$path" ] || return 1
  ( umask 077; set -C; rm -f -- "$path"
    printf '%s' "$body" > "$path" ) 2>/dev/null
}
