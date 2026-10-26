#!/usr/bin/env bash
set -euo pipefail

echo "=== WORKFLOW COMPLETE ==="
echo ""

export GH_PAGER=cat PAGER=cat LESS=FRX

HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
PR_URL="${PR_URL:-${PR_PUBLISH_RESULT_PR_URL:-${RECIPE_VAR_pr_publish_result__pr_url:-}}}"
PR_NUMBER="${PR_NUMBER:-${PR_PUBLISH_RESULT_PR_NUMBER:-${RECIPE_VAR_pr_publish_result__pr_number:-}}}"
PUBLISH_STATE="${PR_PUBLISH_RESULT_STATE:-${RECIPE_VAR_pr_publish_result__state:-}}"
TASK_DESC="${TASK_DESCRIPTION:-}"
ISSUE_NUMBER="${ISSUE_NUMBER:-}"

terminal_success="false"
terminal_state_out=""
terminal_reason_out=""
terminal_failure="false"
implementation_completed="false"
verification_completed="false"
publish_state_reached="false"
terminal_no_op="false"
missing_evidence=""
invalid_evidence=""
normalized_bool="false"
final_status_rc=0
final_status_uncertain="false"
# Issue #1268: the scoped-PR match is a SIGNAL, not the arbiter of terminal
# success. These carry the adjudicated verdict when the signal misses.
scope_failure_reason=""
terminal_verdict_out=""
terminal_evidence_json=""
terminal_evidence_collected="false"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PR_SCOPE_HELPER="${WORKFLOW_PR_SCOPE_HELPER:-${SCRIPT_DIR}/workflow_pr_scope.sh}"
# workflow_pr_scope.sh validates headRefName, baseRefName, headRefOid,
# isCrossRepository, expected_pr_title_prefix, and created_after.
TERMINAL_EVIDENCE_HELPER="${WORKFLOW_TERMINAL_EVIDENCE_HELPER:-${SCRIPT_DIR}/workflow_terminal_evidence.sh}"
GH_RETRY_HELPER="${WORKFLOW_GH_RETRY_HELPER:-${SCRIPT_DIR}/workflow_gh_retry.sh}"
[ -f "$GH_RETRY_HELPER" ] || { echo "ERROR: workflow_final_status.sh requires the shared retry helper at $GH_RETRY_HELPER" >&2; exit 2; }
# shellcheck source=/dev/null
. "$GH_RETRY_HELPER"

# Best-effort final PR status read. This is a display-only read that the caller
# already tolerates failing (`|| true`); it must NOT stall the workflow's final
# reporting step waiting on a rate-limit reset (which can be up to an hour out).
# Pin GH_RETRY_MAX_RL_WINDOWS=0 so a rate-limit fails fast instead of sleeping
# through reset windows. Transient errors still get the short bounded retries.
# No REST fallback here: this is a display-only read.
gh_pr_view_with_retry() {
  GH_RETRY_MAX_RL_WINDOWS=0 _gh_retry_core "final PR status pr view" pr view "$@"
}

normalize_bool() {
  local name="$1"
  local raw="$2"
  local value
  value="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]')"
  case "$value" in
    ""|"false"|"0"|"no"|"n") normalized_bool="false" ;;
    "true"|"1"|"yes"|"y") normalized_bool="true" ;;
    *)
      invalid_evidence="${invalid_evidence}${invalid_evidence:+,}${name}"
      normalized_bool="false"
      ;;
  esac
}

join_missing_evidence() {
  local result=""
  [ "$implementation_completed" = "true" ] || result="${result}${result:+,}implementation_completed"
  [ "$verification_completed" = "true" ] || result="${result}${result:+,}verification_completed"
  [ "$publish_state_reached" = "true" ] || result="${result}${result:+,}publish_state_reached"
  [ "$terminal_no_op" = "true" ] || result="${result}${result:+,}terminal_no_op"
  printf '%s\n' "$result"
}

