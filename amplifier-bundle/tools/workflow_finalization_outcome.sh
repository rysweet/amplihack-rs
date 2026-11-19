#!/usr/bin/env bash
# workflow_finalization_outcome.sh - what the run actually produced (#2343).
#
#   finalization_outcome_facts <target_dir> <branch> [<pr_ref>]
#
# Reports facts, never policy. It always exits 0 and always prints exactly one
# compact JSON object on stdout, so a caller never has to tell "the read
# failed" from "the read says no": an unreadable fact is reported as `unknown`
# and carries a sentence naming what could not be read.
#
# The object (schema_version 1, every other value a String):
#   branch              the run branch, or "" when it was rejected
#   local_head          the worktree's HEAD commit, or ""
#   remote              owner/repo from origin, or "" (never the origin URL)
#   github_origin       "true" when the worktree's origin is a github.com
#                       repository whose pull requests were read; "false" when
#                       this run has no such repository to read an outcome
#                       from at all; "unknown" when that itself is unclear
#   branch_pushed       "true" when the remote carries the local head, either
#                       because an open PR is serving it or because the remote
#                       branch tip is it or a descendant of it; "false" when
#                       the remote demonstrably does not; else "unknown"
#   remote_head         the remote branch tip, or ""
#   pr_number/pr_url/pr_state/pr_head   the open PR for the branch, or ""
#   pr_head_relation    local | descendant | different | none | unknown
#   checks              green | failing | pending | none | unknown | ""
#   verdict             SUCCESS | BLOCKED | GAP | UNCERTAIN
#   gap                 one sentence naming the gap, carrying the SHAs, branch
#                       and PR number it is about; "" only on SUCCESS
#
# SUCCESS means an open same-repo PR sits at the local head, or at a descendant
# of it, and no check of that PR is failing. An open PR at the local head is
# itself proof the head is published, so the remote is asked directly only when
# that proof is missing: the healthy path costs no extra remote read. BLOCKED means that PR has a failing
# check. GAP means a fact was read and it is negative: nothing published, no
# PR, or a PR at an unrelated head. UNCERTAIN means a fact could not be read.
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
# must be sourced after workflow_finalization_pr_signal.sh and before
# workflow_finalization_validate.sh. It replaces the open-PR check that used
# to live in workflow_finalization_open_pr.sh: that check asked whether this
# run's own bookkeeping permitted it to look, and refused to look otherwise.

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

# ---------------------------------------------------------------------------
# The validator's half of the outcome read (#2343).
#
# These four functions are called only from validate_finalization and read and
# set its variables (target_dir, pr_url, pr_number, outcome_*) through bash's
# dynamic scoping, exactly as workflow_finalization_open_pr.sh does. They must
# not be called from anywhere else.
#
#   read_recorded_outcome          take the outcome collect-finalization-evidence
#                                  recorded, or leave outcome_verdict empty
#   read_live_outcome              read the outcome now, once
#   classify_confirmed_outcome     classify and exit on a confirmed outcome
#   apply_finalization_assessment  use the agentic assessment (#2809) without
#                                  ever taking its success on trust
# shellcheck disable=SC2154

# Narrow every field to the values this contract defines, so a field that
# reached the context from somewhere else cannot name a pull request that the
# outcome read did not confirm.
_finalization_outcome_narrow() {
  case "$outcome_github_origin" in true|false) ;; *) outcome_github_origin="unknown" ;; esac
  case "$outcome_checks" in green|failing|pending|none|unknown) ;; *) outcome_checks="" ;; esac
  case "$outcome_pr_head_relation" in local|descendant) ;; *) outcome_pr_head_relation="local" ;; esac
  outcome_gap="$(_finalization_outcome_clean "$outcome_gap")"
  if ! _finalization_pr_number_valid "$outcome_pr_number" \
    || [ "$outcome_pr_url" != "https://github.com/${outcome_remote}/pull/$outcome_pr_number" ] \
    || [ -z "$outcome_remote" ]; then
    outcome_pr_number=""; outcome_pr_url=""
    # A confirmed state we cannot name a pull request for is not confirmed.
    case "$outcome_verdict" in
      SUCCESS|BLOCKED)
        outcome_verdict="UNCERTAIN"
        outcome_gap="the pull request the run outcome reported could not be identified" ;;
    esac
  fi
}

_finalization_outcome_field() {
  printf '%s' "$2" | jq -r --arg key "$1" \
    '.[$key] // "" | if type == "string" then . else "" end' 2>/dev/null || :
}

