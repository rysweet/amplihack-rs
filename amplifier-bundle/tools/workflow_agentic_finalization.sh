#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat GH_PAGER=cat PAGER=cat LESS=FRX

mode="${1:-}"
# collect [--base BRANCH]: BRANCH is the run's base_branch, the base step-16
# gave the pull request. The evidence's diff is taken against origin/BRANCH.
# Empty: the base of the pull request this run recorded (delivery_run_base in
# workflow_delivery_inputs.sh); when that cannot be read, the collector keeps
# its origin/HEAD discovery and a warning says so
# (docs/reference/workflow-delivery-inputs.md).
FINALIZATION_BASE_BRANCH=""
if [ "$mode" = collect ] && [ "${2:-}" = "--base" ]; then FINALIZATION_BASE_BRANCH="${3:-}"; fi
if [ -n "$FINALIZATION_BASE_BRANCH" ] && ! git check-ref-format --branch "$FINALIZATION_BASE_BRANCH" >/dev/null 2>&1; then
  printf 'ERROR: workflow_agentic_finalization.sh: --base %q is not a valid branch name\n' "$FINALIZATION_BASE_BRANCH" >&2
  exit 2
fi

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
  # shellcheck source=/dev/null
  ( . "$helper" 2>/dev/null && finalize_workflow_cleanup_entry "$repo_root" "$worktree" "$intended" ) >&2 || true
  return 0
}

boolish() {
  case "$1" in
    true|1|yes|y) printf 'true' ;;
    *) printf 'false' ;;
  esac
}

# The finalization libraries classify origins through the shared GitHub repo
# identity parser (issue #1642). They only warn and fail closed when it is
# missing, so the entry point checks it up front: a missing or unreadable
# helper means a broken install, which must fail loudly before anything runs.
GITHUB_REPO_HELPER="${WORKFLOW_GITHUB_REPO_HELPER:-$(dirname "${BASH_SOURCE[0]}")/workflow_github_repo.sh}"
if [ ! -f "$GITHUB_REPO_HELPER" ] || [ ! -r "$GITHUB_REPO_HELPER" ]; then
  echo "ERROR: workflow GitHub repo helper not found: $GITHUB_REPO_HELPER (workflow_github_repo.sh is required by workflow_agentic_finalization.sh)" >&2
  exit 2
fi

. "$(dirname "${BASH_SOURCE[0]}")/workflow_finalization_context.sh"

resolve_publish_state_reached() {
  boolish "$(workflow_finalization_metadata publish_state_reached || :)"
}
resolve_pr_url() { workflow_finalization_metadata pr_url || :; }
resolve_pr_number() { workflow_finalization_metadata pr_number || :; }

here="$(dirname "${BASH_SOURCE[0]}")"
# rv_field (#1561) and delivery_run_base (#2520), for resolve_finalization_base.
. "$here/recipe_var.sh"
DELIVERY_INPUTS_HELPER="${WORKFLOW_DELIVERY_INPUTS_HELPER:-$here/workflow_delivery_inputs.sh}"
if [ ! -f "$DELIVERY_INPUTS_HELPER" ] || [ ! -r "$DELIVERY_INPUTS_HELPER" ]; then
  echo "ERROR: workflow_agentic_finalization.sh requires the delivery-input helper at $DELIVERY_INPUTS_HELPER" >&2
  exit 2
fi
# shellcheck source=workflow_delivery_inputs.sh disable=SC1090
. "$DELIVERY_INPUTS_HELPER"
. "$here/workflow_finalization_metadata.sh"
. "$here/workflow_finalization_collect.sh"
. "$here/workflow_gh_retry.sh"
. "$here/workflow_finalization_pr_signal.sh"
. "$here/workflow_finalization_open_pr.sh"
. "$here/workflow_finalization_handoff.sh"
. "$here/workflow_finalization_validate.sh"
. "$here/workflow_finalization_complete.sh"

# Sets FINALIZATION_BASE_BRANCH from the run's pull request when --base was
# empty and no explicit base_ref was recorded (#2520). Evidence only: a base
# that cannot be read leaves it empty, so workflow_finalization_collect.sh
# keeps its discovery, and a warning names that fallback.
resolve_finalization_base() {
  local target pr_ref base reason
  [ -z "$FINALIZATION_BASE_BRANCH" ] || return 0
  [ -z "$(workflow_finalization_metadata base_ref || :)" ] || return 0
  target="$(workflow_finalization_metadata worktree_path || :)"
  if [ -z "$target" ] || [ "$target" = "''" ]; then target="$(workflow_finalization_metadata repo_path || :)"; fi
  [ -n "$target" ] || target="."
  git -C "$target" rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  pr_ref="$(resolve_pr_url)"
  [ "$pr_ref" != "''" ] || pr_ref=""
  [ -n "$pr_ref" ] || pr_ref="$(resolve_pr_number)"
  [ "$pr_ref" != "''" ] || pr_ref=""
  # delivery_run_base prints only the base on success and only its reason on
  # failure; the reason goes into the one WARNING below, not an ERROR line.
  if base="$(cd "$target" && delivery_run_base "" "$(rv_field pr_publish_result base_branch)" "$pr_ref" \
      "$(parse_github_repo_identity "$(git config --get remote.origin.url 2>/dev/null || true)" 2>/dev/null || true)" \
      "" "${RECIPE_VAR_forbidden_base_branches:-}" 2>&1)" && [ -n "$base" ]; then
    FINALIZATION_BASE_BRANCH="${base##*$'\n'}"
  else
    reason="${base%%$'\n'*}"
    reason="${reason#ERROR: }"
    reason="${reason%; no other base is substituted}"
    echo "WARNING: workflow_agentic_finalization.sh: the base of the run's pull request was not resolved (${reason:-no reason given}); the evidence diff uses the discovered base (origin/HEAD)" >&2
  fi
}

[ "$mode" != collect ] || resolve_finalization_base
case "$mode" in
  collect) run_finalization_cleanup; collect_evidence ;;
  validate) validate_finalization ;;
  complete) complete_workflow ;;
  *)
    echo "ERROR: workflow_agentic_finalization.sh requires mode: collect, validate, or complete" >&2
    exit 2
    ;;
esac