emit_terminal_evidence() {
  echo "terminal_success=$terminal_success"
  echo "terminal_state=$terminal_state_out"
  [ -z "$terminal_reason_out" ] || echo "terminal_reason=$terminal_reason_out"
  echo "implementation_completed=$implementation_completed"
  echo "verification_completed=$verification_completed"
  echo "publish_state_reached=$publish_state_reached"
  echo "terminal_no_op=$terminal_no_op"
  echo "terminal_failure=$terminal_failure"
  [ -z "${OBSERVED_PHASES:-}" ] || echo "observed_phases=${OBSERVED_PHASES}"
  [ -z "$missing_evidence" ] || echo "missing_evidence=$missing_evidence"
  [ -z "$invalid_evidence" ] || echo "invalid_evidence=$invalid_evidence"
}

impl_input="${IMPLEMENTATION_COMPLETED:-${IMPLEMENTATION_TERMINAL_EVIDENCE_IMPLEMENTATION_COMPLETED:-${RECIPE_VAR_implementation_terminal_evidence__implementation_completed:-}}}"
verify_input="${VERIFICATION_COMPLETED:-${VERIFICATION_TERMINAL_EVIDENCE_VERIFICATION_COMPLETED:-${RECIPE_VAR_verification_terminal_evidence__verification_completed:-}}}"
publish_input="${PUBLISH_STATE_REACHED:-${PUBLISH_TERMINAL_EVIDENCE_PUBLISH_STATE_REACHED:-${RECIPE_VAR_publish_terminal_evidence__publish_state_reached:-}}}"
no_op_input="${TERMINAL_NO_OP:-${IMPLEMENTATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_implementation_terminal_evidence__terminal_no_op:-${VERIFICATION_TERMINAL_EVIDENCE_TERMINAL_NO_OP:-${RECIPE_VAR_verification_terminal_evidence__terminal_no_op:-}}}}}"
failure_input="${TERMINAL_FAILURE:-${TERMINAL_STATE_TERMINAL_FAILURE:-${RECIPE_VAR_terminal_state__terminal_failure:-}}}"
allow_no_op_input="${ALLOW_NO_OP:-${RECIPE_VAR_allow_no_op:-false}}"
probe_success_input="${TERMINAL_STATE_TERMINAL_SUCCESS:-${RECIPE_VAR_terminal_state__terminal_success:-}}"
probe_state_input="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-}}}"

normalize_bool "IMPLEMENTATION_COMPLETED" "$impl_input"
implementation_completed="$normalized_bool"
normalize_bool "VERIFICATION_COMPLETED" "$verify_input"
verification_completed="$normalized_bool"
normalize_bool "PUBLISH_STATE_REACHED" "$publish_input"
publish_state_reached="$normalized_bool"
normalize_bool "TERMINAL_NO_OP" "$no_op_input"
terminal_no_op="$normalized_bool"
normalize_bool "TERMINAL_FAILURE" "$failure_input"
terminal_failure="$normalized_bool"
normalize_bool "ALLOW_NO_OP" "$allow_no_op_input"
allow_no_op="$normalized_bool"
normalize_bool "TERMINAL_STATE_TERMINAL_SUCCESS" "$probe_success_input"
probe_success="$normalized_bool"

if [ "$allow_no_op" = "true" ]; then
  terminal_no_op="true"
fi

if [ "$probe_success" = "true" ]; then
  case "$probe_state_input" in
    MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS)
      terminal_no_op="true"
      ;;
  esac
fi

