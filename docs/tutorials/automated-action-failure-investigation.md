---
title: Tutorial: Investigate an Automated Action Failure
description: Walk through a complete automated GitHub Actions failure investigation from user report to merge-ready PR.
last_updated: 2026-06-14
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: tutorial
related: "../howto/investigate-automated-action-failures.md, ../reference/automated-action-failure-investigation.md"
---

# Tutorial: Investigate an Automated Action Failure

[PLANNED - Implementation Pending]

This tutorial shows the intended end-to-end flow for the planned automated failure-investigation workflow. Until implementation lands, use it as a manual walkthrough and keep examples tied to current repository workflows.

This tutorial walks through the standard flow for a user report such as "many scheduled action runs are failing."

You will prove whether scheduled runs exist, identify the actual failing automation, fix only repo-caused failures, and open a merge-ready PR.

## What You'll Do

1. Confirm whether true `schedule` event failures exist.
2. Find recent failed automated runs.
3. Capture an evidence table.
4. Classify failures.
5. Fix the repo-caused failure.
6. Add regression coverage.
7. Open and monitor a PR.

## Prerequisites

From the repository root:

```bash
gh auth status
git status --short
export NODE_OPTIONS=--max-old-space-size=32768
```

`gh` is the GitHub service adapter for this workflow. Use the existing authenticated session only; do not create, print, or persist tokens.

Start from a dedicated branch:

```bash
git switch -c fix/automated-actions-failure
```

## Step 1: Check Scheduled Runs First

Run:

```bash
gh run list --event schedule --limit 50
```

If the repository has no scheduled runs, record that finding:

```text
No recent `schedule` event workflow runs were found. The report is treated as failing automated GitHub Actions, not true scheduled workflow failure.
```

## Step 2: List Recent Failures

Run:

```bash
gh run list --status failure --limit 50
```

Pick the failures that match the user's report. Prefer runs on maintained branches and recent SHAs.

Include `schedule`, `push`, `pull_request`, and relevant repository-owned `workflow_dispatch` runs. Exclude unrelated manual experiments, stale branches, deleted workflow definitions, forks, and platform-managed or bot-owned automation unless the user report explicitly targets them.

## Step 3: Inspect One Failed Run

Collect metadata:

```bash
gh run view 1234567890 \
  --json databaseId,name,event,status,conclusion,headBranch,headSha,url,createdAt,updatedAt,jobs
```

Collect failed-step logs:

```bash
gh run view 1234567890 --log-failed
```

If `gh` reports a transient rate limit or network error, retry once. If it still fails, classify the run as `external-transient`. If metadata is visible but failed logs are unavailable because of permissions or retention, classify it as `inaccessible`. Do not infer a repo defect from partial GitHub metadata.

Extract only the relevant failing lines. Example:

```text
test	Run cargo test	error: completion verifier accepted a workflow handoff without evidence records
test	Run cargo test	assertion failed: result.requires_follow_up()
```

## Step 4: Build the Evidence Table

Create the investigation table:

| Workflow | Run | Event | Branch/SHA | Failed job | Failed step | Root cause | Classification |
| --- | --- | --- | --- | --- | --- | --- | --- |
| CI | `https://github.com/rysweet/amplihack-rs/actions/runs/1234567890` | `push` | `main@0123456` | `test` | `Run cargo test` | Completion verifier accepted a workflow handoff without required evidence records. | `repo-caused` |

The table is the decision point. If the root cause does not point to a repository surface, do not edit repository files.

## Step 5: Fix the Narrowest Surface

For the example above, edit the Rust surface that owns completion verification:

| File | Purpose |
| --- | --- |
| `crates/amplihack-launcher/src/completion_verifier.rs` | Implements completion checks. |
| `crates/amplihack-launcher/src/completion_verifier_tests.rs` | Proves incomplete handoffs stay incomplete. |

Do not edit `.github/workflows/ci.yml` unless the failed log points to CI YAML behavior rather than Rust behavior.

## Step 6: Add Regression Coverage

Add or update the closest local guard. For a completion-verifier defect, use a targeted Rust test:

```bash
cargo test -p amplihack-launcher completion_verifier
```

The test asserts that a workflow handoff without required evidence records remains incomplete.

## Step 7: Validate

Run the repository gate:

```bash
pre-commit run --all-files
```

Run targeted validation for the changed surface:

```bash
cargo test -p amplihack-launcher completion_verifier
```

## Step 8: Commit and Push

Review the diff:

```bash
git diff --stat
git diff -- crates/amplihack-launcher/src/completion_verifier.rs crates/amplihack-launcher/src/completion_verifier_tests.rs
```

Commit:

```bash
git add crates/amplihack-launcher/src/completion_verifier.rs crates/amplihack-launcher/src/completion_verifier_tests.rs
git commit -m "Require evidence for workflow completion"
git push --set-upstream origin fix/automated-actions-failure
```

## Step 9: Open the PR

Write the PR body with evidence:

````markdown
## Evidence

| Workflow | Run | Event | Branch/SHA | Failed job | Failed step | Classification |
| --- | --- | --- | --- | --- | --- | --- |
| CI | https://github.com/rysweet/amplihack-rs/actions/runs/1234567890 | push | main@0123456 | test | Run cargo test | repo-caused |

## Schedule-event finding

No recent `schedule` event failures were found. The failing automation is a push-triggered `CI` workflow run.

## Root cause

The completion verifier accepted a workflow handoff without evidence records. The CI test failed because the verifier returned success for an incomplete investigation.

## Fix

Required evidence records before marking automated action investigations complete.

## Regression coverage

Updated the completion-verifier test so missing evidence fails locally before CI.

## Validation

```bash
pre-commit run --all-files
cargo test -p amplihack-launcher completion_verifier
```
````

Open it:

```bash
gh pr create \
  --title "Require evidence for automated action investigations" \
  --body-file /tmp/automated-actions-failure-pr.md
```

## Step 10: Monitor CI to Closure

Watch the PR checks:

```bash
gh pr checks --watch
```

If CI fails, compare the failure to your evidence table:

| CI result | Action |
| --- | --- |
| Same fixed surface fails | Fix the PR. |
| New failure caused by the change | Fix the PR. |
| External or transient failure | Document the failing check and rerun if appropriate. |
| Pre-existing unrelated failure | Document it as a blocker; do not expand the PR scope. |

The work is closed when the PR is merge-ready or blocked only by documented external or pre-existing failures.

## Related

- [How to Investigate Automated GitHub Actions Failures](../howto/investigate-automated-action-failures.md)
- [Automated Action Failure Investigation Reference](../reference/automated-action-failure-investigation.md)
