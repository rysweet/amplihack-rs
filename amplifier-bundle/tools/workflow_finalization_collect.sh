#!/usr/bin/env bash
# Deterministic repository and completion evidence collection.
collect_evidence() {
  if ! command -v jq >/dev/null 2>&1; then
    echo "ERROR: collect-finalization-evidence requires jq for structured JSON evidence" >&2
    exit 2
  fi

  target_dir="$(workflow_finalization_metadata worktree_path || :)"
  if [ -z "$target_dir" ] || [ "$target_dir" = "''" ]; then
    target_dir="$(workflow_finalization_metadata repo_path || :)"
  fi
  [ -n "$target_dir" ] || target_dir="."

  repo_valid="false"
  dirty_worktree="unknown"
  branch_name="$(workflow_finalization_metadata branch_name || :)"
  head_sha=""
  base_ref="$(workflow_finalization_metadata base_ref || :)"
  remote_host_type="$(workflow_finalization_metadata remote_host_type || :)"
  [ -n "$remote_host_type" ] || remote_host_type=other
  meaningful_diff="unknown"
  commits_ahead="$(workflow_finalization_metadata commits_ahead || :)"
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
  prior_terminal_state="$(workflow_finalization_metadata prior_terminal_state || :)"
  prior_terminal_success="$(boolish "$(workflow_finalization_metadata prior_terminal_success || :)")"
  prior_terminal_reason="$(workflow_finalization_metadata prior_terminal_reason || :)"
  branch_diff_status="$(workflow_finalization_metadata branch_diff_status || :)"
  [ -n "$branch_diff_status" ] || branch_diff_status="$meaningful_diff"
  pr_url="$(resolve_pr_url)"
  pr_number="$(resolve_pr_number)"
  publish_status="$(workflow_finalization_metadata publish_status || :)"
  if [ "$publish_status" = "FOLLOWUP_CREATED" ] || [ -n "$pr_url" ]; then
    publish_state_reached="true"
  fi
  case "$pr_url" in
    https://github.com/*|http://github.com/*|https://*@github.com/*|http://*@github.com/*) github_remote="true" ;;
  esac
  if { [ "$remote_host_type" = "github" ] || [ "$github_remote" = "true" ]; } && { [ "$meaningful_diff" != "false" ] || [ -n "$pr_url" ] || [ -n "$pr_number" ]; }; then
    gh_required="true"
  fi

  # --rawfile preserves UTF-8 across jq raw-parser chunk boundaries.
  # Serialize scalar observations on stdin too: a file-spilled value must
  # not re-enter argv and hit its per-argument limit. Keys are a fixed list.
  schema_version="1"
  repo_path="$(workflow_finalization_metadata repo_path || :)"
  worktree_path="$target_dir"
  observed_phases="workflow-prep,workflow-worktree,workflow-design,workflow-tdd,workflow-refactor-review,workflow-precommit-test,workflow-publish,workflow-pr-review,workflow-finalize"
  {
    printf '{'
    separator=''
    for field in schema_version repo_path worktree_path repo_valid branch_name head_sha base_ref dirty_worktree meaningful_diff commits_ahead missing_tooling remote_host_type github_remote gh_required pr_url pr_number publish_status implementation_completed verification_completed publish_state_reached terminal_no_op allow_no_op prior_terminal_state prior_terminal_success prior_terminal_reason branch_diff_status observed_phases; do
      printf '%s"%s":' "$separator" "$field"
      printf '%s' "${!field}" | jq -n --rawfile value /dev/stdin '$value'
      separator=','
    done
    printf '}'
  } | jq '
    .schema_version as $schema_version |
    .repo_path as $repo_path |
    .worktree_path as $worktree_path |
    .repo_valid as $repo_valid |
    .branch_name as $branch_name |
    .head_sha as $head_sha |
    .base_ref as $base_ref |
    .dirty_worktree as $dirty_worktree |
    .meaningful_diff as $meaningful_diff |
    .commits_ahead as $commits_ahead |
    .missing_tooling as $missing_tooling |
    .remote_host_type as $remote_host_type |
    .github_remote as $github_remote |
    .gh_required as $gh_required |
    .pr_url as $pr_url |
    .pr_number as $pr_number |
    .publish_status as $publish_status |
    .implementation_completed as $implementation_completed |
    .verification_completed as $verification_completed |
    .publish_state_reached as $publish_state_reached |
    .terminal_no_op as $terminal_no_op |
    .allow_no_op as $allow_no_op |
    .prior_terminal_state as $prior_terminal_state |
    .prior_terminal_success as $prior_terminal_success |
    .prior_terminal_reason as $prior_terminal_reason |
    .branch_diff_status as $branch_diff_status |
    .observed_phases as $observed_phases |
    {
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