case "$PUBLISH_STATE" in
  BLOCKED_PROVIDER)
    terminal_success="false"
    terminal_state_out="BLOCKED_PROVIDER"
    terminal_reason_out="${PR_PUBLISH_RESULT_MESSAGE:-${RECIPE_VAR_pr_publish_result__message:-provider publication failed or was ambiguous}}"
    terminal_failure="true"
    emit_terminal_evidence
    echo "ERROR: workflow publish provider is blocked: $terminal_reason_out" >&2
    exit 1
    ;;
  no-diff|NO_DIFF_SUCCESS|CLOSED_OBSOLETE)
    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
      RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${SCRIPT_DIR}/workflow_runtime_artifacts.sh}"
      [ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found: $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
      # shellcheck source=/dev/null
      . "$RUNTIME_ARTIFACT_HELPER"
      preflight_known_workflow_runtime_artifacts .
      base_ref="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
      [ -n "$base_ref" ] || base_ref="origin/main"
      if git rev-parse --verify --quiet "${base_ref}^{commit}" >/dev/null && [ -z "$(git status --porcelain)" ] && git diff --quiet "${base_ref}..HEAD"; then
        if [ "$PUBLISH_STATE" = "no-diff" ]; then
          PUBLISH_STATE="NO_DIFF_SUCCESS"
        fi
      else
        echo "ERROR: publish reported terminal no-diff/obsolete state but final clean-worktree diff could not confirm that state" >&2
        exit 1
      fi
    else
      echo "ERROR: publish reported terminal no-diff/obsolete state but final clean-worktree diff could not confirm that state" >&2
      exit 1
    fi
    ;;
esac

parse_github_repo_identity() {
  local url="$1" path owner repo
  case "$url" in
    git@github.com:*) path="${url#git@github.com:}" ;;
    ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
    https://*@github.com/*|http://*@github.com/*) path="${url#*://}"; path="${path#*@github.com/}" ;;
    https://github.com/*) path="${url#https://github.com/}" ;;
    http://github.com/*) path="${url#http://github.com/}" ;;
    *) return 1 ;;
  esac
  path="${path%%\?*}"
  path="${path%%#*}"
  path="${path%.git}"
  case "$path" in */*) ;; *) return 1 ;; esac
  owner="${path%%/*}"
  repo="${path#*/}"
  repo="${repo%%/*}"
  [ -n "$owner" ] && [ -n "$repo" ] || return 1
  printf '%s/%s\n' "$owner" "$repo"
}

resolve_expected_base_branch() {
  local candidate
  candidate="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null || true)"
  if [ -n "$candidate" ] && git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "${candidate#origin/}"; return 0; fi
  for candidate in origin/main origin/master origin/develop; do
    if git rev-parse --verify --quiet "${candidate}^{commit}" >/dev/null; then printf '%s\n' "${candidate#origin/}"; return 0; fi
  done
  return 1
}

validate_final_pr_scope() {
  local current_branch local_head repo_identity expected_base scoped_json reason created_after
  [ -x "$PR_SCOPE_HELPER" ] || { echo "ERROR: workflow_pr_scope.sh missing or not executable at $PR_SCOPE_HELPER" >&2; return 1; }
  command -v jq >/dev/null 2>&1 || { echo "ERROR: jq CLI not found — cannot validate scoped final PR metadata" >&2; return 127; }
  current_branch="$(git branch --show-current 2>/dev/null || true)"
  local_head="$(git rev-parse --verify HEAD 2>/dev/null || true)"
  repo_identity="$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" || true)"
  expected_base="$(resolve_expected_base_branch || true)"
  [ -n "$repo_identity" ] && [ -n "$current_branch" ] && [ -n "$local_head" ] && [ -n "$expected_base" ] || {
    echo "ERROR: scoped final PR validation lacks repo, branch, headRefOid, or baseRefName context" >&2
    return 1
  }
  created_after="${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}"
  scoped_issue_id="${ISSUE_NUMBER:-${RECIPE_VAR_issue_number:-}}"
  # issue_number can carry a non-numeric local tracking reference (e.g.
  # local-5d904cff4398) when the workflow fell back to local tracking (#815,
  # #804). PR-scope matching only understands numeric ids, so coerce a
  # non-numeric ref to empty: a local ref must not filter out the current-work PR.
  case "$scoped_issue_id" in ''|*[!0-9]*) scoped_issue_id="" ;; esac
  scope_args=(
    --repo "$repo_identity"
    --head "$current_branch"
    --base "$expected_base"
    --issue "$scoped_issue_id"
    --work-item "$scoped_issue_id"
    --head-sha "$local_head"
  )
  title_prefix="${EXPECTED_PR_TITLE_PREFIX:-${PR_EXPECTED_TITLE_PREFIX:-}}"
  [ -n "$title_prefix" ] && scope_args+=(--expected-pr-title-prefix "$title_prefix")
  [ -n "$PR_URL" ] && scope_args+=(--pr-url "$PR_URL")
  [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] && scope_args+=(--pr-number "$PR_NUMBER")
  [ -n "$created_after" ] && scope_args+=(--created-after "$created_after")
  if scoped_json="$("$PR_SCOPE_HELPER" "${scope_args[@]}")"; then
    PR_URL="$(printf '%s' "$scoped_json" | jq -r '.url // empty')"
    PR_NUMBER="$(printf '%s' "$scoped_json" | jq -r '(.number // "") | tostring')"
    return 0
  fi
  reason="$(printf '%s' "$scoped_json" | jq -r '.reason // ""' 2>/dev/null || true)"
  scope_failure_reason="${reason:-unknown}"
  echo "WARNING: scoped final PR match did not resolve: ${reason:-unknown}" >&2
  echo "WARNING: this is a SIGNAL, not a verdict (issue #1268); the run's actual artifacts decide." >&2
  return 1
}