# Leave every outcome field unread, with one sentence saying why.
_finalization_outcome_unread() {
  outcome_verdict="UNCERTAIN"; outcome_gap="$(_finalization_outcome_clean "$1")"
  outcome_github_origin="$2"; outcome_checks=""; outcome_pr_number=""
  outcome_pr_url=""; outcome_remote=""; outcome_pr_head_relation="local"
}

read_recorded_outcome() {
  local verdict
  outcome_verdict=""; outcome_gap=""; outcome_github_origin="unknown"
  outcome_checks=""; outcome_pr_number=""; outcome_pr_url=""; outcome_remote=""
  outcome_pr_head_relation=""
  verdict="$(workflow_finalization_field finalization_evidence outcome.verdict || :)"
  case "$verdict" in SUCCESS|BLOCKED|GAP|UNCERTAIN) ;; *) return 0 ;; esac
  outcome_verdict="$verdict"
  outcome_gap="$(workflow_finalization_field finalization_evidence outcome.gap || :)"
  outcome_github_origin="$(workflow_finalization_field finalization_evidence outcome.github_origin || :)"
  outcome_checks="$(workflow_finalization_field finalization_evidence outcome.checks || :)"
  outcome_pr_number="$(workflow_finalization_field finalization_evidence outcome.pr_number || :)"
  outcome_pr_url="$(workflow_finalization_field finalization_evidence outcome.pr_url || :)"
  outcome_remote="$(workflow_finalization_field finalization_evidence outcome.remote || :)"
  outcome_pr_head_relation="$(workflow_finalization_field finalization_evidence outcome.pr_head_relation || :)"
  _finalization_outcome_narrow
}

read_live_outcome() {
  local run_branch pr_ref raw remote_host_type
  # Two preconditions the outcome read cannot establish for itself. Without a
  # recorded worktree the read would classify whatever directory the caller
  # happens to sit in, and a remote the run itself declared non-GitHub has no
  # GitHub pull request to read (#1641).
  #
  # A context file that was selected but could not be read is the exception
  # (#2343). There the run did record its worktree and the transport lost it,
  # which is not the run's failure: finalization reads the directory it was
  # run in and judges by what it finds there, where every other proof -- a
  # worktree on the run branch, a github.com origin, an open pull request at
  # the local head -- still has to hold.
  if [ "${target_dir_explicit:-false}" != "true" ] \
    && ! { [ -n "${_workflow_context_has_file:-}" ] && [ "${_workflow_context_file_status:-0}" != 0 ]; }; then
    # A run whose own evidence says a GitHub pull request was required has one
    # that could not be read, not none to read. It never falls back to its own
    # bookkeeping; a run with no such requirement still may (#1641, D5).
    if [ "${evidence_gh_required:-}" = "true" ]; then
      _finalization_outcome_unread "the run required a GitHub pull request but recorded no worktree or repository path in which its outcome could be read" "unknown"
    else
      _finalization_outcome_unread "the run outcome could not be read because the run recorded no worktree or repository path to read it in" "false"
    fi
    return 0
  fi
  if [ "${metadata_corrupt:-false}" = "true" ]; then
    _finalization_outcome_unread "the run outcome could not be read because the run's own workflow metadata is invalid" "unknown"
    return 0
  fi
  remote_host_type="$(workflow_finalization_metadata remote_host_type || :)"
  case "$remote_host_type" in
    ''|github|GitHub|GITHUB) ;;
    *)
      _finalization_outcome_unread "the run recorded a $remote_host_type remote, which has no GitHub pull request whose outcome could be read" "false"
      return 0 ;;
  esac
  run_branch="$(workflow_finalization_metadata branch_name || :)"
  [ "$run_branch" != "''" ] || run_branch=""
  # #1762: an alias worktree is on <B>, not <B>--wt-<n>. A resolver that is
  # missing or fails leaves the branch empty, which the reader reports as an
  # unread fact; the local branch name is never substituted for it.
  if [ -z "$run_branch" ] && [ -n "${_finalization_pr_signal_remote_branch:-}" ]; then
    run_branch="$(bash "$_finalization_pr_signal_remote_branch" "$target_dir")" || run_branch=""
  fi
  # A canonical pr_number wins, so a malformed pr_url cannot hide it. A bad
  # pr_number with no pr_url is passed through as-is, so the reader rejects it
  # rather than widening to discovery and adopting some other pull request.
  pr_ref="$pr_number"
  if ! _finalization_pr_number_valid "$pr_ref" && [ -n "$pr_url" ]; then pr_ref="$pr_url"; fi
  [ "$pr_ref" != "''" ] || pr_ref=""
  raw="$(finalization_outcome_facts "$target_dir" "$run_branch" "$pr_ref")" || raw=""
  outcome_verdict="$(_finalization_outcome_field verdict "$raw")"
  outcome_gap="$(_finalization_outcome_field gap "$raw")"
  outcome_github_origin="$(_finalization_outcome_field github_origin "$raw")"
  outcome_checks="$(_finalization_outcome_field checks "$raw")"
  outcome_pr_number="$(_finalization_outcome_field pr_number "$raw")"
  outcome_pr_url="$(_finalization_outcome_field pr_url "$raw")"
  outcome_remote="$(_finalization_outcome_field remote "$raw")"
  outcome_pr_head_relation="$(_finalization_outcome_field pr_head_relation "$raw")"
  case "$outcome_verdict" in
    SUCCESS|BLOCKED|GAP|UNCERTAIN) ;;
    *) outcome_verdict="UNCERTAIN"
       outcome_gap="the run outcome reader did not report a usable verdict" ;;
  esac
  _finalization_outcome_narrow
}

