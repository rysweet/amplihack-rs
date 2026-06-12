# How to Configure Scoped Workflow Closure

> [Home](../index.md) > How-To > Configure Scoped Workflow Closure

Use this guide when `default-workflow` needs to publish, monitor, check
readiness, or finalize the PR that belongs to the current workflow run.

## Before You Start

You need:

- a writable checkout of the target repository
- the branch that the workflow owns
- the target base branch
- the issue number or work item id for the task
- `gh auth status` working when the repository is hosted on GitHub
- `amplihack` and the bundled `amplifier-bundle/tools/` files available

Optional: for large nested agent runs, set the Node heap once:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

Persist the same value in `~/.amplihack/config` when nested workflow agents need
to inherit it automatically.

## 1. Start from the Owned Worktree

Run from the worktree that owns the workflow:

```bash
cd /home/user/src/amplihack-rs/worktrees/feat/issue-754-scoped-closure

REPO_PATH=$(git rev-parse --show-toplevel)
BRANCH=$(git branch --show-current)
REPOSITORY=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
BASE_REF=main
STARTED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
ISSUE_NUMBER=754
```

Do not use `gh pr list --author @me` to discover current work. The workflow
identity comes from the scope values above.

## 2. Run the Workflow with Explicit Scope

Pass the scope into `default-workflow`:

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
  -c "task_description=Fix monitor over-notifications for issue #754"
```

The launcher records the same repository, workdir, branch, recipe run, tree,
workstream, PID, and process start metadata in multitask state. The
`expected_pr_title_prefix` field is the canonical recipe context name for
title-prefix lookup.

## 3. Validate a Known PR

After the workflow creates or receives a PR, persist and validate the exact PR
identity:

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
  --head-sha "$EXPECTED_HEAD_SHA"
```

The command prints a `valid` JSON object only when the PR belongs to this exact
workflow scope.

Known PR identity takes precedence after it is persisted. If `PR_NUMBER` or
`PR_URL` conflicts with the repository, head branch, base branch, issue, or head
SHA, scoped validation fails instead of searching for another PR.

## 4. Check Readiness

Use workflow-owned readiness, not a broad PR survey:

```bash
PR_NUMBER="$PR_NUMBER" \
PR_URL="$PR_URL" \
ISSUE_NUMBER="$ISSUE_NUMBER" \
EXPECTED_PR_TITLE_PREFIX="Fix issue #754:" \
WORKFLOW_STARTED_AT="$STARTED_AT" \
amplifier-bundle/tools/workflow_pr_ready.sh
```

`workflow_pr_ready.sh` calls `workflow_pr_scope.sh` first. If scoped validation
fails, readiness is blocked before checks, CI state, or mergeability are
reported as current-work evidence.

## 5. Read Monitor Output

Multitask monitor output distinguishes current work from stale history:

```text
workstream issue-754-closure: running
scope: valid
pid: 41872
branch: feat/issue-754-scoped-closure
recipe_run_id: run-01JZ7G7R4Z8KX8F2M9S5YH1VKT
```

Legacy or mismatched records are visible but non-authoritative:

```text
workstream old-default-workflow: display-only
scope: missing_scope
notification_authority: false
```

Only `scope: valid` can trigger monitor notifications or closure behavior.

## 6. Finalize with Scoped PR Status

Terminal status uses the same PR identity:

```bash
PR_NUMBER="$PR_NUMBER" \
PR_URL="$PR_URL" \
ISSUE_NUMBER="$ISSUE_NUMBER" \
EXPECTED_PR_TITLE_PREFIX="Fix issue #754:" \
WORKFLOW_STARTED_AT="$STARTED_AT" \
amplifier-bundle/tools/workflow_final_status.sh
```

If another PR is newer, authored by the same user, or mentions the same issue in
free text, it is ignored unless it matches the explicit scope.

Fork and cross-repository PRs are rejected by default. The PR base repository
and head repository must both match `REPOSITORY`.

## Troubleshooting

| Symptom | Cause | Fix |
| --- | --- | --- |
| `no_scoped_pr` | No PR matches the exact repository, branch, base, and start time. | Pass the known `pr_number` or correct the branch/base/start time. |
| `multiple_scoped_prs` | Scope is not specific enough. | Add `pr_number`, `pr_url`, `issue_number`, `work_item_id`, or `expected_pr_title_prefix`. |
| `branch_mismatch` | The PR head ref differs from the workflow branch. | Use the branch attached to the PR or block the workflow until branch ownership is resolved. |
| `invalid_pr_url` | The provided PR URL is not a GitHub pull request URL. | Correct the persisted PR URL before readiness/final-status steps. |
| `invalid_pr_number` | The provided PR number is not positive numeric text. | Correct the persisted PR number. |
| `missing_scope` | Persisted multitask state predates scoped fields. | Treat the record as display-only; relaunch the workstream to capture scope. |
| `pid_reused` | The PID exists but start metadata differs. | Ignore the old record; relaunch the workstream if work is still needed. |
| `repo_mismatch` or `workdir_mismatch` | State belongs to another checkout or worktree. | Run the monitor from the owning worktree or discard stale state. |

## Related

- [Scoped Workflow Closure Reference](../reference/scoped-workflow-closure.md)
- [Scoped Workflow Closure](../concepts/scoped-workflow-closure.md)
- [Tutorial: Scoped Workflow Closure](../tutorials/scoped-workflow-closure.md)
