#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq CLI not found - cannot validate workflow agentic finalization JSON" >&2
  exit 2
fi
if ! command -v git >/dev/null 2>&1; then
  echo "ERROR: git CLI not found - cannot collect workflow agentic finalization evidence" >&2
  exit 2
fi
if ! command -v timeout >/dev/null 2>&1; then
  echo "ERROR: timeout CLI not found - cannot bound workflow agentic finalization" >&2
  exit 2
fi

tmp_files=()
cleanup_tmp_files() {
  local file
  for file in "${tmp_files[@]}"; do
    [ ! -e "$file" ] || rm -f "$file"
  done
}
trap cleanup_tmp_files EXIT

new_tmp_file() {
  local file
  file="$(mktemp -t workflow-agentic-finalization-XXXXXX)"
  tmp_files+=("$file")
  printf '%s\n' "$file"
}

redact_sensitive_output() {
  sed -E 's#(https?://)[^@[:space:]]+@#\1REDACTED@#g' "$1" | tr '\n' ' ' | head -c 1000
}

json_array_from_lines() {
  if [ "$#" -eq 0 ]; then
    jq -nc '[]'
    return 0
  fi
  printf '%s\n' "$@" | jq -R -s -c 'split("\n")[:-1]'
}

normalize_bool() {
  local raw="$1"
  case "$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')" in
    true|1|yes|y) printf 'true\n' ;;
    *) printf 'false\n' ;;
  esac
}

repo_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
[ -n "$repo_dir" ] || repo_dir="${REPO_PATH:-${RECIPE_VAR_repo_path:-.}}"
cd "$repo_dir"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
  echo "ERROR: workflow_agentic_finalization.sh requires a git worktree at $repo_dir" >&2
  exit 2
}

current_branch="$(git branch --show-current)"
expected_branch="${BRANCH_NAME:-${RECIPE_VAR_branch_name:-}}"
case "$expected_branch" in "{{branch_name}}"|"''") expected_branch="" ;; esac
if [ -n "$expected_branch" ] && [ "$current_branch" != "$expected_branch" ]; then
  jq -nc \
    --arg reason "branch mismatch: current branch '$current_branch' does not match expected '$expected_branch'" \
    '{schema_version:"1",decision:"blocked",terminal_success:false,terminal_state:"FAILED_INVALID_EVIDENCE",terminal_reason:$reason,publish_status:"FAILED_INVALID_EVIDENCE",ready_for_review:false,confidence:"0",blocking_reasons:[$reason],blocking_reasons_text:$reason,evidence_summary:"branch identity mismatch",artifact_scope:{safe:true,blocked_paths:[],runtime_artifacts_present:false},evidence:{},agent_assessment:{summary:"blocked before agentic finalization",hollow_success:true,required_actions:[$reason]}}'
  echo "ERROR: workflow agentic finalization blocked: branch mismatch" >&2
  exit 1
fi

base_ref="${BASE_REF:-${RECIPE_VAR_base_ref:-}}"
case "$base_ref" in "{{base_ref}}"|"''") base_ref="" ;; esac
if [ -z "$base_ref" ]; then
  for candidate in origin/HEAD origin/main origin/master origin/develop main master develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null 2>&1; then
      base_ref="$candidate"
      break
    fi
  done
fi

status_porcelain="$(git status --porcelain)"
staged_paths="$(git diff --cached --name-only)"
untracked_paths="$(git ls-files --others --exclude-standard)"
runtime_artifacts_present="false"
generated_runtime_artifacts=()
if [ -e ".claude/runtime" ]; then
  runtime_artifacts_present="true"
  generated_runtime_artifacts+=(".claude/runtime")
