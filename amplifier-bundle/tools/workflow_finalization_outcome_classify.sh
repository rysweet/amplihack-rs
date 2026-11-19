#!/usr/bin/env bash
# shellcheck disable=SC2034,SC2154  # the validator's locals, read and set through dynamic scoping (see below)
# workflow_finalization_outcome_classify.sh - the validator's half of the
# outcome read (#2343).
#
# Sourced by workflow_agentic_finalization.sh after
# workflow_finalization_outcome.sh and before workflow_finalization_validate.sh.
# These functions are called only from validate_finalization and read and set
# its variables (target_dir, pr_url, pr_number, outcome_*) through bash's
# dynamic scoping, exactly as the open-PR check used to. They must not be
# called from anywhere else.
#
#   read_recorded_outcome          take the outcome collect-finalization-evidence
#                                  recorded, or leave outcome_verdict empty
#   read_live_outcome              read the outcome now, once
#   classify_confirmed_outcome     classify and exit on a confirmed outcome
#   finalization_outcome_facts_line  the confirmed PR's evidence facts
#   apply_finalization_assessment  use the agentic assessment (#2809) without
#                                  ever taking its success on trust

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
