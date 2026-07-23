#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

mode="${1:-}"

# run_finalization_cleanup (issue #808)
# Deterministic, fail-soft finalization cleanup invoked before evidence
# collection so the agentic finalizer observes a clean state: no run-created
# fallback branches left on the shared remote, no leaked nested worktrees. All
# output is redirected to stderr so the evidence JSON on stdout stays intact;
# the cleanup never aborts the caller and only ever removes worktrees from a
# dedicated per-task worktree (sibling task worktrees are never touched).
run_finalization_cleanup() {
  local here helper repo_root worktree intended
  here="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd)" || return 0
  helper="$here/workflow_runtime_artifacts.sh"
  [ -f "$helper" ] || return 0
  repo_root="${REPO_PATH:-${RECIPE_VAR_repo_path:-$(pwd)}}"
  worktree="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
  intended="${BRANCH_NAME:-${RECIPE_VAR_branch_name:-${WORKTREE_SETUP_BRANCH_NAME:-${RECIPE_VAR_worktree_setup__branch_name:-}}}}"
  ( . "$helper" 2>/dev/null && finalize_workflow_cleanup_entry "$repo_root" "$worktree" "$intended" ) >&2 || true
  return 0
}

boolish() {
  case "$1" in
    true|1|yes|y) printf 'true' ;;
    *) printf 'false' ;;
  esac
}

# Single source of truth for the typed recipe-state fallback precedence. Both
# collect_evidence() and validate_finalization() read the same durable signals;
# centralizing the fallback chains here keeps the two call sites from drifting.
resolve_implementation_completed() {
  boolish "${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-false}}}"
}
resolve_verification_completed() {
  boolish "${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-false}}}"
}
resolve_terminal_no_op() {
  boolish "${TERMINAL_NO_OP:-${IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_implementation_terminal_evidence__terminal_no_op:-false}}}"
}
resolve_allow_no_op() {
  boolish "${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}"
}
resolve_publish_state_reached() {
  boolish "${PUBLISH_STATE_REACHED:-${PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED:-false}}"
}
resolve_pr_url() {
  printf '%s' "${TERMINAL_STATE_PR_URL:-${RECIPE_VAR_terminal_state__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}"
}
resolve_pr_number() {
  printf '%s' "${TERMINAL_STATE_PR_NUMBER:-${RECIPE_VAR_terminal_state__pr_number:-${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}}}"
}

