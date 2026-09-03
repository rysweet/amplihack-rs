#!/usr/bin/env bash
# workflow_branch_name.sh — decide the branch name a workflow run should use.
#
# ISSUE #1426. `workflow-worktree.yaml` derived its branch name — and therefore
# its worktree DIRECTORY name — by slugifying the first 50 characters of
# `task_description`. A task whose opening line was
#
#     Repository: /Users/ryan/src/mistt-qa/ws/142/jamestown (GitHub mistt-repo/jamestown).
#
# produced the ref
#
#     feat/issue-142-repository-usersryansrcmistt-qaws142jamestown-gith
#
# — sixty-five characters of path-mangled prose, cut mid-word at "gith", and
# created as a SECOND branch alongside `fix/142-band-edge-previous-slice`, which
# the same task had explicitly pinned and told the run not to branch away from.
# The same shape produced `feat/issue-1277-skip-workflow-launch-this-agent-is-
# already-executi`: a truncated prompt fragment leaked into a git ref.
#
# Two rules follow. This helper implements the FIRST one:
#
#   `explicit`  An explicitly NAMED branch wins. A task that says
#               `Branch: <ref>` (or `BRANCH — already created …:` with the ref on
#               the next line) has already answered the question; the caller uses
#               that answer verbatim and never derives a competitor.
#
# The second rule — absent an explicit branch, key the name to the ISSUE NUMBER
# with a bounded, word-boundary-truncated tail — is implemented INLINE in
# step-04-setup-worktree and deliberately NOT here. It is load-bearing: the phase
# bricks are executed with no amplifier-bundle on disk by design (see
# amplifier-bundle/recipes/tests/), so a derived name living behind an optional
# helper would change with the environment, and a re-run would fail to recognise
# the worktree its predecessor registered (issue #1121). Detection of an explicit
# branch is different in kind: when this file is absent the caller simply does not
# detect one, which is pre-#1426 behaviour, never a DIFFERENT name.
#
# The task text is read from the TASK_DESCRIPTION *environment variable*, never
# from argv. On Linux a single argument is capped at MAX_ARG_STRLEN (128 KB), so
# a 150 KB task description passed as an argument would die with E2BIG before
# this script ever ran; and only the first 64 KB is ever examined, so the cost
# does not grow with the description either.
#
# PORTABILITY: bash 3.2 (the system bash on macOS) — no `${VAR,,}`, no `mapfile`,
# no associative arrays. Issue #1423 was exactly this mistake.
#
# `set -e` is deliberately NOT used: this subcommand is best-effort by contract,
# returns its answer on stdout, and prints nothing on stdout when it has no
# answer, so the caller's own ladder stays in charge.
#
# Usage:
#   TASK_DESCRIPTION=... workflow_branch_name.sh explicit [--repo-path PATH]
#
# Direct tests: tests/issue_1426_branch_name_not_prose.sh

set -uo pipefail

# Bound on how much task text is examined. Overridable for tests; the default is
# the contract, and matches the 65536-character bound step-04 applies inline.
MAX_INPUT_BYTES="${AMPLIHACK_BRANCH_INPUT_MAX:-65536}"

# Branches a run must never adopt from prose: "Branch: main" in a task
# description describes the base, never the branch to commit onto.
RESERVED_REFS=" main master develop trunk head "

# NEVER `| head -c`. Bounded with a shell substring instead.
#
# ISSUE #1426 CI FAILURE, and the reason step-04's inline derivation is written the
# same way. `printf '%s' "$TASK" | head -c 65536 | tr … | sed …` looks harmless and
# is not: `head` stops reading once it has its bytes, so with a task larger than the
# 64 KiB pipe buffer the producer is left writing into a closed pipe. It then dies of
# SIGPIPE (status 141), or — where SIGPIPE is ignored, a disposition that survives
# exec and is common in CI — bash's printf reports "write error: Broken pipe" and
# returns 1. `set -o pipefail` promotes either to the pipeline's status, the command
# substitution yields "", and `set -e` kills the step before it emits any JSON.
#
# Reproduction:
#   $ trap '' PIPE; set -euo pipefail
#   $ X="$(printf '%s' "$BIG" | head -c 10)"; echo REACHED
#   bash: printf: write error: Broken pipe        # rc=1, "REACHED" never printed
#
# Whether it fires at a given size is a race on the pipe buffer, so it was green on
# bash 5.3.9 locally and red on the runner's 5.2.21 with a 100 KB task description.
# A branch name is load-bearing: an empty one is worse than an ugly one. The rule
# that follows — no pipeline stage may stop reading before its producer is done.
task_text() { local t="${TASK_DESCRIPTION:-}"; printf '%s' "${t:0:$MAX_INPUT_BYTES}"; }

lower() { printf '%s' "${1:-}" | tr '[:upper:]' '[:lower:]'; }