# --- terminal-state evidence and adjudication (issue #1268) ----------------
#
# `workflow_pr_scope.sh` answers a semantic question with string matching over
# head/base refs. When it misses, this used to `exit 1` and the whole run was
# declared a failure — after its PR had been created, reviewed, audited and
# MERGED, and while two follow-up PRs it had opened were left with nobody to
# drive them. One rotted into a conflicted state.
#
# So the miss now hands off to evidence: is there a PR, did it merge, is the
# work in the base branch. The adjudicator is fail-CLOSED on a readable
# negative (no PR anywhere and unlanded work is FAILED, and no judgement can
# lift that) and fail-SOFT on an unreadable one (UNCERTAIN claims nothing but
# does not throw away live artifacts).
collect_terminal_evidence() {
  local created_after evidence_args
  [ "$terminal_evidence_collected" = "true" ] && return 0
  terminal_evidence_collected="true"
  [ -f "$TERMINAL_EVIDENCE_HELPER" ] || {
    echo "WARNING: terminal-evidence helper not found at $TERMINAL_EVIDENCE_HELPER; terminal state cannot be adjudicated from artifacts" >&2
    return 1
  }
  created_after="${WORKFLOW_STARTED_AT:-${RECIPE_STARTED_AT:-${TASK_STARTED_AT:-}}}"
  evidence_args=(--scope-reason "$scope_failure_reason")
  [ -n "$PR_URL" ] && evidence_args+=(--pr-url "$PR_URL")
  [[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] && evidence_args+=(--pr-number "$PR_NUMBER")
  [ -n "$created_after" ] && evidence_args+=(--created-after "$created_after")
  terminal_evidence_json="$(bash "$TERMINAL_EVIDENCE_HELPER" collect "${evidence_args[@]}" 2>/dev/null || true)"
  [ -n "${terminal_evidence_json//[[:space:]]/}" ] || return 1
  return 0
}

# Enumerate every outstanding open PR, whatever the verdict turns out to be.
# The incident's `no_scoped_pr` message named neither the PR that HAD merged
# nor the two that were left open, so nothing and nobody drove them.
report_outstanding_prs() {
  collect_terminal_evidence || return 0
  printf '%s' "$terminal_evidence_json" \
    | jq -r '"outstanding_pr_count=" + ((.outstanding_pr_count // 0) | tostring)' 2>/dev/null || return 0
  printf '%s' "$terminal_evidence_json" \
    | jq -r '(.outstanding_prs // [])[] | "outstanding_pr=#\(.number) [\(.state)] \(.url) — \(.title)"' 2>/dev/null || true
  return 0
}

adjudicate_terminal_state() {
  local agent_verdict adjudication rc
  terminal_verdict_out=""
  if ! collect_terminal_evidence; then
    # No evidence at all. Nothing is proven either way; refuse to claim success
    # and refuse to discard the run.
    terminal_verdict_out="UNCERTAIN"
    echo "WARNING: terminal state is UNCERTAIN: the run's artifacts could not be inspected." >&2
    return 0
  fi
  # A verdict supplied by the agentic evaluation step. It is passed through the
  # SAME lattice as any other judgement — it can downgrade freely, but it can
  # only confirm success when the collected evidence carries a positive
  # artifact, and it can never lift a hard negative.
  agent_verdict="${WORKFLOW_TERMINAL_AGENT_VERDICT:-${TERMINAL_ADJUDICATION_AGENT_VERDICT:-${RECIPE_VAR_terminal_adjudication__agent_verdict:-}}}"
  rc=0
  adjudication="$(printf '%s' "$terminal_evidence_json" \
    | bash "$TERMINAL_EVIDENCE_HELPER" adjudicate - --agent-verdict "$agent_verdict" --report-only)" || rc=$?
  if [ "$rc" -ne 0 ] || [ -z "${adjudication//[[:space:]]/}" ]; then
    terminal_verdict_out="UNCERTAIN"
    echo "WARNING: terminal state adjudication produced no verdict; treating as UNCERTAIN." >&2
    return 0
  fi
  terminal_verdict_out="$(printf '%s' "$adjudication" | jq -r '.terminal_verdict // "UNCERTAIN"' 2>/dev/null || echo UNCERTAIN)"
  echo "terminal_verdict=$terminal_verdict_out"
  printf '%s' "$adjudication" | jq -r '"terminal_verdict_source=" + (.verdict_source // "unknown")' 2>/dev/null || true
  printf '%s' "$adjudication" | jq -r '"terminal_verdict_reason=" + (.reason // "")' 2>/dev/null || true
  return 0
}

# --- branch PR lookup (issue #1596) -----------------------------------------
#
# A development run can open its PR from an agent step instead of the publish
# step. pr_publish_result.pr_url is then empty and the run used to close with
# FAILED_MISSING_TERMINAL_EVIDENCE while its PR sat open. Before deciding that
# publish evidence is missing, ask GitHub (read-only) whether the run's branch
# already has an open, same-repository PR.
#
# The run's branch is read from the run's worktree (WORKTREE_SETUP_WORKTREE_PATH),
# never blindly from cwd: the finalize step historically ran from REPO_PATH, the
# main checkout, whose branch is main, detached, or something unrelated. When a
# worktree path is given but is gone, or the observed branch disagrees with
# WORKTREE_SETUP_BRANCH_NAME / BRANCH_NAME, the lookup does not run. cwd is used
# only when the caller gives no worktree path at all AND cwd is not the REPO_PATH
# checkout (same toplevel); a caller that runs in REPO_PATH without a worktree
# path, such as step-22b's fallback, gets no lookup.
#
# Issue #1679: the lookup must not depend on outputs a run may not have
# recorded. When REMOTE_HOST_TYPE is unset (no workflow-prep output), the host
# is taken from the run worktree's origin: only a github.com origin parses, so
# any other host still gets no lookup. An explicit non-GitHub host type still
# wins. The PR's head commit must be the run worktree's HEAD, so a PR that does
# not hold exactly this run's work never counts.
#
# Fail closed: every guard, gh error, malformed result or candidate that fails
# validation leaves PR_URL, PR_NUMBER and PUBLISH_STATE untouched. stdout is
# never written; at most one fixed-phrase line goes to stderr. When the run has
# no recorded PR and the lookup is skipped because the run's worktree or branch
# cannot be established, one fixed-phrase INFO line names the reason, so a
# missing-evidence failure says which check stopped the lookup (#1679). Always
# returns 0 and never exits, so it is safe under `set -euo pipefail`.
branch_pr_lookup_warn() {
  echo "WARNING: branch PR lookup failed (#1596): $1" >&2
}
branch_pr_lookup_skip() {
  echo "INFO: branch PR lookup skipped (#1679): $1" >&2
}

resolve_open_pr_for_current_branch() {
  local branch base repo json pick cand_url cand_number url_number previous run_dir expected_branch head
  # Unset host type: decided below from the run worktree's origin (#1679).
  [ "$HOST_TYPE" = "github" ] || [ -z "${REMOTE_HOST_TYPE:-}" ] || return 0
  [ -z "$PR_URL" ] || return 0
  [ "$publish_state_reached" != "true" ] || return 0
  case "$PUBLISH_STATE" in
    FOLLOWUP_CREATED|EXISTING_OPEN_PR|MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS) return 0 ;;
  esac
  [ -z "$invalid_evidence" ] || return 0
  [ "$terminal_failure" != "true" ] || return 0
  [ "$terminal_no_op" != "true" ] || return 0
  # Implementation+verification evidence already closes the run; the lookup
  # only exists for the missing-evidence branch.
  if [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
    return 0
  fi
  command -v gh >/dev/null 2>&1 || return 0
  command -v jq >/dev/null 2>&1 || return 0
  run_dir="${WORKTREE_SETUP_WORKTREE_PATH:-${RECIPE_VAR_worktree_setup__worktree_path:-}}"
  if [ -n "$run_dir" ]; then
    [ -d "$run_dir" ] || { branch_pr_lookup_skip "recorded worktree path is gone"; return 0; }
  else
    run_dir="."
  fi
  git -C "$run_dir" rev-parse --is-inside-work-tree >/dev/null 2>&1 || { branch_pr_lookup_skip "run directory is not a git work tree"; return 0; }
  # No worktree path and cwd is the REPO_PATH checkout: REPO_PATH's branch is
  # not the run's branch (step-22b lands here on its unset-worktree fallback),
  # so do not look it up.
  if [ "$run_dir" = "." ] && [ -n "${REPO_PATH:-}" ]; then
    local cwd_top repo_top
    cwd_top="$(git rev-parse --show-toplevel 2>/dev/null || true)"
    repo_top="$(git -C "$REPO_PATH" rev-parse --show-toplevel 2>/dev/null || true)"
    [ -z "$repo_top" ] || [ "$cwd_top" != "$repo_top" ] || { branch_pr_lookup_skip "no worktree path recorded and cwd is REPO_PATH"; return 0; }
  fi

  branch="$(git -C "$run_dir" branch --show-current 2>/dev/null || true)"
  [ -n "$branch" ] || { branch_pr_lookup_skip "detached HEAD"; return 0; }
  expected_branch="${WORKTREE_SETUP_BRANCH_NAME:-${BRANCH_NAME:-${RECIPE_VAR_worktree_setup__branch_name:-}}}"
  [ -z "$expected_branch" ] || [ "$branch" = "$expected_branch" ] || { branch_pr_lookup_skip "worktree branch is not the recorded branch"; return 0; }
  case "$branch" in -*|main|master) return 0 ;; esac
  git check-ref-format --branch "$branch" >/dev/null 2>&1 || return 0
  base="$(cd "$run_dir" && resolve_expected_base_branch 2>/dev/null || true)"
  [ -z "$base" ] || [ "$branch" != "$base" ] || return 0

  # Never log the raw origin URL: it may embed credentials.
  repo="$(parse_github_repo_identity "$(git -C "$run_dir" config --get remote.origin.url 2>/dev/null || true)" 2>/dev/null || true)"
  [[ "$repo" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || return 0
  head="$(git -C "$run_dir" rev-parse --verify HEAD 2>/dev/null || true)"
  [[ "$head" =~ ^[0-9a-f]{40}$ ]] || return 0

  # Raw gh/retry stderr is dropped: it is untrusted and may carry PR content.
  if ! json="$(GH_RETRY_MAX_RL_WINDOWS=0 GH_RETRY_REST_FALLBACK="" _gh_retry_core "final status branch PR lookup" \
      pr list --repo "$repo" --head "$branch" --state open \
      --json url,number,isCrossRepository,headRefOid 2>/dev/null)"; then
    branch_pr_lookup_warn "gh failed"
    return 0
  fi
  # First same-repository entry with a non-empty url whose head is the run's
  # HEAD; fork PRs never count. Anything other than a JSON array is malformed.
  if ! pick="$(printf '%s' "$json" | jq -c --arg h "$head" 'if type != "array" then error("not an array") else
      [.[] | objects
      | select((.isCrossRepository // false) == false)
      | select((.url // "") != "")
      | select(.headRefOid == $h)][0] // empty end' 2>/dev/null)"; then
    branch_pr_lookup_warn "malformed JSON"
    return 0
  fi
  [ -n "$pick" ] || return 0

  # The trailing sentinel stops $(...) from stripping a trailing newline, so a
  # URL carrying one fails the anchored regex below instead of slipping past.
  cand_url="$(printf '%s' "$pick" | jq -j '.url | tostring' 2>/dev/null && printf '.')" || cand_url=""
  cand_url="${cand_url%.}"
  cand_number="$(printf '%s' "$pick" | jq -r '(.number // "") | tostring' 2>/dev/null || true)"

  if [[ ! "$cand_url" =~ ^https://github\.com/([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)/pull/([1-9][0-9]*)$ ]] \
      || [ "$(printf '%s' "${BASH_REMATCH[1]}" | tr '[:upper:]' '[:lower:]')" != "$(printf '%s' "$repo" | tr '[:upper:]' '[:lower:]')" ]; then
    branch_pr_lookup_warn "URL failed validation"
    return 0
  fi
  url_number="${BASH_REMATCH[2]}"
  if [[ ! "$cand_number" =~ ^[1-9][0-9]*$ ]] || [ "$cand_number" != "$url_number" ]; then
    branch_pr_lookup_warn "PR number mismatch"
    return 0
  fi

  previous="${PUBLISH_STATE//[^A-Za-z0-9_-]/}"
  # The PR came from a github.com origin, so the status block below may use gh.
  HOST_TYPE="github"
  PR_URL="$cand_url"
  PR_NUMBER="$cand_number"
  PUBLISH_STATE="EXISTING_OPEN_PR"
  echo "INFO: resolved open PR $PR_URL for branch $branch (previous publish state: '$previous'); treating as EXISTING_OPEN_PR (#1596)" >&2
  return 0
}

resolve_open_pr_for_current_branch

case "$PUBLISH_STATE" in
  FOLLOWUP_CREATED|EXISTING_OPEN_PR|MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS)
    publish_state_reached="true"
    ;;
esac
if [ -n "$PR_URL" ]; then
  publish_state_reached="true"
fi

if [ -n "$invalid_evidence" ]; then
  terminal_success="false"
  terminal_state_out="FAILED_INVALID_EVIDENCE"
  terminal_reason_out="invalid boolean evidence marker(s): $invalid_evidence"
  emit_terminal_evidence
  echo "ERROR: workflow terminal evidence contains invalid boolean value(s): $invalid_evidence" >&2
  exit 2
fi

if [ "$terminal_failure" = "true" ]; then
  terminal_success="false"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-TERMINAL_FAILURE}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-explicit terminal failure evidence was reported}}}"
  emit_terminal_evidence
  echo "ERROR: workflow terminal failure: $terminal_state_out: $terminal_reason_out" >&2
  exit 1
fi

if [ "$terminal_no_op" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-ALLOW_NO_OP}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-allow_no_op was explicitly selected for a non-code-change path}}}"
elif [ "$publish_state_reached" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-${PUBLISH_STATE:-FOLLOWUP_CREATED}}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-${PR_PUBLISH_RESULT_MESSAGE:-${RECIPE_VAR_pr_publish_result__message:-publish/PR state reached}}}}}"
elif [ "$implementation_completed" = "true" ] && [ "$verification_completed" = "true" ]; then
  terminal_success="true"
  terminal_state_out="${TERMINAL_STATE:-${TERMINAL_STATE_TERMINAL_STATE:-${RECIPE_VAR_terminal_state__terminal_state:-IMPLEMENTED_VERIFIED}}}"
  terminal_reason_out="${TERMINAL_REASON:-${TERMINAL_STATE_TERMINAL_REASON:-${RECIPE_VAR_terminal_state__terminal_reason:-implementation and verification evidence present}}}"
