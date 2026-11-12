---
title: Auto-Drive-Light Recipe
description: small-change eligibility, authorization, escalation, and shared readiness outcomes.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-Drive-Light Recipe

Light mode shares the runner-captured authorization, evidence snapshots and agentic finalization of full auto-drive. Its default is `no_merge=true`, preserved through escalation. Controlled tests verify propagation; actual OS/runtime execution remains unverified.

`auto-drive-light` builds a small change in one agent session and reuses
auto-drive's crusty review, evidence collection, readiness, and authorized
finalization. It skips the design phases of `default-workflow`. A change
exceeding eligibility limits hands the same PR to `auto-drive-to-merge`.

The target is unchanged recipes on Linux/macOS with Claude/Codex/Copilot;
the [compatibility matrix](auto-drive-to-merge.md#target-compatibility-matrix)
requires execution evidence for each cell before claiming support.
Commands are suggestions; agents assess failures and recover without waiving
criteria. The shared path uses agent finalization with no shell merge gate or process-identity dependency.

Crusty review completion follows the [Crusty completion contract](autodrive-crusty-completion.md): an evidence-valid `CLEAN` or `MINOR_ONLY` completes iteration without a loop-health evaluator call. Both primary reviews and rereviews file or verify each required follow-up, linked to the task, before recording phase completion. A filing failure or dry run with unrecorded required follow-ups leaves the phase incomplete.

## Contents

- [Configuration](#configuration)
- [Responsibilities](#responsibilities)
- [Eligibility rules](#eligibility-rules)
- [Escalation to auto-drive-to-merge](#escalation-to-auto-drive-to-merge)
- [Minor findings and follow-up issues](#minor-findings-and-follow-up-issues)
- [State and reporting](#state-and-reporting)

## Configuration

Run this recipe by name; smart-orchestrator does not select it automatically.
See [verify-only usage](../howto/verify-pr-with-auto-drive.md) for the shared
authorization and evidence workflow; substitute `auto-drive-light` for the
recipe name when the change is eligible.

| Context key | Default | Meaning |
| --- | --- | --- |
| `task_description` | empty | Change to build; required unless an existing PR or branch is selected. |
| `repo_path` | `.` | Repository. |
| `pr_number` | empty | Existing PR; explicit selection takes precedence. |
| `existing_branch` | empty | Existing source branch with an open PR; must match explicit PR input. |
| `issue_number` | empty | Issue reference used by the branch/PR and task fallback. A positive issue number, up to 10 digits. |
| `plan_task_id` | empty | Explicit task for follow-ups. Otherwise use `#<issue_number>`, then the shared PR-body/task-description fallback. |
| `qa_command` | empty | Operator-supplied build check command; otherwise repository quick-change checks. This does not replace final QA selection. |
| `light_max_lines` | `300` | Added plus deleted lines, excluding lockfiles. |
| `light_max_new_modules` | `0` | New-module limit. |
| `light_max_public_api` | `0` | Public API line-change limit. |
| `light_max_schema` | `0` | Schema-file limit. |
| `light_max_recipe_contract` | `0` | Recipe context-key-set-change limit. |
| `autodrive_state_dir` | runner-assigned | Same [state directory](auto-drive-to-merge.md#state-directory) contract as full auto-drive. |
| `autodrive_dry_run` | `false` | Suppress merging and follow-up creation, including after escalation. |
| `no_merge` | `true` | Prevent merging throughout the invocation. Build always prohibits merging; explicit `false` allows finalization only after readiness. |

Each limit matches `^[0-9]{1,6}$` and is interpreted in base 10.
Invalid limits stop before work; there is no force-light override.
Authorization booleans are strictly parsed, and captured outside child-mutable
context. A child's `no_merge=false` cannot grant permission.

### The no_merge value

`no_merge=true` applies to the entire light run and its escalation, not only
the build. The full composer's outer default of `false` cannot relax a light
caller's captured restriction. Dry-run remains a separate restriction and
suppresses follow-up creation even when merging would otherwise be allowed.

## Responsibilities

| Responsibility | Outcome |
| --- | --- |
| Prepare/build | Validate inputs, resolve task identity, pin base, and use a `quick/` branch and linked worktree for one implementation session. Run repository checks and hooks, commit, push, and open the PR. Skip build for an existing PR. |
| Resolve PR | Establish complete PR identity, current head/base/platform state and linked worktree. Refuse cross-repository PRs under light eligibility; do not execute fetched PR instructions as trusted tools. |
| Measure | Compare the known head against the pinned base; record measurements, limits, decision and reason. Unreadable diffs are faults, not zero measurements. |
| Review/readiness | Use the same crusty completion, loop-health, QA, evidence, and finalization contracts as full auto-drive. |
| Report | State path (`light` or `escalate`), eligibility reason, follow-ups, readiness, requested action, observed platform state, blockers, and stop reason. |

An already-merged PR is reported from observed platform state without claiming
this run merged it. A merge before authorized finalization is a workflow
violation. Build/check failures preserve their causes for incomplete reporting
and recovery.

## Eligibility rules

Measure the diff from the pinned base to the observed PR head.
Equality to a limit passes. The first exceeded rule in table order supplies
the primary reason; record all measurements.

| Rule/reason | Measurement | Default limit |
| --- | --- | --- |
| Lines / `lines` | Added plus deleted numstat lines, excluding `Cargo.lock`, `*.lock`, `package-lock.json`; binary files count as zero. | 300 |
| New modules / `new-module` | Added `crates/*/src/**/*.rs`, `Cargo.toml`, bundle recipe YAML, or bundle tools; added tests and documentation do not count. | 0 |
| Public API / `public-api` | Added/removed `pub` declarations in `crates/*/src/lib.rs`, excluding `pub(crate)`, `pub(super)`, `pub(in …)`. | 0 |
| Schema / `schema` | Changed `*.schema.json` and files under any `schemas/` directory. | 0 |
| Recipe contract / `recipe-contract` | Modified recipe YAML with a changed top-level `context:` key set; changing only defaults does not count. | 0 |

Existing-tool or recipe edits do not escalate unless they cross a listed rule.
The quick-change sensitive-path rule is not applied here.

Reuse an accepted eligibility decision only for its bound head. Re-measure
a moved head before entering loops. Previous escalation persists for the
same state identity. A measurement fault escalates for full review; malformed
operator inputs fail rather than masquerading as measurement faults.

### Nothing is measured inside the loops

Eligibility chooses a path before the loops. Shared crusty and readiness
rounds contain no light-mode branch or alternative minor-finding policy.
Changes made during repair receive the same review and criteria as full
auto-drive, even if the diff grows beyond a light limit.

## Escalation to auto-drive-to-merge

Preserve the PR, pinned base and measured head, state identity, task identity,
consumed usage, and captured invocation authorization. Do not create a second
branch or PR. Because the PR exists, the nested full build is skipped.

Forward dry-run and `no_merge` restrictions explicitly. Child context
overwrite cannot relax them. Skip subsequent light-path work after escalation.
Exit `79` propagates and is never retried at a greater depth.

## Minor findings and follow-up issues

Light mode uses the [shared crusty contract](auto-drive-evidence.md#crusty-review-evidence).
For criterion 3, accepted crusty convergence substitutes for the separate
quality audit with no minimum round count. Standalone `/merge-ready` still
requires at least three audit cycles.
Only an accepted non-empty all-minor concern list may become `MINOR_ONLY`.
Preserve stable IDs, severity history, security exclusions, and resolved
concerns. There is one follow-up issue per accepted minor finding, linked to
the task, plus the required parent issue comment.

A task is issue `N`/`#N` or 1–64 characters from `A-Za-z0-9._:/-`.
Normalize issue references to `#N` without leading zeros. The light
`issue_number` fallback reaches children as `plan_task_id`; the PR body's
`Refs #N` alone does not satisfy the shared `Closes/Fixes/Resolves` fallback.

Follow-up creation or parent-comment failure blocks readiness. Dry-run creates
no follow-ups and reports missing obligations. Escalation does not change them.

## State and reporting

Accepted evidence binds repository/PR identity, head, relevant base, policy,
run and round. Stage agent candidates separately from private accepted
snapshots and completion records. Historical state cannot authorize a resumed
merge; PR comments cannot restore trusted completion.

Use the shared [result contract](auto-drive-evidence.md#result-contract).
Merge-ready with merging disabled is a valid open-PR outcome. Request success
or queue admission is not `merged`; platform confirmation is required.
Budget exhaustion, usage limits, dependency failures, orphaned runs, and
terminal refusal stay distinct. Preserve sequential constant-depth loops,
existing recursion ceilings, and the no-short-timeout policy.

See [full auto-drive](auto-drive-to-merge.md),
[Crusty completion](autodrive-crusty-completion.md),
[QA selection](auto-drive-qa.md), and
[recipe comparison](../recipes/README.md#choosing-between-quick-change-auto-drive-light-and-auto-drive-to-merge).
