#!/usr/bin/env bash
# Finalization's terminal state (issues #2343, #2809).
#
# This file does not decide what the run produced. It does three things:
#   1. the blockers about the local checkout that no published outcome can
#      answer: a dirty worktree, missing git or jq, and a prior deterministic
#      terminal probe that already named a blocker;
#   2. confirm that the facts of the run's outcome were gathered -- the ones
#      collect-finalization-evidence recorded, or one live read when it
#      recorded none;
#   3. report the terminal state assess-finalization decided on those facts,
#      refusing to certify only the one decision that contradicts its own
#      definition: a state that IS an open pull request, naming none.
#
# What it does NOT do: compare the local head with a remote head or a pull
# request head, read a check class as a success or a failure, infer a terminal
# state from typed completion booleans or from a missing narrative, or re-read
# the outcome behind the assessment's back. Collecting the facts is
# workflow_finalization_outcome.sh's job and it only collects; judging them is
# assess-finalization's (workflow-finalization-assessment.yaml); whether the
# state it names is one finalization publishes is
# workflow_finalization_result.sh's. Every exit from here that is not the
# assessment's own answer is a failure to reach a decision, not a decision: it
# leaves validate-agentic-finalization failing for its recovery agent, which
# stops with the reason (docs/reference/step-failure-recovery.md).
#
# #2819 R2: that recovery agent cannot repair its way out, and the step is
# wired `--no-repair` so it is never offered the attempt. Everything this file
# decides on was recorded before the step first ran -- the decision from
# finalization_assessment, the facts and the blockers from
# finalization_evidence -- so a second run of the same body reads the same
# values and reaches the same end. Nothing the agent is permitted to do
# changes them: it may make no GitHub write, and may start no recipe, so it
# cannot produce an assessment where there was none. Repairing the run's
# outcome belongs to assess-finalization. #2819 R3: retrying THAT step is the
# recipe's job -- assessment-decision-status asks whether it produced a
# decision and reassess-finalization runs it once more when it did not, both
# upstream of here, so a failed model call costs a second call rather than a
# finished run.
#
# The facts themselves are obtained and rendered by
# workflow_finalization_facts.sh, which this file requires to be sourced
# first: gather_outcome_facts, outcome_facts_line and corrupt_evidence_note.

