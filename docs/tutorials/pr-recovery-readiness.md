# Tutorial: Recover PR 579 Readiness

**Time to complete**: 20 minutes
**Skill level**: Intermediate

This tutorial walks through the finished recovery flow for PR 579. You resume
the existing branch, verify Copilot hook readiness for issue 577, verify
additive-copy readiness for issue 578, and let `workflow-finalize` produce the
final status.

## What You'll Learn

By the end of this tutorial you can:

1. Run `default-workflow` against an existing PR and branch.
2. Confirm the workflow reused the PR branch.
3. Read Copilot plugin and native hook readiness evidence.
4. Read mapped-directory additive-copy readiness evidence.
5. Interpret the final `ready`, `blocked`, or `finalized` decision.

## Prerequisites

You need:

- a writable clone of `rysweet/amplihack-rs`
- `gh auth status` working for the account allowed to update PR 579
- the existing branch `fix/issues-577-578-copilot-hooks-and-additive-copy`
- the `amplihack` CLI on `PATH`

## Step 1: Start from the Recovery Worktree

Use the repository that owns PR 579:

```bash
cd /home/user/src/amplihack-rs
git remote -v | head -1
gh pr view 579 --json number,headRefName,baseRefName,state,url,headRefOid
```

The PR identity matches:

```json
{
  "number": 579,
  "headRefName": "fix/issues-577-578-copilot-hooks-and-additive-copy",
  "state": "OPEN",
  "headRefOid": "4041d4b650a245501d8e381b1dfed95a94b65fca"
}
```

This is a read-only confirmation. Do not merge, close, retarget, or recreate
the PR.

Gate exact-head recovery before running any no-op readiness path:

```bash
expected_head_sha=4041d4b650a245501d8e381b1dfed95a94b65fca
local_head_sha=$(git rev-parse HEAD)
pr_head_sha=$(gh pr view 579 --json headRefOid --jq .headRefOid)

if [ "$local_head_sha" != "$expected_head_sha" ] ||
   [ "$pr_head_sha" != "$expected_head_sha" ]; then
  printf 'blocked: local HEAD (%s), PR head (%s), and expected_head_sha (%s) must match\n' \
    "$local_head_sha" "$pr_head_sha" "$expected_head_sha" >&2
  exit 1
fi
```

Only the equality `local HEAD == PR headRefOid == expected_head_sha` permits an
exact-head no-op readiness decision.

## Step 2: Set the Heap for Nested Agent Work

Large nested workflow runs inherit `NODE_OPTIONS`:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

The same value can be persisted in `~/.amplihack/config` for environments that
need nested workflow agents to inherit it automatically.

## Step 3: Run `default-workflow` with Recovery Context

Launch the recovery run:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "pr_number=579" \
  -c "existing_branch=fix/issues-577-578-copilot-hooks-and-additive-copy" \
  -c "expected_head_sha=4041d4b650a245501d8e381b1dfed95a94b65fca" \
  -c "task_description=Recover PR #579 after interrupted workflow; resolve Copilot hook readiness and additive-copy readiness only; do not manually merge" \
  -c "issue_requirements=#577: Copilot plugin and native hooks are staged, registered, idempotent, and verified. #578: mapped framework directories replace stale amplihack-owned trees safely, preserve rollback, and guard source/destination aliasing."
```

The workflow owns all changes after this point. If remediation is required, it
is made by workflow-owned implementation steps and published by
`workflow-publish`.

## Step 4: Confirm the Existing Branch Was Reused

Find the recovery identity block:

```json
{
  "pr_recovery": {
    "repository": "rysweet/amplihack-rs",
    "pr_number": 579,
    "existing_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "branch_reused": true,
    "replacement_branch_created": false,
    "manual_merge_performed": false
  }
}
```

A silent replacement branch is not valid recovery. If the workflow cannot reuse
the branch, it reports `blocked` or emits an explicit replacement decision with
the reason.

## Step 5: Inspect Copilot Hook Readiness

Issue 577 is ready when Copilot plugin and native hook evidence is complete:

```json
{
  "hook_readiness": {
    "status": "ready",
    "copilot_plugin": "verified",
    "plugin_json": "verified",
    "hooks_json": "verified",
    "copilot_config_registered": true,
    "native_claude_hooks": "verified",
    "user_config_preserved": true,
    "duplicates_created": false,
    "verification_failures": []
  }
}
```

The workflow proves these results:

- `~/.copilot/installed-plugins/amplihack@local/plugin.json` declares the local
  `amplihack` plugin and points at `./hooks.json`.
- `hooks.json` maps `sessionStart`, `sessionEnd`, `userPromptSubmitted`,
  `preToolUse`, and `postToolUse` to the deployed `amplihack-hooks` binary.
- `userPromptSubmitted` runs `workflow-classification-reminder` before
  `user-prompt-submit`.
- `~/.copilot/config.json` contains exactly one enabled `amplihack` plugin
  entry and preserves unrelated Copilot settings.
- Missing `~/.copilot/` is a visible skip for hosts without Copilot CLI.
- Malformed Copilot config is visible. Current install behavior warns and
  continues for Copilot registration problems, so readiness is blocked only when
  the workflow cannot prove a required `installedPlugins` registration.
- `~/.claude/settings.json` keeps native Claude hooks in canonical order and
  preserves unrelated user-owned entries.

The finished evidence often includes targeted checks:

```json
{
  "hook_checks": {
    "writes_plugin_manifest_with_hooks_field": "passed",
    "writes_hooks_json_with_amplihack_hooks_subcommands": "passed",
    "stages_commands_when_present_and_advertises_them": "passed",
    "idempotent_registration_does_not_duplicate_entries": "passed",
    "preserves_unrelated_config_entries": "passed",
    "native_hook_contract_matches_settings": "passed"
  }
}
```

If any required item is missing, PR 579 remains blocked until the workflow fixes
the concrete hook gap and reruns the check.

## Step 6: Inspect Additive-Copy Readiness

Issue 578 is ready when mapped framework directories replace stale destination
trees safely:

```json
{
  "additive_copy_readiness": {
    "status": "ready",
    "source_layout": "bundle",
    "mapped_directories_replaced": true,
    "stale_destination_files_removed": true,
    "rollback_on_swap_failure": "verified",
    "same_path_alias_guard": "verified",
    "source_derived_verification": "passed",
    "install_manifest_complete": true,
    "verification_failures": []
  }
}
```

The finished copy flow uses staged replacement:

```text
source directory
  -> destination.staging
  -> destination

