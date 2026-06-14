---
title: Automated Action Failure Investigation Reference
description: Reference for inputs, configuration, evidence records, classifications, and closure semantics for automated GitHub Actions failure investigations.
last_updated: 2026-06-14
review_schedule: quarterly
owner: amplihack-maintainers
doc_type: reference
related: "../howto/investigate-automated-action-failures.md, ../tutorials/automated-action-failure-investigation.md"
---

# Automated Action Failure Investigation Reference

[PLANNED - Implementation Pending]

This reference defines the intended contract for investigating user-reported scheduled or automated GitHub Actions failures. Remove this notice after the workflow and examples are proven in CI.

## Command Interface

The workflow is invoked through `smart-orchestrator`:

```bash
amplihack recipe run smart-orchestrator \
  -c task_description="Investigate failing automated GitHub Actions runs, prove root causes from gh evidence, fix repo-caused failures, add regression coverage, validate, push a PR, and monitor CI." \
  -c repo_path=.
```

Direct recipe invocation is an adaptive fallback, not the primary interface. Use `default-workflow` directly only after `smart-orchestrator` fails at the infrastructure level, such as parse, decomposition, or launch failure:

```bash
amplihack recipe run default-workflow \
  -c task_description="Investigate failing automated GitHub Actions runs, prove root causes from gh evidence, fix repo-caused failures, add regression coverage, validate, push a PR, and monitor CI." \
  -c repo_path=.
```

### Context Parameters

| Parameter | Required | Description |
| --- | --- | --- |
| `task_description` | Yes | Human-readable scope. Include the issue number, reported symptom, and requirement to prove schedule-event evidence before changing code. |
| `repo_path` | Yes | Repository root or worktree path. Use `.` when running from the root. |

## Required Tools

| Tool | Purpose |
| --- | --- |
| `gh` | Source of truth for workflow run metadata, failed jobs, logs, PR creation, and PR CI monitoring. |
| `git` | Branch, diff, commit, and push operations. |
| `pre-commit` | Repository-wide validation gate. |
| Repository test runner | Targeted regression validation for changed files. |

## Configuration

### `NODE_OPTIONS`

Large nested workflow runs use:

```bash
NODE_OPTIONS=--max-old-space-size=32768
```

The saved project preference is stored in:

```text
~/.amplihack/config
```

### GitHub Authentication

`gh auth status` must show access to the repository. The workflow uses the existing authenticated GitHub CLI session. It does not create, print, or persist tokens.

## GitHub Service Adapter Contract

The workflow integrates with GitHub through the GitHub CLI, not a custom API client. Treat `gh` as the service adapter boundary:

- Check `gh auth status` before collecting evidence.
- Use the existing authenticated session only.
- Record the failed command and error category when a GitHub call fails.
- Retry once only for transient rate limit or network failures.
- Do not retry permission, authentication, missing log access, or not-found failures as if they were transient.
- If metadata or logs remain unavailable, classify the run as `external-transient` or `inaccessible` and document the next access step.
- Do not infer a repository root cause from workflow names, run titles, or partial metadata.

### Repository Permissions

The operator needs permission to:

- read Actions runs and logs
- create branches
- push commits
- open pull requests
- read PR checks

The investigation must not weaken workflow permissions, branch protection, secret handling, or validation gates to make CI pass.

## Evidence Collection Contract

The workflow queries true schedule-event runs before broad failure searches:

```bash
gh run list --event schedule --limit 50
```

If no schedule-event failures exist, the workflow records that as a finding and queries recent failed runs:

```bash
gh run list --status failure --limit 50
```

### Automated Run Scope

Treat a failed run as in scope when it is both automated and related to the user report:

| Event or source | Include? | Notes |
| --- | --- | --- |
| `schedule` | Yes | Check first and report explicitly whether true scheduled failures exist. |
| `push` | Yes | Include maintained branches and current workflow definitions. |
| `pull_request` | Yes | Include PR checks when they match the reported automation or candidate fix. |
| Repository-owned `workflow_dispatch` | Sometimes | Include only when the user report names manual automation or the run is part of the maintained automation contract. |
| Bot-authored runs | Sometimes | Include when the bot is executing repository-owned automation, not when it is a platform-managed background task outside the repo contract. |
| Deleted branches or old SHAs | No fix by default | Classify as `stale` unless the same defect exists on the current branch. |
| Forks, unrelated experiments, platform-managed workflows | No | Classify as `unrelated` unless the user report explicitly targets them. |

For each candidate failed run, the workflow collects:

```bash
gh run view RUN_ID \
  --json databaseId,name,event,status,conclusion,headBranch,headSha,url,createdAt,updatedAt,jobs
```

Failed-step logs are collected with:

```bash
gh run view RUN_ID --log-failed
```

## Evidence Record Schema

Each investigated run produces an evidence record:

