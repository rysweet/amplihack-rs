---
title: Recovery after a recipe usage limit
description: How relaunched recipe trees reuse verified work after a usage-limit stop.
doc_type: explanation
last_updated: 2026-11-11
---

# Recovery after a recipe usage limit

Durable recovery reuses verified work after a usage-limit stop. The CLI requires
a runner advertising `durable_recovery_v1`; custom recipes must explicitly
permit persistence and declare their dependencies.

A relaunched `smart-orchestrator` run for the same repository and issue reuses
compatible completed work when the latest matching run stopped on a usage
limit. Classification, decomposition, and delegated `default-workflow` work
remain available even after the original process exits.

Recovery happens at step boundaries. The interrupted step executes again;
its partial output is never treated as a completed result.

## Contents

- [Three ways to continue work](#three-ways-to-continue-work)
- [Why only the latest run qualifies](#why-only-the-latest-run-qualifies)
- [How a recipe tree resumes](#how-a-recipe-tree-resumes)
- [Why workflow changes can remain compatible](#why-workflow-changes-can-remain-compatible)
- [What survives another stop](#what-survives-another-stop)
- [Durability and trust](#durability-and-trust)
- [Limits of recovery](#limits-of-recovery)

## Three ways to continue work

| Mechanism | When it applies | What it restores |
| --- | --- | --- |
| Same-process usage-limit wait/retry | The running process survives until capacity returns | Retries the blocked agent step within the existing execution |
| Durable relaunch recovery | A new process starts after an eligible usage-limit stop | Validated step results, typed context changes, and required artifacts across the recipe tree |
| `resume_checkpoint` | A caller selects an existing workflow checkpoint | The workflow's declared checkpoint behavior; it does not discover an earlier run or supply missing outputs |

Durable recovery preserves the existing wait/retry behavior. A recognized
usage-limit blockage becomes durable before the runner waits or unwinds. If
that process disappears, a later launch can use the record. If it instead
continues and succeeds or fails for another reason, that newer outcome
controls recovery eligibility.

`--fresh` bypasses historical recovery for the entire new execution tree.
It still records the new run, which can itself become a recovery source.
It does not clear an explicitly supplied `resume_checkpoint` or reset Git.

## Why only the latest run qualifies

The recovery identity combines the recipe and normalized work identity.
For `smart-orchestrator`, the work identity includes the canonical Git
common directory, repository identity, and positive issue number. Git
worktrees of the same repository share that identity; independent clones
remain separate. Nested invocations also carry their call path, branch,
and iteration identity so repeated recipe calls cannot exchange results.

Admission selects the latest matching attempt **before** inspecting its
outcome. Only a durable usage-limit stop from a predecessor whose owner is
no longer active permits recovery. A later success, ordinary failure, or
unknown termination blocks access to older usage-limit history.

For example, if run A stops on a usage limit and run B later fails a test,
run C starts fresh. It cannot skip B and recover A. The same rule applies
when B's detailed records have been removed: a retained latest-attempt
marker prevents older history from becoming eligible again.

An unidentified task runs fresh outside shared recovery namespaces. Text
that happens to mention an issue number does not establish an identity.

## How a recipe tree resumes

Each finalized successful step publishes a completion record. The record
includes the step's execution definition, effective inputs, compatibility
observations, structured result, typed context changes, and required
resources. A conditionally skipped step is reusable only after its skip
decision has been validated and recorded.

On relaunch, the runner walks completions in execution order, validates
each one, and applies its context changes using the ordinary execution
semantics. Objects, arrays, booleans, numbers, strings, null assignments,
and removed keys keep their meanings. Restoring a result does not invoke
its completed agent or repeat its completed side-effect hooks.

Consider a single-workstream development run:

```text
smart-orchestrator
  smart-classify-route                 completed: classification and decomposition
  smart-execute-routing               incomplete
    execute-single-round-1-development -> default-workflow
      workflow-prep                   completed
      workflow-worktree               completed
      workflow-design                 incomplete
        step-05-architecture          completed
        step-05b-api-design           stopped on a usage limit
```

The new run restores compatible classification, decomposition, preparation,
worktree, and architecture results. It enters the incomplete routing and
design invocations and executes `step-05b-api-design` again. An incomplete
parent does not hide completed children. A completed parent cannot hide a
changed child definition or missing child resource either.

Before dependent work can be reused, the new process tree establishes its
own session ownership, capacity-governor admission, recursion accounting,
and current authorization. Saved session IDs and admission receipts do not
grant access to live resources.

The progress log identifies the source run and the actual restart step,
including its nested invocation path. Structured results carry the same
decision; see [recovery result fields](../reference/recipe-result.md#recovery-metadata).

## Why workflow changes can remain compatible

A workflow can create a branch, commit changes, or materialize an artifact
before it stops. Comparing every saved step only to its original `HEAD`
would reject that workflow's own progress.

Instead, records track cumulative repository and worktree observations
through completed steps. The current checkout is checked against that
recorded progression, and replay reconstructs the recorded logical inputs
in order. A branch move accounted for by completed work can remain valid.
An unexplained revision change, dirty-file change, or changed relevant
artifact invalidates affected work.

Compatibility also covers effective typed inputs, execution settings,
execution-relevant recipe and hook definitions, and declared file
dependencies. Opaque dependencies read by shell commands or agents require
explicit resource declarations. A matching filename alone proves nothing
about its contents.

The first incomplete or incompatible step must execute again. Its result
and every later result within the affected invocation must be invalidated
in execution order. Invalidation also reaches dependent enclosing results.
Independent branches and iterations retain compatible progress unless they
depend on invalidated work. Recovery never
silently resets a branch or discards local edits to make records match.

## What survives another stop

Before executing work, each recovering attempt imports its inherited
completion candidates and required artifacts into its own durable state.
New completions are recorded alongside them. Inherited candidates still
require validation before reuse.

If A completes preparation, B recovers A and completes design, and C
recovers B, C can reuse both preparation and design. Deleting A's detailed
history does not break B's self-contained copy. Paths into disposable
runtime storage must be rebound through a declared artifact contract or
invalidate the affected output.

## Durability and trust

Recovery state lives under the run's private state directory, outside
`runtime/`. A single runner-owned writer publishes complete records
atomically and acknowledges completion only after durable publication.
Interrupted writes and interrupted steps never become completions. Failure
to register a current attempt or durably publish its state stops execution.

Delegated runners attach through an inherited private local channel. The
root writer limits each child to its attempt, invocation subtree,
operations, and current generation. A child usage-limit cause propagates
to the enclosing run. A historical child blockage cannot override a later
root outcome.

Private recovery records can contain prompts, context, outputs, and artifact
contents allowed by the recipe's persistence policy. They are sensitive
data, unlike the summary-only `run.json`. Private permissions do not
encrypt them or protect them from another process running as the same user.
Compatibility hashes detect changes; they do not authenticate records.

See the [recovery reference](../reference/recipe-recovery.md) for persistence
policies, resources, storage validation, and outcome synchronization.

## Limits of recovery

Recovery requires surviving, verifiable durable state. A missing, corrupt,
unsupported, or incomplete historical record causes a logged fresh
fallback. Termination before usage-limit evidence becomes durable is an
unknown outcome and starts fresh. An active owner is not taken over.

Recovery does not provide exactly-once execution for an interrupted step.
That step may already have created an external side effect. Existing
idempotency checks and terminal-state guards still apply when it runs again.
Unverifiable external resources and values prohibited from persistence
make dependent work non-replayable.

For launch commands and troubleshooting, use
[Resume a recipe after a usage limit](../howto/resume-recipe-after-usage-limit.md).
For retention, use
[Clean Up After Recipe Runs](../howto/clean-up-after-recipe-runs.md#preserve-or-retire-recovery-state).
