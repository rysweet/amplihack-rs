#!/usr/bin/env bash
# Private bounded checkpoint support. The three phase scripts alone use this
# interface. Authority never changes when repair changes reviewed contents.
# The checkpoint engine is `amplihack internal workflow-checkpoint` (#1554,
# decision 17.126); docs/reference/workflow-checkpoint-cli.md is its contract.
# AMPLIHACK_BIN is trusted on the same terms as PATH. A missing engine stops
# the caller with 78; the checkpoint is never skipped.
workflow_checkpoint() {
  command -v -- "${AMPLIHACK_BIN:-amplihack}" >/dev/null 2>&1 || {
    echo "ERROR: workflow checkpoint engine not found (AMPLIHACK_BIN=${AMPLIHACK_BIN:-amplihack}); refusing to continue without the checkpoint (#1554)" >&2
    return 78
  }
  "${AMPLIHACK_BIN:-amplihack}" internal workflow-checkpoint "$@"
}

workflow_checkpoint_check() {
  workflow_checkpoint check
}

workflow_checkpoint_stager_identity() {
  declare -f stage_workflow_changes workflow_stage_strict |
    workflow_checkpoint sha256
}

workflow_checkpoint_stage() {
  workflow_checkpoint_check || return $?
  [ "$(workflow_checkpoint_stager_identity)" = "${WF_CHECKPOINT_STAGE_HASH:?strict helper identity required}" ] || {
    echo 'ERROR: checkpoint strict staging implementation changed; refusing broad fallback' >&2; return 78;
  }
  local list rc=0
  list="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-paths.XXXXXX")" || return $?
  if workflow_checkpoint paths >"$list"; then
    stage_workflow_changes --strict --path-list "$list" || rc=$?
  else rc=$?; fi
  rm -f -- "$list"
  [ "$rc" -eq 0 ] || return "$rc"
  workflow_checkpoint_check
}

# Read only the fixed private result fields. Validate the entire stream before
# assigning checkpoint state; values remain data even when they contain shell
# syntax, newlines or empty strings. These reads also work with Bash 3.2.
workflow_checkpoint_read_values() {
  local mode="$1" file="$2" record record_id record_hash invocation
  local head upstream upstream_ref base_ref trailing
  case "$mode" in admit|after) ;; *) return 78 ;; esac
  {
    if [ "$mode" = admit ]; then
      IFS= read -r -d '' record &&
        IFS= read -r -d '' record_id &&
        IFS= read -r -d '' record_hash &&
        IFS= read -r -d '' invocation || return 78
    fi
    IFS= read -r -d '' head &&
      IFS= read -r -d '' upstream &&
      IFS= read -r -d '' upstream_ref &&
      IFS= read -r -d '' base_ref || return 78
    if IFS= read -r -d '' trailing || [ -n "${trailing-}" ]; then return 78; fi
  } < "$file" || return 78
  if [ "$mode" = admit ]; then
    WF_CHECKPOINT_RECORD="$record"
    WF_CHECKPOINT_RECORD_ID="$record_id"
    WF_CHECKPOINT_HASH="$record_hash"
    WF_CHECKPOINT_INVOCATION="$invocation"
    export WF_CHECKPOINT_RECORD WF_CHECKPOINT_RECORD_ID WF_CHECKPOINT_HASH WF_CHECKPOINT_INVOCATION
  fi
  WF_CHECKPOINT_HEAD="$head"
  WF_CHECKPOINT_UPSTREAM="$upstream"
  WF_CHECKPOINT_UPSTREAM_REF="$upstream_ref"
  WF_CHECKPOINT_BASE_REF="$base_ref"
  export WF_CHECKPOINT_HEAD WF_CHECKPOINT_UPSTREAM WF_CHECKPOINT_UPSTREAM_REF WF_CHECKPOINT_BASE_REF
}

