# Tutorial: Scoped Workflow Closure

**Time to complete**: 15 minutes
**Skill level**: Intermediate

This tutorial walks through a `default-workflow` run that ignores unrelated PRs
and stale process records while closing the PR owned by the current workstream.

## What You'll Learn

By the end of this tutorial you can:

1. Launch `default-workflow` with explicit PR and process scope.
2. Validate that a PR belongs to the current workflow.
3. Recognize stale process records that must not notify.
4. Read final status from scoped PR metadata.

## Prerequisites

You need:

- a writable clone of `rysweet/amplihack-rs`
- `gh auth status` working
- the `amplihack` CLI on `PATH`
- a branch for issue 754

## Step 1: Prepare Scope Variables

Start in the worktree for the issue:

```bash
cd /home/user/src/amplihack-rs/worktrees/feat/issue-754-scoped-closure

REPO_PATH=$(git rev-parse --show-toplevel)
REPOSITORY=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
BRANCH=$(git branch --show-current)
BASE_REF=main
ISSUE_NUMBER=754
STARTED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
```

The important values are `REPOSITORY`, `BRANCH`, `BASE_REF`, `ISSUE_NUMBER`,
and `STARTED_AT`. They define the current-work boundary.

## Step 2: Run `default-workflow`

Launch the recipe with the scope:

```bash
amplihack recipe run default-workflow \
  -c "repo_path=${REPO_PATH}" \
  -c "repository=${REPOSITORY}" \
  -c "branch=${BRANCH}" \
  -c "head_ref=${BRANCH}" \
  -c "base_ref=${BASE_REF}" \
  -c "issue_number=${ISSUE_NUMBER}" \
  -c "expected_pr_title_prefix=Fix issue #754:" \
  -c "created_after=${STARTED_AT}" \
  -c "task_description=Fix default-workflow monitor notifications for issue #754"
```

The launcher records process scope for each workstream. The persisted state
contains the PID, workdir, branch, issue number, recipe run id, tree id,
workstream id, base branch, and process start metadata.

## Step 3: Validate the Workflow PR

After the workflow publishes a PR, validate the exact PR:

```bash
PR_NUMBER=812
PR_URL=https://github.com/rysweet/amplihack-rs/pull/812
EXPECTED_HEAD_SHA=$(gh pr view "$PR_NUMBER" --json headRefOid --jq .headRefOid)

amplifier-bundle/tools/workflow_pr_scope.sh \
  --repo "$REPOSITORY" \
  --pr-number "$PR_NUMBER" \
  --pr-url "$PR_URL" \
  --head "$BRANCH" \
  --base "$BASE_REF" \
  --created-after "$STARTED_AT" \
  --issue "$ISSUE_NUMBER" \
  --expected-pr-title-prefix "Fix issue #754:" \
  --head-sha "$EXPECTED_HEAD_SHA"
```

Valid output looks like this:

```json
{
  "ok": true,
  "scoped": true,
  "number": 812,
  "headRefName": "feat/issue-754-scoped-closure",
  "baseRefName": "main",
  "headRefOid": "6c8e3b2a4f2e55d0fb65d51d94d8f4c16f37a111"
}
```

If PR 813 is newer but uses `feat/unrelated-work`, the helper rejects it as
`branch_mismatch`. The newer PR is not current work.

If the persisted `PR_NUMBER` or `PR_URL` points at a different repository,
branch, base, issue, or head SHA, validation fails. It does not search for a
different same-author or recent PR.

## Step 4: Observe Stale Process Rejection

A stale workstream record can appear in monitor state:

```json
{
  "workstream_id": "old-default-workflow",
  "scope": null,
  "process_scope": {
    "pid": 41872,
    "process_start_marker": "41872:73112200"
  }
}
```

When PID `41872` now belongs to a different process, validation returns:

```json
{
  "validation": "pid_reused",
  "authoritative": false,
  "notification_authority": false
}
```

The monitor may show the record for diagnostics. It does not notify that the old
workstream is active and does not close the current workflow based on it.

## Step 5: Check Readiness and Final Status

Run readiness with the same scope:

```bash
PR_NUMBER="$PR_NUMBER" \
PR_URL="$PR_URL" \
ISSUE_NUMBER="$ISSUE_NUMBER" \
EXPECTED_PR_TITLE_PREFIX="Fix issue #754:" \
WORKFLOW_STARTED_AT="$STARTED_AT" \
amplifier-bundle/tools/workflow_pr_ready.sh
```

Then read terminal status:

```bash
PR_NUMBER="$PR_NUMBER" \
PR_URL="$PR_URL" \
ISSUE_NUMBER="$ISSUE_NUMBER" \
EXPECTED_PR_TITLE_PREFIX="Fix issue #754:" \
WORKFLOW_STARTED_AT="$STARTED_AT" \
amplifier-bundle/tools/workflow_final_status.sh
```

Both commands validate PR scope before reporting readiness or terminal status.
If the PR head changes, `headRefOid` validation blocks stale success and the
workflow reruns checks against the new head.

## Step 6: Interpret the Result

Scoped closure produces one of these outcomes:

| Outcome | Meaning |
| --- | --- |
| `valid` | The PR or process belongs to the current workflow. |
| `blocked` | Required current-work scope is missing or mismatched. |
| `display-only` | State is readable for diagnostics but cannot notify or close. |
| `not_authoritative` | A candidate exists but fails one or more scope checks. |

The final workflow may close the tracking issue only after the scoped PR is
merged or the issue is explicitly superseded.

Fork and cross-repository PRs are not current work by default. The base
repository and head repository must both match the workflow repository.

## Related

- [How to Configure Scoped Workflow Closure](../howto/configure-scoped-workflow-closure.md)
- [Scoped Workflow Closure Reference](../reference/scoped-workflow-closure.md)
- [Scoped Workflow Closure](../concepts/scoped-workflow-closure.md)
