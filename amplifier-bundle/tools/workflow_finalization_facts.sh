#!/usr/bin/env bash
# workflow_finalization_facts.sh - the facts the validator carries (#2343).
#
# Sourced, never executed. Holds the three helpers that read the run's outcome
# into the validator's locals and render them, and nothing else, so it can be
# sourced twice. They are split out of workflow_finalization_validate.sh to
# keep one subject per brick: this file obtains and renders facts, that file
# reports the terminal state assess-finalization decided on them.
#
# Every function here reads and writes the locals of the validate_finalization
# call that is running, through bash's dynamic scoping, so they must be called
# only from inside it. None of them reads a fact as a success or a failure,
# names a terminal state, or ends a run.
#
#   corrupt_evidence_note  the evidence_used suffix for unreadable evidence
#   gather_outcome_facts   the recorded outcome, else one live read
#   outcome_facts_line     those facts as one line for a result

# The fact fields the validator carries into its result and its reasons; the
# rest of what finalization_outcome_facts reports is for assess-finalization.
_FINALIZATION_OUTCOME_LEAVES="branch local_head remote remote_head pr_number pr_url pr_state pr_head checks unread"

# corrupt_evidence_note: the suffix that says the collected evidence could not
# be read, for a result's evidence_used. Empty unless evidence_corrupt is
# exactly "true"; the variable is always set, to "false" or "true", so a
# ${evidence_corrupt:+...} expansion here would fire on "false" too.
# shellcheck disable=SC2154
corrupt_evidence_note() {
  [ "$evidence_corrupt" = "true" ] || return 0
  printf ',finalization_evidence=unreadable'
}

# gather_outcome_facts: the outcome collect-finalization-evidence recorded,
# else one live read here. Sets outcome_* and outcome_present on the
# validate_finalization call that is running, through dynamic scoping. It
# reads nothing as a success or a failure, and ends nothing.
# shellcheck disable=SC2154
gather_outcome_facts() {
  local leaf raw run_branch pr_ref
  outcome_present="false"; outcome_unread=""
  for leaf in $_FINALIZATION_OUTCOME_LEAVES; do
    printf -v "outcome_$leaf" '%s' \
      "$(workflow_finalization_field finalization_evidence "outcome.$leaf" || :)"
  done
  if [ -n "$outcome_branch$outcome_local_head$outcome_pr_number$outcome_unread" ]; then
    outcome_present="true"; return 0
  fi

  # No recorded outcome: read it once here. Two preconditions the read cannot
  # establish for itself: without a recorded worktree it would report on
  # whatever directory the caller sits in, and corrupt metadata cannot say
  # which branch or pull request to read (#1641). A context file selected but
  # unreadable is the exception (#2343) -- the run did record its worktree and
  # the transport lost it -- so finalization reads the directory it ran in.
  if [ "${target_dir_explicit:-false}" != "true" ] \
    && ! { [ -n "${_workflow_context_has_file:-}" ] && [ "${_workflow_context_file_status:-0}" != 0 ]; }; then
    outcome_unread="the run recorded no worktree or repository path in which its outcome could be read"
    return 0
  fi
  if [ "${metadata_corrupt:-false}" = "true" ]; then
    outcome_unread="the run's own workflow metadata is invalid, so there is no branch or pull request to read an outcome for"
    return 0
  fi
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
  # #1641 D5: what the run said its remote is, so a run that declared a
  # non-GitHub one is never asked about a GitHub pull request.
  raw="$(finalization_outcome_facts "$target_dir" "$run_branch" "$pr_ref" \
    "$(workflow_finalization_metadata remote_host_type || :)")" || raw=""
  for leaf in $_FINALIZATION_OUTCOME_LEAVES; do
    printf -v "outcome_$leaf" '%s' "$(printf '%s' "$raw" | jq -r --arg key "$leaf" \
      '.[$key] // "" | if type == "string" then . else "" end' 2>/dev/null || :)"
  done
  [ -z "$outcome_branch$outcome_local_head$outcome_pr_number$outcome_unread" ] || outcome_present="true"
}

# One line of the facts the result was reported against.
# shellcheck disable=SC2154
outcome_facts_line() {
  printf 'outcome.branch=%s,outcome.local_head=%s,outcome.remote_head=%s,outcome.pr=%s,outcome.pr_state=%s,outcome.pr_head=%s,outcome.checks=%s' \
    "${outcome_branch:-none}" "${outcome_local_head:-unread}" \
    "${outcome_remote_head:-none}" "${outcome_pr_number:-none}" \
    "${outcome_pr_state:-unread}" "${outcome_pr_head:-none}" "${outcome_checks:-unread}"
}