# The wrapper observes each real operation and its actual result. It never
# rolls an index, a worktree or a commit back after an authorised operation.
workflow_checkpoint_operation() {
  local op="$1" token rc=0 values_file engine_rc=0 parse_rc=0 sync_mode
  shift
  values_file="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-values.XXXXXX")" || return 78
  if [ "$op" = push ]; then
    token="$(workflow_checkpoint before "$op" "$@")" || rc=$?
  elif [ "$op" = sync ] && [ "${1:-}" = workflow_checkpoint_sync_body ]; then
    token="$(workflow_checkpoint before "$op" workflow_checkpoint_sync_body "${remote_branch:-${current_branch:-}}")" || rc=$?
  else
    token="$(workflow_checkpoint before "$op")" || rc=$?
  fi
  if [ "$rc" -ne 0 ]; then
    rm -f -- "$values_file" || :
    [ "$rc" -ne 79 ] || return 79
    return 78
  fi
  if [ "$op" = sync ] && [ "${1:-}" = workflow_checkpoint_sync_body ]; then
    sync_mode="$(workflow_checkpoint sync-fetch-mode "$token")" || rc=$?
    if [ "$rc" -ne 0 ]; then
      rm -f -- "$values_file" || :
      [ "$rc" -ne 79 ] || return 79
      return 78
    fi
    case "$sync_mode" in
      fetch) "$@" || rc=$? ;;
      skip-absent) : ;; # No fetch ran; this is not recovery from a failure.
      *) rm -f -- "$values_file" || :; return 78 ;;
    esac
  else
    "$@" || rc=$?
  fi
  if [ "$rc" -eq 79 ]; then rm -f -- "$values_file" || :; return 79; fi
  workflow_checkpoint after "$token" "$op" "$rc" >"$values_file" || engine_rc=$?
  if [ "$engine_rc" -eq 0 ]; then
    workflow_checkpoint_read_values after "$values_file" || parse_rc=$?
  fi
  rm -f -- "$values_file" || :
  if [ "$rc" -ne 0 ]; then
    case "$op:${1:-}" in
      fetch:*|pull:*|sync:workflow_checkpoint_sync_body)
        # The strict cleanup sync propagates its nested fetch status and stops.
        # No later operation follows this failure. Keep its real status even
        # when missing post-failure metadata produces an additional refusal.
        [ "$engine_rc" -ne 79 ] || return 79
        return "$rc" ;;
    esac
  fi
  if [ "$engine_rc" -ne 0 ] || [ "$parse_rc" -ne 0 ]; then
    # A scope failure must stop retries even in contexts that disable errexit.
    [ "$engine_rc" -ne 79 ] || return 79
    return 78
  fi
  return "$rc"
}

workflow_checkpoint_enter() {
  local values_file engine_rc=0 parse_rc=0
  values_file="$(mktemp "${TMPDIR:?assigned TMPDIR required}/checkpoint-values.XXXXXX")" || return $?
  workflow_checkpoint admit "$1" >"$values_file" || engine_rc=$?
  if [ "$engine_rc" -eq 0 ]; then
    workflow_checkpoint_read_values admit "$values_file" || parse_rc=$?
  fi
  rm -f -- "$values_file" || :
  [ "$engine_rc" -eq 0 ] || return "$engine_rc"
  [ "$parse_rc" -eq 0 ] || return "$parse_rc"
  readonly WF_CHECKPOINT_RECORD WF_CHECKPOINT_RECORD_ID WF_CHECKPOINT_HASH WF_CHECKPOINT_INVOCATION
  export GIT_OPTIONAL_LOCKS=0 WF_CHECKPOINT_REQUIRED=1
  readonly WF_CHECKPOINT_REQUIRED
  . "${BASH_SOURCE[0]%/*}/workflow_stage_changes.sh"
  WF_CHECKPOINT_STAGE_HASH="$(workflow_checkpoint_stager_identity)" || return $?
  readonly WF_CHECKPOINT_STAGE_HASH
  git() {
    local operation="${1:-}"
    [ "$operation" != --no-pager ] || operation="${2:-}"
    case "$operation" in
      commit|push|pull|fetch)
        workflow_checkpoint_operation "$operation" command git "$@" ;;
      *) command git "$@" ;;
    esac
  }
}