else
  terminal_success="false"
  terminal_state_out="FAILED_MISSING_TERMINAL_EVIDENCE"
  terminal_reason_out="development workflow reached closure without implementation+verification, publish/PR state, or explicit no-op evidence"
  missing_evidence="$(join_missing_evidence)"
  emit_terminal_evidence
  echo "ERROR: workflow terminal evidence is incomplete for a development/code-change workflow." >&2
  echo "ERROR: missing_evidence=$missing_evidence" >&2
  echo "ERROR: observed_phases=${OBSERVED_PHASES:-unknown}" >&2
  echo "ERROR: expected one of: implementation_completed=true with verification_completed=true; publish_state_reached=true; terminal_no_op=true; or terminal_failure=true." >&2
  exit 1
fi

emit_terminal_evidence

echo ""
case "$terminal_state_out" in
  MERGED|CLOSED_OBSOLETE|NO_DIFF_SUCCESS|FOLLOWUP_CREATED|EXISTING_OPEN_PR|IMPLEMENTED_VERIFIED|ALLOW_NO_OP|closed-after-merge|already-merged|no-diff)
    echo "terminal_status=$terminal_state_out"
    ;;
esac

if [ -z "$PR_URL" ]; then
  if [ "$HOST_TYPE" = "azdo" ]; then
    echo "PR Status: N/A (manual creation required)" >&2
  else
    echo "PR Status: N/A (no remote provider)" >&2
  fi
