#!/usr/bin/env bash
# workflow_finalization_outcome.sh - what the run actually produced (#2343).
#
#   finalization_outcome_facts <target_dir> <branch> [<pr_ref>]
#
# Reports facts, never policy. It always exits 0 and always prints exactly one
# compact JSON object on stdout, so a caller never has to tell "the read
# failed" from "the read says no": an unreadable fact is reported as `unknown`
# and carries a sentence naming what could not be read. The full field list is
# in docs/reference/default-workflow-agentic-finalization.md.
#
# `verdict` is SUCCESS when an open same-repo PR sits at the local head, or at
# a descendant of it, and no check of that PR is failing; BLOCKED when one is;
# GAP when a fact was read and it is negative (nothing published, no PR, or a
# PR at an unrelated head); UNCERTAIN when a fact could not be read. An open PR
# at the local head is itself proof the head is published, so the remote is
# asked directly only when that proof is missing: the healthy path costs no
# extra remote read.
#
# No wait, poll, retry or timer: every state is read exactly once. The rules
# for "is this PR this run's PR" are not restated here; they come from
# _finalization_pr_checks, _finalization_pr_head_descends and
# _finalization_pr_number_valid in workflow_finalization_pr_signal.sh and
# workflow_pr_head_fetch.sh, so the checks cannot drift apart.
#
# The origin URL never appears in the output, in `gap`, or on stderr: the
# remote is reported as owner/repo through parse_github_repo_identity, which
# discards any userinfo. git and gh stderr is discarded for the same reason.
#
# This file only DEFINES functions; sourcing it has no other side effects. It
# must be sourced after workflow_finalization_pr_signal.sh. It knows nothing
# about the validator; turning these facts into a terminal state is
# workflow_finalization_outcome_classify.sh's job. Together they replace the
# open-PR check that lived in workflow_finalization_open_pr.sh, which asked
# whether this run's own bookkeeping permitted it to look, and refused to look
# otherwise.

# A literal object for the one case where the JSON builder itself is missing.
_FINALIZATION_OUTCOME_NO_JQ='{"schema_version":1,"branch":"","local_head":"","remote":"","github_origin":"false","branch_pushed":"unknown","remote_head":"","pr_number":"","pr_url":"","pr_state":"","pr_head":"","pr_head_relation":"unknown","checks":"","verdict":"UNCERTAIN","gap":"the run outcome could not be read because jq is unavailable"}'

# Control characters and runaway length never reach a result field or a log.
_finalization_outcome_clean() {
  LC_ALL=C printf '%s' "$1" | LC_ALL=C tr -d '\001-\037\177' | LC_ALL=C cut -c1-600
}

# Fixed key order; every field arrives through --arg, never concatenation.
_finalization_outcome_emit() {
  jq -cn \
    --arg branch "$1" --arg local_head "$2" --arg remote "$3" \
    --arg github_origin "$4" --arg branch_pushed "$5" --arg remote_head "$6" \
    --arg pr_number "$7" --arg pr_url "$8" --arg pr_state "$9" \
    --arg pr_head "${10}" --arg pr_head_relation "${11}" --arg checks "${12}" \
    --arg verdict "${13}" --arg gap "$(_finalization_outcome_clean "${14}")" \
    '{schema_version:1, branch:$branch, local_head:$local_head, remote:$remote,
      github_origin:$github_origin, branch_pushed:$branch_pushed,
      remote_head:$remote_head, pr_number:$pr_number, pr_url:$pr_url,
      pr_state:$pr_state, pr_head:$pr_head, pr_head_relation:$pr_head_relation,
      checks:$checks, verdict:$verdict, gap:$gap}' 2>/dev/null \
    || printf '%s\n' "$_FINALIZATION_OUTCOME_NO_JQ"
}

# _finalization_outcome_sha <value> -> 0 when <value> is a full commit id.
_finalization_outcome_sha() {
  [[ "${1:-}" =~ ^([0-9a-f]{40}|[0-9a-f]{64})$ ]]
}