collect_evidence() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: collect-finalization-evidence requires jq for structured JSON evidence" >&2
    exit 2
  fi

  target_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
  if [ -z "$target_dir" ] || [ "$target_dir" = "''" ]; then
    target_dir="${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}"
  fi
  [ -n "$target_dir" ] || target_dir="."

  repo_valid="false"
  dirty_worktree="unknown"
  branch_name="${BRANCH_NAME:-${RECIPE_VAR_branch_name:-}}"
  head_sha=""
  base_ref="${BASE_REF:-${RECIPE_VAR_base_ref:-}}"
  remote_host_type="${REMOTE_HOST_TYPE:-${RECIPE_VAR_remote_host_type:-other}}"
  meaningful_diff="unknown"
  commits_ahead="${TERMINAL_STATE_COMMITS_AHEAD:-${RECIPE_VAR_terminal_state__commits_ahead:-}}"
  missing_tooling=""
  github_remote="false"
  gh_required="false"

  if ! command -v git >/dev/null 2>&1; then
    missing_tooling="${missing_tooling}${missing_tooling:+,}git"
  elif [ -d "$target_dir" ] && git -C "$target_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    repo_valid="true"
    if [ -z "$branch_name" ]; then
      branch_name="$(git -C "$target_dir" branch --show-current)"
    fi
    head_sha="$(git -C "$target_dir" rev-parse --verify HEAD)"
    remote_origin="$(git -C "$target_dir" config --get remote.origin.url 2>/dev/null || true)"
    case "$remote_origin" in
      git@github.com:*|ssh://git@github.com/*|https://github.com/*|http://github.com/*|https://*@github.com/*|http://*@github.com/*) github_remote="true" ;;
    esac
    if [ -n "$(git -C "$target_dir" status --porcelain)" ]; then
      dirty_worktree="true"
    else
      dirty_worktree="false"
    fi
    if [ -z "$base_ref" ]; then
      base_ref="$(git -C "$target_dir" symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || printf '')"
    fi
    [ -n "$base_ref" ] || base_ref="origin/main"
    if git -C "$target_dir" rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null; then
      if git -C "$target_dir" diff --quiet "${base_ref}..HEAD"; then
        meaningful_diff="false"
      else
        meaningful_diff="true"
      fi
      if [ -z "$commits_ahead" ]; then
        commits_ahead="$(git -C "$target_dir" rev-list --count "${base_ref}..HEAD")"
      fi
    fi
  fi

  if ! command -v gh >/dev/null 2>&1; then
    missing_tooling="${missing_tooling}${missing_tooling:+,}gh"
  fi

  implementation_completed="$(resolve_implementation_completed)"
  verification_completed="$(resolve_verification_completed)"
  publish_state_reached="$(resolve_publish_state_reached)"
  terminal_no_op="$(resolve_terminal_no_op)"
  allow_no_op="$(resolve_allow_no_op)"
  prior_terminal_state="${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${TERMINAL_STATE:-}}}"
  prior_terminal_success="$(boolish "${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-false}}")"
  prior_terminal_reason="${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-}}"
  branch_diff_status="${TERMINAL_STATE_BRANCH_DIFF_STATUS:-${RECIPE_VAR_terminal_state__branch_diff_status:-$meaningful_diff}}"
  pr_url="$(resolve_pr_url)"
  pr_number="$(resolve_pr_number)"
  publish_status="${TERMINAL_STATE_PUBLISH_STATUS:-${RECIPE_VAR_terminal_state__publish_status:-${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}}}"
  if [ "$publish_status" = "FOLLOWUP_CREATED" ] || [ -n "$pr_url" ]; then
    publish_state_reached="true"
  fi
  case "$pr_url" in
    https://github.com/*|http://github.com/*|https://*@github.com/*|http://*@github.com/*) github_remote="true" ;;
  esac
  if { [ "$remote_host_type" = "github" ] || [ "$github_remote" = "true" ]; } && { [ "$meaningful_diff" != "false" ] || [ -n "$pr_url" ] || [ -n "$pr_number" ]; }; then
    gh_required="true"
  fi

  jq -nc \
    --arg schema_version "1" \
    --arg repo_path "${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}" \
    --arg worktree_path "$target_dir" \
    --arg repo_valid "$repo_valid" \
    --arg branch_name "$branch_name" \
    --arg head_sha "$head_sha" \
    --arg base_ref "$base_ref" \
    --arg dirty_worktree "$dirty_worktree" \
    --arg meaningful_diff "$meaningful_diff" \
    --arg commits_ahead "$commits_ahead" \
    --arg missing_tooling "$missing_tooling" \
    --arg remote_host_type "$remote_host_type" \
    --arg github_remote "$github_remote" \
    --arg gh_required "$gh_required" \
    --arg pr_url "$pr_url" \
    --arg pr_number "$pr_number" \
    --arg publish_status "$publish_status" \
    --arg implementation_completed "$implementation_completed" \
    --arg verification_completed "$verification_completed" \
    --arg publish_state_reached "$publish_state_reached" \
    --arg terminal_no_op "$terminal_no_op" \
    --arg allow_no_op "$allow_no_op" \
    --arg prior_terminal_state "$prior_terminal_state" \
    --arg prior_terminal_success "$prior_terminal_success" \
    --arg prior_terminal_reason "$prior_terminal_reason" \
    --arg branch_diff_status "$branch_diff_status" \
    --arg observed_phases "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize" \
    '{
      schema_version: ($schema_version | tonumber),
      git: {
        repo_path: $repo_path,
        worktree_path: $worktree_path,
        repo_valid: $repo_valid,
        branch_name: $branch_name,
        head_sha: $head_sha,
        base_ref: $base_ref,
        dirty_worktree: $dirty_worktree,
        meaningful_diff: $meaningful_diff,
        branch_diff_status: $branch_diff_status,
        commits_ahead: $commits_ahead
      },
      tooling: {
        jq: "present",
        git_missing: ($missing_tooling | split(",") | index("git") != null),
        gh_missing: ($missing_tooling | split(",") | index("gh") != null),
        gh_required: $gh_required,
        missing: $missing_tooling
      },
      remote: {
        host_type: $remote_host_type,
        github_remote: $github_remote
      },
      pr: {
        present: (($pr_url != "") or ($pr_number != "")),
        url: $pr_url,
        number: $pr_number,
        publish_status: $publish_status
      },
      ci: {
        state: (if $prior_terminal_state == "BLOCKED_CI" then "FAILURE" else "UNKNOWN" end)
      },
      completion: {
        implementation_completed: $implementation_completed,
        verification_completed: $verification_completed,
        publish_state_reached: $publish_state_reached,
        terminal_no_op: $terminal_no_op,
        allow_no_op: $allow_no_op
      },
      prior_terminal_state: {
        terminal_success: $prior_terminal_success,
        terminal_state: $prior_terminal_state,
        terminal_reason: $prior_terminal_reason
      },
      observed_phases: ($observed_phases | split(",")),
      agent_outputs: {
        hollow_success_signals: "unknown"
      }
    }'
}

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

  finalization_evidence="${FINALIZATION_EVIDENCE:-${RECIPE_VAR_finalization_evidence:-}}"
  evidence_dirty_worktree="${FINALIZATION_EVIDENCE_GIT_DIRTY_WORKTREE:-${RECIPE_VAR_finalization_evidence__git__dirty_worktree:-}}"
  evidence_tooling_missing="${FINALIZATION_EVIDENCE_TOOLING_MISSING:-${RECIPE_VAR_finalization_evidence__tooling__missing:-}}"
  evidence_gh_required="${FINALIZATION_EVIDENCE_TOOLING_GH_REQUIRED:-${RECIPE_VAR_finalization_evidence__tooling__gh_required:-}}"
  evidence_prior_terminal_state="${FINALIZATION_EVIDENCE_PRIOR_TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_finalization_evidence__prior_terminal_state__terminal_state:-}}"
  evidence_hollow_success="${FINALIZATION_EVIDENCE_AGENT_OUTPUTS_HOLLOW_SUCCESS_SIGNALS:-${RECIPE_VAR_finalization_evidence__agent_outputs__hollow_success_signals:-}}"

  implementation_completed="$(resolve_implementation_completed)"
  verification_completed="$(resolve_verification_completed)"
  allow_no_op="$(resolve_allow_no_op)"
  terminal_no_op="$(resolve_terminal_no_op)"

  # Typed status of the reporting/finalization step recorded by the deterministic
  # finalizer-step-status recipe step (never scraped from agent prose).
  finalizer_step_status="${FINALIZER_STEP_STATUS:-${RECIPE_VAR_finalizer_step_status__status:-ok}}"
  reporting_failure="$(boolish "${RECIPE_VAR_finalizer_step_status__reporting_failure:-${FINALIZER_REPORTING_FAILURE:-false}}")"
  case "$finalizer_step_status" in
    failed|FAILED|error|ERROR) reporting_failure="true" ;;
  esac

  pr_url="$(resolve_pr_url)"
  pr_number="$(resolve_pr_number)"
  prior_terminal_state="${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-}}"

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
    jq -nc \
      --arg terminal_success "$terminal_success" \
      --arg terminal_state "$terminal_state" \
      --arg terminal_reason "$terminal_reason" \
      --arg required_next_action "$required_next_action" \
      --arg hollow_success_detected "$hollow_success_detected" \
      --arg evidence_used "$evidence_used" \
      --arg finalizer_schema_version "$finalizer_schema_version" \
      --arg finalizer_confidence "$finalizer_confidence" \
      --arg finalizer_output_valid "$finalizer_output_valid" \
      --arg reporting_failure "$reporting_failure" \
      --arg implementation_completed "$implementation_completed" \
      --arg verification_completed "$verification_completed" \
      --arg publish_state_reached "$(resolve_publish_state_reached)" \
      --arg terminal_no_op "$terminal_no_op" \
      --arg terminal_failure "$terminal_failure" \
      --arg pr_url "$pr_url" \
      --arg pr_number "$pr_number" \
      --arg observed_phases "workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize" \
      --arg missing_evidence "" \
      '{
        terminal_success: $terminal_success,
        terminal_state: $terminal_state,
        terminal_reason: $terminal_reason,
        required_next_action: $required_next_action,
        hollow_success_detected: $hollow_success_detected,
        evidence_used: $evidence_used,
        finalizer_schema_version: $finalizer_schema_version,
        finalizer_confidence: $finalizer_confidence,
        finalizer_output_valid: $finalizer_output_valid,
        reporting_failure: $reporting_failure,
        implementation_completed: $implementation_completed,
        verification_completed: $verification_completed,
        publish_state_reached: $publish_state_reached,
        terminal_no_op: $terminal_no_op,
        terminal_failure: $terminal_failure,
        pr_url: $pr_url,
        pr_number: $pr_number,
        observed_phases: $observed_phases,
        missing_evidence: $missing_evidence
      }'
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

  # 1. Deterministic evidence must be a structurally valid JSON object when
  #    provided. Malformed *evidence* (never agent prose) fails closed as
  #    FAILED_INVALID_EVIDENCE.
  if [ -n "$finalization_evidence" ]; then
    # Perf (issue #969): avoid re-parsing the same JSON document once per field
    # (the pre-#969 code spawned a printf+jq per field). We do one type-check
    # pass (fail-close on non-object/malformed input) plus one extraction pass.
    #
    # Security (issue #969, fail-CLOSED invariant): a terminal success here
    # authorizes a merge, so the extraction MUST NOT silently drop a later
    # blocker field. Fields are emitted NUL-delimited and consumed with per-field
    # `read -rd ''` fed by process substitution. NUL is the one byte that cannot
    # appear in the collected evidence values, so — unlike a whitespace/0x1f
    # delimiter with a single space-splitting `read` — an embedded newline or
    # control byte in an early value can never truncate the record and blank a
    # trailing blocker (dirty_worktree / tooling.missing / hollow_success).
    # Empty fields stay positional. Process substitution (not command
    # substitution) is required so NUL bytes survive to `read`.
    if ! printf '%s' "$finalization_evidence" | jq -e 'type == "object"' >/dev/null 2>&1; then
      fail_result "FAILED_INVALID_EVIDENCE" "deterministic finalization evidence was not a JSON object" "Rerun collect-finalization-evidence and inspect its structured output." "finalization_evidence=malformed"
    fi
    {
      IFS= read -rd '' ev_dirty
      IFS= read -rd '' ev_missing
      IFS= read -rd '' ev_gh
      IFS= read -rd '' ev_prior
      IFS= read -rd '' ev_hollow
    } < <(printf '%s' "$finalization_evidence" | jq -j '
        [ (.git.dirty_worktree // ""),
          (.tooling.missing // ""),
          (.tooling.gh_required // ""),
          (.prior_terminal_state.terminal_state // ""),
          (.agent_outputs.hollow_success_signals // "") ]
        | map(tostring + "\u0000") | join("")')
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
  target_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}}}"
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

complete_workflow() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: workflow-complete requires jq to report workflow_result" >&2
    exit 2
  fi
  jq -n \
    --arg task "${TASK_DESCRIPTION:-}" \
    --arg issue "${ISSUE_NUMBER:-}" \
    --arg pr "${WORKFLOW_RESULT_PR_URL:-${RECIPE_VAR_workflow_result__pr_url:-${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}}}" \
    --arg terminal_state "${WORKFLOW_RESULT_TERMINAL_STATE:-${RECIPE_VAR_workflow_result__terminal_state:-FAILED_INVALID_EVIDENCE}}" \
    --arg terminal_success "${WORKFLOW_RESULT_TERMINAL_SUCCESS:-${RECIPE_VAR_workflow_result__terminal_success:-false}}" \
    --arg terminal_reason "${WORKFLOW_RESULT_TERMINAL_REASON:-${RECIPE_VAR_workflow_result__terminal_reason:-validated workflow_result missing from context}}" \
    --arg required_next_action "${WORKFLOW_RESULT_REQUIRED_NEXT_ACTION:-${RECIPE_VAR_workflow_result__required_next_action:-Inspect validate-agentic-finalization output.}}" \
    --arg hollow_success_detected "${WORKFLOW_RESULT_HOLLOW_SUCCESS_DETECTED:-${RECIPE_VAR_workflow_result__hollow_success_detected:-false}}" \
    --arg evidence_used "${WORKFLOW_RESULT_EVIDENCE_USED:-${RECIPE_VAR_workflow_result__evidence_used:-workflow_result=missing}}" \
    --arg finalizer_schema_version "${WORKFLOW_RESULT_FINALIZER_SCHEMA_VERSION:-${RECIPE_VAR_workflow_result__finalizer_schema_version:-0}}" \
    --arg finalizer_confidence "${WORKFLOW_RESULT_FINALIZER_CONFIDENCE:-${RECIPE_VAR_workflow_result__finalizer_confidence:-low}}" \
    --arg finalizer_output_valid "${WORKFLOW_RESULT_FINALIZER_OUTPUT_VALID:-${RECIPE_VAR_workflow_result__finalizer_output_valid:-false}}" \
    --arg reporting_failure "${WORKFLOW_RESULT_REPORTING_FAILURE:-${RECIPE_VAR_workflow_result__reporting_failure:-false}}" \
    --arg terminal_failure "${WORKFLOW_RESULT_TERMINAL_FAILURE:-${RECIPE_VAR_workflow_result__terminal_failure:-true}}" \
    '{
      workflow: "default-workflow",
      version: "2.0.0",
      task: $task,
      issue_number: $issue,
      pr_url: $pr,
      terminal_outcome: $terminal_state,
      terminal_state: $terminal_state,
      terminal_success: $terminal_success,
      terminal_reason: $terminal_reason,
      workflow_result: {
        terminal_success: $terminal_success,
        terminal_state: $terminal_state,
        terminal_reason: $terminal_reason,
        required_next_action: $required_next_action,
        hollow_success_detected: $hollow_success_detected,
        evidence_used: $evidence_used,
        finalizer_schema_version: $finalizer_schema_version,
        finalizer_confidence: $finalizer_confidence,
        finalizer_output_valid: $finalizer_output_valid,
        reporting_failure: $reporting_failure,
        terminal_failure: $terminal_failure
      },
      required_next_action: $required_next_action,
      hollow_success_detected: $hollow_success_detected,
      evidence_used: $evidence_used,
      finalizer_schema_version: $finalizer_schema_version,
      finalizer_confidence: $finalizer_confidence,
      finalizer_output_valid: $finalizer_output_valid,
      reporting_failure: $reporting_failure,
      terminal_failure: $terminal_failure,
      terminal_vocabulary: ["MERGED", "CLOSED_OBSOLETE", "NO_DIFF_SUCCESS", "FOLLOWUP_CREATED", "SUPERSEDED", "IMPLEMENTED_VERIFIED", "ALLOW_NO_OP", "BLOCKED_CI", "FAILED_IMPLEMENTATION", "FAILED_REPORTING", "FAILED_MISSING_TOOLING", "FAILED_PR_METADATA_UNAVAILABLE", "FAILED_DIRTY_WORKTREE", "FAILED_MEANINGFUL_DIFF", "FAILED_CLOSED_UNMERGED", "FAILED_INVALID_EVIDENCE", "FAILED_MISSING_TERMINAL_EVIDENCE", "HOLLOW_SUCCESS", "INCOMPLETE"]
    }'
}

case "$mode" in
  collect) run_finalization_cleanup; collect_evidence ;;
  validate) validate_finalization ;;
  complete) complete_workflow ;;
  *)
    echo "ERROR: workflow_agentic_finalization.sh requires mode: collect, validate, or complete" >&2
    exit 2
    ;;
esac
