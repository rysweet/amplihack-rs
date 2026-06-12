# Scoped Workflow Closure

> [Home](../index.md) > Concepts > Scoped Workflow Closure

Scoped workflow closure prevents `default-workflow` monitors from treating an
unrelated pull request or stale process as current work.

## The Problem

Workflow closure used to infer ownership from broad signals: recent pull
requests, PR author, grep hits in titles or bodies, and PID-only process
records. Those signals are convenient, but they are not authoritative. They can
point at the wrong work when:

- the same user has multiple open PRs
- a newer unrelated PR appears while an older workflow is closing
- a process ID is reused by the operating system
- a persisted workstream record outlives the process it described
- a branch, repository, or work item is copied into text but does not identify
  the current workflow

Scoped workflow closure makes identity explicit. A workflow may notify,
publish, check readiness, or report terminal PR state only after it validates
the current work against first-class identifiers.

## The Core Rule

Current-work identity is never inferred from recency, authorship, broad text
matching, or a same-user PR list.

The workflow accepts a PR, process, or workstream as current only when it
matches the persisted scope for the run:

```text
repository + head branch + base branch + PR/tracking item + recipe/workstream + start time
```

Missing scope is not a soft match. It is display-only history.

## PR Scope

PR scope identifies the pull request owned by the current workflow. The
authoritative fields are:

| Scope field | Purpose |
| --- | --- |
| `repository` | Exact GitHub repository owner/name. |
| `head_ref` | Exact PR branch expected for the workflow. |
| `base_ref` | Exact target branch. |
| `pr_number` or `pr_url` | Concrete PR identity when already known. |
| `issue_number` or `work_item_id` | Tracking item that the PR is expected to close or reference. |
| `expected_pr_title_prefix` | Deterministic title prefix when the PR has not been persisted yet. |
| `created_after` | Run start time; older PRs are rejected. |
| `head_sha` | Exact head SHA required for readiness, final status, and no-op success paths. |

The shell helper `workflow_pr_scope.sh` is the single authority for current-work
PR ownership. `workflow_publish_pr.sh`, `workflow_pr_ready.sh`, and
`workflow_final_status.sh` call it before mutating or reporting PR state.

Persisted PR identity has priority. If the workflow already knows `pr_number` or
`pr_url`, the helper validates that concrete PR first and rejects any mismatch.
Scoped lookup is allowed only when concrete PR identity has not been persisted,
and it must use exact repository, head branch, base branch, start time, and a
tracking discriminator such as issue, work item, or title prefix. The helper
never falls back to recent PRs, PR author, or broad title/body search.

Cross-repository and fork PRs are rejected by default. A workflow that starts in
`rysweet/amplihack-rs` may close only a PR whose base repository and head
repository both match `rysweet/amplihack-rs`, unless a future recipe explicitly
opts into a narrower fork policy.

## Process Scope

Process scope identifies a running agent or workstream process owned by the
current workflow. The authoritative fields are:

| Scope field | Purpose |
| --- | --- |
| `pid` | Process ID captured at launch. |
| `process_started_at` or platform start marker | Runtime start metadata used to detect PID reuse. |
| `repo_path` | Canonical repository root. |
| `workdir` | Canonical worktree or workstream working directory. |
| `branch` | Git branch captured at launch. |
| `base_ref` | Base branch captured at launch. |
| `recipe_run_id` | Current recipe run identity. |
| `tree_id` | Recipe tree identity for nested runs. |
| `workstream_id` | Logical workstream identity. |
| `issue_number` or `work_item_id` | Tracking item owned by the workstream. |
| `started_at` | Workstream start time. |

The Rust validator rejects dead, reused, too-old, missing-scope, and
metadata-mismatched process records before monitors emit notifications or close
workstreams.

## Fail-Closed Behavior

Every scoped validator returns either an authoritative match or a named invalid
reason. Invalid scope blocks current-work notifications and closure decisions.

Common invalid reasons:

| Reason | Meaning |
| --- | --- |
| `missing_scope` | The persisted state predates scoped closure fields. |
| `no_scoped_pr` | No PR matches the explicit workflow scope. |
| `multiple_scoped_prs` | The scope is not specific enough to identify one PR. |
| `repo_mismatch` | Repository differs from the current workflow. |
| `workdir_mismatch` | Work directory differs from persisted process scope. |
| `branch_mismatch` | Branch or head ref differs from scope. |
| `workstream_mismatch` | Recipe, tree, or workstream id differs from scope. |
| `pid_reused` | Runtime process start metadata differs from the launch record. |
| `too_old` | Persisted process started outside the accepted age window. |
| `missing_scope` | Required PR or process scope is incomplete. |

Invalid records can still be shown in diagnostic output, but they are not
authoritative. A monitor may say that a stale record exists; it must not notify
as though that record is current work.

## Why This Is Simpler

The design centralizes identity decisions:

- one shell helper for PR ownership
- one Rust validator for process ownership
- one persisted scope model shared by multitask launcher and orchestrator

Recipes and monitors no longer duplicate partial matching rules. They ask the
scope validator whether a candidate is current work and respect the result.

## Related

- [Scoped Workflow Closure Reference](../reference/scoped-workflow-closure.md)
- [How to Configure Scoped Workflow Closure](../howto/configure-scoped-workflow-closure.md)
- [Tutorial: Scoped Workflow Closure](../tutorials/scoped-workflow-closure.md)
- [Recipe Runner Overview](../recipes/README.md)
