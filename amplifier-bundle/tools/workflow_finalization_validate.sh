#!/usr/bin/env bash
# Strict finalization classification; hard blockers precede success.
emit_without_jq() {
  printf '{"terminal_success":"false","terminal_state":"FAILED_MISSING_TOOLING","terminal_reason":"jq is required to validate finalization evidence","required_next_action":"Install jq or run from an environment with the bundled workflow tooling available.","hollow_success_detected":"false","evidence_used":"tooling.jq=missing","finalizer_schema_version":"1","finalizer_confidence":"low","finalizer_output_valid":"false","reporting_failure":"false","terminal_failure":"true"}\n'
}

validate_finalization() {
  if ! command -v jq >/dev/null 2>&1; then
    emit_without_jq
    echo "ERROR: FAILED_MISSING_TOOLING: jq is required to validate finalization" >&2
    exit 1
  fi

  # Issue #969: terminal classification is derived EXCLUSIVELY from typed
  # deterministic evidence (FINALIZATION_EVIDENCE) plus typed recipe state
  # (implementation/verification completion + the finalizer reporting step
  # status). The agentic finalizer emits a human-readable narrative only; that
  # prose is NEVER read here and no jq/regex/fence-stripping is applied to any
  # agent-generated text. Implementation failure is classified separately from
  # reporting failure so durable evidence survives a failed reporting step.

  evidence_status=0
  finalization_evidence="$(workflow_context_read finalization_evidence)" || evidence_status=$?
  evidence_invalid="false"
  case "$evidence_status" in 0|2) ;; *) evidence_invalid="true" ;; esac
  # Retain invalid-field status instead of turning a failed read into absence.
  # Missing selected fields stay conservative and never trigger alias fallback.
  while read -r evidence_name evidence_path evidence_alias; do
    if evidence_value="$(workflow_finalization_field finalization_evidence "$evidence_path" "$evidence_alias")"; then
      printf -v "$evidence_name" '%s' "$evidence_value"
    else
      field_status=$?
      [ "$field_status" = 2 ] || evidence_invalid="true"
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
    publish_state_reached="$(resolve_publish_state_reached)"
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

  # 1. Reject invalid selected transport/fields before completion or no-op.
  if [ "$evidence_invalid" = "true" ]; then
    fail_result "FAILED_INVALID_EVIDENCE" "deterministic finalization evidence contained an invalid object or field" "Rerun collect-finalization-evidence and inspect its structured output." "finalization_evidence=malformed"
  fi
  if [ -n "$finalization_evidence" ]; then
    # Validate the complete array before emitting any delimiters. jq failures in
    # process substitution do not propagate through set -e, and partial output
    # could hide an invalid later field. Strings cannot contain the NUL separator;
    # absent/null and Boolean fields retain their supported semantics.
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
      fail_result "FAILED_INVALID_EVIDENCE" "deterministic finalization evidence contained an invalid object or field" "Rerun collect-finalization-evidence and inspect its structured output." "finalization_evidence=malformed"
    fi
    if ! {
      IFS= read -rd '' ev_dirty &&
        IFS= read -rd '' ev_missing &&
        IFS= read -rd '' ev_gh &&
        IFS= read -rd '' ev_prior &&
        IFS= read -rd '' ev_hollow
    } < <(printf '%s' "$evidence_values" | jq -j '.[] + "\u0000"' 2>/dev/null); then
      fail_result "FAILED_INVALID_EVIDENCE" "deterministic finalization evidence could not be completely extracted" "Rerun collect-finalization-evidence and inspect its structured output." "finalization_evidence=malformed"
    fi
    [ -n "$evidence_dirty_worktree" ] || evidence_dirty_worktree="$ev_dirty"
    [ -n "$evidence_tooling_missing" ] || evidence_tooling_missing="$ev_missing"
    [ -n "$evidence_gh_required" ] || evidence_gh_required="$ev_gh"
    [ -n "$evidence_prior_terminal_state" ] || evidence_prior_terminal_state="$ev_prior"
    [ -n "$evidence_hollow_success" ] || evidence_hollow_success="$ev_hollow"
  fi

  [ -n "$prior_terminal_state" ] || prior_terminal_state="$evidence_prior_terminal_state"

  # 2. Hard blockers. These cannot be masked by completion state and are checked
  #    before any success classification.
  if [ "$evidence_dirty_worktree" = "true" ]; then
    fail_result "FAILED_DIRTY_WORKTREE" "collected evidence reported a dirty worktree" "Commit, stash, or remove uncommitted changes before finalization." "finalization_evidence.git.dirty_worktree=true"
  fi
  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  [ -n "$target_dir" ] || target_dir="$(workflow_finalization_metadata repo_path || :)"
  [ -n "$target_dir" ] || target_dir="."
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
  if [ "$evidence_gh_required" = "true" ]; then
    case ",$evidence_tooling_missing," in
      *,gh,*)
        fail_result "FAILED_MISSING_TOOLING" "GitHub finalization requires gh, but collected evidence reported gh missing" "Install/authenticate gh or rerun from an environment with GitHub PR tooling available." "finalization_evidence.tooling.gh=missing"
        ;;
    esac
  fi
  case "$prior_terminal_state" in
    BLOCKED_CI|FAILED_MEANINGFUL_DIFF|FAILED_CLOSED_UNMERGED|FAILED_PR_METADATA_UNAVAILABLE|FAILED_INVALID_INPUT|FAILED_WRONG_BRANCH)
      fail_result "$prior_terminal_state" "deterministic terminal probe reported $prior_terminal_state" "Resolve the deterministic blocker before finalization." "prior_terminal_state=$prior_terminal_state"
      ;;
  esac
  if [ "$evidence_hollow_success" = "true" ]; then
    hollow_success_detected="true"
    fail_result "HOLLOW_SUCCESS" "collected evidence reported hollow-success signals" "Continue implementation/verification or report the inaccessible/empty agent output." "finalization_evidence.agent_outputs.hollow_success_signals=true"
  fi

  # 3. Implementation-vs-reporting split (issue #969). A failed reporting step is
  #    classified distinctly from an implementation failure and preserves the
  #    durable implementation/verification/PR evidence.
  if [ "$reporting_failure" = "true" ]; then
    if [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
      finalizer_output_valid="true"
      terminal_state="FAILED_REPORTING"
      terminal_success="false"
      terminal_failure="true"
      terminal_reason="implementation and verification succeeded but a reporting/finalization step failed; durable evidence is preserved"
      required_next_action="Re-run the failed reporting step; implementation and verification evidence is durable and does not need to be redone."
      evidence_used="implementation_completed=true,verification_completed=true,reporting_failure=true"
      emit_result
      echo "ERROR: workflow finalization reached non-success terminal state: FAILED_REPORTING: $terminal_reason" >&2
      exit 1
    fi
    fail_result "FAILED_IMPLEMENTATION" "reporting failed and durable implementation/verification evidence is absent" "Complete implementation and verification before finalization." "implementation_completed=$implementation_completed,verification_completed=$verification_completed,reporting_failure=true"
  fi

  # 4. Success and no-op paths from durable typed evidence.
  if [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
    classify_success "IMPLEMENTED_VERIFIED" "implementation and verification evidence is complete" "No action required." "implementation_completed=true,verification_completed=true"
  fi
  if [ "$allow_no_op" = "true" ] && [ "$terminal_no_op" = "true" ]; then
    classify_success "ALLOW_NO_OP" "explicit no-op task with durable allow_no_op evidence" "No action required." "allow_no_op=true,terminal_no_op=true"
  fi

  # 5. Default: implementation/verification evidence absent.
  fail_result "FAILED_IMPLEMENTATION" "implementation/verification evidence was absent or incomplete" "Complete implementation and verification before finalization." "implementation_completed=$implementation_completed,verification_completed=$verification_completed"
}
