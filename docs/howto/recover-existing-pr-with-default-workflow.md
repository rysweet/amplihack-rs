# How to Recover an Existing PR with `default-workflow`

> [Home](../index.md) > How-To > Recover an Existing PR with `default-workflow`

Use this guide when a pull request already exists and `default-workflow` needs
to resume it on the same branch after interruption, owner exit, or rate limit.

## Before You Start

You need:

- a writable checkout of the target repository
- the existing PR number
- the exact existing PR branch name
- issue requirements or acceptance criteria for the recovery scope
- `gh auth status` working so the workflow can inspect PR and check metadata

Do not create a replacement branch and do not merge the PR manually. The
workflow owns branch reuse, readiness remediation, publish, and finalization.

## 1. Configure the Node Heap

Large workflow-owned agent runs use this heap setting:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

Persist the same value in `~/.amplihack/config` when nested workflow agents need
it without re-exporting the variable.

## 2. Confirm the Existing PR and Branch

From the repository root, inspect the PR without mutating it:

```bash
PR_NUMBER=579
EXISTING_BRANCH=fix/issues-577-578-copilot-hooks-and-additive-copy

gh pr view "$PR_NUMBER" --json number,headRefName,baseRefName,state,url,headRefOid
```

For the PR 579 recovery example, the head branch is:

```text
fix/issues-577-578-copilot-hooks-and-additive-copy
```

If the PR points at a different branch, stop and recover through workflow
context. Do not retarget, merge, close, or recreate the PR by hand.

When the recovery is tied to a known head, require the local checkout, PR head,
and expected head to match before any readiness verdict:

```bash
expected_head_sha=$(gh pr view "$PR_NUMBER" --json headRefOid --jq .headRefOid)
local_head_sha=$(git rev-parse HEAD)
pr_head_sha=$(
  gh pr view "$PR_NUMBER" --json headRefOid --jq .headRefOid
)

if [ "$local_head_sha" != "$expected_head_sha" ] ||
   [ "$pr_head_sha" != "$expected_head_sha" ]; then
  printf 'blocked: local HEAD (%s), PR head (%s), and expected_head_sha (%s) must match\n' \
    "$local_head_sha" "$pr_head_sha" "$expected_head_sha" >&2
  exit 1
fi
```

A different head blocks the exact-head no-op path because the workflow cannot
claim readiness for a commit it did not inspect.

## 3. Launch `default-workflow` with Recovery Context

Run the workflow from the recovery worktree:

```bash
REPO_PATH=/home/user/src/amplihack-rs
BASE_REF=$(gh pr view "$PR_NUMBER" --json baseRefName --jq .baseRefName)
PR_URL=$(gh pr view "$PR_NUMBER" --json url --jq .url)

amplihack recipe run default-workflow \
  -c "repo_path=${REPO_PATH}" \
  -c "pr_number=${PR_NUMBER}" \
  -c "pr_url=${PR_URL}" \
  -c "existing_branch=${EXISTING_BRANCH}" \
  -c "base_ref=${BASE_REF}" \
  -c "expected_head_sha=${expected_head_sha}" \
  -c "goal_already_met=false" \
  -c "task_description=Recover PR #579 after interrupted workflow; resolve Copilot hook readiness and additive-copy readiness only; do not manually merge" \
  -c "issue_requirements=#577: Copilot plugin and native hooks are staged, registered, idempotent, and verified. #578: mapped framework directories replace stale amplihack-owned trees safely, preserve rollback, and guard source/destination aliasing."
```

The workflow verifies that the PR belongs to the repository at `repo_path` and
that the PR head branch matches `existing_branch`.

Terminal-state recovery depends on the same configuration context:

| Context value | Purpose |
| --- | --- |
| `repo_path` | Repository where the worktree, diff, and status checks run. |
| `pr_number` | Existing PR to inspect for merged, closed, or active state. |
| `pr_url` | Same-repository PR URL when the workflow receives a URL alongside or instead of the number. |
| `existing_branch` | Branch expected to match the PR head and local checkout. |
| `base_ref` | Comparison base for no-diff and obsolete proof. Resolve it from PR metadata or the repository default branch before terminal-state checks. |
| `expected_head_sha` | Optional exact head guard for no-op and recovery readiness evidence. |
| `goal_already_met` | Optional design signal. Keep it false unless external evidence says the goal is already satisfied; it never overrides Git, PR, diff, or CI evidence. |
| `task_description` and `issue_requirements` | Scope context for recovery and remediation. They do not prove obsolete state; obsolete proof must come from Git and PR evidence. |

## 4. Check Branch Reuse Evidence

The workflow emits a recovery identity block. This example shows PR 579:

```json
{
  "pr_recovery": {
    "pr_number": 579,
    "existing_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "branch_reused": true,
    "replacement_branch_created": false,
    "manual_merge_performed": false
  }
}
```

Expected behavior:

- `branch_reused` is `true`
- `replacement_branch_created` is `false`
- `manual_merge_performed` is `false`

A branch mismatch is a blocker unless the workflow records why the original
branch cannot be used.

## 5. Review Copilot Hook Readiness

