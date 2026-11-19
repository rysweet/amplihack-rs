#!/usr/bin/env bash
# Finalization classification from the run's real outcome (issue #2343).
emit_without_jq() {
  printf '{"terminal_success":"false","terminal_state":"FAILED_MISSING_TOOLING","terminal_reason":"jq is required to validate finalization evidence","required_next_action":"Install jq or run from an environment with the bundled workflow tooling available.","hollow_success_detected":"false","evidence_used":"tooling.jq=missing","finalizer_schema_version":"1","finalizer_confidence":"low","finalizer_output_valid":"false","reporting_failure":"false","terminal_failure":"true"}\n'
}

validate_finalization() {
  if ! command -v jq >/dev/null 2>&1; then
    emit_without_jq
    echo "ERROR: FAILED_MISSING_TOOLING: jq is required to validate finalization" >&2
    exit 1
  fi

  # Issue #2343. The terminal state is read from what the run produced: an open
  # same-repo pull request at the local head (or a descendant of it), and that
  # request's check class, with the remote asked directly when no such request
  # vouches for the head. Bookkeeping about the run no longer decides it. A
  # missing or corrupt context file, an unset typed boolean and an empty
  # narrative are absence of bookkeeping, and absence of bookkeeping is not
  # failure; the outcome read answers what they stood in for. Agent prose is
  # still never read. The blockers that are about this run rather than its
  # output -- a dirty worktree, missing git or jq, a prior deterministic
  # blocker, empty agent output, a failed reporting step -- are unchanged and
  # still precede the outcome read: no published pull request can answer them.
  # A missing gh is not among them any more: without gh the outcome cannot be
  # read, which the result says in words instead of ending the run on it.
  #
  # Checks that are pending, or not reported at all, are no longer gated on
  # this run's own verification_completed (#2784): the pull request standing
  # open at the pushed head is the evidence, and the result hands that request
  # on with its number and URL rather than classifying finished work as an
  # implementation failure.

  # Set only from gh-confirmed facts (#1641, #1714); cleared here so an ambient
  # environment variable can never spoof them.
  local publish_state_override="" open_pr_confirmed_number="" open_pr_confirmed_url="" open_pr_confirmed_facts=""
  finalization_evidence="$(workflow_context_read finalization_evidence || :)"
  # A selected field that is present but corrupt is not absence. It is not a
  # terminal state of its own any more, and it never ends the run; but it also
  # cannot be repaired from a stale alias, and it withholds the bookkeeping
  # fallback further down, so unreadable evidence can never grant success.
  evidence_corrupt="false"
  while read -r evidence_name evidence_path evidence_alias; do
    if evidence_value="$(workflow_finalization_field finalization_evidence "$evidence_path" "$evidence_alias")"; then
      printf -v "$evidence_name" '%s' "$evidence_value"
    else
      field_status=$?
      [ "$field_status" = 2 ] || evidence_corrupt="true"
      printf -v "$evidence_name" '%s' ''
    fi
  done <<'FIELDS'
evidence_dirty_worktree git.dirty_worktree FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE
evidence_tooling_missing tooling.missing FINALIZATION_EVIDENCE_TOOLING_MISSING
evidence_gh_required tooling.gh_required FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED
evidence_prior_terminal_state prior_terminal_state.terminal_state FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE
evidence_hollow_success agent_outputs.hollow_success_signals FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS
FIELDS

  implementation_completed="$(resolve_implementation_completed)"
  verification_completed="$(resolve_verification_completed)"
  allow_no_op="$(resolve_allow_no_op)"
  terminal_no_op="$(resolve_terminal_no_op)"

  # Typed status of the reporting/finalization step recorded by the deterministic
  # finalizer-step-status recipe step (never scraped from agent prose).
  finalizer_step_status="$(workflow_finalization_field finalizer_step_status status FINALIZER_STEP_STATUS || :)"
  reporting_failure="$(boolish "$(workflow_finalization_field finalizer_step_status reporting_failure FINALIZER_REPORTING_FAILURE || :)")"
  case "$finalizer_step_status" in
    failed|FAILED|error|ERROR) reporting_failure="true" ;;
  esac

  pr_url="$(resolve_pr_url)"
  pr_number="$(resolve_pr_number)"
  prior_terminal_state="$(workflow_finalization_metadata prior_terminal_state || :)"

  finalizer_output_valid="false"
  finalizer_schema_version="1"
  finalizer_confidence="low"
  terminal_state="FAILED_IMPLEMENTATION"
  terminal_success="false"
  terminal_reason="implementation and verification evidence was absent or incomplete"
  required_next_action="Complete implementation and verification before finalization."
  hollow_success_detected="false"
  evidence_used="implementation_completed=$implementation_completed,verification_completed=$verification_completed"
  terminal_failure="true"

  emit_result() {
    local field separator publish_state_reached observed_phases missing_evidence
    publish_state_reached="${publish_state_override:-$(resolve_publish_state_reached)}"
    observed_phases="workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize"
    missing_evidence=""
    # File-backed metadata and diagnostics can exceed argv's per-value limit.
    # Stream every fixed field, retaining the existing all-String schema, then
    # validate the assembled object before emitting one complete JSON result.
    {
      printf '{'
      separator=''
      for field in terminal_success terminal_state terminal_reason required_next_action hollow_success_detected evidence_used finalizer_schema_version finalizer_confidence finalizer_output_valid reporting_failure implementation_completed verification_completed publish_state_reached terminal_no_op terminal_failure pr_url pr_number observed_phases missing_evidence; do
        printf '%s"%s":' "$separator" "$field"
        printf '%s' "${!field}" | jq -n --rawfile value /dev/stdin '$value'
        separator=','
      done
      printf '}'
    } | jq -c .
  }

  fail_result() {
    terminal_state="$1"
    terminal_reason="$2"
    required_next_action="$3"
    evidence_used="$4"
    terminal_success="false"
    terminal_failure="true"
    finalizer_output_valid="false"
    emit_result
    echo "ERROR: workflow finalization failed closed: $terminal_state: $terminal_reason" >&2
    exit 1
  }

  classify_success() {
    terminal_state="$1"
    terminal_reason="$2"
    required_next_action="$3"
    evidence_used="$4"
    terminal_success="true"
    terminal_failure="false"
    finalizer_output_valid="true"
    finalizer_confidence="high"
    hollow_success_detected="false"
    emit_result
    exit 0
  }

  # 1. Selected evidence fields. A whole object that cannot be parsed, or a
  #    field of the wrong type, marks the evidence corrupt and is then simply
  #    unread; the outcome below is what classifies the run.
  if [ -n "$finalization_evidence" ]; then
    if ! evidence_values="$(printf '%s' "$finalization_evidence" | jq -cs '
        if length == 1 and (.[0] | type) == "object" then
          .[0] | [ .git.dirty_worktree, .tooling.missing, .tooling.gh_required,
                   .prior_terminal_state.terminal_state,
                   .agent_outputs.hollow_success_signals ]
          | if all(.[]; if type == "string" then index("\u0000") == null
                        else type == "boolean" or type == "null" end) then
              map(if . == null then "" else tostring end)
            else error("invalid finalization evidence field") end
        else error("finalization evidence is not one JSON object") end' 2>/dev/null)"; then
      evidence_corrupt="true"
    elif ! {
      IFS= read -rd '' ev_dirty &&
        IFS= read -rd '' ev_missing &&
        IFS= read -rd '' ev_gh &&
        IFS= read -rd '' ev_prior &&
        IFS= read -rd '' ev_hollow
    } < <(printf '%s' "$evidence_values" | jq -j '.[] + "\u0000"' 2>/dev/null); then
      evidence_corrupt="true"
    else
      [ -n "$evidence_dirty_worktree" ] || evidence_dirty_worktree="$ev_dirty"
      [ -n "$evidence_tooling_missing" ] || evidence_tooling_missing="$ev_missing"
      [ -n "$evidence_gh_required" ] || evidence_gh_required="$ev_gh"
      [ -n "$evidence_prior_terminal_state" ] || evidence_prior_terminal_state="$ev_prior"
      [ -n "$evidence_hollow_success" ] || evidence_hollow_success="$ev_hollow"
    fi
  fi

  [ -n "$prior_terminal_state" ] || prior_terminal_state="$evidence_prior_terminal_state"

  # 2. Blockers about the local checkout, which no outcome can answer. gh is
  #    deliberately absent from this list (#2343): see step 4.
  if [ "$evidence_dirty_worktree" = "true" ]; then
    fail_result "FAILED_DIRTY_WORKTREE" "collected evidence reported a dirty worktree" "Commit, stash, or remove uncommitted changes before finalization." "finalization_evidence.git.dirty_worktree=true"
  fi
  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$target_dir" ] || target_dir="$(workflow_finalization_metadata repo_path || :)"
  target_dir_explicit="true"
  [ -n "$target_dir" ] || { target_dir="."; target_dir_explicit="false"; }
  # Fall back to a live worktree probe only when the deterministic evidence did
  # not already report a dirty-worktree signal; collected evidence is
  # authoritative (issue #969, requirement R2).
  if [ -z "$evidence_dirty_worktree" ] || [ "$evidence_dirty_worktree" = "unknown" ]; then
    if command -v git >/dev/null 2>&1 && [ -d "$target_dir" ] && git -C "$target_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      if [ -n "$(git -C "$target_dir" status --porcelain)" ]; then
        fail_result "FAILED_DIRTY_WORKTREE" "dirty worktree prevents terminal success" "Commit, stash, or remove uncommitted changes before finalization." "git.dirty_worktree=true"
      fi
    fi
  fi
  case ",$evidence_tooling_missing," in
    *,git,*|*,jq,*)
      fail_result "FAILED_MISSING_TOOLING" "collected evidence reported missing deterministic tooling: $evidence_tooling_missing" "Run finalization from an environment with required git and jq tooling available." "finalization_evidence.tooling.missing=$evidence_tooling_missing"
      ;;
  esac

  # A deterministic terminal probe that already named a blocker is answered
  # from its own record, without asking the remote again.
  case "$prior_terminal_state" in
    BLOCKED_CI|FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_PR_METADATA_UNAVAILABLE|FAILED_INVALID_INPUT|FAILED_WRONG_BRANCH)
      finalization_fail_or_handoff "$prior_terminal_state" "deterministic terminal probe reported $prior_terminal_state" "Resolve the deterministic blocker before finalization." "prior_terminal_state=$prior_terminal_state"
      ;;
  esac

  # Empty or inaccessible agent output means the run may have produced nothing
  # at all, so it stays a blocker ahead of any success classification (#969).
  if [ "$evidence_hollow_success" = "true" ]; then
    hollow_success_detected="true"
    fail_result "HOLLOW_SUCCESS" "collected evidence reported hollow-success signals" "Continue implementation/verification or report the inaccessible/empty agent output." "finalization_evidence.agent_outputs.hollow_success_signals=true"
  fi

  # Authoritative metadata that is present but corrupt is swallowed into "" by
  # the reads above. It must never feed a success path, so any such key leaves
  # the outcome unread (#1641). Absent metadata is not corrupt (#1726): it
  # leaves the reader to discover what it can instead.
  metadata_corrupt="false"
  for metadata_key in worktree_path repo_path pr_url pr_number publish_state_reached prior_terminal_state publish_status branch_name remote_host_type; do
    metadata_status=0
    (workflow_finalization_metadata "$metadata_key" >/dev/null 2>&1) || metadata_status=$?
    [ "$metadata_status" != 3 ] || metadata_corrupt="true"
  done

  # 3. Implementation-vs-reporting split (issue #969). A failed reporting step
  #    is classified distinctly from an implementation failure and preserves
  #    the durable implementation/verification/PR evidence. It is answered
  #    before the outcome read: a missing narrative is a fact about this run's
  #    own reporting, which no published pull request can speak to, and a
  #    confirmed PR never turns a reporting failure into success. Inside
  #    auto-drive (#1714) a PR the outcome read confirms is handed to the
  #    crusty and merge-ready loops on both exits; the read happens only with
  #    the hand-off flag, so a standalone run makes no extra gh call.
  if [ "$reporting_failure" = "true" ]; then
    if [ "$(finalization_handoff_flag)" = "true" ]; then
      read_live_outcome
      case "$outcome_verdict" in
        SUCCESS|BLOCKED)
          open_pr_confirmed_number="$outcome_pr_number"
          open_pr_confirmed_url="$outcome_pr_url"
          open_pr_confirmed_facts="$(finalization_outcome_facts_line)" ;;
      esac
    fi
    if [ "$evidence_corrupt" != "true" ] && [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
      reporting_reason="implementation and verification succeeded but a reporting/finalization step failed; durable evidence is preserved"
      reporting_next="Re-run the failed reporting step; implementation and verification evidence is durable and does not need to be redone."
      reporting_evidence="implementation_completed=true,verification_completed=true,reporting_failure=true"
      finalization_try_handoff_open_pr "FAILED_REPORTING" "$reporting_reason" "$reporting_next" "$reporting_evidence" || :
      finalizer_output_valid="true"
      terminal_state="FAILED_REPORTING"
      terminal_success="false"
      terminal_failure="true"
      terminal_reason="$reporting_reason"
      required_next_action="$reporting_next"
      evidence_used="$reporting_evidence"
      emit_result
      echo "ERROR: workflow finalization reached non-success terminal state: FAILED_REPORTING: $terminal_reason" >&2
      exit 1
    fi
    finalization_fail_or_handoff_open_pr "FAILED_IMPLEMENTATION" "reporting failed and durable implementation/verification evidence is absent" "Complete implementation and verification before finalization." "implementation_completed=$implementation_completed,verification_completed=$verification_completed,reporting_failure=true"
  fi

  # 4. The run's real outcome: the recorded one when collect-finalization-evidence
  #    wrote it, otherwise one live read. An absent, unparseable or unrecognised
  #    record routes to the live read, never to a terminal failure.
  read_recorded_outcome
  [ -n "$outcome_verdict" ] || read_live_outcome

  classify_confirmed_outcome ""

  # 5. The agentic assessment of an ambiguous situation (#2809). Its own
  #    verdict is never taken on trust: a SUCCESS, or any action it reports
  #    taking, costs exactly one mechanical re-read, and the run is classified
  #    on that re-read. The assessment only ever supplies the explanation and
  #    the record of what it did.
  apply_finalization_assessment

  # 6. The bookkeeping fallback. Evidence that could not be read withholds it
  #    entirely, so an unreadable run can never borrow success from it.
  if [ "$evidence_corrupt" != "true" ]; then
    # Typed completion booleans speak only for a run that has no GitHub pull
    # request to read an outcome from at all: no GitHub origin, a remote the
    # run itself declared non-GitHub, or no recorded worktree to look in.
    # Against a GitHub origin an unread fact is a gap, never a success, so
    # they can never stand in for a push.
    if [ "$outcome_github_origin" = "false" ] && [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
      classify_success "IMPLEMENTED_VERIFIED" "implementation and verification evidence is complete, and the finalization worktree has no GitHub remote whose published outcome could be read" "No action required." "implementation_completed=true,verification_completed=true,github_origin=false"
    fi
    # An explicit no-op declares that there was nothing to publish, which a
    # missing pull request confirms rather than contradicts.
    if [ "$allow_no_op" = "true" ] && [ "$terminal_no_op" = "true" ]; then
      classify_success "ALLOW_NO_OP" "explicit no-op task with durable allow_no_op evidence" "No action required." "allow_no_op=true,terminal_no_op=true"
    fi
  fi

  # 7. No published outcome and no fallback. The reason names the real gap,
  #    and inside auto-drive a PR the outcome confirmed goes to the loops.
  final_reason="$outcome_gap"
  [ -n "$final_reason" ] || final_reason="implementation/verification evidence was absent or incomplete"
  [ "$evidence_corrupt" != "true" ] || final_reason="$final_reason; the collected finalization evidence could not be read"
  final_evidence="implementation_completed=$implementation_completed,verification_completed=$verification_completed,outcome=$outcome_verdict"
  [ "$evidence_corrupt" != "true" ] || final_evidence="$final_evidence,finalization_evidence=unreadable"
  finalization_fail_or_handoff_open_pr "FAILED_IMPLEMENTATION" "$final_reason" "Close the gap named above, then finalize again." "$final_evidence"
}
