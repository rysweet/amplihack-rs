---
title: Dev-orchestrator routing contract
description: Classification, leaf plans, optional issues and trusted execution routes.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Dev-orchestrator routing contract

> **[IMPLEMENTED - Validation Pending]** This describes the intended issue
> #2553 behaviour. New JSON interfaces are contract examples, pending
> implementation and the [acceptance checks](../howto/verify-dev-orchestrator-routing.md).

The hook routing prompt, dev-orchestrator, classification helpers and execution
recipes use the same action-based rules. The Rust normaliser validates the
complete plan before tracking creation or child execution.

## Contents

- [Classification](#classification)
- [Route selection](#route-selection)
- [Decomposition JSON](#decomposition-json)
- [Normalised configuration](#normalised-configuration)
- [Optional issue parsing](#optional-issue-parsing)
- [Explicit tracking](#explicit-tracking)
- [Administrative-output tracking bindings](#administrative-output-tracking-bindings)
- [Normalisation API](#normalisation-api)
- [Execution controls](#execution-controls)
- [Related documentation](#related-documentation)

## Classification

`Operations` is the canonical label for operations and project management.
Classification boundaries accept equivalent labels, including `Ops`, `PM`,
`Project Management` and `project-management`, ignoring case and surrounding
whitespace. Stored plans and child contexts use `Operations`.

| Required action and destination | Classification |
| --- | --- |
| Triage, create or assign issues | `Operations` |
| Answer research questions, report status or coordinate work | `Operations` |
| Write a research answer in chat or an issue | `Operations` |
| Change tracked source, documentation, recipes or configuration | `Development` |
| Rename a term in documentation across repositories | `Development` for each repository edit |
| Investigate the implementation of an existing system | `Investigation` |
| Answer an ordinary explanation question | Existing `Q&A` route |
| Resolve a decision through structured consensus | Existing `Consensus` route |

The action and its destination determine the route. Generic verbs such as
“write” and “create”, numbered requirements and administrative complexity do
not promote Operations to Development. Reading repository files as research
does not make an activity Development; changing those files does.

A mixed request becomes separately executable leaves. Each leaf has one task,
one repository and one classification. Dependencies connect leaves whose
inputs require earlier results. Only those leaves execute: the original mixed
request is never also submitted to a direct Operations step.

### Classification precedence

Per-leaf fields have this precedence: `classification`, then `task_type`, then
`type`. Top-level `task_type` supplies a fallback for compatible homogeneous
plans and legacy configurations. An invalid explicit classification produces
a diagnostic rather than falling through to another field.

New mixed plans require explicit per-leaf classifications. A top-level label
cannot turn their Operations leaves into Development. Aliases are normalised
before route selection.

## Route selection

| Canonical leaf classification | Authoritative route |
| --- | --- |
| `Operations` | `operations-direct`: one agent following the leaf's checklist |
| `Development` | `default-workflow`, or eligible `quick-change` selection |
| `Investigation` | Existing investigation route, normally `investigation-workflow` |
| `Q&A` | Existing direct answer or recognised Q&A recipe route |
| `Consensus` | Existing consensus route |

A model-provided `recipe` is advisory. Operations always becomes
`operations-direct`, including when its recipe says `default-workflow` or
`quick-change`. Classic mode uses an equivalent direct adapter. Operations
creates no clone, worktree, branch or implementation PR.

Development retains the [quick-change selection rules](quick-change-recipe.md#precedence).
A model recipe alone cannot select `quick-change` in a decomposition. Valid
`workflow=quick-change` and `workflow=default-workflow` overrides apply only
to eligible Development leaves. Existing-branch, named-branch and PR guards
continue to apply; an override cannot change an Operations leaf's route.

Investigation, Q&A and Consensus retain their existing behaviour. A supplied
recipe cannot select an arbitrary executable or evade the trusted route map.
An inapplicable `quick-change` is replaced by the classification's own route.

## Decomposition JSON

`amplihack orch helper build-workstreams-config` reads a decomposition object
from standard input and prints the path of its atomically written configuration.
The output file has mode `0600` on Unix. Diagnostics go to standard error.

This complete input describes two issue-less leaves in the current repository:

```json
{
  "task_type": "Operations",
  "goal": "Explain routing, then document the distinction",
  "workstreams": [
    {
      "name": "research-routing",
      "repository": "rysweet/amplihack-rs",
      "classification": "Project Management",
      "description": "Explain in chat how Operations and Development differ.",
      "issue": " TBD ",
      "recipe": "default-workflow"
    },
    {
      "name": "document-routing",
      "repository": "rysweet/amplihack-rs",
      "classification": "Development",
      "description": "Update docs/operations-routing.md using the research result.",
      "depends_on": ["research-routing"]
    }
  ]
}
```

| Input field | Contract |
| --- | --- |
| `name` | Unique, non-blank plan key; dependency names refer to this key |
| `description` | Non-blank task handed to this leaf's agent |
| `repository` | Repository identifier authorised by the trusted run context |
| `classification` | Preferred classification; required on each new mixed-plan leaf |
| `task_type`, `type` | Compatibility classification fields, in that order |
| `workstream_id` | Existing UUID when continuing an established plan; otherwise allocated before normalisation |
| `issue` | Optional positive issue number; see parsing below |
| `depends_on` | Names of prerequisite leaves; defaults to an empty list |
| `tracking` | Optional tracking requirement and creation details; defaults to optional |
| `required_outputs` | Named output kinds validated before success; see the [result contract](workstream-state.md#completion-and-prerequisite-json) for declarations and defaults |
| `recipe` | Advisory route hint, checked against the canonical classification |

Omitted repository context is resolved from the trusted current repository for
compatible single-repository plans. Cross-repository leaves require explicit,
authorised repositories. A model cannot provide an arbitrary working directory.

## Normalised configuration

The helper emits a JSON array accepted by both `amplihack orch run` and
`amplihack multitask run`. The first entry above has this shape; generated UUIDs
vary between new plans and remain stable when the plan is resumed:

```json
[
  {
    "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
    "key": "research-routing",
    "repository": "rysweet/amplihack-rs",
    "classification": "Operations",
    "task": "Explain in chat how Operations and Development differ.",
    "description": "research-routing",
    "recipe": "operations-direct",
    "depends_on": [],
    "required_outputs": {"answer": "answer"},
    "tracking": {"required": false}
  }
]
```

The second entry retains its own UUID, selects `default-workflow`, and refers
to the first entry's UUID in `depends_on`. A Development branch is supplied or
generated without depending on an issue number. Operations has no generated
branch. Absent issues are omitted from canonical output.

`key` is the plan-local reference; `workstream_id` is the immutable execution
identity. Neither an issue number nor an array position is an execution identity.
The [execution schema](orch-run-command.md#workstreams-json-schema) lists the
remaining compatible fields.

## Optional issue parsing

One shared parser is used by decomposition, configuration, runtime state,
persistence and launcher context construction.

| Input | Result |
| --- | --- |
| Omitted, JSON `null`, `""` or whitespace-only string | Absent |
| `TBD`, `TBC`, `TODO`, `N/A`, `none`, `null`, `unknown`, `unassigned`, `pending` | Absent after trimming and case-insensitive comparison |
| Positive JSON integer through `18446744073709551615` (`u64::MAX`) | Present, preserved exactly |
| Trimmed ASCII decimal string in the same range, including `" 002553 "` | Present; canonical integer `2553` |
| `0`, negative, signed string, fraction, exponent notation, Boolean, object, array, URL, `"#2553"`, overflow or other text | Validation error |

For example, `" tBd "`, `" N/a "` and `"PENDING"` all mean absence. A malformed
value is diagnosed with its leaf and field; removing an optional issue is a
valid correction, while replacing it with another invented number is not.

`IssueNumber` holds a validated positive `u64`. `IssueRef` binds that number to
its repository; its JSON shape is `{"repository":"rysweet/amplihack-rs","number":2553}`.
The config's `issue` is scoped by its leaf's `repository`. A reference from a
different repository cannot satisfy this leaf's tracking requirement.

Absence remains absence throughout execution. It does not become `0`, `TBD`,
an empty environment variable or a requirement to create an issue.

## Explicit tracking

A request for new tracked work sets `tracking.required` to `true`. When there
is no applicable supplied or persisted issue or explicit producer binding, the
leaf supplies a non-blank `tracking.title` and `tracking.body`. For example:

```json
{
  "required": true,
  "title": "Document Operations routing in amplihack-rs",
  "body": "Update docs/operations-routing.md to distinguish administrative actions from tracked repository edits."
}
```

Tracking resolution follows this order:

1. Reuse an applicable supplied issue or a persisted successful resolution
   bound to this workstream and repository.
2. Resolve an explicit `from_result` binding after its prerequisite succeeds.
   An unresolved or invalid binding blocks execution; it never falls back to creation.
3. If tracking is optional, continue without creating an issue.
4. If tracking is required, validate creation details, persist an attempt,
   invoke `gh` once, validate the returned issue and persist success before
   execution.

Conflicting tracking references require reconciliation. A definite creation
failure is reported as such; the leaf does not run claiming to be tracked.
An ambiguous external outcome remains unresolved and is not retried
automatically. See [tracking and resumption](workstream-state.md#tracking-attempts).

“Open an issue for a colleague” is itself an Operations task. Its created issue
is an administrative result, not automatically the tracking issue for that
leaf or another leaf. Downstream tracking reuse requires an explicit binding
to the producer's result, the consuming workstream and the target repository.

## Administrative-output tracking bindings

Use a producer binding when issue creation is an explicit administrative leaf.
For a simple request to create tracking and then edit a document, tracking
resolution alone suffices; do not also schedule a duplicate issue-creation task.
This planned decomposition makes the producer/consumer interface explicit:

```json
{
  "workstreams": [
    {
      "name": "open-tracking",
      "repository": "rysweet/amplihack-rs",
      "classification": "Operations",
      "description": "Create an issue titled Clarify Operations routing examples, requesting clearer administrative and repository edit examples in docs/operations-routing.md.",
      "required_outputs": {"created_issue": "issue"}
    },
    {
      "name": "clarify-examples",
      "repository": "rysweet/amplihack-rs",
      "classification": "Development",
      "description": "Clarify the administrative and repository edit examples in docs/operations-routing.md under the created issue.",
      "depends_on": ["open-tracking"],
      "tracking": {
        "required": true,
        "from_result": {"workstream": "open-tracking", "output": "created_issue"}
      }
    }
  ]
}
```

The consuming leaf owns its binding through its own UUID and repository.
Normalisation resolves `from_result.workstream` to the producer UUID and emits
`workstream_id` in its place, preserving `output`:

```json
{
  "required": true,
  "from_result": {
    "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
    "output": "created_issue"
  }
}
```

`from_result` requires `required: true`, exactly one producer reference, and a
non-blank output name. It is mutually exclusive with a supplied `issue` or
creation `title`/`body`. The producer must be an explicit direct dependency
whose `required_outputs` declares that name as `issue`. Missing producers,
undeclared outputs, incompatible kinds and repository mismatches invalidate
the entire plan before any external action. Normalised input uses only the UUID
form; decomposition uses only the plan-key form.

At dispatch, validate the producer's persisted successful result and its
repository-scoped `IssueRef`, then atomically save the resolved reference on
the consumer before launch. Missing or conflicting evidence releases no
consumer. A producer failure blocks its dependants. Never create a substitute
issue when a binding cannot be resolved. See [completion and prerequisite
JSON](workstream-state.md#completion-and-prerequisite-json) for exact result fields.

## Normalisation API

`normalise_plan` is an internal pure Rust boundary, not an agent action or a
public library API. It receives the full leaf plan and trusted context,
including preallocated identities and authorised repository resolution.
It returns a complete normalised plan or typed diagnostics. It performs no
filesystem writes, UUID allocation, issue creation or process launch.

It validates tasks, repository bindings, keys, UUIDs, issue values, tracking
details and classifications; rejects duplicate identities, unknown or repeated
dependency references, self-dependencies and cycles; resolves dependency keys
to execution identities; and applies the trusted route map. An Operations
leaf requesting tracked-file edits is rejected for correction or decomposition
before external actions.

The configuration wrapper allocates identities once, calls the pure boundary
and atomically persists the complete result. Validation never launches a
valid prefix of an invalid plan. A failed persistence step prevents dispatch.

## Execution controls

| Setting or condition | Effect |
| --- | --- |
| `workflow=quick-change` or `workflow=default-workflow` | Development selection only, subject to existing eligibility guards |
| Unknown workflow override | Warning; no arbitrary recipe selection |
| `force_single_workstream=true` | Serial execution of the same leaves and dependencies |
| Strict delivery, depth limit or sequential fallback | Same leaves and routes; concurrency changes only |
| Resume | Same identities; completed leaves are skipped and uncertain mutations require reconciliation |
| Dirty initial repository | Does not promote Operations or trigger Development review, reflection or PR phases |
| `AMPLIHACK_AGENT_BINARY` | Chooses the agent backend, without changing the leaf's route |
| `AMPLIHACK_HOME` | Locates assets, without changing classification |

Development reflection, review and delivery validation apply to relevant
Development results. Operations results are assessed against their own task
and required outputs. Fallback never sends the entire mixed request through
Development or through the direct Operations adapter.

## Related documentation

- [Operations routing explained](../operations-routing.md)
- [Run Operations and mixed requests](../howto/run-operations-and-mixed-work.md)
- [Workstream state and migration](workstream-state.md)
- [Verify routing behaviour](../howto/verify-dev-orchestrator-routing.md)
- [Workflow classifier](workflow-classifier.md)
- [Coordination messages](coordination-messages.md)