validate_finalization() {
  if ! command -v jq >/dev/null 2>&1; then
    emit_without_jq
    echo "ERROR: FAILED_MISSING_TOOLING: jq is required to validate finalization" >&2
    exit 1
  fi

  # Set only from the gathered facts and the assessment's answer; cleared here
  # so an ambient environment variable can never spoof them.
  local no_decision="" publish_state_override="" open_pr_confirmed_number="" open_pr_confirmed_url="" open_pr_confirmed_facts=""
  local outcome_present="false" outcome_branch="" outcome_local_head="" outcome_remote=""
  local outcome_remote_head="" outcome_pr_number="" outcome_pr_url="" outcome_pr_state=""
  local outcome_pr_head="" outcome_checks="" outcome_unread=""
  local assessment_state="" assessment_action="" assessment_pr="" assessment_pr_raw="" assessment_reason="" assessment_next=""
  finalization_evidence="$(workflow_context_read finalization_evidence || :)"
  # A selected field that is present but corrupt is not absence. It is not a
  # terminal state of its own and it never ends the run; it is reported beside
  # the result so the reason says the evidence could not be read. That is safe
  # because no blocker rests on this record alone: a dirty_worktree that does
  # not read as exactly "true" or exactly "false" is probed live against the
  # worktree below -- the probe fires on an unread field for the same reason it
  # fires on "yes" or "1", because none of them is an answer (R4) -- git and jq
  # are proven present by this script running at all, and
  # prior_terminal_state falls back to the run's own metadata. The terminal
  # state itself is decided by assess-finalization on facts collected fresh,
  # not on this record, which is the whole point of #2343: a bookkeeping
  # artefact must not fail a run that finished its work.
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

  # Typed status of the reporting step, never scraped from agent prose. Since
  # #2343 it is a fact for assess-finalization and for the result, not a
  # terminal state this file reaches on its own.
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
  terminal_state="FAILED_MISSING_TERMINAL_EVIDENCE"
  terminal_success="false"
  terminal_reason="finalization reached no decision"
  required_next_action="Re-run finalization."
  hollow_success_detected="false"
  evidence_used="implementation_completed=$implementation_completed,verification_completed=$verification_completed"
  terminal_failure="true"

  # 1. Selected evidence fields. A whole object that cannot be parsed, or a
  #    field of the wrong type, marks the evidence corrupt and is then simply
  #    unread; it decides no terminal state.
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
  [ "$evidence_hollow_success" != "true" ] || hollow_success_detected="true"

  # 2. Blockers about the local checkout, which no published outcome can
  #    answer. gh is deliberately absent (#2343): without it the outcome
  #    cannot be read, which the facts say in words instead.
  if [ "$evidence_dirty_worktree" = "true" ]; then
    fail_result "FAILED_DIRTY_WORKTREE" "collected evidence reported a dirty worktree" "Commit, stash, or remove uncommitted changes before finalization." "finalization_evidence.git.dirty_worktree=true"
  fi
  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$target_dir" ] || target_dir="$(workflow_finalization_metadata repo_path || :)"
  target_dir_explicit="true"
  [ -n "$target_dir" ] || { target_dir="."; target_dir_explicit="false"; }
  # Live worktree probe unless the collected record answered the question, and
  # it answers it only by saying exactly "true" (handled above) or exactly
  # "false" (#969, R2, R4). Anything else is not an answer and never stands in
  # for one: empty, "unknown", "yes", "1", a JSON number rendered as "0". R4
  # reproduced the cost of treating one of those as a negative -- a worktree
  # with uncommitted changes whose record read "1" published EXISTING_OPEN_PR
  # -- so a record that does not say true or false sends the question to the
  # worktree, which can always answer it.
  if [ "$evidence_dirty_worktree" != "false" ]; then
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

  # Metadata present but corrupt is swallowed into "" by the reads above, so it
  # must not direct a live outcome read (#1641). Absent metadata is not corrupt
  # (#1726): it leaves the reader to discover what it can instead.
  metadata_corrupt="false"
  for metadata_key in worktree_path repo_path pr_url pr_number publish_state_reached prior_terminal_state publish_status branch_name remote_host_type; do
    metadata_status=0
    (workflow_finalization_metadata "$metadata_key" >/dev/null 2>&1) || metadata_status=$?
    [ "$metadata_status" != 3 ] || metadata_corrupt="true"
  done

  # 3. The facts. Nothing here reads them as a success or a failure.
  gather_outcome_facts
  if [ "$outcome_present" != "true" ]; then
    fail_result "FAILED_MISSING_TERMINAL_EVIDENCE" \
      "the facts of this run's outcome could not be gathered: ${outcome_unread:-the outcome reader produced nothing}" \
      "Make the run's outcome readable -- a recorded worktree on the run branch -- and finalize again." \
      "outcome=ungathered$(corrupt_evidence_note)"
  fi

  # 4. The terminal state assess-finalization decided on those facts. The
  #    result names the pull request the assessment judged, or the one the
  #    facts carry; no URL is ever adopted from the assessment's own text, the
  #    canonical one is built for the remote the facts report.
  assessment_state="$(workflow_finalization_field finalization_assessment terminal_state || :)"
  assessment_action="$(workflow_finalization_field finalization_assessment action_taken || :)"
  assessment_pr="$(workflow_finalization_field finalization_assessment pr_number || :)"
  assessment_reason="$(_finalization_outcome_clean "$(workflow_finalization_field finalization_assessment reason || :)")"
  assessment_next="$(_finalization_outcome_clean "$(workflow_finalization_field finalization_assessment next_action || :)")"
  case "$assessment_action" in ''|none) assessment_action="none" ;; *) assessment_action="$(_finalization_outcome_clean "$assessment_action")" ;; esac
  # What came back, kept only to quote it back when it is not a pull request
  # number, the way an unusable terminal_state is quoted below: cleaned of
  # control characters and cut to 40 characters, since a pull request number is
  # at most ten digits. It is a sentence for a human and reaches no identity
  # field of the result.
  assessment_pr_raw="$(LC_ALL=C printf '%s' "$(_finalization_outcome_clean "$assessment_pr")" | LC_ALL=C cut -c1-40)"
  # The result names the pull request the assessment judged. A value that is
  # not a pull request number names none, and an assessment that names none is
  # taken at its word, so a decision about no pull request is never handed to
  # the auto-drive loops carrying one (#1714, case 2). Only when nothing
  # decided does the record fall back to what the facts read.
  _finalization_pr_number_valid "$assessment_pr" || assessment_pr=""
  _finalization_state_allowed "$assessment_state" || assessment_pr="$outcome_pr_number"
  if _finalization_pr_number_valid "$assessment_pr"; then
    pr_number="$assessment_pr"
    pr_url=""
    [ -z "$outcome_remote" ] || pr_url="https://github.com/$outcome_remote/pull/$assessment_pr"
    open_pr_confirmed_number="$pr_number"
    open_pr_confirmed_url="$pr_url"
    [ "$outcome_pr_state" != "OPEN" ] || publish_state_override="true"
  fi

  evidence_used="$(outcome_facts_line),assessment_state=${assessment_state:-none},assessment_action=$assessment_action,reporting_failure=$reporting_failure"
  evidence_used="$evidence_used$(corrupt_evidence_note)"
  open_pr_confirmed_facts="$evidence_used"

  if _finalization_state_allowed "$assessment_state"; then
    [ -n "$assessment_reason" ] || assessment_reason="assess-finalization reported $assessment_state on the run's outcome"
    [ -n "$assessment_next" ] || assessment_next="See the finalization assessment."
    # #2819 R1: a state whose definition is an open pull request, reported
    # without one, has not been shown. open_pr_confirmed_number is set only from a pull
    # request number the assessment itself returned in canonical form, so this
    # refuses exactly the decision that contradicts its own definition; it
    # does not re-judge the facts, and it never substitutes a number from the
    # run's own bookkeeping for the one the assessment was asked to read back.
    # That is no decision, which is the recovery agent's to close, not a
    # verdict about the run.
    if _finalization_state_names_pr "$assessment_state" && [ -z "$open_pr_confirmed_number" ]; then
      no_decision="assess-finalization reported $assessment_state, which is a pull request open at this run's head, but named none"
      if [ -n "$assessment_pr_raw" ]; then
        no_decision="$no_decision: it returned pr_number \"$assessment_pr_raw\", which is not a pull request number"
      else
        no_decision="$no_decision: it returned an empty pr_number"
      fi
      fail_result "FAILED_MISSING_TERMINAL_EVIDENCE" "$no_decision" \
        "Run finalization again so assess-finalization reports the number of the open pull request it read, or the state the facts do support; re-running this step alone reads the same recorded answer." \
        "$evidence_used"
    fi
    if _finalization_state_is_success "$assessment_state"; then
      classify_success "$assessment_state" "$assessment_reason" "$assessment_next" "$evidence_used"
    fi
    finalization_fail_or_handoff_open_pr "$assessment_state" "$assessment_reason" "$assessment_next" "$evidence_used"
  fi

  # 5. No decision. The facts were gathered and nothing judged them: a gap in
  #    finalization itself, not a verdict about the run. The step is left
  #    failing for its recovery agent, whose move is to hand off with this
  #    reason: no repair it may make can put a decision here (#2819 R2).
  #
  #    The two gaps are named apart (#2819 R3): an answer naming a state
  #    finalization does not publish is quoted back, while an absent one is
  #    reported as the step having delivered nothing, which is what the
  #    operator has to act on. The absent case is reached only after
  #    assessment-status gave that step a second attempt through
  #    reassess-finalization, so both attempts came back empty.
  if [ -n "$assessment_state" ]; then
    no_decision="assess-finalization reported no terminal state for this run's outcome; it returned \"$(_finalization_outcome_clean "$assessment_state")\", which is not a state finalization publishes"
  else
    no_decision="assess-finalization delivered no terminal state for this run's outcome: neither it nor the re-run after it returned an answer a state could be read from, so the run was never judged"
  fi
  [ -z "$outcome_unread" ] || no_decision="$no_decision; the facts it would have judged are incomplete: $outcome_unread"
  fail_result "FAILED_MISSING_TERMINAL_EVIDENCE" "$no_decision" \
    "Run finalization again so assess-finalization decides on the gathered outcome, or report why it cannot; re-running this step alone reads the same recorded answer." \
    "$evidence_used"
}