# Sourcing this file provides only the private support functions.
if [ "${BASH_SOURCE[0]}" != "$0" ]; then return 0; fi
if [ "${1:-}" = --scope-capture ]; then
  workflow_checkpoint capture "${2:?phase required}"
  exit $?
fi
workflow_checkpoint selectors || {
  scope_rc=$?
  workflow_checkpoint admission-refused-result
  exit "$scope_rc"
}
set -euo pipefail
export GIT_PAGER=cat
export PAGER=cat
export LESS=FRX
WT_TOOL=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/worktree_setup_path.sh" ] && { WT_TOOL="$c/amplifier-bundle/tools/worktree_setup_path.sh"; break; }; done
WORKTREE_SETUP_WORKTREE_PATH="$(bash "${WT_TOOL:?worktree_setup_path.sh not found}" step-15-commit-push)" || WORKTREE_SETUP_WORKTREE_PATH=""
# Issue #1528: stage and commit only at the top of the linked task worktree; no REPO_PATH fallback.
# Hard-fail rather than skip: a skipped commit here would lose work silently.
cd -P "${WORKTREE_SETUP_WORKTREE_PATH:?step-15 requires worktree_setup.worktree_path from step-04 (workflow-worktree); ensure parent recipe ran worktree-setup and propagated outputs}"
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "ERROR: step-15-commit-push requires a git repo at $(pwd); either \`git init\` or rerun from a checkout" >&2
  exit 1
fi
{ gd=$(git rev-parse --absolute-git-dir) && cd_=$(git rev-parse --path-format=absolute --git-common-dir) && [ "$gd" != "$cd_" ] && [ "$(git rev-parse --show-toplevel)" = "$(pwd -P)" ]; } 2>/dev/null || { echo "ERROR: step-15-commit-push: $(pwd -P) is not the top of a linked task worktree; refusing to stage or commit; never commits in REPO_PATH (#1528)" >&2; exit 1; }
# Security: refuse to commit/push on protected branches, even inside a linked worktree.
_CURRENT_BRANCH=$(git branch --show-current 2>/dev/null || echo "")
if [ "$_CURRENT_BRANCH" = "main" ] || [ "$_CURRENT_BRANCH" = "master" ]; then echo "ERROR: step-15 landed on protected branch '$_CURRENT_BRANCH' — refusing to commit. Worktree setup likely failed." >&2; exit 1; fi
workflow_checkpoint_enter publish || {
  scope_rc=$?
  workflow_checkpoint admission-refused-result
  exit "$scope_rc"
}
echo "=== Step 15: Commit and Push ===" >&2
# FIX (#495): commit_result MUST be emitted on every exit path so step-16
# (PR creation) can be gated on the actual push outcome. Trap installed
# AFTER var init, BEFORE any failing command — reordering produces invalid JSON.
if ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: jq is required by step-15 (commit_result emission)." >&2
  exit 2