fi
while IFS= read -r path; do
  [ -n "$path" ] || continue
  case "$path" in
    .claude/runtime|.claude/runtime/*) generated_runtime_artifacts+=("$path") ;;
  esac
done <<EOF
$staged_paths
$untracked_paths
EOF
while IFS= read -r line; do
  [ -n "$line" ] || continue
  case "$line" in
    *".claude/runtime"*) generated_runtime_artifacts+=("$line") ;;
  esac
done <<EOF
$status_porcelain
EOF

generated_runtime_artifacts_json="$(json_array_from_lines "${generated_runtime_artifacts[@]}")"
artifact_safe="true"
if [ "$(printf '%s' "$generated_runtime_artifacts_json" | jq 'length')" -gt 0 ]; then
  artifact_safe="false"
fi

issue_number="${ISSUE_NUMBER:-${RECIPE_VAR_issue_number:-}}"
task_description="${TASK_DESCRIPTION:-${RECIPE_VAR_task_description:-}}"
pr_url="${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}"
pr_number="${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}"
terminal_success="$(normalize_bool "${FINAL_STATUS_TERMINAL_SUCCESS:-${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-false}}}")"
terminal_state="${FINAL_STATUS_TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-FOLLOWUP_CREATED}}}"
terminal_reason="${FINAL_STATUS_TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-agentic finalization is evaluating deterministic terminal evidence}}}"
publish_status="${FINAL_STATUS_PUBLISH_STATUS:-${TERMINAL_STATE_PUBLISH_STATUS:-${RECIPE_VAR_terminal_state__publish_status:-$terminal_state}}}"
commits_ahead="unknown"
if [ -n "$base_ref" ]; then
  commits_ahead="$(git rev-list --count "${base_ref}..HEAD")"
fi

staged_paths_json="$(printf '%s\n' "$staged_paths" | jq -R -s -c 'split("\n")[:-1] | map(select(length > 0))')"
untracked_paths_json="$(printf '%s\n' "$untracked_paths" | jq -R -s -c 'split("\n")[:-1] | map(select(length > 0))')"
status_porcelain_json="$(printf '%s\n' "$status_porcelain" | jq -R -s -c 'split("\n")[:-1] | map(select(length > 0))')"
evidence_file="${FINALIZATION_EVIDENCE_FILE:-}"
if [ -z "$evidence_file" ]; then
  evidence_file="$(new_tmp_file)"
fi

jq -nc \
  --arg task "$task_description" \
  --arg issue_number "$issue_number" \
  --arg current_branch "$current_branch" \
  --arg base_ref "$base_ref" \
  --arg commits_ahead "$commits_ahead" \
  --arg pr_url "$pr_url" \
  --arg pr_number "$pr_number" \
  --arg terminal_success "$terminal_success" \
  --arg terminal_state "$terminal_state" \
  --arg terminal_reason "$terminal_reason" \
  --arg publish_status "$publish_status" \
  --argjson status_porcelain "$status_porcelain_json" \
  --argjson staged_paths "$staged_paths_json" \
  --argjson untracked_paths "$untracked_paths_json" \
  --argjson generated_runtime_artifacts "$generated_runtime_artifacts_json" \
  --arg artifact_safe "$artifact_safe" \
  --arg runtime_artifacts_present "$runtime_artifacts_present" \
  '{
    task:$task,
    issue_number:$issue_number,
    branch:$current_branch,
    base_ref:$base_ref,
    commits_ahead:$commits_ahead,
    pr_url:$pr_url,
    pr_number:$pr_number,
    terminal:{success:($terminal_success == "true"),state:$terminal_state,reason:$terminal_reason,publish_status:$publish_status},
    git:{status_porcelain:$status_porcelain,staged_paths:$staged_paths,untracked_paths:$untracked_paths},
    artifact_scope:{safe:($artifact_safe == "true"),blocked_paths:$generated_runtime_artifacts,runtime_artifacts_present:($runtime_artifacts_present == "true")}
  }' >"$evidence_file"

emit_blocked() {
  local reason="$1" summary="$2"
  local terminal_state_value="${3:-FAILED_INVALID_EVIDENCE}"
  jq -nc \
    --arg reason "$reason" \
    --arg summary "$summary" \
    --arg terminal_state_value "$terminal_state_value" \
    --slurpfile evidence "$evidence_file" \
    '{
      schema_version:"1",
      decision:"blocked",
      terminal_success:false,
      terminal_state:$terminal_state_value,
      terminal_reason:$reason,
      publish_status:$terminal_state_value,
      ready_for_review:false,
      confidence:"0",
      blocking_reasons:[$reason],
      blocking_reasons_text:$reason,
      evidence_summary:$summary,
      artifact_scope:($evidence[0].artifact_scope // {safe:false,blocked_paths:[],runtime_artifacts_present:false}),
      evidence:($evidence[0] // {}),
      agent_assessment:{summary:$summary,hollow_success:true,required_actions:[$reason]}
    }'
}

if [ "$artifact_safe" != "true" ]; then
  emit_blocked "generated_runtime_artifacts present in finalization scope" "Artifact scope contains generated .claude/runtime artifacts"
  echo "ERROR: workflow agentic finalization blocked: generated runtime artifacts are present in artifact_scope" >&2
  exit 1
fi

if [ -n "$status_porcelain" ]; then
  emit_blocked "dirty worktree blocks agentic finalization" "git status --porcelain reported uncommitted work" "FAILED_DIRTY_WORKTREE"
  echo "ERROR: workflow agentic finalization blocked: dirty worktree" >&2
  exit 1
fi

agent_binary="${AMPLIHACK_AGENT_BINARY:-}"
if [ -z "$agent_binary" ]; then
  emit_blocked "AMPLIHACK_AGENT_BINARY is required for bounded agentic finalization" "missing agent binary"
  echo "ERROR: AMPLIHACK_AGENT_BINARY is required for agentic finalization" >&2
  exit 2
fi
if ! command -v "$agent_binary" >/dev/null 2>&1; then
  emit_blocked "AMPLIHACK_AGENT_BINARY '$agent_binary' is not executable on PATH" "agent binary unavailable"
  echo "ERROR: AMPLIHACK_AGENT_BINARY '$agent_binary' is not executable on PATH" >&2
  exit 2
fi

prompt_file="$(new_tmp_file)"
finalizer_output_file="$(new_tmp_file)"
stderr_file="$(new_tmp_file)"
cat >"$prompt_file" <<EOF
# agentic finalization

Assess whether this workflow branch is ready, blocked, needs_human, or finalized.
Use only the deterministic evidence below. Do not run commands.
Return exactly one JSON object with:
schema_version, decision, terminal_success, terminal_state, terminal_reason,
publish_status, ready_for_review, confidence, blocking_reasons, evidence_summary,
artifact_scope, evidence, and agent_assessment.

Valid decisions: ready, blocked, needs_human, finalized.
Valid terminal states include FOLLOWUP_CREATED, MERGED, CLOSED_OBSOLETE,
NO_DIFF_SUCCESS, BLOCKED_CI, FAILED_INVALID_EVIDENCE, and FAILED_DIRTY_WORKTREE.
Set agent_assessment.hollow_success=true if the evidence cannot prove the claim.

Evidence:
$(cat "$evidence_file")
EOF

timeout_seconds="${AGENTIC_FINALIZATION_TIMEOUT_SECONDS:-120}"
if ! timeout "$timeout_seconds" "$agent_binary" <"$prompt_file" >"$finalizer_output_file" 2>"$stderr_file"; then
  stderr_summary="$(redact_sensitive_output "$stderr_file")"
  emit_blocked "agentic finalization invocation failed; stderr: $stderr_summary" "bounded agent invocation failed"
  echo "ERROR: agentic finalization invocation failed; stderr: $stderr_summary" >&2
  exit 1
fi

schema_filter='
  type == "object" and
  (.schema_version | type == "string") and
  (.decision | type == "string") and
  (.terminal_success | type == "boolean") and
  (.terminal_state | type == "string" and length > 0) and
  (.terminal_reason | type == "string" and length > 0) and
  (.publish_status | type == "string" and length > 0) and
  (.ready_for_review | type == "boolean") and
  (.artifact_scope | type == "object") and
  (.artifact_scope.safe | type == "boolean") and
  (.artifact_scope.blocked_paths | type == "array") and
  (.evidence | type == "object") and
  (.agent_assessment | type == "object") and
  (.agent_assessment.hollow_success | type == "boolean") and
  (.agent_assessment.required_actions | type == "array")
'

if ! jq -e "$schema_filter" "$finalizer_output_file" >/dev/null; then
  emit_blocked "malformed_agentic_finalization" "agentic finalizer output was malformed"
  echo "ERROR: malformed_agentic_finalization: finalizer_output failed schema validation" >&2
  exit 1
fi

decision="$(jq -r '.decision' "$finalizer_output_file")"
case "$decision" in
  ready|blocked|needs_human|finalized) ;;
  *)
    emit_blocked "malformed_agentic_finalization: unknown decision '$decision'" "unknown decision"
    echo "ERROR: malformed_agentic_finalization: unknown decision '$decision'" >&2
    exit 1
    ;;
esac

hollow_success="$(jq -r '.agent_assessment.hollow_success' "$finalizer_output_file")"
if [ "$hollow_success" = "true" ] && { [ "$decision" = "ready" ] || [ "$decision" = "finalized" ]; }; then
  emit_blocked "hollow_success" "agentic finalizer claimed success without sufficient evidence"
  echo "ERROR: hollow_success: ready/finalized decisions require non-hollow evidence" >&2
  exit 1
fi

agent_artifact_safe="$(jq -r '.artifact_scope.safe' "$finalizer_output_file")"
if [ "$agent_artifact_safe" != "true" ] && [ "$decision" != "blocked" ] && [ "$decision" != "needs_human" ]; then
  emit_blocked "malformed_agentic_finalization: unsafe artifact_scope cannot be ready" "artifact_scope contradiction"
  echo "ERROR: malformed_agentic_finalization: unsafe artifact_scope cannot be ready" >&2
  exit 1
fi

agent_terminal_state="$(jq -r '.terminal_state' "$finalizer_output_file")"
agent_terminal_success="$(jq -r '.terminal_success' "$finalizer_output_file")"
case "$agent_terminal_state" in
  MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS)
    if [ "$agent_terminal_success" != "true" ] || [ "$decision" = "ready" ]; then
      emit_blocked "malformed_agentic_finalization: terminal success contradiction" "terminal_state contradiction"
      echo "ERROR: malformed_agentic_finalization: terminal success contradiction" >&2
      exit 1
    fi
    ;;
  FAILED_*|BLOCKED_CI|FAILED_DIRTY_WORKTREE)
    if [ "$decision" = "ready" ] || [ "$decision" = "finalized" ]; then
      emit_blocked "malformed_agentic_finalization: blocked terminal_state cannot be successful" "terminal_state contradiction"
      echo "ERROR: malformed_agentic_finalization: blocked terminal_state cannot be successful" >&2
      exit 1
    fi
    ;;
esac

jq -c \
  --slurpfile deterministic_evidence "$evidence_file" \
  '{
    schema_version,
    decision,
    terminal_success,
    terminal_state,
    terminal_reason,
    publish_status,
    ready_for_review,
    confidence: ((.confidence // .agent_assessment.confidence // 0) | tostring),
    blocking_reasons: (.blocking_reasons // .agent_assessment.required_actions // []),
    blocking_reasons_text: ((.blocking_reasons // .agent_assessment.required_actions // []) | join("\n")),
    evidence_summary: (.evidence_summary // .agent_assessment.summary // ""),
    artifact_scope,
    evidence: (.evidence + {deterministic:$deterministic_evidence[0]}),
    agent_assessment
  }' "$finalizer_output_file"