Hook readiness is complete only when the workflow proves the Copilot plugin and
native hook contracts are usable.

Look for evidence like:

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

The evidence covers these files when Copilot CLI is installed:

| File | Ready condition |
| --- | --- |
| `~/.copilot/installed-plugins/amplihack@local/plugin.json` | Declares `name: "amplihack"` and `hooks: "./hooks.json"`. |
| `~/.copilot/installed-plugins/amplihack@local/hooks.json` | Maps Copilot events to the intended `amplihack-hooks` binary path and subcommands. |
| `~/.copilot/config.json` | Preserves unrelated fields and contains one enabled `installedPlugins` entry for `amplihack`. |
| `~/.claude/settings.json` | Contains native amplihack hook entries in canonical order while preserving unrelated user entries. |

Missing `~/.copilot/` is a supported no-op for hosts without Copilot CLI:

```json
{
  "copilot_plugin": {
    "status": "skipped",
    "reason": "~/.copilot missing"
  }
}
```

Malformed Copilot config is visible evidence, not proof of readiness. Current
install behavior warns and continues when it cannot parse the config, and it
does not fail the whole install for Copilot plugin registration errors. If the
workflow cannot prove the required `installedPlugins` entry after such a
warning, hook readiness is `blocked`.

Use the right Copilot config schema for the surface you are checking:
`amplihack install` registers the plugin under `installedPlugins`, while older
`copilot_setup` helpers use a legacy `plugins` array.

Typical targeted checks include:

```bash
cargo test -p amplihack-cli writes_plugin_manifest_with_hooks_field --lib
cargo test -p amplihack-cli writes_hooks_json_with_amplihack_hooks_subcommands --lib
cargo test -p amplihack-cli preserves_unrelated_config_entries --lib
cargo test -p amplihack-cli validate_amplihack_native_hook_contract --lib
```

The exact command set may vary, but the evidence must prove the same behavior.

## 6. Review Additive-Copy Readiness

Additive-copy readiness is complete when mapped framework directories are
replaced safely and install verification proves the staged tree is complete.

Look for evidence like:

```json
{
  "additive_copy_readiness": {
    "status": "ready",
    "source_layout": "bundle",
    "mapped_directories_replaced": true,
    "stale_destination_files_removed": true,
    "rollback_on_swap_failure": "verified",
    "same_path_alias_guard": "verified",
    "install_manifest_complete": true,
    "verification_failures": []
  }
}
```

The workflow verifies:

| Check | Ready condition |
| --- | --- |
| Source root | `amplifier-bundle/` or legacy `.claude/` is a real directory, not a symlink. |
| Staged replacement | Copy happens through a sibling `.staging` directory before replacing the destination. |
| Stale cleanup | Files removed from the source bundle are absent from the replaced destination. |
| Rollback | A failed final rename restores the previous destination tree. |
| Same-path guard | Source and destination aliases are detected before destructive replacement. |
| Verification | Source-derived verification proves required destination directories and bundle assets exist. |
| Layout marker | `.layout` records `bundle` or `legacy` so later checks use the correct mapping. |
| Manifest | `install/amplihack-manifest.json` records staged files, directories, binaries, and hook registrations. |

Typical targeted checks use real test filters such as:

```bash
cargo test -p amplihack-cli copy_amplifier_bundle_replaces_existing_atomically --lib
cargo test -p amplihack-cli copy_dir_recursive_skips_same_path --lib
cargo test -p amplihack-cli install_writes_layout_marker_atomically --lib
cargo test -p amplihack-cli read_manifest_rejects_path_traversal_in_dirs --lib
```

Missing source assets, symlinked source roots, unsafe path entries, failed
copy/swap, incomplete verification, or an incomplete manifest are blockers.

## 7. Interpret Terminal-State Outcomes

During recovery, the workflow checks terminal state before any version bump,
commit, push, PR creation, CI wait, or merge. A successful terminal state means
the recovery is complete without more Git or GitHub mutation.

The terminal-state block uses the same shape in publish, PR review, and
finalization:

```json
{
  "workflow_terminal_state": {
    "terminal_success": true,
    "terminal_state": "NO_DIFF_SUCCESS",
    "terminal_reason": "Worktree is clean and the branch has no meaningful diff against origin/main.",
    "publish_status": "NO_DIFF_SUCCESS",
    "should_publish": false,
    "should_finalize": true,
    "should_run_ci_wait": false,
    "should_merge": false
  }
}
```

Use the status exactly as emitted:

| Status | Meaning | What the workflow does next |
| --- | --- | --- |
| `MERGED` | The PR is already merged, or it is closed with merge evidence. | Stops successfully before publish, CI wait, or merge. |
| `CLOSED_OBSOLETE` | The PR is closed without merge evidence, but the branch is clean and the intended changes are already upstream or no meaningful work remains. | Stops successfully and records the obsolete proof. |
| `NO_DIFF_SUCCESS` | The worktree is clean and no meaningful diff or branch-only commits remain against the intended base. | Stops successfully without creating a no-op commit or follow-up PR. |
| `FOLLOWUP_CREATED` | Meaningful unmerged work remains and the workflow created or updated a PR for it. | Continues through normal CI and merge readiness checks. |
| `BLOCKED_CI` | Required checks are failing or policy blocks merge. | Fails loudly; do not reinterpret as terminal success. |

