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

The workflow's own pull request is identified by a **primary key**: the exact
`repository + head branch + base branch` tuple, restricted to same-repository
(non-fork, non-cross-repository) PRs. GitHub allows only one OPEN pull request
per `head → base` within a repository, so this tuple is already unique for the
workflow's PR:

```text
repository + head branch + base branch  (same-repository, not cross-repo)
```

Tracking discriminators — PR/issue/work-item token, recipe/workstream id, title
prefix, start time, and head SHA — are **tie-breakers**. They resolve the rare
case of multiple candidates (for example, an OPEN and a previously CLOSED PR on
the same head and base). A stale or drifted discriminator never rejects the
unique primary-key PR.

For processes and workstreams the persisted scope still requires the full
identity tuple:

```text
repository + workdir + branch + recipe/workstream + start time + process marker
```

Missing primary-key scope is not a soft match. It is display-only history.

## PR Scope

PR scope identifies the pull request owned by the current workflow. The fields
divide into the primary key and the tie-breakers:

| Scope field | Role | Purpose |
| --- | --- | --- |
| `repository` | Primary key | Exact GitHub repository owner/name (same-repo only). |
| `head_ref` | Primary key | Exact PR branch expected for the workflow. |
| `base_ref` | Primary key | Exact target branch. |
| `pr_number` or `pr_url` | Authoritative override | Concrete PR identity when already known; bypasses primary-key and tie-breaker resolution. |
| `issue_number` or `work_item_id` | Tie-breaker | Tracking item the PR is expected to close or reference. |
| `expected_pr_title_prefix` | Tie-breaker | Deterministic title prefix. |
| `created_after` | Tie-breaker | Run start time; used to discard older candidates. |
| `head_sha` | Tie-breaker | Exact head SHA for exact-head operations. |

The shell helper `workflow_pr_scope.sh` is the single authority for current-work
PR ownership. `workflow_publish_pr.sh`, `workflow_pr_ready.sh`, and
`workflow_final_status.sh` call it before mutating or reporting PR state.
They take the persisted PR and issue only from `RECIPE_VAR_pr_number`,
`RECIPE_VAR_pr_url` and `RECIPE_VAR_issue_number`, never from a bare
`PR_NUMBER`, `PR_URL` or `ISSUE_NUMBER` inherited from the launching shell.
An inherited value could otherwise name another workflow's PR and decide what
gets closed (see
[Step outputs ignore inherited bare variables](../reference/auto-drive-to-merge.md#step-outputs-ignore-inherited-bare-variables)).

Persisted PR identity has priority: a known `pr_number` or `pr_url` is
authoritative and validated directly. Otherwise the helper resolves the primary
key. A single primary-key candidate is adopted as-is — no issue token, head SHA,
or title prefix is additionally required. Tie-breakers apply only when more than
one candidate shares the same head and base; if they eliminate everything but one
OPEN candidate remains, that candidate is adopted. OPEN is always preferred over
closed. The helper never falls back to recent PRs, PR author, or broad
title/body search.

This is why the workflow reliably adopts its own PR even when the branch bakes in
a local tracking issue (`feat/issue-1014-...`) while the PR body references a
different upstream issue (`#4610`), or when the remote head advances past the
locally captured SHA. The unique `(head, base, same-repo)` PR is matched;
the stale discriminators are ignored rather than treated as hard rejections.

Cross-repository and fork PRs are rejected by default. A workflow that starts in
`rysweet/amplihack-rs` may close only a PR whose base repository and head
repository both match `rysweet/amplihack-rs`, unless a future recipe explicitly
opts into a narrower fork policy.

When publishing, `workflow_publish_pr.sh` is collision-tolerant: if
`gh pr create` fails because a PR already exists for the branch, the helper
re-runs the scoped lookup and adopts the existing OPEN PR as an `existing-open-pr`
success instead of hard-failing the recipe. Only a genuinely absent PR combined
with a failed create is reported as `FAILED_PR_CREATE`.

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
| `no_scoped_pr` | No PR matches the primary key (repository, head, base). This is an empty result, not a stale-discriminator rejection. |
| `multiple_scoped_prs` | Two or more OPEN candidates share the same head and base and survive tie-breaking. |
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