# Exits the run when the outcome confirmed a published pull request at this
# run's head. Returns 0 and changes nothing otherwise, so the caller carries
# on to its remaining paths. <extra_facts> is appended to evidence_used.
# The confirmed pull request's evidence facts, in the vocabulary the open-PR
# check established. A PR head that fast-forwards the local HEAD is the same
# PR (#1714), and the facts say which of the two it was.
finalization_outcome_facts_line() {
  local head_facts="pr_head_sha_matches=true"
  [ "$outcome_pr_head_relation" != "descendant" ] \
    || head_facts="pr_head_sha_matches=false,pr_head_fast_forwards_local_head=true"
  printf 'publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,%s,pr_number=%s,all_checks=%s' \
    "$head_facts" "$outcome_pr_number" "${outcome_checks:-unknown}"
}

classify_confirmed_outcome() {
  local extra="${1:-}" facts
  case "$outcome_verdict" in SUCCESS|BLOCKED) ;; *) return 0 ;; esac
  pr_number="$outcome_pr_number"; pr_url="$outcome_pr_url"; publish_state_override="true"
  facts="$(finalization_outcome_facts_line)$extra"
  if [ "$outcome_verdict" = "BLOCKED" ]; then
    open_pr_confirmed_number="$outcome_pr_number"
    open_pr_confirmed_url="$outcome_pr_url"
    open_pr_confirmed_facts="$facts"
    finalization_fail_or_handoff "BLOCKED_CI" "$outcome_gap" \
      "Fix the failing CI checks on PR #$outcome_pr_number before merge." "$facts"
  fi
  if [ "$outcome_checks" = "green" ]; then
    classify_success "EXISTING_OPEN_PR" \
      "the run branch has a published, open, same-repo PR at the run's head whose checks are all green" \
      "No action required." "$facts"
  fi
  # #2784: finished work whose checks have not reported yet is not an
  # implementation failure. The pull request is handed on, named, with its URL.
  classify_success "EXISTING_OPEN_PR" \
    "the run branch has a published, open, same-repo PR at the run's head; its CI checks are $outcome_checks" \
    "Watch CI on PR #$outcome_pr_number ($outcome_pr_url): its checks are $outcome_checks; a failing check is BLOCKED_CI." \
    "$facts"
}

apply_finalization_assessment() {
  local verdict action gap
  verdict="$(workflow_finalization_field finalization_assessment verdict || :)"
  action="$(workflow_finalization_field finalization_assessment action_taken || :)"
  gap="$(workflow_finalization_field finalization_assessment gap || :)"
  case "$verdict" in SUCCESS|BLOCKED|FAILED|UNCERTAIN) ;; *) verdict="" ;; esac
  case "$action" in ''|none) action="none" ;; *) action="$(_finalization_outcome_clean "$action")" ;; esac
  { [ -n "$verdict" ] || [ "$action" != "none" ]; } || return 0

  # S1: the assessment's word is never the success. A claimed SUCCESS or
  # BLOCKED, and any action it reports taking, costs exactly one mechanical
  # re-read, and the run is classified on that re-read alone. The assessment
  # supplies the explanation and the record of what it did, nothing else.
  if [ "$verdict" = "SUCCESS" ] || [ "$verdict" = "BLOCKED" ] || [ "$action" != "none" ]; then
    read_live_outcome
    classify_confirmed_outcome ",assessment=${verdict:-none},assessment_action=$action"
  fi
  gap="$(_finalization_outcome_clean "$gap")"
  if [ -n "$gap" ]; then
    outcome_gap="${outcome_gap:+$outcome_gap; }the finalization assessment reported: $gap"
  fi
  return 0
}