fi
redact_sensitive_file() {
  sed -E 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g' "$1"
}
pushed="false"; sha=""; branch=""; reason="aborted"
_emit_commit_result() {
  jq -nc --arg pushed "$pushed" --arg sha "$sha" --arg branch "$branch" --arg reason "$reason" \
    '{pushed:$pushed,sha:$sha,branch:$branch,reason:$reason}'
}
trap _emit_commit_result EXIT
branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
echo "--- Staging Changes ---" >&2
RUNTIME_ARTIFACT_HELPER="${WORKFLOW_RUNTIME_ARTIFACT_HELPER:-${AMPLIHACK_HOME:-${REPO_PATH:-$(pwd)}}/amplifier-bundle/tools/workflow_runtime_artifacts.sh}"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${REPO_PATH:-$(pwd)}/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="$(pwd)/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || RUNTIME_ARTIFACT_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/workflow_runtime_artifacts.sh"
[ -f "$RUNTIME_ARTIFACT_HELPER" ] || { echo "ERROR: workflow runtime artifact helper not found (searched WORKFLOW_RUNTIME_ARTIFACT_HELPER, AMPLIHACK_HOME, REPO_PATH, worktree, ~/.copilot, ~/.amplihack): $RUNTIME_ARTIFACT_HELPER" >&2; exit 2; }
. "$RUNTIME_ARTIFACT_HELPER"
workflow_checkpoint_check || exit $?
preflight_known_workflow_runtime_artifacts .
# #1671: every step-15 helper comes from the bundle that supplied the runtime
# artifact helper above; the older search order is only the fallback.
GIT_IDENTITY_HELPER="${RUNTIME_ARTIFACT_HELPER:+${RUNTIME_ARTIFACT_HELPER%/*}/git-identity.sh}"; [ -n "$GIT_IDENTITY_HELPER" ] && [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${AMPLIHACK_HOME:-${REPO_PATH:-$(git rev-parse --show-toplevel)}}/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(git rev-parse --show-toplevel)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="$(pwd)/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.copilot/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || GIT_IDENTITY_HELPER="${HOME:-/root}/.amplihack/amplifier-bundle/tools/git-identity.sh"; [ -f "$GIT_IDENTITY_HELPER" ] || { echo "ERROR: git identity helper not found: $GIT_IDENTITY_HELPER" >&2; exit 2; }; . "$GIT_IDENTITY_HELPER"
# #1671: stage_workflow_changes is `git add -A` minus build artefacts, and a
# rejecting git hook goes to a fixing agent before the commit or push is retried.
WF_TOOLS="${RUNTIME_ARTIFACT_HELPER%/*}"; [ -f "$WF_TOOLS/workflow_hook_repair.sh" ] || WF_TOOLS="${GIT_IDENTITY_HELPER%/*}"; REMOTE_BRANCH="$(bash "$WF_TOOLS/workflow_remote_branch.sh" .)" || exit 2; [ "$REMOTE_BRANCH" != "$(git branch --show-current)" ] || REMOTE_BRANCH=""; SETUP_BRANCH="$(printf '%s' "${RECIPE_VAR_worktree_setup:-}" | jq -r '.branch_name // empty' 2>/dev/null)" || SETUP_BRANCH=""; [ -z "$REMOTE_BRANCH" ] || [ "$REMOTE_BRANCH" = "$SETUP_BRANCH" ] || { echo "ERROR: push target '$REMOTE_BRANCH' does not match worktree_setup.branch_name '$SETUP_BRANCH'; refusing to push (issue #1762)." >&2; exit 1; }  # #1762: an alias pushes to <B>
for wf_helper in "${WORKFLOW_STAGE_CHANGES_HELPER:-$WF_TOOLS/workflow_stage_changes.sh}" "${WORKFLOW_HOOK_REPAIR_HELPER:-$WF_TOOLS/workflow_hook_repair.sh}"; do [ -f "$wf_helper" ] || { echo "ERROR: workflow helper not found: $wf_helper" >&2; exit 2; }; . "$wf_helper"; done
amplihack hygiene artifact-guard --repo . --mode pre-publish
workflow_checkpoint_stage; amplihack hygiene artifact-guard --repo . --mode pre-publish
echo "--- Creating Commit ---" >&2
# FIX (#469/#311): env vars are injection-safe; ${VAR:-} nounset reads so an unset value can't abort before the #944 fallback below.
TASK_DESC="${TASK_DESCRIPTION:-}"
ISSUE_NUMBER="${ISSUE_NUMBER:-}"
# FIX (#944): commit prefix from step-14's real change type (PATCH->fix:,
# MINOR->feat:, MAJOR->feat!:), not a hardcoded 'feat:'. Unknown -> bugfix heuristic.
CHANGE_CLASS="$(printf '%s' "${RECIPE_VAR_version_bump:-}" | jq -r 'if type=="object" and has("change_classification") and .change_classification != null then .change_classification | tostring else empty end' 2>/dev/null)" || { echo "WARNING: step-15-commit-push: version_bump is not valid JSON; treated as missing (#1561)" >&2; CHANGE_CLASS=""; }
case "$(printf '%s' "$CHANGE_CLASS" | tr '[:lower:]' '[:upper:]')" in
  PATCH) COMMIT_PREFIX="fix: " ;;
  MINOR) COMMIT_PREFIX="feat: " ;;
  MAJOR) COMMIT_PREFIX="feat!: " ;;
  *)
    case "$(printf '%s' "$TASK_DESC" | tr '[:upper:]' '[:lower:]')" in
      *bug*|*bugfix*|*fix*) COMMIT_PREFIX="fix: " ;;
      *)                    COMMIT_PREFIX="feat: " ;;
    esac ;;
