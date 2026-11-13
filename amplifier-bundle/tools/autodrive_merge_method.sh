#!/usr/bin/env bash
# autodrive_merge_method.sh — which of --merge, --squash and --rebase an
# auto-drive merge uses (#1888), and the `gh pr merge` arguments for it.
#
# The caller is autodrive-finalize's `finalize-observed-action` agent step. It
# sources this file from the trusted launch bundle by its absolute
# $AMPLIHACK_HOME path, never from the pull request's tree, in the same shell
# call that sources autodrive_merge_message.sh and merges. This file only
# DEFINES functions; sourcing it has no side effects, and nothing in it can
# merge.
#
# The method is decided in one place, `amplihack orch helper
# select-merge-method` (crates/amplihack-cli/src/commands/merge_method/): the
# explicit merge_method context; else .amplihack/auto-drive.toml at the base
# branch tip, refusing a pull request whose own changes touch it; else the
# first of squash, merge and rebase that is allowed. Allowed means the
# repository settings allow it AND every ruleset pull_request rule on the base
# branch that lists allowed_merge_methods lists it AND, for merge, no ruleset
# required_linear_history rule is active on the base branch. A method that is
# not allowed is never used and nothing falls back. This file runs that
# selector, checks its output and turns the method into arguments; it chooses
# nothing itself.
# Contract: docs/reference/auto-drive-to-merge.md, "Choosing the merge method".
#
#   autodrive_merge_method <pr-url> <repo-root> <base-ref> <base-sha> <head-sha> <merge_method>
#
# On success sets MERGE_METHOD (merge, squash or rebase), MERGE_FLAG
# (--merge, --squash or --rebase) and MERGE_METHOD_SELECTION (the selector's
# JSON object on one line) and returns 0. Otherwise leaves those three empty,
# sets MERGE_METHOD_ERROR to one printable line, and returns 79 when the
# selector reported a policy refusal from `gh api`, else 1.
#
#   autodrive_merge_argv <pr-number> <verified-head> <subject>
#
# Sets the array MERGE_ARGV to the `gh pr merge` arguments for MERGE_METHOD.
# The body, when there is one, goes in on standard input:
#
#   squash  <N> --squash --match-head-commit <head> --subject <subject> --body-file -
#   merge   <N> --merge --match-head-commit <head> --body-file -
#   rebase  <N> --rebase --match-head-commit <head>
#
# A merge commit takes the body alone: GitHub keeps its own headline
# (merge_commit_title), and the body replaces what merge_commit_message would
# write, which under PR_BODY is the whole pull request body, the merge-ready
# report included. A rebase writes no commit of its own and takes no message.
# Returns 1 with MERGE_ARGV empty when no method was selected, the number is
# not a positive integer, the head is not a full lowercase SHA, or a squash
# has no subject.

# shellcheck disable=SC2034 # the caller reads these after the functions return
autodrive_merge_method() {
  local out="" rc=0 reason=""
  MERGE_METHOD=""; MERGE_FLAG=""; MERGE_METHOD_SELECTION=""; MERGE_METHOD_ERROR=""
  if ! command -v jq >/dev/null 2>&1; then
    MERGE_METHOD_ERROR="merge-method selection failed: jq is not available, so the selector's output cannot be checked"
    return 1
  fi
  out="$("${AMPLIHACK_BIN:-amplihack}" orch helper select-merge-method --pr-url "${1:-}" --repo-root "${2:-}" \
    --base-ref "${3:-}" --base "${4:-}" --head "${5:-}" --merge-method "${6:-}")"
  rc=$?
  if [ "$rc" -ne 0 ]; then
    # The selector prints {"error": reason}: kept to one printable line.
    reason="$(printf '%s' "$out" | jq -ej 'if type == "object" and (.error | type) == "string" then .error else empty end' 2>/dev/null \
      | tr -c '[:print:]' ' ' | cut -c1-400)"
    MERGE_METHOD_ERROR="merge-method selection failed (exit ${rc}): ${reason:-the selector gave no reason}"
    [ "$rc" -eq 79 ] && return 79
    return 1
  fi
  # Exactly one object, every field in its closed set; anything else, prose
  # around it included, is a version mismatch or a broken selector.
  if ! printf '%s' "$out" | jq -e -s 'length == 1 and (.[0] | type == "object"
      and (.method == "merge" or .method == "squash" or .method == "rebase")
      and (.source == "explicit" or .source == "repository" or .source == "default")
      and (.repository | type == "string" and test("^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$"))
      and (.config_commit == null or (.config_commit | type == "string" and test("^([0-9a-f]{40}|[0-9a-f]{64})$")))
      and ((.source == "explicit") == (.config_commit == null))
      and (.rationale | type == "string" and length > 0))' >/dev/null 2>&1; then
    MERGE_METHOD_ERROR="merge-method selection failed: the selector returned malformed or incompatible output"
    return 1
  fi
  MERGE_METHOD_SELECTION="$(printf '%s' "$out" | jq -c .)"
  case "$(printf '%s' "$out" | jq -r .method)" in
    merge) MERGE_METHOD="merge"; MERGE_FLAG="--merge" ;;
    squash) MERGE_METHOD="squash"; MERGE_FLAG="--squash" ;;
    rebase) MERGE_METHOD="rebase"; MERGE_FLAG="--rebase" ;;
  esac
  return 0
}

# shellcheck disable=SC2034 # the caller reads MERGE_ARGV after this returns
autodrive_merge_argv() {
  local pr="${1:-}" head="${2:-}" subject="${3:-}"
  MERGE_ARGV=()
  case "$pr" in '' | 0* | *[!0-9]*) return 1 ;; esac
  case "${#head}" in 40 | 64) ;; *) return 1 ;; esac
  case "$head" in *[!0123456789abcdef]*) return 1 ;; esac
  case "${MERGE_METHOD:-}:${MERGE_FLAG:-}" in
    squash:--squash)
      [ -n "$subject" ] || return 1
      MERGE_ARGV=("$pr" --squash --match-head-commit "$head" --subject "$subject" --body-file -) ;;
    merge:--merge) MERGE_ARGV=("$pr" --merge --match-head-commit "$head" --body-file -) ;;
    rebase:--rebase) MERGE_ARGV=("$pr" --rebase --match-head-commit "$head") ;;
    *) return 1 ;;
  esac
  return 0
}
