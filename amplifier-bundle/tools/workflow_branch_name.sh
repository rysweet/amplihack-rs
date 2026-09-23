#!/usr/bin/env bash
# workflow_branch_name.sh — find the branch a task description explicitly NAMES.
#
# Issue #1426: a workflow run must use the branch its task pins, never invent a
# competitor from prose. This helper answers only "did the task name a branch?".
# The fallback, issue-keyed name is derived INLINE in step-04-setup-worktree,
# because it is load-bearing and must not depend on the bundle being on disk
# (#1121). History: docs/features/branch-name-generation.md.
#
# Usage:
#   TASK_DESCRIPTION=... workflow_branch_name.sh explicit --repo-path REPO --main-repo MAIN
#
#   REPO  the checkout the run was pointed at (the caller checkout).
#   MAIN  the main repository whose worktrees/<branch> the run would use.
#
# Output: the named branch on stdout, or nothing.
# Exit codes:
#   0   a branch is named and already exists (local or origin) — REUSE it
#   10  a branch is named and does not exist yet            — CREATE it
#   1   no branch is named                                  — derive one
#   2   usage error (unknown flag, missing or non-directory path)
#   3   the named branch is checked out in a worktree that is neither REPO nor
#       MAIN/worktrees/<branch>: another session owns it; refuse, never share it
#
# Rules:
#   * Only these directive forms count, case-insensitive, at the start of a line:
#       Branch: <ref>          Branch name: <ref>        Branch ref: <ref>
#       Branch = <ref>         (value may instead be on the next non-blank line)
#       Branch — <any text>:   (value MUST be on the next non-blank line; `—`, `–`
#                               or `-`; this is the form from the #1426 incident)
#     Anything else — "Branch coverage is low in: src/foo-bar.rs" — is prose.
#   * The value must be a valid ref containing `/` or `-`, must not be a base
#     branch (main, master, develop, trunk, head), and must not start with
#     origin/, refs/ or heads/.
#   * Only the first 65536 characters and the first MAX_DIRECTIVES directive
#     lines are examined, so cost is bounded whatever the task contains.
#
# Portable to bash 3.2 (#1423). Input is bounded with a shell substring, never
# `| head -c`: an early-exit pipeline stage under pipefail empties the result.

set -uo pipefail

readonly MAX_INPUT_CHARS=65536
readonly MAX_DIRECTIVES=20
readonly RESERVED_REFS=" main master develop trunk head "

# `[[ =~ ]]` has no case-insensitive flag, hence the spelled-out classes. The
# patterns MUST stay unquoted at the match site, or bash matches them literally.
readonly KEY_RE='^[[:space:]]*[Bb][Rr][Aa][Nn][Cc][Hh]([[:space:]]+([Nn][Aa][Mm][Ee]|[Rr][Ee][Ff]))?[[:space:]]*[:=]'
readonly DASH_RE='^[[:space:]]*[Bb][Rr][Aa][Nn][Cc][Hh][[:space:]]+(—|–|-)[^:=]*:[[:space:]]*$'

usage() {
  echo "usage: TASK_DESCRIPTION=... workflow_branch_name.sh explicit --repo-path REPO --main-repo MAIN" >&2
  exit 2
}

ref_is_valid() {
  local ref="$1"
  [ -n "$ref" ] && [ "${#ref}" -le 200 ] || return 1
  # Shape check first, so nothing with a leading dash or metacharacter reaches git.
  [[ $ref =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] || return 1
  case "$ref" in *..*|*//*|*/) return 1 ;; esac
  git check-ref-format --branch "$ref" >/dev/null 2>&1
}

# First whitespace-delimited token, stripped of quotes, brackets and trailing punctuation.
first_token() {
  printf '%s' "$1" \
    | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]].*$//' \
          -e 's/[`"'"'"'<>(){}]//g' -e 's/[][]//g' -e 's/[.,;:]*$//'
}

candidate_ok() {
  local c="$1" lc
  ref_is_valid "$c" || return 1
  case "$c" in *[/-]*) ;; *) return 1 ;; esac
  lc="$(printf '%s' "$c" | tr '[:upper:]' '[:lower:]')"
  case "$lc" in origin/*|refs/*|heads/*) return 1 ;; esac
  case "$RESERVED_REFS" in *" $lc "*) return 1 ;; esac
}

real_dir() { (cd "$1" 2>/dev/null && pwd -P); }

# decide REF REPO MAIN — print REF and exit with the code documented above.
decide() {
  local ref="$1" repo="$2" main="$3" holder holder_real repo_real intended
  holder="$(git -C "$repo" worktree list --porcelain 2>/dev/null | awk -v b="refs/heads/$ref" '
    $1=="worktree" { wt=substr($0, 10) }
    $1=="branch" && $2==b { print wt; exit }')"
  if [ -n "$holder" ]; then
    holder_real="$(real_dir "$holder" || printf '%s' "$holder")"
    repo_real="$(real_dir "$repo")"
    intended="$(real_dir "$main")/worktrees/$ref"
    # The caller checkout is left to step-04's own #858 refusal; our own
    # worktree from a previous run of this task is an idempotent re-run.
    if [ "$holder_real" != "$repo_real" ] && [ "$holder_real" != "$intended" ]; then
      echo "ERROR: task_description names branch '$ref', but it is checked out in another worktree ('$holder'). Refusing to share another session's branch. Pass it as existing_branch to adopt it deliberately (issue #1426)." >&2
      exit 3
    fi
  fi
  printf '%s' "$ref"
  git -C "$repo" rev-parse --verify --quiet "refs/heads/$ref" >/dev/null 2>&1 && exit 0
  git -C "$repo" rev-parse --verify --quiet "refs/remotes/origin/$ref" >/dev/null 2>&1 && exit 0
  exit 10
}

cmd_explicit() {
  local repo="" main="" line rest cand text pending=0 seen=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --repo-path|--main-repo)
        [ $# -ge 2 ] && [ -n "$2" ] && [ -d "$2" ] || { echo "ERROR: $1 needs an existing directory" >&2; usage; }
        if [ "$1" = --repo-path ]; then repo="$2"; else main="$2"; fi
        shift 2 ;;
      *) echo "ERROR: unknown argument '$1'" >&2; usage ;;
    esac
  done
  [ -n "$repo" ] && [ -n "$main" ] || usage

  text="${TASK_DESCRIPTION:-}"
  text="${text:0:$MAX_INPUT_CHARS}"
  # `|| [ -n "$line" ]`: keep a final line that has no trailing newline.
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    if [ "$pending" -eq 1 ]; then
      # A `case` glob, not `${line#"${line%%[![:space:]]*}"}`: that idiom is
      # quadratic in the length of a whitespace run.
      case "$line" in *[![:space:]]*) ;; *) continue ;; esac
      pending=0
      cand="$(first_token "$line")"
      candidate_ok "$cand" && decide "$cand" "$repo" "$main"
    fi
    if [[ $line =~ $DASH_RE ]]; then
      pending=1
    elif [[ $line =~ $KEY_RE ]]; then
      rest="${line#*[:=]}"
      case "$rest" in
        *[![:space:]]*) cand="$(first_token "$rest")"
                        candidate_ok "$cand" && decide "$cand" "$repo" "$main" ;;
        *)              pending=1 ;;
      esac
    else
      continue
    fi
    seen=$((seen + 1))
    [ "$seen" -lt "$MAX_DIRECTIVES" ] || break
  done <<< "$text"
  exit 1
}

case "${1:-}" in
  explicit) shift; cmd_explicit "$@" ;;
  *) usage ;;
esac