esac
COMMIT_TITLE=$(printf '%s%.72s' "$COMMIT_PREFIX" "$(printf '%s' "$TASK_DESC" | tr '\n\r' ' ' | head -1)")
# FIX #684: Host-aware commit references.
# FIX #1361: on GitHub the closing keyword is EARNED BY THE DIFF. PR #1283's
# body said `Closes #1277` while its whole change was one added planning
# document; had it merged, a genuine bug report would have been auto-closed
# by an artifact describing what somebody intended to do about it. A change
# that only writes down a plan gets `Refs`, which still links the PR to the
# issue but leaves it open for the work itself. Unreachable helper -> `Refs`:
# under-linking costs a manual close, over-linking closes a live bug silently.
HOST_TYPE="${REMOTE_HOST_TYPE:-other}"
CLOSE_KW="Refs"; IR=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_issue_reference.sh" ] && { IR="$c/amplifier-bundle/tools/workflow_issue_reference.sh"; break; }; done
if [ -n "$IR" ]; then CLOSE_KW="$(git diff --cached --name-only | bash "$IR" --keyword)" || CLOSE_KW="Refs"; else echo "WARNING: workflow_issue_reference.sh not found (searched AMPLIHACK_HOME, REPO_PATH, cwd, ~/.copilot, ~/.amplihack) — using the non-closing 'Refs' keyword (#1361)" >&2; fi
case "$HOST_TYPE" in
  github) ISSUE_REF="${CLOSE_KW} #${ISSUE_NUMBER}"; ISSUE_IMPL="issue #${ISSUE_NUMBER}" ;;
  azdo)   ISSUE_REF="AB#${ISSUE_NUMBER}"; ISSUE_IMPL="AB#${ISSUE_NUMBER}" ;;
  *)      ISSUE_REF="Ref #${ISSUE_NUMBER}"; ISSUE_IMPL="issue #${ISSUE_NUMBER}" ;;
esac
COMMIT_MSG=$(printf '%s\n\nImplements %s\n\nChanges:\n- Implementation as per design specification\n- Tests added for new functionality\n- Documentation updated\n\n%s' "$COMMIT_TITLE" "$ISSUE_IMPL" "$ISSUE_REF")
# PERF: --quiet short-circuits on the first staged change.
if ! git diff --cached --quiet; then
  amplihack_prepare_git_commit_identity
  commit_output_file=$(mktemp -t step15-commit-XXXXXX)
  # A plain commit, as before #1671: PRE_COMMIT_ALLOW_NO_CONFIG is not set here.
  hook_repair_run commit step-15-commit-push --output "$commit_output_file" -- git commit -m "$COMMIT_MSG" || {
    commit_rc=$?
    # A commit that fails for any other reason reports commit-failed, not aborted (#1671).
    reason="$(hook_repair_step15_reason commit)" || { reason="commit-failed"; redact_sensitive_file "$commit_output_file" >&2; }
    rm -f "$commit_output_file"
    exit "$commit_rc"
  }
  redact_sensitive_file "$commit_output_file" >&2; rm -f "$commit_output_file"
