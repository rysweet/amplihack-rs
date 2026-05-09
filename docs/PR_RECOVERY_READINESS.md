# PR Recovery Readiness

PR recovery readiness is documented as a Diataxis set. Use this page as the
entry point, then follow the document that matches your task.

## Start Here

| Need | Document |
| --- | --- |
| Understand the feature | [Workflow-Owned PR Recovery Readiness](features/pr-recovery-readiness.md) |
| Recover an existing PR | [How to Recover an Existing PR with `default-workflow`](howto/recover-existing-pr-with-default-workflow.md) |
| Walk through the PR 579 example | [Tutorial: Recover PR 579 Readiness](tutorials/pr-recovery-readiness.md) |
| Look up fields, status mapping, and readiness gates | [PR Recovery Readiness Reference](reference/pr-recovery-readiness.md) |

The [reference](reference/pr-recovery-readiness.md) is the canonical contract
for workflow inputs, Copilot hook readiness, additive-copy readiness, no-op
readiness, workflow-finalize states, and final `MERGE_READY` /
`NOT_MERGE_READY` reporting.

## Core Rule

Recover an existing PR through `default-workflow` with `pr_number`,
`existing_branch`, and the current `headRefOid` as `expected_head_sha`.

```bash
PR_NUMBER=579
EXISTING_BRANCH=fix/issues-577-578-copilot-hooks-and-additive-copy
EXPECTED_HEAD_SHA="$(gh pr view "$PR_NUMBER" --json headRefOid --jq .headRefOid)"

amplihack recipe run default-workflow \
  -c "repo_path=." \
  -c "pr_number=${PR_NUMBER}" \
  -c "existing_branch=${EXISTING_BRANCH}" \
  -c "expected_head_sha=${EXPECTED_HEAD_SHA}" \
  -c "task_description=Recover PR #579 through the workflow-owned readiness gate; do not manually merge" \
  -c "issue_requirements=#577: Copilot plugin and native hooks are staged, registered, idempotent, and verified. #578: mapped framework directories replace stale amplihack-owned trees safely, preserve rollback, and guard source/destination aliasing."
```

Do not use `branch` for recovery context. Do not manually merge, bypass branch
protection, or treat workflow completion as merge readiness unless every
merge-readiness gate in the reference passes for the current PR head.
