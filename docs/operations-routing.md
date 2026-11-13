---
title: Operations and project-management routing
description: How requested actions determine routing, tracking and workspace preparation.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: explanation
---

# Operations and project-management routing

Operations and project-management work runs in the supplied execution context.
The native engine resolves each leaf before development setup, preserves mixed
plans and treats tracking issues as optional. Completion and cancellation events
drive execution. See [state and recovery](reference/workstream-state.md) for the
canonical plan, resumption and platform limits.

## The destination determines the boundary

Reading source files, tests or documentation does not request an edit.
Research answers delivered in chat, a report outside the repository or an
issue comment are Operations work. Committing the same answers into a
repository document is Development work.

Issue triage, issue creation and assignment, cross-repository coordination and
status requests are Operations. Their complexity, length or references to
code do not turn them into Development. An explicit repository edit still
requires Development handling when the surrounding request uses
administrative wording.

## Mixed requests retain every part

Consider this request:

> Remove a programme name from code and documentation across repositories,
> research a question set, and open a cross-check issue for a colleague.

The repository edits become Development leaves. The research and issue
creation become Operations leaves. Dependencies carry results between them:
an issue that needs the research answers waits for those answers.

The overall classification describes the request; each executable leaf has
its own route. An Operations-led plan therefore still executes its
Development leaves through Development workflows. Sequential execution,
depth limits and delivery settings reduce concurrency without dropping,
merging or rerouting leaves.

## Operations resolves before Development setup

A pure Operations request executes directly in the session, or through one
agent following the `operations-direct` checklist. An Operations leaf always
uses `operations-direct`, even if a model or configuration requests another
recipe. Routing is resolved before Development preparation, so Operations
does not create a clone, worktree, development branch or development pull
request.

Operations uses the supplied execution directory. Its private runtime
artefacts have a separate owned directory. Cleanup can remove those
artefacts without treating the caller's repository as disposable workspace.

`operations-direct` controls routing and setup. It does not provide
filesystem confinement: its agent still acts with the permissions of the
supplied environment. A request that also needs repository changes must
give those changes their own Development leaves.

## Tracking does not determine execution identity

A tracking issue is optional metadata. A workstream key identifies execution,
dependencies, results and owned artefacts. Initially tracked work keeps its
numeric identity. Issue-less work receives a safe key which is persisted
before execution and reused on resume.

Recognised placeholders such as `TBD` mean absence. They do not become zero
or stop an otherwise valid workstream. Attaching an issue later keeps the
same key, paths and dependency references.

Issue acquisition happens only when tracking is explicitly required through
`create_issue: true` and no issue is already present. The orchestrator saves
the acquisition intent and reconciliation reference before making the
request. If creation may have succeeded without a saved response, recovery
reconciles that request rather than blindly creating another issue.

## Related documentation

- [Run Operations and mixed requests](howto/run-operations-and-mixed-work.md)
- [Dev-orchestrator routing contract](reference/dev-orchestrator-routing.md)
- [Workflow classifier](reference/workflow-classifier.md)
- [Workstream state and recovery](reference/workstream-state.md)
