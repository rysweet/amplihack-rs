---
title: Workstream state, ownership and migration
description: Execution identities, tracking attempts, completion and legacy state adaptation.
last_updated: 2026-11-12
review_schedule: quarterly
owner: amplihack
doc_type: reference
---

# Workstream state, ownership and migration

> **[IMPLEMENTED - Validation Pending]** This is the intended issue #2553
> contract. The JSON records and reconciliation command below must be implemented
> and behaviourally validated before use; the examples are contract fixtures.

This reference describes the persistence and execution boundaries used by the
[normalised leaf plan](dev-orchestrator-routing.md#normalised-configuration).
The types belong to the CLI's internal multitask implementation; they do not
introduce a separate public Rust library API.

## Contents

- [Identity and persisted records](#identity-and-persisted-records)
- [Tracking attempts](#tracking-attempts)
- [Reconciliation and safe resume](#reconciliation-and-safe-resume)
- [Dependency dispatch and results](#dependency-dispatch-and-results)
- [Completion and prerequisite JSON](#completion-and-prerequisite-json)
- [Lifecycle values](#lifecycle-values)
- [Child context and environment](#child-context-and-environment)
- [Legacy configuration and saved state](#legacy-configuration-and-saved-state)
- [Ownership and cleanup](#ownership-and-cleanup)
- [Diagnostics](#diagnostics)

## Identity and persisted records

| Type or record | Contract |
| --- | --- |
| `WorkstreamId` | Immutable UUID, allocated once and serialised as a UUID string |
| `IssueNumber` | Positive `u64`, parsed by the shared optional-issue parser |
| `IssueRef` | Repository-scoped issue number; identity is repository plus number |
| Configuration | Execution identity, plan key, repository, classification, trusted route, task, dependencies and optional tracking |
| Runtime state | The same identity and tracking, lifecycle, validated result and owned paths |
| Tracking attempt | Workstream and repository binding, requested creation details and known or uncertain outcome |
| Process scope | Execution identity plus repository, working directory, branch where applicable, run identity and process start evidence |

Process maps, completion events, logs, progress, cancellation and cleanup are
keyed by `WorkstreamId`. Multiple issue-less leaves remain separate, as do
leaves sharing an issue number. Issue-less descriptions identify the task or
execution UUID rather than displaying `Issue #0` or `TBD`.

The repository working directory and private artefact directory are distinct.
Operations works in its trusted repository; its state, context and logs live
under an owned runtime root. Development keeps its existing isolated workspace
behaviour, with branch and directory identity independent of optional tracking.

Configuration, state and result writes are atomic. Private context and state
files have restrictive permissions. A failed durable write is surfaced; it is
not equivalent to a missing record or successful checkpoint.

## Tracking attempts

Required tracking is resolved before launching the affected task. Applicable
supplied tracking and persisted success are reused before any creation attempt.
Omitted tracking requirements default to optional for both new and legacy data.

| Recorded condition | Execution behaviour |
| --- | --- |
| Optional, no issue | Continue without creating one |
| Applicable supplied issue or persisted success | Reuse that repository-scoped reference |
| Required, explicit producer binding | Wait for the declared persisted issue output; never create a substitute |
| Required, no issue or binding, incomplete creation details | Reject before invoking `gh` |
| Required, no reusable issue or binding, validated creation details | Persist attempt before invoking `gh` once |
| Validated successful creation | Persist reference before child execution |
| Definite creation failure | Report failure without substituting tracking |
| Ambiguous creation outcome or interrupted mutation | Require reconciliation before another attempt or affected task launch |

The adapter passes `gh` arguments directly, binds the target repository
explicitly and supplies bodies through standard input or an owned file. Task
text and issue bodies are never interpolated into shell commands. Existing
authentication is reused without persisting credentials.

If a process stops after GitHub accepts the request but before local success
is durable, absence of a success record does not prove failure. Reconciliation
checks the external result and records the established outcome against the
same workstream and repository. Unknown outcomes remain visible until resolved;
there is no automatic recreation or timed retry.

### Tracking-attempt JSON

Each creation attempt has this persisted shape (planned):

```json
{
  "schema_version": 1,
  "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c",
  "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
  "repository": "rysweet/amplihack-rs",
  "revision": 3,
  "purpose": "tracking",
  "request": {
    "title": "Clarify Operations routing examples",
    "body": "Clarify administrative and repository edit examples in docs/operations-routing.md."
  },
  "status": "reconciliation_required",
  "diagnostic": "GitHub response was lost before success could be persisted"
}
```

| Field | Contract |
| --- | --- |
| `schema_version` | Integer `1`; unsupported versions fail closed |
| `attempt_id` | Fresh UUID per explicitly authorised attempt; never reused for a second invocation |
| `workstream_id`, `repository` | Must match the saved leaf and trusted repository |
| `revision` | Monotonically increasing positive integer for the leaf's durable state; shared by attempts, lifecycle and reconciliation |
| `purpose` | `tracking` for tracking resolution; `operation` for an external mutation performed by an Operations leaf |
| `request` | For tracking, the exact non-blank `title` and `body`; for an operation, a non-blank `action` and typed `arguments` object describing the authorised adapter call, without credentials |
| `status` | `prepared`, `succeeded`, `failed` or `reconciliation_required` |
| `issue` | Required `IssueRef` on successful tracking creation; absent on other tracking statuses |
| `diagnostic` | Required non-blank explanation for `failed` and `reconciliation_required` |

An operation attempt additionally records `output_name`, a declared output of
its leaf. On success it records `output` using the corresponding typed result
below. Multiple external mutations in a checklist each have their own attempt.
Successful attempts are retained and supplied to the resumed leaf so it does
not repeat those mutations.

`prepared` is durable before the adapter invocation. A validated response moves
it to `succeeded`; proven rejection without a side effect moves it to `failed`.
Transport loss, uncertain rejection or a recovered `prepared` attempt moves it
to `reconciliation_required`. Neither failure nor uncertainty retries
automatically. A failed persistence write leaves the last durable record
authoritative and stops dispatch. Supplied or producer-bound issues are saved
as resolved tracking on the leaf without inventing a creation attempt.

An issue created by an administrative leaf remains an output of that task.
Using it as another leaf's tracking requires an explicit producer/consumer
binding and repository validation. It is never inferred from a matching number
in task prose, an unrelated result or an inherited environment variable.

## Reconciliation and safe resume

The planned supported entry point is
`amplihack multitask reconcile <CONFIG> --resolution-file <PATH>`. It records a
decision without launching children or mutating GitHub. The operator first
stops the affected run and inspects saved attempts and external evidence.
Absence from a search, a timeout or a missing local result is insufficient proof
that a mutation did not happen.

A resolution file contains one decision bound to the saved identity and state:

```json
{
  "schema_version": 1,
  "resolution_id": "92fdc5a0-ac4d-4a22-9f25-c9832475f5bc",
  "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
  "repository": "rysweet/amplihack-rs",
  "attempt_id": "aebff9b8-b66b-4c03-924f-d5881c07d21c",
  "expected_revision": 3,
  "outcome": "succeeded",
  "issue": {"repository": "rysweet/amplihack-rs", "number": 2553},
  "evidence": [
    {"source": "github", "reference": "rysweet/amplihack-rs#2553", "observation": "Issue title and body match the saved creation request"}
  ],
  "reason": "Verified the accepted creation against the saved request"
}
```

`resolution_id` is a UUID for idempotent submission. `expected_revision` must
equal the current positive state revision. `evidence` is a non-empty array of
objects with non-blank `source`, `reference` and `observation`; `reason` is
non-blank. References identify inspectable external records or owned evidence
files. The resolver validates their provenance and task-specific content;
operator assertion alone cannot bypass result validation.

| `outcome` | Required payload and effect |
| --- | --- |
| `succeeded` | Tracking: repository-matching `issue`. Operation attempt: matching `output_name` and typed `output`. Whole-leaf interruption: complete `result` using the completion schema. The same validators used for normal execution must accept the evidence. |
| `not_applied` | Evidence proving no effect, plus Boolean `retry_authorized`. `true` moves the unresolved action to pending and permits one new attempt on explicit resume; `false` leaves it failed. Omit success payloads. |
| `unresolved` | Evidence and reason explaining the remaining uncertainty; omit success payloads and `retry_authorized`. Retain the reconciliation barrier. |

`attempt_id` is required for an uncertain tracking or operation attempt. For an
interrupted leaf without a specific attempt record, omit it and use the leaf
revision; `succeeded` then requires the whole result, and `not_applied` must
prove that none of the leaf's unrecorded mutations took effect. Already recorded
successful attempts remain authoritative. A known partial effect must be
reconciled per attempt or remain unresolved; it cannot be labelled `not_applied`.

New decisions are accepted only for `failed` or `reconciliation_required`
state. A proven failure can use `not_applied` to authorise a retry; resolved
success cannot be reopened through this command.

The resolver takes the same exclusive state lock as dispatch, verifies no
owned child is active, checks repository and path ownership, and rejects a
stale revision or conflicting decision. It atomically persists the resolution,
outcome and incremented revision before reporting success. The same
`resolution_id` and identical payload returns the saved receipt without another
write; reuse with different contents fails. The receipt includes
`resolution_id`, `workstream_id`, `repository`, `revision` and resulting
`lifecycle`. Persistence failure releases no dependant and authorises no retry.

After a successful resolution, explicitly run the **same** saved configuration
with `amplihack multitask run <CONFIG>` and its original mode/options. Tracking
success makes the leaf pending with its resolved issue. Operation success
preserves that action's output and makes remaining work pending only when all
uncertain attempts have been resolved. Adapters must reuse recorded successful
actions rather than invoke them again. A validated
whole-leaf result makes the leaf succeeded and skips it on resume. Retry
authorisation creates a new attempt UUID while retaining previous attempts and
resolutions. Dependants blocked solely by that leaf are reconsidered using the
same graph; unrelated failures remain blocked.

Manual JSON edits, state deletion, replacement UUIDs and a fresh decomposition
are not reconciliation procedures. If ownership is ambiguous or evidence
cannot establish the outcome, resume stays blocked.

## Dependency dispatch and results

The dispatcher validates the entire dependency graph before external actions.
A leaf is ready when all its prerequisite leaves have validated, persisted
success. Each ready leaf starts once, within the concurrency limit. A child
completion event releases capacity; dispatch uses no interval probes, polling,
timers or wall-clock readiness windows.

Each child has one completion waiter. Standard output and error are drained to
end-of-file so final output is included before result validation. Exit code
zero is necessary but does not itself establish task success.

A completion record identifies its workstream and repository, states the
outcome and includes the task's required output evidence. Evidence can include
a research answer, an issue reference or Development delivery artefacts. The
dispatcher validates its provenance and required fields before persisting it
and releasing dependants. A result cannot satisfy another workstream merely
because it uses the same issue number.

The persisted lifecycle distinguishes pending and running work, validated
completion, failure, dependency blockage and outcomes requiring reconciliation.
Existing Development checkpoint states retain their meaning. A failed
prerequisite records its dependants as blocked; they are not silently skipped
or launched without their inputs.

Sequential fallback changes only concurrency. It preserves the graph, routes,
identities and prerequisite results. On resume, durable successes are skipped.
Interrupted Operations that may have changed an external system require
reconciliation before replay. This provides at-most-one automatic launch per
leaf; it does not claim exactly-once external effects across crashes.

## Completion and prerequisite JSON

Every normalised leaf declares `required_outputs`, a non-empty map from output
names to `answer`, `issue`, `operation` or `delivery`. The planner names outputs
for Operations tasks; a read-only answer may default to `{"answer":"answer"}`.
Development uses `{"delivery":"delivery"}`. Investigation, Q&A and Consensus
adapters declare their required answer outputs. These declarations are validated
and persisted with the complete plan before dispatch. Mutating Operations
tasks must declare every requested external outcome; a generic answer cannot
substitute for mutation evidence.

For a leaf declaring `{"created_issue":"issue"}`, a planned success result is:

```json
{
  "schema_version": 1,
  "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
  "repository": "rysweet/amplihack-rs",
  "classification": "Operations",
  "effective_recipe": "operations-direct",
  "outcome": "succeeded",
  "outputs": {
    "created_issue": {
      "kind": "issue",
      "issue": {"repository": "rysweet/amplihack-rs", "number": 2553},
      "evidence": [{"source": "github", "reference": "rysweet/amplihack-rs#2553", "observation": "Created issue matches the requested title and body"}]
    }
  }
}
```

The envelope requires integer `schema_version: 1`, the producer UUID, trusted
repository, canonical classification, trusted effective recipe and an
`outcome` of `succeeded`, `failed` or `reconciliation_required`. Failures and
uncertain outcomes require a non-blank `diagnostic`; they never release
dependants. `outputs` contains only validated declared outputs; success
requires every declaration. Unknown output names or mismatched kinds fail
validation. Unknown outcomes or schema versions are rejected.

| Output `kind` | Required fields in addition to `kind` |
| --- | --- |
| `answer` | Non-blank `text`, `destination` (`chat` or `issue`) and non-empty `evidence`; an issue destination also requires a repository-scoped `issue` |
| `issue` | Repository-matching `issue` and non-empty `evidence` establishing the requested creation outcome |
| `operation` | Non-blank `action`, repository-matching target `issue` and non-empty `evidence` establishing the requested administrative change, such as assignment |
| `delivery` | Non-blank `branch`, `commit`, repository-scoped `pull_request` (`repository` and positive `number`) and non-empty `evidence` satisfying the selected Development workflow's delivery gates |

Every `evidence` item uses the `source`, `reference`, `observation` shape above.
Evidence must establish the leaf's requested outcome in its declared repository;
an issue number or narrative alone does not prove creation, assignment or
delivery. Existing Development terminal adjudication supplies delivery evidence
for a legitimate no-change result; that variant uses
`terminal_outcome: "no_change"` with non-empty `evidence` and omits branch, commit
and PR fields.
It cannot satisfy an output binding requiring an issue or PR.

`effective_recipe` records an actual hand-over, such as `default-workflow`
after `quick-change`, and must agree with the launcher's recorded route and
hand-over. The parent validates each result against that effective
workflow before persisting success, for both serial and parallel runs.
Operations receives its task/output checks; a child's exit status or missing
parent context cannot exempt a Development result from its applicable gates.

Only after result validation and durable persistence does a consumer receive
`prerequisite_results`. Its planned JSON shape is:

```json
{
  "prerequisite_results": [
    {
      "workstream_id": "8f9f57b5-9232-49df-b985-c2c92ee997c1",
      "repository": "rysweet/amplihack-rs",
      "result_revision": 4,
      "outcome": "succeeded",
      "outputs": {
        "created_issue": {
          "kind": "issue",
          "issue": {"repository": "rysweet/amplihack-rs", "number": 2553},
          "evidence": [{"source": "github", "reference": "rysweet/amplihack-rs#2553", "observation": "Created issue matches the requested title and body"}]
        }
      }
    }
  ]
}
```

`result_revision` identifies the positive durable producer revision containing
the validated result. Each entry must match a UUID in the consumer's
`depends_on`, the producer's repository and saved result. It contains the
producer's declared outputs, without unrestricted sibling context. Cross-repository
prerequisite results retain their producer repository; tracking reuse still
requires the consumer's repository to match the referenced issue. No inputs are
derived from unvalidated stdout.

## Lifecycle values

The persisted `lifecycle` field uses exactly these strings:

| Value | Meaning and transitions |
| --- | --- |
| `pending` | No active launch; waiting on dependencies or resolved tracking. Moves to `running` only after readiness checks and a durable launch record. |
| `running` | One owned child is active. Moves to `succeeded` after validation and persistence, `failed` on proven failure, or `reconciliation_required` on an uncertain effect. |
| `succeeded` | Validated result is durable; terminal and skipped on resume. |
| `failed` | Definite failure with a diagnostic; no automatic retry. An explicit reconciled `not_applied` decision with retry authorisation can make an external action pending. |
| `blocked` | A dependency failed or needs reconciliation. Stores `blocked_by` as producer UUIDs; reconsidered only after those dependencies change. |
| `reconciliation_required` | Tracking resolution or task outcome is uncertain. No affected launch or replay until a supported resolution is durable. |

A leaf may enter `failed` or `reconciliation_required` from `pending` during
tracking resolution before any child launches. Recovered `running` Operations
with possible external effects becomes `reconciliation_required`; a durable
validated result is reused. Existing Development checkpoints remain in their
own checkpoint field and retain their resume semantics. They are not extra
values of this lifecycle. Unrecognised states are errors, never aliases for
pending. Preserve diagnostics and attempt history across all transitions.

Legacy `lifecycle_state` strings remain checkpoint data during adaptation.
For example, `completed` maps to `succeeded` only with a validated durable
result; exit code zero alone is insufficient. Interrupted or timed-out resumable
checkpoints retain their preservation policy. An apparent completed Operations
mutation without verifiable result evidence requires reconciliation.

## Child context and environment

Recipe mode uses a private `context.json`, passed through `--context-file`.
Classic mode provides equivalent scoped input. The child receives its leaf
task, canonical classification, trusted repository context, execution identity
and only relevant prerequisite results with their producer and repository
provenance. It does not receive the original mixed request as an executable
task or unrestricted sibling outputs.

Present tracking supplies its validated number and repository binding. Absent
tracking omits issue-specific context keys, including `issue_number` and
`issue_url`. The launcher clears inherited issue-specific variables, including
`RECIPE_VAR_issue_number` and `RECIPE_VAR_issue_url`, before constructing the
child environment. It does not replace absence with empty values or `0`.
Required non-issue inputs such as backend and authentication configuration
remain available through the controlled environment; secrets are excluded from
saved contexts and diagnostics.

Rust supplies required session identifiers. Context propagation across
classification, execution, reflection and validation preserves the leaf plan;
dirty initial repository files do not manufacture Development results.

## Legacy configuration and saved state

Existing valid positive issue numbers and numeric strings remain accepted.
Configurations without tracking requirements default to optional. Missing
issues and recognised placeholders use the shared absence rules. Malformed
non-placeholder values still produce actionable diagnostics.

Legacy configurations can omit UUIDs, plan keys and explicit classification
when their existing homogeneous route is unambiguous. Normalisation supplies
the missing identity once and persists the adapted plan before launch. New
mixed plans require explicit classifications and dependency keys.

Saved state is adapted only after its repository, branch where applicable and
recorded directory ownership have been validated against trusted context.
Issue-derived names and UUIDs alone do not prove ownership. Adaptation records
the stable execution identity while retaining recorded worktree, working,
state, log and progress paths; it does not recompute paths from an issue number.

Ambiguous legacy matches are rejected. The operator needs to resolve ownership
using the recorded repository, branch, directory and run evidence before
resuming. Re-running the configuration helper as a new plan is not a substitute
for resuming the saved identity: it can duplicate an external action.

## Ownership and cleanup

Artefact paths stay beneath owned runtime roots. Traversal and symlink escapes
are rejected. Repository locations come from trusted resolution, not arbitrary
model-supplied directories.

Cancellation checks process ownership, including start evidence that guards
against PID reuse. Cleanup retains repository, directory, branch where
applicable, run and process checks. An issue number or UUID alone never
authorises signalling a process or removing a directory.

Operations cleanup may remove only its owned artefacts and owned processes;
it never removes its repository working directory. Issue-less Development
cleanup still requires workspace and branch ownership evidence and the
existing delivery safeguards. Unknown external outcomes remain available for
reconciliation rather than being discarded as failed temporary work.

## Diagnostics

| Condition | Actionable information |
| --- | --- |
| Invalid issue value | Leaf, field and accepted input rules |
| Invalid plan | Leaf/key or dependency edge, with duplicate, missing reference or cycle details |
| Unauthorised repository or path | The rejected binding and expected trusted scope |
| Tracking creation failure | Target repository, attempt and definite or uncertain outcome |
| Invalid completion result | Producer identity, repository and missing or conflicting evidence |
| Blocked dependant | Failed prerequisite identity and reason |
| Ambiguous legacy ownership | Conflicting records and ownership evidence required to resolve them |
| Persistence failure | Affected record and failed operation; execution does not claim durable success |

For behavioural checks, see [Verify routing behaviour](../howto/verify-dev-orchestrator-routing.md).
