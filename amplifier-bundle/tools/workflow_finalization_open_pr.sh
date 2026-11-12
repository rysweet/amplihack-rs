#!/usr/bin/env bash
# workflow_finalization_open_pr.sh - the validator's open-PR check (#1641, #1714).
# shellcheck disable=SC2034,SC2154  # reads and sets validate_finalization's locals (see below)
#
# Sourced by workflow_agentic_finalization.sh after
# workflow_finalization_pr_signal.sh and before workflow_finalization_validate.sh.
# Defines functions only. They are called from validate_finalization and read
# and set its locals (pr_url, pr_number, prior_terminal_state, target_dir,
# target_dir_explicit, open_pr_signal, open_pr_confirmed_*) through bash's
# dynamic scoping, so they must not be called from anywhere else.
#
#   confirm_open_pr               0 with open_pr_signal set when gh confirms a PR
#   open_pr_facts_from_signal     print the confirmed PR's evidence facts
#   remember_confirmed_open_pr    keep the confirmed PR for the auto-drive hand-off

# #1762: the run branch of an alias worktree (<B>--wt-<n> tracking origin/<B>)
# is <B>. A missing resolver means no open PR is confirmed.
_finalization_open_pr_remote_branch="$(dirname "${BASH_SOURCE[0]}")/workflow_remote_branch.sh"

# Open-PR check (#1641, #1714): a branch with an OPEN same-repo PR, confirmed
# live by gh, whose head is the local HEAD or a fast-forward of it. gh is
# called only after every blocker, with a publish signal, an explicit target
# directory and a GitHub remote. Returns 0 with open_pr_signal set only when
# a PR is confirmed; classification is left to the caller.
confirm_open_pr() {
  local publish_signal publish_status published_state remote_host_type remote_is_github
  local metadata_valid metadata_key metadata_status run_branch open_pr_ref
  publish_signal="false"
  [ -z "$pr_url$pr_number" ] || publish_signal="true"
  [ "$(resolve_publish_state_reached)" != "true" ] || publish_signal="true"
  publish_status="$(workflow_finalization_metadata publish_status || :)"
  for published_state in "$publish_status" "$prior_terminal_state"; do
    case "$published_state" in
      EXISTING_OPEN_PR|existing-open-pr|FOLLOWUP_CREATED|MERGED|closed-after-merge) publish_signal="true" ;;
    esac
  done
  remote_host_type="$(workflow_finalization_metadata remote_host_type || :)"
  # An unset host type is allowed through: it cannot widen the check, because
  # finalization_open_pr_signal still requires a github.com origin URL.
  case "$remote_host_type" in ''|github|GitHub|GITHUB) remote_is_github="true" ;; *) remote_is_github="false" ;; esac
  # Invalid authoritative metadata (status 3) is swallowed into "" above; it
  # must never feed a success path, so any such key disables this path.
  metadata_valid="true"
  for metadata_key in worktree_path repo_path pr_url pr_number publish_state_reached prior_terminal_state publish_status branch_name remote_host_type; do
    metadata_status=0
    (workflow_finalization_metadata "$metadata_key" >/dev/null 2>&1) || metadata_status=$?
    [ "$metadata_status" != 3 ] || metadata_valid="false"
  done
  [ "$metadata_valid" = "true" ] || { [ "$publish_signal" != "true" ] || echo "WARNING: invalid workflow metadata; not classified as EXISTING_OPEN_PR" >&2; }
  if [ "$publish_signal" = "true" ] && [ "$metadata_valid" = "true" ] && [ "$target_dir_explicit" = "true" ] && [ "$remote_is_github" = "true" ]; then
    run_branch="$(workflow_finalization_metadata branch_name || :)"
    [ -n "$run_branch" ] || run_branch="$(bash "$_finalization_open_pr_remote_branch" "$target_dir")" \
      || { echo "WARNING: workflow_remote_branch.sh is missing or failed; not classified as EXISTING_OPEN_PR (#1762)" >&2; return 1; }
    # A canonical pr_number is preferred, so a malformed pr_url (trailing
    # slash, /files suffix) cannot hide it. A bad pr_number with no pr_url is
    # passed as-is so the helper rejects it rather than widening to pr list.
    open_pr_ref="$pr_number"
    if ! _finalization_pr_number_valid "$pr_number" && [ -n "$pr_url" ]; then open_pr_ref="$pr_url"; fi
    if open_pr_signal="$(finalization_open_pr_signal "$target_dir" "$run_branch" "$open_pr_ref")"; then
      return 0
    fi
    # finalization_open_pr_signal already printed the specific reason.
    echo "WARNING: publish signal present but the open-PR check failed (reason above); not classified as EXISTING_OPEN_PR" >&2
  fi
  return 1
}

# The confirmed PR's facts, from open_pr_signal. The helper emits only a
# validated number and the URL it built itself.
open_pr_facts_from_signal() {
  local head_facts="pr_head_sha_matches=true"
  # A PR head that fast-forwards the local HEAD is the same PR (#1714).
  [ "$(printf '%s' "$open_pr_signal" | jq -r '.head // empty')" != "descendant" ] || head_facts="pr_head_sha_matches=false,pr_head_fast_forwards_local_head=true"
  printf 'publish_state_reached=true,pr_state=OPEN,pr_head_matches_branch=true,%s,pr_number=%s' \
    "$head_facts" "$(printf '%s' "$open_pr_signal" | jq -r '.pr_number')"
}

# Keep a confirmed PR for the auto-drive hand-off only; the result keeps
# the run's own PR evidence unless the hand-off happens.
remember_confirmed_open_pr() {
  # all three are read by finalization_try_handoff_open_pr
  open_pr_confirmed_number="$(printf '%s' "$open_pr_signal" | jq -r '.pr_number')" \
    open_pr_confirmed_url="$(printf '%s' "$open_pr_signal" | jq -r '.pr_url')" \
    open_pr_confirmed_facts="$(open_pr_facts_from_signal),all_checks=$(printf '%s' "$open_pr_signal" | jq -r '.checks // empty')"
}