# ref_is_valid REF — a conservative shape check plus git's own authoritative
# checker. The regex runs first so a name containing shell metacharacters or a
# leading dash never reaches `git` as an argument at all.
ref_is_valid() {
  local ref="${1:-}"
  [ -n "$ref" ] || return 1
  [ "${#ref}" -le 200 ] || return 1
  printf '%s' "$ref" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9._/-]*$' || return 1
  case "$ref" in *..*|*//*|*/) return 1 ;; esac
  if command -v git >/dev/null 2>&1; then
    git check-ref-format --branch "$ref" >/dev/null 2>&1 || return 1
  fi
  return 0
}

# first_token LINE — the first whitespace-delimited token, stripped of the
# decoration a human puts around a branch name in prose: backticks, quotes,
# angle brackets, brackets, and trailing sentence punctuation.
first_token() {
  printf '%s' "${1:-}" \
    | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]].*$//' \
          -e 's/[`"'"'"'<>(){}]//g' -e 's/[][]//g' -e 's/[.,;:]*$//'
}

# candidate_ok REF — would this token be an acceptable explicit branch name?
candidate_ok() {
  local c="${1:-}" lc
  ref_is_valid "$c" || return 1
  # Conservative on purpose. A prose line such as
  #     Branch protection rules: enabled
  # matches the directive shape, and "enabled" must NOT hijack the run into the
  # existing-branch path. Requiring a '/' or a '-' — the shape essentially every
  # real branch name has — rejects that class of false positive. A genuinely
  # single-word branch is still reachable through the `existing_branch` context
  # key, which is unambiguous and needs no guessing.
  case "$c" in *[/-]*) ;; *) return 1 ;; esac
  lc="$(lower "$c")"
  case "$RESERVED_REFS" in *" $lc "*) return 1 ;; esac
  return 0
}

# ---------------------------------------------------------------------------
# `explicit` — find the branch the task NAMES, if it names one.
#
# Recognised shapes (case-insensitive, the directive anchored at line start so
# the word "branch" buried in a sentence never triggers it):
#
#     Branch: fix/142-band-edge-previous-slice
#     BRANCH = fix/142-band-edge-previous-slice
#     Branch name: fix/142-band-edge-previous-slice
#     BRANCH — already created and checked out in this worktree:
#         fix/142-band-edge-previous-slice
#
# The last shape — value on the following line — is the one from the incident.
#
# Prints the branch on stdout, or nothing at all.
#
# EXIT CODE is the "does it already exist?" answer, and only when --repo-path is
# given: 0 when the named branch resolves to a local or origin ref (the caller
# should REUSE it through its existing-branch path), 10 when no branch was named
# or the named branch does not exist yet (the caller should CREATE it under
# exactly that name). Without --repo-path, 0 means "a branch was named".
# ---------------------------------------------------------------------------
cmd_explicit() {
  local line rest cand pending=0 repo=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --repo-path) repo="${2:-}"; shift 2 || break ;;
      *)           shift ;;
    esac
  done
  # `|| [ -n "$line" ]`: a task description with no trailing newline leaves its
  # last (often only) line in $line with read returning non-zero at EOF.
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    # A directive line whose value was empty: the value is the first token of
    # the next non-blank line.
    if [ "$pending" -eq 1 ]; then
      if [ -n "$(printf '%s' "$line" | tr -d '[:space:]')" ]; then
        pending=0
        cand="$(first_token "$line")"
        if candidate_ok "$cand"; then emit_explicit "$cand" "$repo"; return $?; fi
      else
        continue
      fi
    fi
    printf '%s' "$line" \
      | grep -Eqi '^[[:space:]]*branch([[:space:]]+[^:=]*)?[[:space:]]*[:=]' || continue
    rest="${line#*[:=]}"
    cand="$(first_token "$rest")"
    if candidate_ok "$cand"; then emit_explicit "$cand" "$repo"; return $?; fi
    if [ -z "$(printf '%s' "$rest" | tr -d '[:space:]')" ]; then pending=1; fi
  done < <(task_text)
  return 10
}

# emit_explicit REF [REPO] — print REF; exit 0 if it already exists (or if no
# repository was given to ask), 10 if it does not exist yet.
emit_explicit() {
  printf '%s' "$1"
  [ -n "${2:-}" ] || return 0
  git -C "$2" rev-parse --verify --quiet "refs/heads/$1" >/dev/null 2>&1 && return 0
  git -C "$2" rev-parse --verify --quiet "refs/remotes/origin/$1" >/dev/null 2>&1 && return 0
  return 10
}

case "${1:-}" in
  explicit) shift; cmd_explicit "$@" ;;
  *)
    echo "usage: workflow_branch_name.sh explicit [--repo-path PATH]" >&2
    exit 2
    ;;
esac
