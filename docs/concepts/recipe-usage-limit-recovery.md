---
title: Recovery after a recipe usage limit
description: How relaunched recipe trees reuse verified work after a usage-limit stop.
doc_type: explanation
last_updated: 2026-11-18
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
- [Repository preservation during finalization](#repository-preservation-during-finalization)
- [Checkout storage and historical evidence](#checkout-storage-and-historical-evidence)
- [CI continuation contract](#ci-continuation-contract)
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

## Repository preservation during finalization

Step recovery and repository preservation solve different parts of continuation.
Recovery records establish which results can be reused; repository checks keep
local edits and commits from disappearing during finalization. A repository
path in a saved result cannot replace the repository itself.

Before checkout release, runtime removal, storage reclamation or the final
sweep deletes a directory, cleanup searches the entire deletion boundary for
repositories. Uncommitted changes or unpushed commits independently preserve
the candidate and the backing metadata needed to use it. The same rule covers
nested clones, linked worktrees, the primary execution checkout and target
destinations. No remote, an internal origin, a temporary location or an
inactive owner makes repository work disposable. Incomplete inspection also
preserves the candidate with an explanation.

Preservation is successful cleanup, so it does not turn a usage-limit stop into
a work failure. Normal output names the retained candidate and repository and
explains why it remains. The sweep applies these rules to older runs as well
as the ending run, and rechecks contents immediately before deletion. Existing
eligibility, ownership, locking and dependency requirements still apply.

Clean, fully pushed repositories remain removable when eligible. Ordinary
runtime artifacts still require declared recovery resources. Retention alone
does not make an incompatible completion replayable.

## Checkout storage and historical evidence

The checkout-location evidence is narrower than the reported loss:

- The private-clone work in #2290 introduced per-run `repository.git` and
  `checkout` storage. In both tags `v1.2.163` and `v1.2.177`,
  `RunCheckout::prepare_with_inputs` creates random `execution-<id>` storage
  under the runtime directory's **parent**. With short runtime storage at
  `r/<id>`, this is a sibling `r/execution-<id>/checkout`, rather than a child
  of `r/<id>`. Runs with a recovery identity instead claim `exec/<identity>`;
  an unavailable claim falls back to random storage.
- #2738 removed Git-command tracing and retained the private clone. It does
  not establish a relocation of the primary checkout inside `r/<id>`.
- In `v1.2.177`, `collect_runs` treats an inactive, unused runtime directory
  as `RunTmp` without checking its repositories for dirty or unpushed work.
  Checkout release also forcibly removes the primary checkout before the final
  sweep. These are confirmed deletion behaviors, distinct from the location
  evidence.

A step can create an independent clone or worktree beneath its runtime
`TMPDIR`. Deleting that runtime directory therefore risks deleting repository
work regardless of where the primary execution checkout lives. Behavioral
fixtures exercise that boundary; the CI continuation acceptance criteria below
require exercising it across a relaunch as well.

The issue reports runtime loss for `r/975c30488cc0`, `r/38e7ac3816f1` and
`r/8814ce575634`. Their original checkout paths, contents immediately before
cleanup, and complete removal records are unavailable here. The source
establishes a deletion risk; it does not prove which path deleted each reported
run's work.

For the contrast run `r/609c83e7f30b`, the owner reports a usage-limit stop
after a push and that the folder survived. Its contemporaneous dirty-state
evidence, cleanup-lock holder evidence and `run.json` are unavailable here.
A push does not establish a clean working tree; survival does not establish
whether a lock prevented a sweep or why the other folders disappeared. The
same limit applies to inferring a cause from the reported surviving v1.2.162
run. The incident discussion remains in issue #2785.

## CI continuation contract

**[CI verification pending]** The required release evidence is a
passing CI-only continuation scenario driven by
`tests/gadugi/run-issue-2785-usage-limit-keeps-work-scenario.sh` and described
by `tests/gadugi/scenarios/issue-2785-usage-limit-keeps-work.yaml`.
The driver declares recovery outputs for the repository path and commit,
relaunches the unpushed case with the same identity, and requires an executed
continuation to read the retained repository. The pushed counterpart checks
clean removal. These assertions require verification in CI; local component
tests and static validation of the driver do not supply release evidence.

Run the scenario only in CI's `install-smoke` job using the installed Amplihack
binary and pinned recipe runner. Missing prerequisites must fail the scenario.
Do not execute it on a development host, directly or through `gadugi-test`;
the driver's build fallback does not authorize local execution.

The scenario must execute a real recipe under a temporary HOME and work root,
with an isolated Git configuration and local bare origin. A completed step must
create a commit in a repository beneath runtime storage and record its path
and commit output using supported recovery declarations. A subsequent step
must exercise the production usage-limit interruption lifecycle. Normal
finalization must run, retain the work and report why.

A second launch must use the same recipe, compatible inputs, repository and
issue identity, and work root. It must verify predecessor and recovery evidence
and execute a continuation step that reads the retained repository, checks the
original commit and verifies its contents. Finding a SHA alone is insufficient.
Synthetic inactive metadata, a classification check alone, and manual cleanup
after interruption do not demonstrate this contract. A clean, fully pushed
counterpart must confirm eligible removal.

The driver uses Bash to exercise installed commands through the existing QA
harness; implementation remains Rust. Fixtures must never touch the real
`~/.claude` or `~/.amplihack`. Tests must neither read source text nor depend
on timers or retries.

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