```json
{
  "workflow": "CI",
  "run_url": "https://github.com/rysweet/amplihack-rs/actions/runs/1234567890",
  "event": "push",
  "head_branch": "main",
  "head_sha": "0123456789abcdef0123456789abcdef01234567",
  "failing_job": "test",
  "failing_step": "Run cargo test",
  "root_cause_excerpt": "assertion failed: result.requires_follow_up()",
  "classification": "repo-caused",
  "repo_surface": "crates/amplihack-launcher/src/completion_verifier.rs"
}
```

### Fields

| Field | Required | Description |
| --- | --- | --- |
| `workflow` | Yes | Workflow display name from GitHub Actions. |
| `run_url` | Yes | Permanent GitHub Actions run URL. |
| `event` | Yes | Trigger event from run metadata. |
| `head_branch` | Yes | Branch associated with the run. |
| `head_sha` | Yes | Commit SHA associated with the run. |
| `failing_job` | Yes | Failed job name. |
| `failing_step` | Yes | Failed step name from the job log. |
| `root_cause_excerpt` | Yes | Short sanitized excerpt that proves the failure cause. |
| `classification` | Yes | One of the supported classifications. |
| `repo_surface` | Conditional | Required for `repo-caused` and `generated-template-caused` failures. |

## Classification Values

| Value | Meaning |
| --- | --- |
| `repo-caused` | Current repository code, workflow YAML, scripts, templates, or tests caused the failure. |
| `generated-template-caused` | Generated workflow output failed because its checked-in source template or lock generation is wrong. |
| `external-transient` | External systems, rate limits, network failures, hosted runner instability, or service outages caused the failure. |
| `stale` | The run came from an old SHA, deleted branch, expired workflow definition, or superseded automation. |
| `unrelated` | The run is outside the user-reported automation scope. |
| `inaccessible` | Metadata is visible but required logs are unavailable because of retention or permissions. |

## Fix Selection Rules

| Classification | Code changes allowed? | Required output |
| --- | --- | --- |
| `repo-caused` | Yes | Narrow fix, regression coverage, validation, PR. |
| `generated-template-caused` | Yes | Fix source, regenerate generated output, add sync coverage, validation, PR. |
| `external-transient` | No by default | Evidence and operator action. |
| `stale` | No by default | Evidence that the current branch no longer contains the failed definition. |
| `unrelated` | No | Reason excluded from scope. |
| `inaccessible` | No until proven | Missing-log explanation and next access step. |

## Regression Coverage Contract

Regression coverage must be as close as practical to the failed surface:

| Surface | Coverage |
| --- | --- |
| Rust crate or binary | `cargo test` for the affected crate, plus CLI smoke test when command behavior changed. |
| Shell script | Script fixture or targeted shell test. |
| Workflow source | Static YAML/frontmatter validation. |
| Generated workflow | Source-vs-lock synchronization test. |
| Documentation-only clarification | Link validation or markdown validation if configured. |

## PR Body Contract

The pull request body contains these sections:

````markdown
## Evidence

| Workflow | Run | Event | Branch/SHA | Failed job | Failed step | Classification |
| --- | --- | --- | --- | --- | --- | --- |
| CI | https://github.com/rysweet/amplihack-rs/actions/runs/1234567890 | push | main@0123456 | test | Run cargo test | repo-caused |

## Schedule-event finding

No recent `schedule` event failures were found. The user-reported failures are failed automated GitHub Actions runs triggered by `push`.

## Root cause

The `Run cargo test` step failed because the completion verifier accepted an incomplete workflow handoff as successful.

## Fix

Tightened completion verification so missing evidence records are reported as incomplete instead of successful.

## Regression coverage

Added a targeted completion-verifier regression test that fails when a workflow lacks required evidence records.

## Validation

```bash
pre-commit run --all-files
cargo test -p amplihack-launcher completion_verifier
```

## Remaining blockers

None.
````

Do not include secrets, full environment dumps, or unsanitized logs.

## Closure Semantics

Closure means:

- a branch is pushed
- a PR is open
- repo-caused failures are fixed
- regression coverage exists where feasible
- required validation has run
- PR CI is passing, or remaining failures are explicitly classified as external, stale, unrelated, or pre-existing

Closure does not mean automatic merge unless the repository policy assigns merge ownership to the workflow.

## Security Requirements

- Treat workflow names, branch names, logs, and PR text as untrusted input.
- Do not execute commands copied from logs unless they are verified against repository source.
- Sanitize log excerpts before including them in commits, PR bodies, or comments.
- Do not print or persist tokens.
- Do not broaden `GITHUB_TOKEN` permissions unless the failed step proves a legitimate missing permission and the new permission is minimal.
- Do not add `eval`, dynamic shell construction, or broad secret exposure paths.

## Related

- [How to Investigate Automated GitHub Actions Failures](../howto/investigate-automated-action-failures.md)
- [Tutorial: Investigate an Automated Action Failure](../tutorials/automated-action-failure-investigation.md)
- [Scoped Workflow Closure Reference](./scoped-workflow-closure.md)
