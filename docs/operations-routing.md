---
title: Operations and project-management routing
description: Why administrative work and repository changes use separate execution paths.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: explanation
---

# Operations and project-management routing

> **[IMPLEMENTED - Validation Pending]** This explains the intended issue
> #2553 feature. Its routing, tracking and recovery contracts require
> implementation and [behavioural validation](howto/verify-dev-orchestrator-routing.md).

Dev-orchestrator classifies work by the action required and where its result
belongs. `Operations` covers administration and project management: issue
triage, issue creation and assignment, research answers, status reporting and
coordination. These tasks run directly or through one agent with a checklist.

Changing tracked repository files is Development. Documentation wording,
configuration and recipes count as repository changes just as source code
does. Writing answers in chat or an issue remains Operations; committing those
answers to a documentation file introduces Development work.

## Contents

- [Why the boundary follows the action](#why-the-boundary-follows-the-action)
- [Mixed requests keep separate leaves](#mixed-requests-keep-separate-leaves)
- [Tracking and execution identity](#tracking-and-execution-identity)
- [Completion and safe resumption](#completion-and-safe-resumption)
- [Further reading](#further-reading)

## Why the boundary follows the action

“Create an issue and assign its owner” and “create a routing module” share a
verb but require different tools. The first needs an administrative action;
the second needs repository changes, tests and delivery. Counting requirements
or matching the word “create” cannot distinguish them.

Operations therefore retains its direct route regardless of checklist length
or a model-supplied recipe. It uses the declared repository as its working
directory and keeps its private artefacts elsewhere. It creates no clone,
worktree, branch or implementation PR. An existing dirty working tree does
not turn an administrative request into Development.

Investigation into an implementation, ordinary Q&A and structured consensus
retain their established routes. Research answers requested as part of an
administrative checklist remain Operations.

## Mixed requests keep separate leaves

Consider this request:

> Remove a programme name from code and documentation across repositories,
> answer a research question set, and open a cross-check issue for a colleague.

The executable plan contains Development leaves for the repository edits and
Operations leaves for the answers and issue creation. Cross-repository wording
changes have a separate Development leaf for each repository. When the
cross-check issue needs the research answers or completed edits, dependencies
make those results available first.

Each leaf receives only its task and relevant prerequisite results. The whole
request is not submitted again to an Operations agent. Otherwise that agent
could repeat issue creation or attempt the same edits outside the Development
workflow.

Route selection survives execution controls. Sequential mode, depth limits,
strict delivery and fallback preserve the leaves and dependencies. They reduce
concurrency without combining all the work into one Development task.
Development overrides and review phases apply only to the appropriate leaves.

## Tracking and execution identity

An issue records tracking; a workstream UUID identifies execution. Keeping the
two separate lets multiple issue-less workstreams have independent state,
logs, directories and process records. Adding tracking later does not rename
the workstream or make it collide with another leaf.

Unassigned issue values stay absent throughout decomposition, configuration,
saved state and child contexts. Blank values and recognised placeholders such
as `TBD` become absence. A malformed non-placeholder is still an error, so a
mistyped issue reference does not silently disappear.

Explicitly requested tracked work gets a real issue before execution unless
an applicable issue already exists. Issue-less work does not acquire tracking
automatically. Creating an issue as an administrative task is a separate
result; it becomes another leaf's tracking only through an explicit binding.

Tracking creation and the administrative issue adapter share one GitHub CLI
boundary. It reuses the existing `gh` authentication, pins `github.com` and the
declared repository, and disables prompts, inherited repository overrides and
debug output. Arguments are passed directly; issue bodies use a private input
file. Each invocation has a 120-second deadline and captures at most 64 KiB per
output stream. Cleanup targets only its own process group before reaping the
child. This mutation boundary requires Unix process-group support; other
platforms refuse the mutation before spawning a child.

Every error, deadline or invalid service receipt requires reconciliation.
The adapter never retries an uncertain mutation, including after a rate-limit
or authentication failure. A service may have accepted a request before its
response was lost. Raw CLI stderr is excluded from persisted diagnostics to
avoid retaining credentials; inspect GitHub to establish the external outcome.

## Completion and safe resumption

The dispatcher starts each ready leaf once. A prerequisite releases its
dependants only after its result is validated and durably recorded. Process
exit alone cannot prove that an issue was created or that a requested answer
was delivered. Failed prerequisites leave their dependants visibly blocked.

Completion events drive dispatch; each child has one waiter and its log
streams are drained to end-of-file. Dispatch does not depend on interval
probes, polling or timed retries.

Persisted success prevents a completed administrative action from being run
again on resume. An interrupted external mutation may have succeeded before
its result was recorded. The workstream then requires reconciliation with the
external system. It does not recreate an issue merely because local success
evidence is missing. A supported reconciliation command records verified
outcomes against the saved identity and revision. Resume reuses successful
actions; a new attempt requires proof of no effect and explicit retry
authorisation. See the [resume procedure](howto/run-operations-and-mixed-work.md#resume-without-repeating-administrative-actions).

UUIDs do not prove ownership. Migration, cancellation and cleanup still check
repository, directory, branch, run and process evidence. Operations cleanup
can remove its owned artefacts, never the repository in which it worked.

## Further reading

- [Run Operations and mixed requests](howto/run-operations-and-mixed-work.md)
- [Routing and JSON contracts](reference/dev-orchestrator-routing.md)
- [State, ownership and migration](reference/workstream-state.md)
- [Verify routing behaviour](howto/verify-dev-orchestrator-routing.md)