# _finalization_outcome_remote_head <dir> <branch> -> the remote branch tip.
# One `git ls-remote` against the remote NAME `origin`, never a URL taken from
# config, so a rewritten or hostile URL cannot redirect the read. The pattern
# is the exact ref path, and the answer is matched on the full ref, so a
# similarly named branch is never mistaken for this one. Prints nothing and
# returns 1 when the remote could not be read at all.
_finalization_outcome_remote_head() {
  local dir="$1" branch="$2" listing="" ssh_cmd="ssh"
  declare -F _finalization_batch_ssh_command >/dev/null 2>&1 \
    && _finalization_batch_ssh_command "$dir" ssh_cmd
  listing="$(GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" git -C "$dir" \
    -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
    ls-remote --heads origin "refs/heads/$branch" </dev/null 2>/dev/null)" || return 1
  LC_ALL=C awk -v ref="refs/heads/$branch" '$2 == ref { print $1; exit }' <<<"$listing"
}

# _finalization_outcome_pr <scratch> <dir> <repo> <branch> <local_head> <ref>
# Prints six newline-separated validated fields (number, url, state, head,
# relation, checks) and returns 0; returns 1 when the PR state could not be
# read at all. Every printed field is validated here, so none can carry a
# newline of its own.
_finalization_outcome_pr() {
  local scratch="$1" dir="$2" repo="$3" branch="$4" local_head="$5" ref="$6"
  local num="" url="" prefix="https://github.com/$3/pull/" list="" view=""
  local state="" head="" cross="" relation="unknown" checks="" subject="$local_head"

  declare -F _finalization_pr_gh >/dev/null 2>&1 || return 1
  if [ -n "$ref" ]; then
    if _finalization_pr_number_valid "$ref"; then
      num="$ref"
    else
      case "$ref" in
        "$prefix"*) num="${ref#"$prefix"}" ;;
        *) num="" ;;
      esac
      _finalization_pr_number_valid "$num" || num=""
    fi
    [ -n "$num" ] || { printf '\n\nNONE\n\nnone\n\n'; return 0; }
  else
    list="$(_finalization_pr_gh "$scratch" "pr list" pr list --repo "$repo" \
      --head "$branch" --state open --json number,url,isCrossRepository --limit 2)" || return 1
    num="$(printf '%s' "$list" | _finalization_pr_list_number)" || num=""
    _finalization_pr_number_valid "$num" || { printf '\n\nNONE\n\nnone\n\n'; return 0; }
  fi

  url="$prefix$num"
  view="$(_finalization_pr_gh "$scratch" "pr view" pr view "$num" --repo "$repo" \
    --json number,url,state,headRefName,headRefOid,isCrossRepository,statusCheckRollup)" || return 1
  state="$(printf '%s' "$view" | jq -r 'if type == "object" and (.state | type) == "string" then .state else empty end' 2>/dev/null || :)"
  head="$(printf '%s' "$view" | jq -r 'if type == "object" and (.headRefOid | type) == "string" then .headRefOid else empty end' 2>/dev/null || :)"
  cross="$(printf '%s' "$view" | jq -r 'if type == "object" and (.isCrossRepository | type) == "boolean" then (.isCrossRepository | tostring) else empty end' 2>/dev/null || :)"
  case "$state" in ''|*[!A-Z_]*) state="UNKNOWN" ;; esac
  _finalization_outcome_sha "$head" || head=""

  if [ "$cross" = "true" ]; then
    relation="different"
  elif [ -z "$head" ]; then
    relation="unknown"
  elif [ "$head" = "$local_head" ]; then
    relation="local"
  elif _finalization_pr_head_descends "$dir" "$branch" "$local_head" "$head"; then
    relation="descendant"; subject="$head"
  else
    relation="different"
  fi

  case "$relation" in
    local|descendant)
      checks="$(printf '%s' "$view" | _finalization_pr_checks "$branch" "$num" "$url" "$subject")" || checks="unknown" ;;
    *) checks="" ;;
  esac
  printf '%s\n%s\n%s\n%s\n%s\n%s\n' "$num" "$url" "$state" "$head" "$relation" "$checks"
}

