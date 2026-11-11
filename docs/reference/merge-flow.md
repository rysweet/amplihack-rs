---
title: Merge Flow Reference
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
---

# Merge flow reference

> [Home](../index.md) > Reference > Merge Flow

> **[PLANNED — Implementation Pending] Proposed, runtime-unverified flow for
> [#2342](https://github.com/rysweet/amplihack-rs/issues/2342).** The flow below
> specifies the feature to be built. Current recipes synchronise behind
> branches unconditionally, run QA before CI, and lack final-gate `no_merge`
> enforcement. Conditional sync, CI-first QA, freshness, protected QA and
> persistent open PR delivery are proposed guarantees, not runtime assurances.

A PR is ready when current-head CI and QA evidence, review requirements and
applicable base policy all agree. Being behind the base alone is not a
reason to update the branch.

## Base updates follow applicable policy

Read the PR's actual base, mergeability, classic branch protection and
effective rulesets. The repository's documented policy is in
[Branch protection on main](../contributing/branch-protection.md); the drive
verifies applicable authoritative platform state for each PR.

| PR state | Base action |
| --- | --- |
| Clean, with no applicable update requirement | Preserve the published head. Do not merge or push merely because the base moved. |
| Conflicting | Resolve conflicts in the owned PR worktree and verify publication. |
| Subject to an unmet base-update requirement | Perform the necessary update and verify publication. |
| Policy unreadable or ambiguous | Retry or obtain equivalent authoritative evidence; keep readiness blocked until resolved. |

A `BEHIND` state does not itself prove that an update is required. Missing
classic protection information does not establish exemption from rulesets.
Never change protection or request bypass privileges to satisfy readiness.

## CI precedes QA

After any necessary update, the merge-ready agent observes CI associated
with the current published PR head. Pending or running work is an event to
await inside preparation. It does not fail a criterion, spend a corrective
round or establish that the loop is stuck.

Once complete green CI is verified, run repository suites and applicable
qa-team scenarios, then assess every remaining merge-ready criterion.
Completed failures require corrective action. Unreadable or incomplete CI
blocks readiness; missing checks require assessment against repository
requirements and trigger configuration, never automatic success.

QA uses a protected execution boundary without GitHub write credentials,
credential stores or authoritative evidence manifests. If that separation is
unavailable, report a security blocker. Head and tested-content identity are
checked before and after QA.

## Readiness follows the current revision

CI head, QA head, assessed head and current published PR head must agree.
Recheck freshness before declaring readiness and at the final action
boundary. A head change refreshes affected CI, QA and reviews; a same-head CI
rerun or relevant base/policy change also requires reassessment. When only
refresh is needed, skip corrective building.

A clean PR with running CI therefore keeps its head and awaits that run.
Updating it unnecessarily would restart CI and invalidate the evidence the
drive is waiting for.

## Delivery and merge gate

Use `no_merge=true` to deliver an open, merge-ready PR. The restriction
persists on resume and is checked at every merge-capable boundary. Automatic
merging stays disabled. Green CI and a `MERGE_READY` verdict do not override
the restriction.

An already-merged PR, whether observed at entry or during phase 3, violates
open PR delivery under `no_merge=true`; report the platform state and the
delivery violation without redoing merged work. Already-merged success
remains valid for merge-enabled runs.

For an authorized merge-enabled drive, the evidence gate revalidates all
criteria, records its evidence and binds the merge to the assessed head.
Hooks and protection remain enforced. A race that changes the head causes
refresh rather than a merge based on stale results.

When earlier work must be recovered, unresolved recovery blocks both
implementation and delivery. Record search findings, retained work and gaps
on [issue #2342](https://github.com/rysweet/amplihack-rs/issues/2342); design
and documentation may proceed. See the
[recovery prerequisite](autodrive-evidence.md#recovery-prerequisite).

See [Auto Drive To Merge](auto-drive-to-merge.md#open-pr-delivery) for usage
and configuration, [Readiness evidence](autodrive-evidence.md) for evidence
interfaces, and [Loop health](loop-health-evaluator.md#ci-waiting-and-evidence-refresh)
for progress during waiting and refresh.
