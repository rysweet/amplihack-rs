#!/usr/bin/env bash
# workflow_finalization_outcome.sh - what the run produced (#2343).
#
#   finalization_outcome_facts <target_dir> <branch> [<pr_ref>] [<host_type>]
#
# Collection, not judgement. This file reaches no verdict, derives no relation
# between two commits, names no terminal state and ends no run. It reports what
# git and gh printed and stops there. Deciding what that means is
# assess-finalization's job, in
# amplifier-bundle/recipes/workflow-finalization-assessment.yaml.
#
# It always exits 0 and always prints exactly one compact JSON object, so a
# caller never has to tell "the read failed" from "the read says no": a fact
# that could not be read is left empty and named in `unread`, with the reason
# the tool gave.
#
# The fields, every value a string, are listed with their meanings in
# docs/reference/default-workflow-agentic-finalization.md#the-facts. In short:
# the run branch and the local head; the remote as owner/repo and whether it
# parsed as github.com; what `git ls-remote --heads origin` listed for the
# branch; how many open pull requests gh listed for it; that pull request's
# number, URL, state, draft flag, head, branch and cross-repository flag; its
# statusCheckRollup both as the shared _finalization_pr_rollup_class and as the
# check states gh reported, so a reader is never handed only the rollup; what
# `git merge-base --is-ancestor` answered for the remote head and the pull
# request head; the remote host type the run declared; and `unread`.
#
# The two `*_descends_local_head` fields are what `git merge-base` answered,
# recorded like any other tool output; they are not a relation this file
# derived. What the answer means -- whether the branch was pushed, whether the
# pull request is this run's, whether any of it is a success -- is the reader's
# business, and this file says none of it.
#
# No wait, poll, retry or timer: every state is read exactly once. The origin
# URL never appears in the output or on stderr: the remote is reported as
# owner/repo through parse_github_repo_identity, which discards any userinfo,
# and git and gh stderr is discarded for the same reason.
#
# This file only DEFINES functions; sourcing it has no other side effects. It
# must be sourced after workflow_finalization_pr_signal.sh, whose
# _finalization_pr_rollup_class, _finalization_pr_number_valid and
# _finalization_pr_gh it reuses rather than restating.

# A literal object for the one case where the JSON builder itself is missing.
_FINALIZATION_OUTCOME_NO_JQ='{"schema_version":3,"branch":"","local_head":"","remote":"","github_origin":"false","declared_remote_host":"","remote_head":"","remote_head_descends_local_head":"","open_pr_count":"","pr_number":"","pr_url":"","pr_state":"","pr_draft":"","pr_head":"","pr_head_ref":"","pr_cross_repository":"","pr_head_descends_local_head":"","checks":"","check_states":"","unread":"the run outcome could not be read because jq is unavailable"}'

# Control characters and runaway length never reach a result field or a log.
_finalization_outcome_clean() {
  LC_ALL=C printf '%s' "$1" | LC_ALL=C tr -d '\001-\037\177' | LC_ALL=C cut -c1-1200
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
# returns 1 when the remote could not be read at all; prints nothing and
# returns 0 when it answered and listed the branch nowhere.
_finalization_outcome_remote_head() {
  local dir="$1" branch="$2" listing="" ssh_cmd="ssh"
  declare -F _finalization_batch_ssh_command >/dev/null 2>&1 \
    && _finalization_batch_ssh_command "$dir" ssh_cmd
  listing="$(GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="$ssh_cmd" git -C "$dir" \
    -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=30 \
    ls-remote --heads origin "refs/heads/$branch" </dev/null 2>/dev/null)" || return 1
  LC_ALL=C awk -v ref="refs/heads/$branch" '$2 == ref { print $1; exit }' <<<"$listing"
}

# Fixed key order; every field arrives through --arg, never concatenation.
_finalization_outcome_emit() {
  jq -cn \
    --arg branch "$1" --arg local_head "$2" --arg remote "$3" \
    --arg github_origin "$4" --arg declared_remote_host "$5" \
    --arg remote_head "$6" --arg remote_desc "$7" \
    --arg open_pr_count "$8" --arg pr_number "$9" --arg pr_url "${10}" \
    --arg pr_state "${11}" --arg pr_draft "${12}" --arg pr_head "${13}" \
    --arg pr_head_ref "${14}" --arg pr_cross_repository "${15}" \
    --arg pr_desc "${16}" --arg checks "${17}" --arg check_states "${18}" \
    --arg unread "$(_finalization_outcome_clean "${19}")" \
    '{schema_version:3, branch:$branch, local_head:$local_head, remote:$remote,
      github_origin:$github_origin, declared_remote_host:$declared_remote_host,
      remote_head:$remote_head,
      remote_head_descends_local_head:$remote_desc,
      open_pr_count:$open_pr_count, pr_number:$pr_number, pr_url:$pr_url,
      pr_state:$pr_state, pr_draft:$pr_draft, pr_head:$pr_head,
      pr_head_ref:$pr_head_ref, pr_cross_repository:$pr_cross_repository,
      pr_head_descends_local_head:$pr_desc, checks:$checks,
      check_states:$check_states, unread:$unread}' 2>/dev/null \
    || printf '%s\n' "$_FINALIZATION_OUTCOME_NO_JQ"
}

