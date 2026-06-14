---
title: Investigate Automated GitHub Actions Failures
description: How to identify failing automated GitHub Actions runs, prove root causes from logs, fix repo-caused failures, and deliver a merge-ready PR.
last_updated: 2026-06-14
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: howto
related: "../reference/automated-action-failure-investigation.md, ../tutorials/automated-action-failure-investigation.md"
---

# Investigate Automated GitHub Actions Failures

[PLANNED - Implementation Pending]

This document describes the intended behavior for an automated failure-investigation workflow. Until implementation lands, treat commands as the planned operator contract and verify each step manually.

Use this guide when a user reports failing scheduled actions, failing automation, or recurring GitHub Actions failures and the exact source is unclear.

The investigation is evidence-first. The workflow proves whether true `schedule` event runs exist before changing repository code.

## Contents

- [Prerequisites](#prerequisites)
- [Run the Investigation](#run-the-investigation)
- [Evidence Rules](#evidence-rules)
- [Classify Failures](#classify-failures)
- [Fix Repo-Caused Failures](#fix-repo-caused-failures)
- [Add Regression Coverage](#add-regression-coverage)
- [Validate and Open the PR](#validate-and-open-the-pr)
- [Troubleshooting](#troubleshooting)

## Prerequisites

- `gh` installed and authenticated for the target repository.
- `git` configured with push access.
- Repository checkout on a dedicated branch or a clean worktree.
- Large nested workflow runs use the project heap preference:

```bash
export NODE_OPTIONS=--max-old-space-size=32768
```

The persisted preference lives in the local Amplihack config file, usually `~/.amplihack/config`.

Before collecting Actions evidence, verify the GitHub CLI service adapter:

```bash
gh auth status
```

Use the existing authenticated session only. Do not create or print tokens.

## Run the Investigation

Run `smart-orchestrator` from the repository root:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Investigate failing automated GitHub Actions runs, prove root causes from gh evidence, fix repo-caused failures, add regression coverage, validate, push a PR, and monitor CI." \
  -c repo_path=.
```

For a known issue number, include it in the task description:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Investigate issue #776: user reports many failures in scheduled action runs. Confirm schedule-event evidence first, then fix repo-caused failures and open a PR." \
  -c repo_path=.
```

Direct `default-workflow` invocation is only an adaptive fallback after `smart-orchestrator` fails at the infrastructure level, such as parse, decomposition, or launch failure:

```bash
amplihack recipe run default-workflow \
  -c task_description="Investigate issue #776: user reports many failures in scheduled action runs. Confirm schedule-event evidence first, then fix repo-caused failures and open a PR." \
  -c repo_path=.
```

The workflow collects GitHub Actions evidence before it edits files. It checks true scheduled runs first:

```bash
gh run list --event schedule --limit 50
```

If no schedule-event runs exist, it records that finding and inspects recent failed automated runs:

```bash
gh run list --status failure --limit 50
```

Candidate automated runs include `schedule`, `push`, `pull_request`, and repository-owned `workflow_dispatch` runs from current workflows such as `CI`, `Docs`, `Code Atlas`, `Release`, `Publish Snapshot`, or `Invisible Character Scan`. Exclude manual experiments, stale branches, deleted workflow definitions, unrelated forks, and platform-managed or bot-owned automation unless the user report explicitly names them.

For each candidate run, it records metadata and failed-step logs:

```bash
gh run view RUN_ID \
  --json databaseId,name,event,status,conclusion,headBranch,headSha,url,createdAt,updatedAt,jobs

gh run view RUN_ID --log-failed
```

If a GitHub call fails, record the exact command and category: permission, authentication, rate limit, network, missing log access, not found, or other service failure. Retry once only for transient rate limit or network errors. If the retry still fails, classify the affected run as `external-transient` or `inaccessible`; do not infer a repository root cause from partial metadata.

## Evidence Rules

Every fix must trace to a failed GitHub Actions step. The workflow records an evidence table with:

| Field | Description |
| --- | --- |
| Workflow | GitHub Actions workflow name. |
| Run URL | Permanent URL for the failed run. |
| Event | Trigger event such as `schedule`, `push`, `pull_request`, `workflow_dispatch`, or automation-specific events. |
| Branch/SHA | `headBranch` and `headSha` from GitHub Actions metadata. |
| Failing job | Job name that concluded with failure. |
| Failing step | Step name from the failed-job log. |
| Root-cause excerpt | Short sanitized log excerpt tied to repository code, configuration, or external state. |
| Classification | One of the failure classes in [Classify Failures](#classify-failures). |

Do not use broad assumptions such as "scheduled actions are failing" as the root cause. If `gh run list --event schedule` returns no runs, the report is classified as failing automated GitHub Actions, not true scheduled workflow failure.

## Classify Failures

Classify each failed run before changing code:

| Classification | When to use it | Action |
| --- | --- | --- |
| `repo-caused` | The failed step points to checked-in code, workflow YAML, scripts, templates, or tests on the current branch. | Fix the implicated file and add regression coverage when feasible. |
| `generated-template-caused` | The failed run uses a generated workflow or lock file whose source lives in the repository. | Fix the source template and regenerate committed output. |
| `external-transient` | Logs show rate limits, network outages, service availability, or runner instability without a repo defect. | Document evidence in the PR or issue; do not patch around it unless retries are already part of the contract. |
| `stale` | The run came from an old SHA, deleted branch, superseded workflow, or workflow definition no longer present. | Document as stale; do not modify current code unless the same defect exists now. |
| `unrelated` | The failed run does not match the user report or target automation. | Exclude it from the fix scope and document why. |
| `inaccessible` | Metadata is visible but required logs are unavailable because of retention or permissions. | Document the missing evidence and next access step; do not infer root cause from workflow names alone. |

## Fix Repo-Caused Failures

Change only the surface implicated by the logs:

| Log points to | Edit |
| --- | --- |
| `.github/workflows/*.yml` behavior | The specific workflow file or its generated source. |
| Repository validation script | The specific `scripts/*.sh` file and its local test or fixture. |
| Rust command or library behavior | The affected crate under `crates/**` or binary under `bins/**`. |
| Generated workflow source | The authoritative template under `amplifier-bundle/**`, then regenerate committed output. |
| Validation tooling defect | The specific pre-commit or validation configuration. |

Avoid broad workflow rewrites, permission expansion, and speculative retries. The default fix is the smallest durable change that makes the proven failure impossible or visibly actionable.

## Add Regression Coverage

Add the closest practical local guard:

| Fixed failure mode | Regression coverage |
| --- | --- |
| Rust behavior | A targeted Rust unit, integration, or CLI smoke test. |
| Shell script behavior | A script test or fixture that exercises the failing branch. |
| Workflow YAML shape | Static validation, pre-commit hook, or template fixture. |
| Generated workflow drift | Source-vs-lock sync test or compiler fixture. |
| External/transient failure | No code regression test; document evidence and expected operator action. |

Regression coverage must fail for the original defect and pass after the fix.

## Validate and Open the PR

Run repository validation from the root:

```bash
pre-commit run --all-files
```

Then run targeted tests for the changed surface. Examples:

```bash
cargo test --workspace --locked

cargo test -p amplihack-launcher completion_verifier

bash scripts/check-recipes-no-python.sh
```

Commit only relevant changes:

```bash
git status --short
git add crates/amplihack-launcher/src/completion_verifier.rs crates/amplihack-launcher/src/completion_verifier_tests.rs
git commit -m "Require evidence for workflow completion"
git push --set-upstream origin fix/automated-actions-failures
```

Open the PR with the evidence chain:

```bash
gh pr create \
  --title "Fix automated Actions failure investigation path" \
  --body-file /tmp/automated-actions-failure-pr.md
```

The PR body includes:

- schedule-event finding
- evidence table
- root cause
- fix summary
- regression coverage
- validation commands
- remaining external, stale, inaccessible, or unrelated blockers

Monitor PR CI and fix only failures caused by the change:

```bash
gh pr checks --watch
```

Closure means the PR is merge-ready or blocked only by documented external or pre-existing failures. Do not merge unless the repository policy explicitly assigns merge ownership to the workflow.

## Troubleshooting

### No scheduled runs exist

If this returns no runs:

```bash
gh run list --event schedule --limit 50
```

Record the result and continue with recent failed automated runs:

```bash
gh run list --status failure --limit 50
```

The final report must say that no true `schedule` event failures were found.

### Failed logs are missing

Use the run metadata first:

```bash
gh run view RUN_ID --json databaseId,name,event,conclusion,url,jobs
```

If logs expired or permissions prevent access, classify the run as stale or inaccessible. Do not infer root cause from workflow names alone.

### CI remains red after the PR fix

Compare the failing PR checks to the original evidence table. Fix failures caused by the change. Document unrelated, external, or pre-existing failures with links to the failing checks and actionable next steps.

## Related

- [Automated Action Failure Investigation Reference](../reference/automated-action-failure-investigation.md)
- [Tutorial: Investigate an Automated Action Failure](../tutorials/automated-action-failure-investigation.md)
- [Scoped Workflow Closure](../concepts/scoped-workflow-closure.md)
