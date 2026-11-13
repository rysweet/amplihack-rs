---
title: Dev-orchestrator routing contract
description: Per-workstream routing, optional tracking and plan validation.
last_updated: 2026-11-13
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Dev-orchestrator routing contract

This reference describes per-workstream classification, routing and tracking.

## Implementation boundary

The native engine accepts branchless Operations leaves, immutable keys,
classification, dependencies and Boolean `create_issue`. Both the decomposition
helper and native execution validate the entire plan. `dependencies` is the
canonical spelling; `depends_on` is accepted for existing decomposition assets.

Operations runs in the caller's directory. Development uses the existing clone
and recipe preparation. Each leaf has a separate owned runtime directory.
Completion and cancellation use events; runtime budgets remain saved metadata
and do not trigger timed interruption. Completed leaves are preserved on resume.
A saved live or uncertain launch is rejected for manual reconciliation because
this engine cannot safely observe a restored child. See
[state and recovery](workstream-state.md#implementation-boundary).

## Intended contract

Every executable workstream has its own classification, route and immutable
key. The complete plan is normalised and validated before issue acquisition,
workspace preparation or agent execution.

## Contents

- [Routing invariant](#routing-invariant)
- [Classification authority](#classification-authority)
- [Classification labels and aliases](#classification-labels-and-aliases)
- [Recipe normalisation](#recipe-normalisation)
- [Operations routing](#operations-routing)
- [Optional tracking issues](#optional-tracking-issues)
- [Workstream identity](#workstream-identity)
- [Workstream JSON API](#workstream-json-api)
- [Configuration](#configuration)
- [Validation and execution](#validation-and-execution)
- [Security contract](#security-contract)

## Routing invariant

Operations always selects `operations-direct`. A conflicting recipe,
including a custom Operations recipe, cannot override that route.

Development selects `default-workflow` or an eligible `quick-change` route.
Existing Investigation, Q&A and Consensus routes retain their established
behaviour. See [Quick-change selection](quick-change-recipe.md#precedence).

These rules apply to every leaf, including Development leaves in an
Operations-led plan. Top-level classification and concurrency settings
cannot suppress required work.

## Classification authority

| Source | Effect |
| --- | --- |
| Per-workstream `classification`, `task_type`, or `type`, in that order | Determines the leaf's classification |
| Top-level `task_type` | Fallback when the leaf has no classification |
| Model-provided `recipe` | Advisory; subject to route validation and the routing invariant |

Classification follows the requested action and destination. Repository-file
edits require Development handling, including wording changes in
documentation. References to source paths, tests or documentation alone do
not establish edit intent. Research reports and issue comments use Operations
handling; writing those reports into repository files uses Development.

Mixed requests are decomposed before dispatch. Each leaf receives its own
task and prerequisite results, so an Operations agent is not asked to execute
the original mixed request again. See [Classifier calibration](workflow-classifier.md#operations-classification).

## Classification labels and aliases

Labels are trimmed and compared without case sensitivity. `Ops`,
`Operations`, `PM`, `Project Management` and `project-management` identify
Operations. Canonical plans and child contexts use `Operations`.

The remaining canonical labels are `Development`, `Investigation`, `Q&A`
and `Consensus`. The compatibility fields `task_type` and `type` follow the
same classification rules as `classification`.

## Recipe normalisation

| Leaf classification | Input selection | Resolved route |
| --- | --- | --- |
| Operations | Any recipe or override | `operations-direct` |
| Development | Missing, blank or model-selected recipe | `default-workflow` |
| Development | Eligible `workflow=quick-change` selection | `quick-change` |
| Development | `workflow=default-workflow` | `default-workflow` |
| Investigation | Missing, blank or Development recipe | `investigation-workflow` |
| Q&A | Missing, blank or Development recipe | `qa-workflow` for a plan leaf; direct answer for top-level Q&A |
| Consensus | Missing, blank or Development recipe | `consensus-workflow` |
| Investigation, Q&A or Consensus | Explicit supported non-default route | Existing selection rules apply |

Recipe identifiers must be supported by the runner. Arbitrary task text is
never interpreted as a recipe identifier or shell command.

## Operations routing

A pure Operations request executes directly or through one scoped checklist
agent. A decomposed Operations leaf uses `operations-direct` regardless of
recipe overrides. Routing occurs before branch lookup, clone creation or
worktree preparation.

Operations executes in the supplied context and stores private runtime
artefacts separately. It does not run `default-workflow`, create a
development workspace or open a development PR. A dirty working tree does
not change the classification or authorise a PR.

The checklist agent reports `completed`, `partial` or `failed`, with the
result and any unfinished actions. An unsuccessful process or runner exit
overrides a claim of success in its prose. Partial work is not sufficient to
release a dependant that requires successful completion.

The route controls orchestration and Development setup. It does not provide
filesystem confinement.

## Optional tracking issues

One parser normalises issue values before numeric validation or downstream
use.

| Input | Result |
| --- | --- |
| Missing field, JSON `null`, empty or whitespace-only string | Absent |
| `TBD`, `TBC`, `TODO`, `N/A`, `none`, `null`, `unknown`, `unassigned`, `pending` | Absent; trimmed and compared without case sensitivity |
| Positive JSON integer through `18446744073709551615` | Present |
| Positive ASCII decimal string, including `"002553"` | Canonical integer `2553` |
| `"#2553"` | Canonical integer `2553` |
| `"https://github.com/rysweet/amplihack-rs/issues/2553"` | Canonical integer `2553` |
| Zero, negative, fraction, overflow, Boolean, container or unknown string | Validation error |

Issue URLs must use the `github.com` host and an `/issues/<number>` path.
Parsing is lexical and performs no network request. The number is tracking
metadata; reading an issue as research material does not attach it as tracking.

Absent issues are omitted from generated configuration and child context.
The child receives neither a fabricated `issue_number` nor an inherited
`AMPLIHACK_WORKSTREAM_ISSUE` from another leaf. Existing positive numbers
remain available to downstream tracking consumers.

Legacy persisted numeric zero reads as absence. This compatibility applies
only to saved state: zero in new configuration is invalid.

### Explicit tracking acquisition

| `issue` after normalisation | `create_issue` | Behaviour before execution |
| --- | --- | --- |
| Present | Missing or false | Use the existing issue |
| Present | True | Use the existing issue; do not create a duplicate |
| Absent | Missing or false | Execute without an issue |
| Absent | True | Acquire and persist a valid issue before executing this leaf |

`create_issue` is a Boolean. Absence alone never requests creation. Explicit
requests for tracked work set it on the affected leaves.

The existing authenticated tracking service performs acquisition. Intent and
a stable reconciliation reference are saved first; the validated result is
saved before launch. Failed acquisition prevents that leaf from executing.
An uncertain result is reconciled on resume, or reported as unresolved with
an actionable error. It is never retried blindly. See [Acquisition recovery](workstream-state.md#tracking-acquisition).

## Workstream identity

The workstream key is independent of optional tracking metadata. Keys contain
1–64 ASCII letters, digits, hyphens or underscores. Unsafe or duplicate keys
are rejected before external actions.

Initially tracked work preserves its numeric identity, for example `2553`.
An issue-less leaf receives a safe unique key during normalisation. The key
is persisted with the canonical plan before execution and reused on resume.
Adding an issue never changes its key, artefact paths or dependency references.

Saved identity or ownership conflicts are errors. A mismatched saved key is
not ignored or replaced with a newly derived identity. See [Workstream state](workstream-state.md).

## Workstream JSON API

The following transformation and schema require implementation.

`amplihack orch helper build-workstreams-config` accepts a decomposition object
with `task_type` and `workstreams`. `amplihack orch run` consumes the
normalised workstream array. The transformation preserves each leaf's task,
description, classification, dependencies and tracking intent.

This is an Operations-led decomposition with a dependent repository edit:

```json
{
  "task_type": "Operations",
  "workstreams": [
    {
      "key": "research-routing",
      "classification": "Operations",
      "task": "Read issue #2553 in rysweet/amplihack-rs and answer its routing questions in the result.",
      "description": "Research routing requirements",
      "issue": "TBD",
      "create_issue": false,
      "recipe": "default-workflow"
    },
    {
      "key": "document-routing",
      "classification": "Development",
      "branch": "feat/document-operations-routing",
      "task": "Use the prerequisite answers to update docs/operations-routing.md.",
      "description": "Document routing requirements",
      "depends_on": ["research-routing"],
      "create_issue": false
    }
  ]
}
```

The first leaf resolves to `operations-direct` and has no tracking issue.
The second resolves to `default-workflow` and runs after successful research,
receiving its result under `research-routing`. Both retain their keys.

The [execution schema](orch-run-command.md#field-reference) defines the fields
used in the normalised array.

## Configuration

| Setting | Routing effect |
| --- | --- |
| `workflow=quick-change` or `workflow=default-workflow` | Applies only to eligible Development work; cannot change Operations routing |
| Unknown `workflow` override | Warning; does not select an arbitrary recipe |
| `force_single_workstream=true` | Executes required leaves sequentially; preserves their tasks, routes and dependencies |
| `strict_delivery` | Preserves delivery requirements on Development leaves without dropping Operations leaves |
| `AMPLIHACK_MAX_DEPTH=0` | Does not discard leaves; the caller must supply a supported execution context |
| `AMPLIHACK_AGENT_BINARY` | Selects the agent backend; does not change classification or routing |
| `AMPLIHACK_HOME` | Locates installed assets; does not change routing |

Installed classification prompts, hook guidance and recipe assets follow the
same boundary as native execution.

## Validation and execution

Validation checks non-empty tasks, safe unique keys, issue values, supported
routes, dependency references and acyclicity across the complete plan.
Independent errors are reported together with workstream keys and field
paths. Invalid plans perform no tracking requests, workspace preparation or
agent execution.

Completion and cancellation events drive scheduling. A completed prerequisite
releases capacity and makes eligible dependants runnable after its result is
persisted. Failed, partial or cancelled prerequisites visibly block affected
dependants. Successful prose cannot turn process failure into completion.
Persistence failure stops additional launches and preserves recovery data.

## Security contract

Plans, saved state and prerequisite results are untrusted input. Keys and
routes are validated before use; task and service values travel through
structured interfaces or separate process arguments. Issue numbers confer
no additional authority.

Private runtime directories and atomic state files use restrictive
permissions. Ownership and filesystem checks guard reuse and deletion,
including symlink substitution. A PID alone cannot authorise observation or
signalling of a restored process. Credentials are excluded from saved plans,
contexts, reconciliation references and diagnostics.

## Related

- [Run Operations and mixed requests](../howto/run-operations-and-mixed-work.md)
- [Operations routing explained](../operations-routing.md)
- [Workstream state and recovery](workstream-state.md)
- [Multitask command](multitask-command.md)
- [Coordination messages](coordination-messages.md)
