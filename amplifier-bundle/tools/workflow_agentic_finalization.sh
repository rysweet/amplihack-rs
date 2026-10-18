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
  repo_root="$(workflow_finalization_metadata repo_path || :)"
  [ -n "$repo_root" ] || repo_root="$(pwd)"
  worktree="$(workflow_finalization_metadata worktree_path || :)"
  intended="$(workflow_finalization_metadata branch_name || :)"
  ( . "$helper" 2>/dev/null && finalize_workflow_cleanup_entry "$repo_root" "$worktree" "$intended" ) >&2 || true
  return 0
}

boolish() {
  case "$1" in
    true|1|yes|y) printf 'true' ;;
    *) printf 'false' ;;
  esac
}

. "$(dirname "${BASH_SOURCE[0]}")/workflow_finalization_context.sh"

resolve_publish_state_reached() {
  boolish "$(workflow_finalization_metadata publish_state_reached || :)"
}
resolve_pr_url() { workflow_finalization_metadata pr_url || :; }
resolve_pr_number() { workflow_finalization_metadata pr_number || :; }

here="$(dirname "${BASH_SOURCE[0]}")"
. "$here/workflow_finalization_metadata.sh"
. "$here/workflow_finalization_collect.sh"
. "$here/workflow_finalization_validate.sh"
. "$here/workflow_finalization_complete.sh"

case "$mode" in
  collect) run_finalization_cleanup; collect_evidence ;;
  validate) validate_finalization ;;
  complete) complete_workflow ;;
  *)
    echo "ERROR: workflow_agentic_finalization.sh requires mode: collect, validate, or complete" >&2
    exit 2
    ;;
esac
