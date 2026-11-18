---
title: Scoped Workflow Closure
description: How explicit workflow identity, publication, and validation keep closure tied to current work.
last_updated: 2026-11-18
review_schedule: as-needed
owner: workflow-team
doc_type: explanation
---

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

During discovery, tracking discriminators — issue/work-item token,
recipe/workstream id, title prefix, start time, and head SHA — are
**tie-breakers**. They resolve multiple candidates, such as an OPEN and a
previously CLOSED PR on the same head and base. A unique primary-key candidate
does not need a tie-breaker. When a PR number or URL is supplied, every
configured discriminator is a hard filter. Readiness also validates all
configured checks against fresh metadata before changing the PR.

For processes and workstreams the persisted scope still requires the full
identity tuple:

```text
repository + workdir + branch + recipe/workstream + start time + process marker
```

Missing primary-key scope is not a soft match. It is display-only history.

## PR Scope

PR scope identifies the pull request owned by the current workflow. The fields
identify the PR and constrain operations on it:

| Scope field | Role | Purpose |
| --- | --- | --- |
| `repository` | Primary key | Exact GitHub repository owner/name (same-repo only). |
| `head_ref` | Primary key | Exact PR branch expected for the workflow. |
| `base_ref` | Primary key | Exact target branch. |
| `pr_number` or `pr_url` | Authoritative identity | Selects a concrete PR; primary-key and configured discriminator checks still apply. |
| `issue_number` or `work_item_id` | Discriminator | Tracking item the PR is expected to close or reference. |
| `expected_pr_title_prefix` | Discriminator | Deterministic title prefix. |
| `created_after` | Discriminator | Run start time; used to discard older candidates. |
| `head_sha` | Discriminator | Exact head SHA for exact-head operations. |

The shell helper `workflow_pr_scope.sh` is the single authority for current-work
PR ownership. `workflow_publish_pr.sh`, `workflow_pr_ready.sh`, and
`workflow_final_status.sh` call it before mutating or reporting PR state.
`workflow_pr_ready.sh` and `workflow_final_status.sh` take the persisted PR
from `RECIPE_VAR_pr_number` and `RECIPE_VAR_pr_url` (or, when those are empty,
the `pr_publish_result` step output) and the issue from
`RECIPE_VAR_issue_number`. A bare `PR_NUMBER`, `PR_URL` or `ISSUE_NUMBER`
inherited from the launching shell is ignored, so it cannot name another
workflow's PR (see
[Step outputs ignore inherited bare variables](../reference/auto-drive-to-merge.md#step-outputs-ignore-inherited-bare-variables)).

Persisted PR identity has priority: a known `pr_number` or `pr_url` is
authoritative and validated directly. Otherwise the helper resolves the primary
key. A single primary-key candidate is adopted as-is — no issue token, head SHA,
or title prefix is additionally required. When multiple candidates share the same
head and base, a unique discriminator match wins, even if it is closed. If
multiple discriminator matches remain, the helper adopts the single OPEN PR
among them. If no discriminator matches remain, it instead looks for a single
OPEN PR among the original primary-key candidates. Either fallback returns
`multiple_scoped_prs` unless exactly one OPEN candidate remains. The helper never
falls back to recent PRs, PR author, or broad title/body search.

Publish is the one caller that may look past the base. On GitHub, with an
empty `base_branch` and no OPEN PR into the resolved base, it also accepts a
single OPEN same-repository PR from the same head into another base. Multiple
eligible OPEN PRs on other bases return `multiple_scoped_prs`. This prevents a
branch that already has an open PR from receiving a second PR into the default
branch (see
[An open pull request on another base](../reference/workflow-delivery-inputs.md#an-open-pull-request-on-another-base)).

Discovery can therefore identify the unique branch/base PR despite stale
tracking metadata. That result does not authorize readiness: readiness requires
the published HEAD and fresh identity evidence. Configured discriminator
mismatches transfer to agentic recovery for assessment rather than ending the
run for its own open PR.

## Publication Before Readiness

An open PR can identify the right branch while still missing audit work.
Finalization publishes from the recorded PR worktree before readiness reads
HEAD. The publication step commits audit edits and pushes outstanding commits
even when staging is empty. The launch checkout can be on `main` without
affecting that choice of worktree.

The recipe orders publication before readiness. Publication runs whenever
either finalization or readiness is requested, retaining terminal-success skips.
Its execution contract also requires a usable recorded worktree and a
successful push. Treating a worktree-resolution failure as a successful skip,
or substituting another
checkout for readiness, breaks that contract. Both operations require recorded
context and route failures through existing recovery. An empty staging area
describes whether a new commit is needed; it says nothing about whether
existing commits reached the remote. Ambient Git directory overrides cannot
redirect either operation to the launch checkout.

Unpublished audit work explains a local HEAD ahead of the PR's head. That
observation alone does not identify which publication operation was skipped or
failed in a particular run. Candidate diagnostics and recovery evidence expose
the failed checks without treating an ancestor SHA as successful publication.
If publication or readiness remains unresolved, recovery preserves work and
records the actual outcome while subsequent workflow steps continue. Completion
does not turn a safely declined mutation into a successful push or ready state.
See [Finalize Publication and PR Readiness](../reference/workflow-finalize-publication.md)
for inputs, execution conditions, and recovery behavior.

## Repository Boundaries

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
| `no_scoped_pr` | No candidate passes the applicable scope checks. Diagnostics distinguish an empty lookup from candidates excluded by primary-key or authoritative discriminator checks. |
| `multiple_scoped_prs` | Candidates remain unresolved after discriminator matching and the single-OPEN fallback, including unresolved closed candidates; also returned by opt-in discovery for multiple eligible OPEN PRs on other bases. |
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

Invalid scope also supplies evidence to finalize recovery. Publication and
readiness can safely omit a mutation while continuing the run; their unresolved
outcomes remain visible in status and completion.

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