# The two collectors below read and set the locals of the
# finalization_outcome_facts call that is running, through bash's dynamic
# scoping. They must not be called from anywhere else.
# shellcheck disable=SC2154

# _finalization_outcome_descends <dir> <branch> <other> <out_var> <label>
# Records what `git merge-base --is-ancestor <local_head> <other>` answered,
# through the shared _finalization_pr_head_descends, which also makes the one
# bounded fetch that may be needed to have the commit to ask about. "false" is
# recorded only when the commit IS present and the answer really was no;
# a commit that could not be obtained leaves the field empty and says so.
_finalization_outcome_descends() {
  local dir="$1" branch="$2" other="$3" out="$4" label="$5"
  printf -v "$out" '%s' ""
  [ -n "$other" ] || return 0
  if declare -F _finalization_pr_head_descends >/dev/null 2>&1 \
    && _finalization_pr_head_descends "$dir" "$branch" "$local_head" "$other"; then
    printf -v "$out" '%s' "true"
  elif git -C "$dir" cat-file -e "${other}^{commit}" 2>/dev/null; then
    printf -v "$out" '%s' "false"
  else
    _finalization_outcome_note "git could not obtain the $label commit $other, so whether the local head $local_head is its ancestor is unknown"
  fi
}

# Append one sentence to `unread`.
_finalization_outcome_note() {
  unread="${unread:+$unread; }$1"
}

# _finalization_outcome_view <scratch> <repo> <branch>
# Reads one pull request and records its fields exactly as gh printed them.
# <pr_number> is already set when this runs.
_finalization_outcome_view() {
  local scratch="$1" repo="$2" branch="$3" dir="$4" view=""
  pr_url="https://github.com/$repo/pull/$pr_number"
  if ! view="$(_finalization_pr_gh "$scratch" "pr view" pr view "$pr_number" --repo "$repo" \
      --json number,url,state,isDraft,headRefName,headRefOid,isCrossRepository,statusCheckRollup)"; then
    _finalization_outcome_note "the state of pull request #$pr_number for the run branch $branch could not be read because gh pr view failed"
    return 0
  fi
  pr_state="$(printf '%s' "$view" | jq -r 'if type == "object" and (.state | type) == "string" then .state else "" end' 2>/dev/null || :)"
  pr_draft="$(printf '%s' "$view" | jq -r 'if type == "object" and (.isDraft | type) == "boolean" then (.isDraft | tostring) else "" end' 2>/dev/null || :)"
  pr_head="$(printf '%s' "$view" | jq -r 'if type == "object" and (.headRefOid | type) == "string" then .headRefOid else "" end' 2>/dev/null || :)"
  # gh's own answer for the branch the pull request is on, reported beside the
  # branch that was asked about so a reader can see them disagree.
  pr_head_ref="$(printf '%s' "$view" | jq -r 'if type == "object" and (.headRefName | type) == "string" then .headRefName else "" end' 2>/dev/null || :)"
  pr_head_ref="$(_finalization_outcome_clean "$pr_head_ref")"
  pr_cross_repository="$(printf '%s' "$view" | jq -r 'if type == "object" and (.isCrossRepository | type) == "boolean" then (.isCrossRepository | tostring) else "" end' 2>/dev/null || :)"
  checks="$(printf '%s' "$view" | _finalization_pr_rollup_class)"
  # #2819 R6: the check states themselves, beside the class. A rollup is
  # already a reading; a reader handed only `failing` cannot see which check
  # failed. Both are reported and neither is a verdict.
  check_states="$(_finalization_outcome_clean "$(printf '%s' "$view" | _finalization_pr_check_states)")"
  # Field shapes, so no value can carry a newline into the result. A value
  # outside its shape is reported as unread, never coerced to another value.
  case "$pr_state" in ''|*[!A-Z_]*) [ -z "$pr_state" ] || _finalization_outcome_note "gh pr view returned a pull request state for #$pr_number that is not a state name"; pr_state="" ;; esac
  _finalization_outcome_sha "$pr_head" || { [ -z "$pr_head" ] || _finalization_outcome_note "gh pr view returned a head for pull request #$pr_number that is not a commit id"; pr_head=""; }
  case "$checks" in
    green|failing|pending|none|unknown) ;;
    *) _finalization_outcome_note "the check rollup of pull request #$pr_number did not classify, so the class is unknown; the states themselves are in check_states"; checks="unknown" ;;
  esac
  _finalization_outcome_descends "$dir" "$branch" "$pr_head" \
    pr_head_descends_local_head "head of pull request #$pr_number"
}