previous destination
  -> destination.old
  -> restored if final swap fails
```

Readiness means:

- the source layout was resolved from `amplifier-bundle/` or a legacy `.claude/`
  source
- source roots and required mapped source directories are real directories, not
  symlinks
- files removed from the source bundle are absent from the replaced destination
- a failed swap restores the previous destination
- source/destination same-path aliases are detected before destructive
  replacement
- install verification proves required staged assets and bundle assets exist
- the install manifest records staged files, directories, binaries, and hook
  registrations
- `.layout` records the selected `bundle` or `legacy` source layout for later
  verification

The finished evidence often includes:

```json
{
  "additive_copy_checks": {
    "copy_amplifier_bundle_replaces_existing_atomically": "passed",
    "copy_dir_recursive_skips_same_path": "passed",
    "bundle_source_symlink_rejected": "passed",
    "install_writes_layout_marker_atomically": "passed",
    "install_completeness_verification": "passed",
    "manifest_path_traversal_rejected": "passed"
  }
}
```

Missing source assets, unsafe path entries, failed replacement, failed rollback,
or incomplete verification are blockers.

## Step 7: Check Publish Evidence

When remediation changed files, `workflow-publish` pushes those changes back to
the existing PR branch:

```json
{
  "workflow_publish": {
    "target_pr": 579,
    "target_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "changes_required": true,
    "pushed": true,
    "replacement_branch_created": false
  }
}
```

When no changes were needed, `changes_required` and `pushed` are `false`. Both
outcomes are valid when readiness evidence is complete.

Dirty readiness documentation or evidence files are not left ambiguous. The
workflow either commits them as workflow-owned evidence or classifies them as
outside final readiness.

## Step 8: Interpret the Final Status

The final workflow-owned decision looks like:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "final_status": "ready",
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "branch_reused": true,
    "manual_merge_performed": false,
    "remaining_blockers": []
  }
}
```

Use the status exactly as emitted:

| Status | What you do |
| --- | --- |
| `ready` | Leave PR 579 in normal workflow-managed completion. |
| `blocked` | Read the named blocker and rerun or remediate through workflow-owned steps. |
| `finalized` | Treat the PR as finalized by the permitted workflow path. |

The recovery is complete only when final status, hook readiness evidence,
additive-copy readiness evidence, and publish evidence are all present.

For the exact-head no-op recovery path, the final status includes the absence of
changes as evidence:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "head_sha": "4041d4b650a245501d8e381b1dfed95a94b65fca",
    "final_status": "ready",
    "changes_required": false,
    "files_modified": [],
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "check_state": {
      "lint_format": "green",
      "builds": "green",
      "test": "in_progress",
      "merge_state": "blocked"
    },
    "no_op_justification": "No workflow-owned hook or additive-copy readiness changes are required at head 4041d4b650a245501d8e381b1dfed95a94b65fca. Lint/Format and build checks are green; Test is still naturally in progress and branch protection keeps the merge state blocked, so the PR is workflow-ready but not merge-ready.",
    "manual_merge_performed": false,
    "merge_bypass_performed": false,
    "nested_default_workflow_launched": false
  }
}
```

This result recovers the PR from the workflow perspective only. It leaves GitHub
branch protection in charge of the pending Test check and blocked merge state.

## Related Documentation

- [Feature overview](../features/pr-recovery-readiness.md)
- [How-to guide](../howto/recover-existing-pr-with-default-workflow.md)
- [Reference](../reference/pr-recovery-readiness.md)