elif [ "$HOST_TYPE" = "github" ]; then
  echo "PR Status:"
  if command -v gh >/dev/null 2>&1; then
    if validate_final_pr_scope; then
      gh_pr_view_with_retry "$PR_URL" --json state,mergeable,reviews,statusCheckRollup || true
    else
      # FIX (#1268): a scope miss no longer ends the run. Adjudicate from the
      # artifacts the run actually produced.
      adjudicate_terminal_state
      case "$terminal_verdict_out" in
        SUCCESS)
          echo "INFO: scoped PR match missed (${scope_failure_reason:-unknown}) but the run's artifacts prove terminal success; not failing the run (issue #1268)." >&2
          ;;
        UNCERTAIN)
          final_status_uncertain="true"
          ;;
        *)
          final_status_rc=1
          ;;
      esac
    fi
    report_outstanding_prs
  else
    echo "WARNING: gh CLI not found; skipping final PR status lookup" >&2
  fi
else
  echo "WARNING: PR_URL set but host is '$HOST_TYPE'; skipping gh pr view" >&2
fi

echo ""
printf '=== Task: %s ===\n' "$TASK_DESC"
case "$HOST_TYPE" in
  github) echo "=== Issue: #${ISSUE_NUMBER} ===" ;;
  azdo) echo "=== Issue: AB#${ISSUE_NUMBER} ===" ;;
  *) echo "=== Issue: Ref #${ISSUE_NUMBER} ===" ;;
esac

if [ -n "$PR_URL" ]; then
  printf '=== PR: %s ===\n' "$PR_URL"
elif [ "$HOST_TYPE" = "azdo" ]; then
  echo "=== PR: N/A (manual creation required) ==="
else
  echo "=== PR: N/A (no remote provider) ==="
fi

echo ""
if [ "$final_status_rc" -ne 0 ]; then
  echo "Workflow final status failed; terminal success was not proven." >&2
  exit "$final_status_rc"
fi

if [ "$final_status_uncertain" = "true" ]; then
  # UNCERTAIN is not success and is not reported as success. It is also not a
  # failure that discards the run: the outstanding artifacts above are live and
  # belong to the goal-seeking loop, not to a bin (issue #1268).
  echo "terminal_verdict=UNCERTAIN"
  echo "WARNING: terminal success was NOT proven and NOT disproven — the run's artifacts could not be read." >&2
  echo "WARNING: the outstanding pull requests listed above are still live and must be driven to a terminal state." >&2
  exit 0
fi

echo "All 23 workflow steps completed successfully."