else
  # FIX (#282): avoid hollow-success false-positive — check the current branch
  # for new commits (in any worktree) before declaring nothing was done.
  CURRENT_BRANCH=$(git branch --show-current)
  ALT_WORKTREE_COMMITS=0
  if [ -n "$CURRENT_BRANCH" ] && git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
    ALT_WORKTREE_COMMITS=$(git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0)
  fi
  if [ "$ALT_WORKTREE_COMMITS" -gt 0 ]; then
    echo "INFO: Nothing staged in ${WORKTREE_SETUP_WORKTREE_PATH:-(unset)}, but the" >&2
    echo "current branch '$CURRENT_BRANCH' is $ALT_WORKTREE_COMMITS commit(s) ahead of upstream." >&2
    git --no-pager log --oneline -"$ALT_WORKTREE_COMMITS" >&2
  else
    echo "INFO: Nothing staged to commit; continuing so already-pushed meaningful work can be published or classified." >&2
  fi
fi
echo "--- Pushing to Remote ---" >&2
# REL-002: Detect missing upstream tracking branch before rev-list
if ! git rev-parse --abbrev-ref '@{u}' >/dev/null 2>&1; then
  echo "WARNING: No upstream tracking branch configured — skipping push" >&2
  reason="no-upstream"
  exit 0