# _finalization_outcome_collect <dir> <branch> <ref>
# Records every fact it can read. Each step that cannot read one says so in
# `unread` and the collection stops where the next step has nothing to stand
# on; nothing here classifies, and nothing here exits.
_finalization_outcome_collect() {
  local dir="${1:-}" ref="${3:-}" origin="" scratch="" list="" live_branch="" count=""
  branch="${2:-}"
  declared_remote_host="$(_finalization_outcome_clean "${4:-}")"
  # A recorded host name outside the shape is not quietly renamed to a real
  # one: it reads "other" and says that the run's own value was unusable, so a
  # reader can tell it from a run that really recorded "other".
  case "$declared_remote_host" in
    *[!A-Za-z0-9_.-]*)
      declared_remote_host="other"
      _finalization_outcome_note "the remote host type this run recorded is not a host name, so it is reported as other" ;;
  esac

  if ! command -v git >/dev/null 2>&1; then
    _finalization_outcome_note "the run outcome could not be read because git is unavailable"; return 0
  fi
  if [ -z "$dir" ] || [ ! -d "$dir" ] \
    || [ "$(git -C "$dir" rev-parse --is-inside-work-tree 2>/dev/null || :)" != "true" ]; then
    _finalization_outcome_note "the run outcome could not be read because the finalization target directory is not a git worktree"; return 0
  fi
  local_head="$(git -C "$dir" rev-parse --verify --quiet 'HEAD^{commit}' 2>/dev/null || :)"
  if ! _finalization_outcome_sha "$local_head"; then
    local_head=""
    _finalization_outcome_note "the local HEAD of the finalization worktree does not resolve to a commit"; return 0
  fi

  origin="$(git -C "$dir" config --get remote.origin.url 2>/dev/null || :)"
  if declare -F parse_github_repo_identity >/dev/null 2>&1 \
    && remote="$(parse_github_repo_identity "$origin" 2>/dev/null)"; then
    github_origin="true"
  else
    remote=""
    _finalization_outcome_note "the origin of the finalization worktree is not a github.com repository, so it has no pull request to read; the local head is $local_head"
    return 0
  fi

  # #1641 D5: a run that recorded a non-GitHub remote has no GitHub pull
  # request to read, whatever this checkout's origin happens to be. The remote
  # is not asked, and the record says why.
  case "$declared_remote_host" in
    ''|github|GitHub|GITHUB) ;;
    *)
      _finalization_outcome_note "the run recorded a $declared_remote_host remote, so no GitHub pull request was read for it; the local head is $local_head"
      return 0 ;;
  esac

  # The branch is validated before it reaches any git or gh argument, and is
  # passed as one argument. A name git itself rejects is reported unread. A
  # default branch is never a run branch, so it is never asked about.
  case "$branch" in ''|main|master|HEAD|-*) branch="" ;; esac
  if [ -n "$branch" ] && ! git -C "$dir" check-ref-format "refs/heads/$branch" >/dev/null 2>&1; then
    branch=""
  fi
  if [ -z "$branch" ]; then
    _finalization_outcome_note "the recorded run branch is empty, a default branch, or not a valid branch name, so no branch or pull request could be read for it; the local head is $local_head"
    return 0
  fi
  # #1762: the run branch of an alias worktree is <B>, not <B>--wt-<n>. A
  # checkout that is on some other branch is not the one to report on.
  if [ -n "${_finalization_pr_signal_remote_branch:-}" ]; then
    live_branch="$(bash "$_finalization_pr_signal_remote_branch" "$dir")" || live_branch=""
  fi
  if [ -n "$live_branch" ] && [ "$live_branch" != "$branch" ]; then
    _finalization_outcome_note "the finalization worktree is not on the run branch $branch, so nothing was read for it; the local head is $local_head"
    branch=""
    return 0
  fi

  if ! remote_head="$(_finalization_outcome_remote_head "$dir" "$branch")"; then
    remote_head=""
    _finalization_outcome_note "git ls-remote --heads origin refs/heads/$branch did not answer, so the commit the remote carries for the run branch is unknown"
  elif [ -n "$remote_head" ] && ! _finalization_outcome_sha "$remote_head"; then
    remote_head=""
    _finalization_outcome_note "git ls-remote --heads origin refs/heads/$branch listed something that is not a commit id"
  fi
  _finalization_outcome_descends "$dir" "$branch" "$remote_head" \
    remote_head_descends_local_head "run branch's remote head"

  if ! command -v gh >/dev/null 2>&1; then
    _finalization_outcome_note "the pull request for the run branch $branch could not be read because gh is unavailable"; return 0
  fi
  scratch="$(mktemp -d 2>/dev/null)" || scratch=""
  if [ -z "$scratch" ] || [ ! -d "$scratch" ]; then
    _finalization_outcome_note "the pull request for the run branch $branch could not be read because no private scratch directory could be created"; return 0
  fi

  if [ -n "$ref" ]; then
    # The run named its own pull request: a canonical number, or the canonical
    # URL for this origin. Anything else is reported unread rather than widened
    # to discovery, which would adopt some other pull request.
    if _finalization_pr_number_valid "$ref"; then
      pr_number="$ref"
    else
      case "$ref" in
        "https://github.com/$remote/pull/"*) pr_number="${ref#"https://github.com/$remote/pull/"}" ;;
        *) pr_number="" ;;
      esac
      _finalization_pr_number_valid "$pr_number" || pr_number=""
    fi
    [ -n "$pr_number" ] || _finalization_outcome_note "the pull request reference this run recorded is neither a pull request number nor the canonical URL of a pull request in $remote, so no pull request was read"
  elif list="$(_finalization_pr_gh "$scratch" "pr list" pr list --repo "$remote" \
      --head "$branch" --state open --json number,url,isCrossRepository --limit 2)"; then
    count="$(printf '%s' "$list" | jq -r 'if type == "array" then (length | tostring) else "" end' 2>/dev/null || :)"
    case "$count" in 0|1|2) open_pr_count="$count" ;; *) _finalization_outcome_note "gh pr list did not return a list of open pull requests for the run branch $branch" ;; esac
    pr_number="$(printf '%s' "$list" | _finalization_pr_list_number)" || pr_number=""
    _finalization_pr_number_valid "$pr_number" || pr_number=""
  else
    _finalization_outcome_note "the open pull requests of the run branch $branch could not be listed because gh pr list failed"
  fi

  [ -z "$pr_number" ] || _finalization_outcome_view "$scratch" "$remote" "$branch" "$dir"
  rm -rf -- "$scratch"
}

finalization_outcome_facts() {
  local branch="" local_head="" remote="" github_origin="false"
  local declared_remote_host="" remote_head="" remote_head_descends_local_head=""
  local open_pr_count="" pr_number="" pr_url="" pr_state="" pr_draft=""
  local pr_head="" pr_head_ref="" pr_cross_repository=""
  local pr_head_descends_local_head="" checks="" check_states="" unread=""

  command -v jq >/dev/null 2>&1 || { printf '%s\n' "$_FINALIZATION_OUTCOME_NO_JQ"; return 0; }
  _finalization_outcome_collect "${1:-}" "${2:-}" "${3:-}" "${4:-}" || :
  _finalization_outcome_emit "$branch" "$local_head" "$remote" "$github_origin" \
    "$declared_remote_host" "$remote_head" "$remote_head_descends_local_head" \
    "$open_pr_count" "$pr_number" "$pr_url" "$pr_state" "$pr_draft" "$pr_head" \
    "$pr_head_ref" "$pr_cross_repository" "$pr_head_descends_local_head" \
    "$checks" "$check_states" "$unread"
  return 0
}
