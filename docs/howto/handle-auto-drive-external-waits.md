---
title: Handle Auto-drive External Waits
description: Planned procedure for running and resuming merge-readiness assessment through CI and API availability changes.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Handle auto-drive external waits [PLANNED - Implementation Pending]

> This guide describes operation after the issue-2333 evidence interface is
> implemented. Its commands illustrate the planned workflow; they have not been
> behaviorally verified. Current `no_merge` constrains the build phase only; it
> does not prevent the final gate from merging. Until full-run enforcement is
> implemented, use `autodrive_dry_run=true` to suppress that gate's merge action.
> Evidence-based resume and attempt inspection below are also planned.

Use this guide when auto-drive is waiting for CI, rate-limit recovery or other
platform evidence. Keep the PR worktree available and GitHub CLI authenticated.
See the [auto-drive reference](../reference/auto-drive-to-merge.md) for QA prerequisites.

## Assess an existing PR while keeping it open

Planned launch from the repository containing PR #2333:

```bash
# [PLANNED] Requires full-run no_merge enforcement from issue #2333.
amplihack recipe run auto-drive-to-merge \
  -c pr_number=2333 \
  -c repo_path=. \
  -c task_description="Make merge readiness recover from unavailable GraphQL evidence" \
  -c no_merge=true
```

The run finds the PR's worktree and performs Crusty and merge-readiness assessment.
With supported clean evidence and evaluator `DONE`, it reports readiness and leaves
the PR open with automatic merging disabled. `no_merge=true` applies to the full
run, including light-mode escalation and the final executor.

To authorize a run to merge, set `no_merge=false` at launch. A trusted no-merge
directive or truthy `NO_MERGE` environment override still disables merging; agent
output cannot re-enable it. Do not change authorization by editing private state.

## Let the agent recover unavailable evidence

When GraphQL is refused, inspect the run's bounded evidence summary. It records
the refusal separately from CI results. The agent chooses whether to collect
through REST, gather another authoritative source or wait for recovery.

For REST CI assessment, confirm that the summary accounts for both current-head
check runs and commit statuses, applicable required checks and complete pagination.
A readable PR with `mergeable_state=clean` alone is insufficient. Review threads,
review requirements, QA and the other criteria still need supported evidence.

The successful REST receipt can replace the unreadable measurement in the accepted
assessment. The refusal remains in the history for diagnosis. A final GraphQL read
is unnecessary when the accepted sources establish all required conditions.

## Allow pending CI to finish

While CI remains pending, leave the run active. The loop-health evaluator can
return `CONTINUE` with an external-wait action across multiple rounds at the same
head. No new commit is required to justify waiting.

The wait ends at the agent-selected time or an existing registered readiness event.
The launcher supplies event references through a trusted adapter; agents cannot
invent them. If no adapter is available, the agent chooses a deadline instead.
See [event registration](../reference/pr-evidence-contract.md#readiness-event-registration).
After waking, the agent collects fresh evidence and reassesses. An event signals
that evidence may have changed; it never substitutes for successful checks.

Once complete evidence supports green CI and every other criterion, the readiness
assessment can become `MERGE_READY`. Advancement also requires evaluator `DONE`.
The final gate revalidates mutable conditions before any authorized merge.

## Resume after an execution-control stop

Read the recorded outcome before relaunching:

| Outcome | Operator action |
| --- | --- |
| External wait | Keep the run active; inspect the selected wake condition if needed |
| `BUDGET_EXHAUSTED` | Relaunch with the same PR/task and an explicitly higher operator budget |
| Usage limit, cancellation or orphaned run | Restore availability or intentionally resume; these are execution controls |
| Evaluation failure | Repair the failed invocation or invalid evaluator record before resuming |
| Agent `STUCK` | Read its cited evidence and recommended action; address the stated blocker |
| Refused before mutation | Fix the cited blocker and collect fresh evidence before requesting execution |
| Confirmed merged | Report completion; perform only permitted cleanup |
| Ambiguous merge attempt or interrupted mutation | Inspect the persisted attempt through `pr merge inspect`; do not rerun execution |

Planned resume with a larger per-loop budget follows below. Relaunching with the
same PR and task does not itself authorize importing old evidence. The trusted
launcher must reopen the original run registry or register a resume binding as
specified in [trusted resume](../reference/pr-evidence-contract.md#trusted-resume-and-cross-run-replay).

```bash
# [PLANNED] Requires trusted evidence resume and full-run no_merge enforcement.
amplihack recipe run auto-drive-to-merge \
  -c pr_number=2333 \
  -c repo_path=. \
  -c task_description="Make merge readiness recover from unavailable GraphQL evidence" \
  -c no_merge=true \
  -c autodrive_budget_loop_rounds=24
```

With a trusted resume binding, the run retains history and reassesses current
evidence. It does not require a new head commit, replay an ambiguous merge or treat
an old readiness decision as fresh.
Preserve private evidence files; publish only sanitized summaries when reporting a
problem.

For an ambiguous attempt, the launcher supplies a private inspect request naming
its registered `attempt_ref`; see the [planned inspect example](../reference/pr-evidence-contract.md#request-and-result-examples).
`CONFIRMED_MERGED` ends merge work. `CONFIRMED_NOT_MERGED` permits a new agent
decision, fresh assessment, evaluator decision and revalidation under current
authorization. `AMBIGUOUS` permits further inspection only. A single observation
that the PR is open does not prove an in-flight request cannot still complete.

See [budget configuration](../reference/auto-drive-to-merge.md#budget-keys-and-defaults)
and the [evidence contract](../reference/pr-evidence-contract.md) for details.