# Emits the fields of the finalization_outcome_facts call that is running.
# It reads that function's locals through bash's dynamic scoping, exactly as
# workflow_finalization_open_pr.sh does, so it must not be called elsewhere.
# shellcheck disable=SC2154
_finalization_outcome_emit_current() {
  _finalization_outcome_emit "$branch" "$local_head" "$remote" "$github_origin" \
    "$branch_pushed" "$remote_head" "$pr_number" "$pr_url" "$pr_state" \
    "$pr_head" "$relation" "$checks" "$verdict" "$gap"
}

finalization_outcome_facts() {
  local dir="${1:-}" branch="${2:-}" ref="${3:-}"
  local local_head="" remote="" github_origin="false" branch_pushed="unknown"
  local remote_head="" pr_number="" pr_url="" pr_state="" pr_head=""
  local relation="unknown" checks="" verdict="UNCERTAIN" gap=""
  local origin="" scratch="" facts="" read_status=0 live_branch=""

  command -v jq >/dev/null 2>&1 || { printf '%s\n' "$_FINALIZATION_OUTCOME_NO_JQ"; return 0; }

  if ! command -v git >/dev/null 2>&1; then
    gap="the run outcome could not be read because git is unavailable"
    _finalization_outcome_emit_current; return 0
  fi
  if [ -z "$dir" ] || [ ! -d "$dir" ] \
    || [ "$(git -C "$dir" rev-parse --is-inside-work-tree 2>/dev/null || :)" != "true" ]; then
    gap="the run outcome could not be read because the finalization target directory is not a git worktree"
    _finalization_outcome_emit_current; return 0
  fi
  local_head="$(git -C "$dir" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null || :)"
  if ! _finalization_outcome_sha "$local_head"; then
    local_head=""
    gap="the run outcome could not be read because the local HEAD does not resolve to a commit"
    _finalization_outcome_emit_current; return 0
  fi

  origin="$(git -C "$dir" config --get remote.origin.url 2>/dev/null || :)"
  if declare -F parse_github_repo_identity >/dev/null 2>&1 \
    && remote="$(parse_github_repo_identity "$origin" 2>/dev/null)"; then
    github_origin="true"
  else
    remote=""
  fi
  if [ "$github_origin" != "true" ]; then
    gap="the run outcome could not be read from a published pull request because the origin of the finalization worktree is not a github.com repository"
    _finalization_outcome_emit_current; return 0
  fi

  # The branch is validated before it reaches any git or gh argument, and is
  # passed as one argument. A name git itself rejects degrades to unknown. A
  # default branch is never a run branch, so it is never asked about.
  case "$branch" in ''|main|master|HEAD|-*) branch="" ;; esac
  if [ -n "$branch" ] && ! git -C "$dir" check-ref-format "refs/heads/$branch" >/dev/null 2>&1; then
    branch=""
  fi
  if [ -z "$branch" ]; then
    gap="the run outcome could not be read because the run branch is empty, a default branch, or not a valid branch name; the local head is $local_head"
    _finalization_outcome_emit_current; return 0
  fi
  # #1762: the run branch of an alias worktree is <B>, not <B>--wt-<n>. A
  # checkout that is on some other branch is not the one to report on.
  live_branch=""
  if [ -n "${_finalization_pr_signal_remote_branch:-}" ]; then
    live_branch="$(bash "$_finalization_pr_signal_remote_branch" "$dir")" || live_branch=""
  fi
  if [ -n "$live_branch" ] && [ "$live_branch" != "$branch" ]; then
    gap="the run outcome could not be read because the finalization worktree is not on the run branch $branch"
    branch=""
    _finalization_outcome_emit_current; return 0
  fi

  if ! command -v gh >/dev/null 2>&1; then
    gap="the pull request for the run branch $branch at $local_head could not be read because gh is unavailable"
    _finalization_outcome_emit_current; return 0
  fi
  scratch="$(mktemp -d 2>/dev/null)" || scratch=""
  if [ -z "$scratch" ] || [ ! -d "$scratch" ]; then
    gap="the pull request for the run branch $branch at $local_head could not be read because no private scratch directory could be created"
    _finalization_outcome_emit_current; return 0
  fi
  facts="$(_finalization_outcome_pr "$scratch" "$dir" "$remote" "$branch" "$local_head" "$ref")" || read_status=$?
  rm -rf -- "$scratch"
  if [ "$read_status" = 0 ]; then
    { IFS= read -r pr_number
      IFS= read -r pr_url
      IFS= read -r pr_state
      IFS= read -r pr_head
      IFS= read -r relation
      IFS= read -r checks; } <<<"$facts"
    [ "$pr_state" != "NONE" ] || { pr_state=""; relation="none"; }
  fi

  # An open pull request whose head IS the local head, or a descendant of it,
  # is itself proof that the local head is published: the remote is serving
  # that commit on this branch. The remote is asked directly only when that
  # proof is missing, so the healthy path costs no extra remote read.
  if [ "$pr_state" = "OPEN" ] && { [ "$relation" = "local" ] || [ "$relation" = "descendant" ]; }; then
    branch_pushed="true"
    case "$checks" in
      failing)
        verdict="BLOCKED"
        gap="pull request #$pr_number for the run branch $branch has a failing CI check" ;;
      green|pending|none)
        verdict="SUCCESS"; gap="" ;;
      *)
        gap="the CI check state of pull request #$pr_number for the run branch $branch could not be read" ;;
    esac
    _finalization_outcome_emit_current; return 0
  fi

  if remote_head="$(_finalization_outcome_remote_head "$dir" "$branch")"; then
    if [ -z "$remote_head" ]; then
      branch_pushed="false"
    elif ! _finalization_outcome_sha "$remote_head"; then
      remote_head=""; branch_pushed="unknown"
    elif [ "$remote_head" = "$local_head" ]; then
      branch_pushed="true"
    elif ! git -C "$dir" cat-file -e "${remote_head}^{commit}" 2>/dev/null; then
      branch_pushed="unknown"
    elif git -C "$dir" merge-base --is-ancestor "$local_head" "$remote_head" 2>/dev/null; then
      branch_pushed="true"
    else
      branch_pushed="false"
    fi
  else
    remote_head=""; branch_pushed="unknown"
  fi

  if [ "$branch_pushed" = "false" ]; then
    verdict="GAP"
    gap="the run branch $branch does not carry the local head $local_head on its remote; the remote branch is at ${remote_head:-no commit at all}"
    _finalization_outcome_emit_current; return 0
  fi
  if [ "$read_status" != 0 ]; then
    gap="the pull request for the run branch $branch at $local_head could not be read from the remote"
    _finalization_outcome_emit_current; return 0
  fi
  if [ "$relation" = "none" ]; then
    verdict="GAP"
    if [ "$branch_pushed" = "true" ]; then
      gap="the run branch $branch carries the local head $local_head on its remote but has no open pull request"
    else
      gap="the run branch $branch has no open pull request for the local head $local_head"
    fi
    _finalization_outcome_emit_current; return 0
  fi
  if [ "$pr_state" != "OPEN" ]; then
    verdict="GAP"
    gap="pull request #$pr_number for the run branch $branch is $pr_state, not open; the local head is $local_head"
    _finalization_outcome_emit_current; return 0
  fi
  if [ "$relation" = "different" ]; then
    verdict="GAP"
    gap="pull request #$pr_number is at head ${pr_head:-an unreadable commit}, which is neither the local head $local_head nor a descendant of it"
    _finalization_outcome_emit_current; return 0
  fi
  gap="the head of pull request #$pr_number could not be compared with the local head $local_head"
  _finalization_outcome_emit_current; return 0
}