Closed-unmerged PRs are failures unless the workflow proves
`CLOSED_OBSOLETE`. A closed PR with remaining diff evidence is reported as a
blocker, not as recovered work.

Dirty worktrees also block terminal success. If `git status --porcelain` has
any output, the workflow cannot prove whether recovery work is complete or safe
to ignore, so it fails before accepting `MERGED`, `CLOSED_OBSOLETE`, or
`NO_DIFF_SUCCESS`.

Malformed inputs, unavailable PR metadata, missing base refs, and `gh` command
errors fail closed. Treat them as blockers to fix in workflow context, not as
accepted no-op recovery.

## 8. Let `workflow-publish` Own Commits and Pushes

If readiness remediation changes files, the workflow commits and pushes those
changes to the existing branch. This example shows PR 579:

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

If no changes are required, `changes_required` and `pushed` are `false`, and the
workflow records that no publish action was necessary.

If the terminal state is already successful, publish is skipped instead of being
reported as a no-op publish attempt:

```json
{
  "workflow_publish": {
    "target_pr": 579,
    "target_branch": "fix/issues-577-578-copilot-hooks-and-additive-copy",
    "publish_status": "MERGED",
    "terminal_success": true,
    "terminal_state": "MERGED",
    "version_bump_ran": false,
    "commit_ran": false,
    "push_ran": false,
    "pr_create_or_update_ran": false,
    "ci_wait_ran": false,
    "merge_ran": false
  }
}
```

Those suppression flags are part of the recovery evidence. They prove the
workflow did not perform Git or GitHub mutation, including version bump, stage,
commit, push, PR create/update, CI wait, merge, or duplicate/no-op follow-up PR
creation after completion was already established.

Dirty readiness documentation or evidence files are handled in one of two ways:

- committed by the workflow as recovery evidence, or
- explicitly classified as outside final readiness.

Unclassified dirty readiness files block final readiness.

## 9. Use `workflow-finalize` as the Final Decision

The finalization step emits the recovery decision. This example shows PR 579:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "final_status": "ready",
    "hook_readiness": "ready",
    "additive_copy_readiness": "ready",
    "manual_merge_performed": false,
    "remaining_blockers": []
  }
}
```

If hook readiness and additive-copy readiness are already satisfied at the
checked head, the workflow emits an accepted no-op instead of inventing a
change. The no-op names the exact head, files modified, and the observed check
state:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "head_sha": "8fb46865fb4412038b9313a62c02cc5aa0693132",
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
    "no_op_justification": "No workflow-owned hook or additive-copy readiness changes are required at head 8fb46865fb4412038b9313a62c02cc5aa0693132. Lint/Format and build checks are green; Test is still naturally in progress and branch protection keeps the merge state blocked, so the PR is workflow-ready but not merge-ready.",
    "manual_merge_performed": false,
    "merge_bypass_performed": false,
    "nested_default_workflow_launched": false
  }
}
```

Do not convert that result into a manual merge. Pending Test and blocked merge
state mean the PR remains under normal GitHub protection while being recovered
from the workflow's point of view.

Finalization re-checks terminal, dirty, diff, PR, and CI state before reporting
completion. A terminal-success finalization looks like this:

```json
{
  "workflow_finalize": {
    "pr_number": 579,
    "final_status": "finalized",
    "terminal_success": true,
    "terminal_state": "CLOSED_OBSOLETE",
    "terminal_reason": "PR #579 is closed without merge evidence, but the branch is clean and equivalent changes are present on origin/main.",
    "remaining_blockers": [],
    "version_bump_ran": false,
    "commit_ran": false,
    "push_ran": false,
    "pr_create_or_update_ran": false,
    "ci_wait_ran": false,
    "merge_ran": false
  }
}
```

If finalization observes a dirty worktree, a closed-unmerged PR without obsolete
proof, a meaningful unmerged diff, or failing required checks, it emits a
blocking status such as `BLOCKED_DIRTY_WORKTREE`, `BLOCKED_CLOSED_UNMERGED`,
`BLOCKED_UNMERGED_DIFF`, or `BLOCKED_CI`. Those statuses require remediation;
they are not successful no-op completions.

Interpret the status exactly as emitted:

| Status | Meaning |
| --- | --- |
| `ready` | The PR is ready for normal workflow-managed completion. |
| `blocked` | A concrete remaining blocker prevents readiness. |
| `finalized` | The workflow completed its permitted finalization path. |

Do not override a `blocked` result by manually merging. Rerun or remediate
through workflow context.

## Related Documentation

- [Workflow-owned PR recovery readiness](../features/pr-recovery-readiness.md)
- [PR recovery readiness reference](../reference/pr-recovery-readiness.md)
- [Tutorial: recover PR 579 readiness](../tutorials/pr-recovery-readiness.md)
- [Workflow execution guardrails](../features/workflow-execution-guardrails.md)
- [amplihack install reference](../reference/install-command.md)