fi
COMMITS_AHEAD=$(git rev-list --count @{u}..HEAD)
rc=0; DESTINATION_IS_HEAD=$(workflow_checkpoint destination-is-head) || rc=$?; [ "$rc" -eq 0 ] || { reason="publication-destination-inspection-failed"; exit "$rc"; }
if [ "$COMMITS_AHEAD" -ne 0 ] || [ "$DESTINATION_IS_HEAD" != true ]; then
  # FIX (#978): ancestry-aware integration — never blind `git pull --rebase`.
  # Refresh upstream, then decide by ancestry: behind==0 -> fast-forward push
  # (preserve tested commit identities); behind>0 -> fail closed with structured
  # merge-base/ahead/behind evidence. Never auto-replay already-integrated history.
  UPSTREAM_REF=$(git rev-parse --abbrev-ref '@{u}' 2>/dev/null || echo "unknown")
  fetch_output_file=$(mktemp -t step15-fetch-XXXXXX)
  if git fetch origin >"$fetch_output_file" 2>&1; then
    redact_sensitive_file "$fetch_output_file" >&2
  else
    fetch_rc=$?
    if [ "$fetch_rc" -eq 78 ] || [ "$fetch_rc" -eq 79 ]; then
      redact_sensitive_file "$fetch_output_file" >&2
      rm -f "$fetch_output_file"
      reason="fetch-policy-refusal"
      exit "$fetch_rc"
    fi
    echo "ERROR: git fetch origin failed before ancestry check (exit $fetch_rc); refusing publication" >&2
    redact_sensitive_file "$fetch_output_file" >&2
    rm -f "$fetch_output_file"
    reason="fetch-failed"
    exit "$fetch_rc"
  fi
  rm -f "$fetch_output_file"
  COMMITS_AHEAD=$(git rev-list --count '@{u}..HEAD' 2>/dev/null || echo 0)
  COMMITS_BEHIND=$(git rev-list --count 'HEAD..@{u}' 2>/dev/null || echo 0)
  rc=0; DESTINATION_IS_HEAD=$(workflow_checkpoint destination-is-head) || rc=$?; [ "$rc" -eq 0 ] || { reason="publication-destination-inspection-failed"; exit "$rc"; }
  if [ "$COMMITS_AHEAD" -eq 0 ] && [ "$DESTINATION_IS_HEAD" = true ]; then
    echo "INFO: HEAD no longer ahead of upstream '$UPSTREAM_REF' after fetch — nothing to push" >&2
    raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
    if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then sha="$raw_sha"; fi
    pushed="true"; reason="already-pushed"
    echo "=== Commit and Push Complete ===" >&2
    exit 0
  fi
  if [ "$COMMITS_BEHIND" -ne 0 ]; then
    MERGE_BASE=$(git merge-base HEAD '@{u}' 2>/dev/null || echo "none")
    HEAD_SHA=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
    echo "ERROR: step-15 refuses to rebase divergent history onto upstream '$UPSTREAM_REF' (issue #978)." >&2
    echo "  ancestry: ahead=$COMMITS_AHEAD behind=$COMMITS_BEHIND merge_base=$MERGE_BASE head=$HEAD_SHA upstream=$UPSTREAM_REF" >&2
    echo "  Auto-rebasing would replay already-integrated commits and create add/add conflicts." >&2
    echo "  Point the branch at its intended PR base and fast-forward, or make an explicit rebase decision." >&2
    reason="diverged-upstream: ahead=$COMMITS_AHEAD behind=$COMMITS_BEHIND merge_base=$MERGE_BASE upstream=$UPSTREAM_REF"
    exit 1
  fi
  echo "INFO: upstream '$UPSTREAM_REF' is an ancestor of HEAD — fast-forward push, no rebase (#978)" >&2
  # FIX (#495): capture push stderr, redact tokens before reason emission.
  push_stderr_file=$(mktemp -t step15-push-XXXXXX)
  # shellcheck disable=SC2064
  trap "rm -f \"$push_stderr_file\"; _emit_commit_result" EXIT
  if hook_repair_run push step-15-commit-push \
    --fix-message "fix: address {hook_ids} pre-push rejection${ISSUE_NUMBER:+ (#$ISSUE_NUMBER)}" \
    --output "$push_stderr_file" -- git push ${REMOTE_BRANCH:+origin "HEAD:$REMOTE_BRANCH"}; then
    redact_sensitive_file "$push_stderr_file" >&2  # kept for the step log; never printed raw (#978)
    # FIX (#1269): confirm the published commit is the tested one. A missing
    # helper warns, never fails -- see verify_pushed_ref.sh and #1268.
    V=""; for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/verify_pushed_ref.sh" ] && { V="$c/amplifier-bundle/tools/verify_pushed_ref.sh"; break; }; done || true
    raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
    if [ -n "$V" ]; then workflow_checkpoint verify-pushed-ref "$V" "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha" || { reason="push-verify-failed"; exit 1; }; else echo "WARNING: verify_pushed_ref.sh not reachable; published commit unverified (#1269)" >&2; fi
    if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then sha="$raw_sha"; fi
    pushed="true"; reason="pushed"  # step-16 gate: only after verification
  else
    push_rc=$?
    # #1671: an unresolved hook rejection (hook-rejected-<status>:push:<ids>) or exit 79 (orchestration-unavailable) has its own reason.
    if reason="$(hook_repair_step15_reason push)"; then exit "$push_rc"; fi
    sanitized=$(redact_sensitive_file "$push_stderr_file" | tr '\n' ' ' | head -c 500)
    reason="push-failed: ${sanitized}"
    echo "ERROR: git push failed (exit $push_rc)" >&2
    redact_sensitive_file "$push_stderr_file" >&2
    exit "$push_rc"
  fi
else
  echo "INFO: Nothing to push - branch is already up to date with remote" >&2
  raw_sha=$(git rev-parse HEAD 2>/dev/null || echo "")
  if printf '%s' "$raw_sha" | grep -qE '^[0-9a-f]{40}$'; then
    sha="$raw_sha"
  fi
  pushed="true"
  reason="already-pushed"
fi
echo "=== Commit and Push Complete ===" >&2
