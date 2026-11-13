---
name: dev-orchestrator
description: Classify actions, separate administrative and repository work, and dispatch validated workstreams.
---

# Dev orchestrator

Classify the action required and where its result belongs. Use the native
`smart-orchestrator` recipe for Development and Investigation. Administrative
work can run directly or with one agent following a checklist.

## Classification

| Class | Action and destination | Execution |
| --- | --- | --- |
| Operations | Issue triage, creation and assignment; research answers in chat or issues; status reporting and coordination | Direct execution or one administrative agent |
| Development | Changes to tracked repository files, including documentation wording | `default-workflow`, or a trusted `quick-change` selection |
| Investigation | Examine an existing implementation and produce findings | `investigation-workflow` |
| Q&A | Answer a question | Direct answer or `qa-workflow` |
| Consensus | Existing consensus work | `consensus-workflow` |

Operations is canonical. Accept Ops, PM, project management and
project-management as equivalent labels. Complexity, requirement counts,
generic verbs such as write or create, and mentions of code paths, coverage or
existing PRs do not turn administration into Development.

Calibration examples:

- Triage these issues; open an issue for X and assign Y: Operations.
- Create an issue about `src/router.rs`: Operations.
- Write up research answers in chat or an issue: Operations.
- Report status or coordinate review of an existing PR: Operations.
- Rename a term across documentation in repositories A and B: Development in
  each repository whose tracked files change.
- Remove a programme name from code/docs, research questions and open a
  cross-check issue for a colleague: Development edits plus Operations research
  and issue creation. Preserve dependencies on the relevant results.

## Executable leaves

Mixed requests require separate leaves with explicit classifications. Each leaf
has a unique name, repository, task description, dependencies and declared
outputs. Only repository-editing leaves enter Development workflows. Execute
each leaf once; never send the entire mixed request to an Operations agent too.

Operations always selects `operations-direct`, regardless of model-supplied
recipes or Development overrides. It uses the trusted repository directly and
keeps private artefacts under its execution identity. It creates no clone,
worktree, branch or implementation PR. A dirty initial checkout does not alter
this route. Reflection, review and delivery gates belong to applicable leaves.

Development defaults to `default-workflow`. The classifier can recommend
`quick-change` for one small workstream; an explicit `workflow=quick-change` or
`workflow=default-workflow` overrides Development selection. Supplied branch or
PR constraints continue to rule out an unsuitable quick-change selection.
Operations, Investigation, Q&A and Consensus retain their own routes.

## Tracking and identity

Tracking is optional. Omitted values, nulls, blanks and recognised placeholders
mean absence. Ignore surrounding whitespace and letter case for TBD, TBC, TODO,
N/A, none, null, unknown, unassigned and pending. Preserve positive numbers
through `u64::MAX`; malformed non-placeholder values require correction.

Every leaf has a stable execution UUID independent of its issue number. Do not
create issues merely because tracking is absent. Explicit new tracking requires
`tracking: {required: true, title: ..., body: ...}` or an explicit binding to a
declared issue output from a prerequisite. Reuse supplied or durably resolved
tracking before creation. Failure or uncertainty is reported; never invent a
number or substitute a placeholder.

## Execution and recovery

Invoke `amplihack recipe run smart-orchestrator -c task_description="..."`.
For long runs use the recipe runner's `--detach` facility. Do not start another
orchestration from inside a recipe-managed step.

The Rust normaliser validates the complete plan, resolves dependency names to
execution identities and selects trusted recipes before external actions.
Repository authority comes from the current checkout plus operator-provided
`AMPLIHACK_REPOSITORY_ROOTS`, a path-separated list of roots whose origins are
verified. Model-supplied paths do not authorise repositories.

Forced serial mode, strict delivery and depth fallback preserve all leaves,
routes and dependencies; they change concurrency only. Child completion events
release capacity. Dependants receive only validated, durably saved prerequisite
results. Exit status alone does not prove success.

Resume the same saved configuration. Successful leaves are skipped. Unknown
external outcomes require `amplihack multitask reconcile`; state deletion, new
UUIDs and timed retries are not recovery procedures. See
[reference.md](reference.md) for JSON contracts and operational details.
