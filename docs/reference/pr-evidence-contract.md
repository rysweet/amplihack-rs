---
title: Pull Request Evidence Contract
description: Planned v1 file interfaces for agent-selected readiness evidence, waiting, validation, and merge execution.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Pull request evidence contract [PLANNED - Implementation Pending]

> **Planned v1 interface.** This reference specifies the feature to build for
> issue #2333. The CLI currently exposes only `amplihack pr watch-and-merge` under
> `pr`; none of the evidence, execute or inspect commands below is implemented.
> Requirements and examples below are normative design examples, not verified
> runtime behavior. Shell or JSON syntax checks do not establish these guarantees.

Auto-drive will use Rust evidence tools to collect observations and enforce evidence
integrity. The readiness agent chooses sources and assesses requirements. The
loop-health agent decides whether to continue, advance, or stop as `STUCK`.
The same contract serves the Crusty and merge-ready loops.

## Contents

- [Command interface](#command-interface)
- [V1 JSON fields and statuses](#v1-json-fields-and-statuses)
- [Request and result examples](#request-and-result-examples)
- [Identity and artifact references](#identity-and-artifact-references)
- [Trusted resume and cross-run replay](#trusted-resume-and-cross-run-replay)
- [Collection requests and receipts](#collection-requests-and-receipts)
- [Assessments and accepted bundles](#assessments-and-accepted-bundles)
- [CI support requirements](#ci-support-requirements)
- [Reviews and repository protections](#reviews-and-repository-protections)
- [Evaluator records and waits](#evaluator-records-and-waits)
- [Readiness-event registration](#readiness-event-registration)
- [Final revalidation and merge execution](#final-revalidation-and-merge-execution)
- [Errors and trust limits](#errors-and-trust-limits)

## Command interface

Each operation reads a private JSON request file and atomically writes a JSON
result file. Large bodies and histories travel as registered artifact references,
with bounded summaries for agents.

| Command | Request content | Result content |
| --- | --- | --- |
| `amplihack pr evidence collect --request PATH --result PATH` | Explicit typed operations, API selection, pagination scope, target revision | Registered receipts, extracted facts, completeness and collection errors |
| `amplihack pr evidence validate --request PATH --result PATH` | Assessment proposal, receipt references, predecessor and supersession references | Accepted bundle or discrepancies; validation can accept a blocked assessment |
| `amplihack pr evidence wait --request PATH --result PATH` | Agent-selected deadline or registered readiness-event reference | Wait record with elapsed time and wake reason |
| `amplihack pr merge execute --request PATH --result PATH` | Accepted readiness bundle, evaluator record, revalidation plan | Reassessment request, merge refusal, confirmed result, or ambiguous attempt |
| `amplihack pr merge inspect --request PATH --result PATH` | Persisted attempt reference | REST-confirmed merge state and whether the attempt remains ambiguous |

`PATH` denotes a file argument in this synopsis. Requests cannot choose an
executable, credentials, host override, arbitrary URL, arbitrary GraphQL document,
or merge flags. Trusted launch tooling supplies authorization independently.

A successfully written, valid result exits zero even when collection is partial,
validation rejects a proposal, or no merge occurs. Callers inspect the structured
status and discrepancies. Invalid input, failure to persist a result, cancellation,
and execution-control failures use nonzero exits. A zero exit alone establishes
neither readiness nor merge success. The existing `pr watch-and-merge` command
retains its separate polling behavior.

## V1 JSON fields and statuses

**[PLANNED]** These field names define the proposed v1 wire contract. All fields
listed as required must be present, including explicit `null` values where allowed.
Unknown fields, unknown enum values, duplicate keys and unsupported schema versions
are rejected. IDs are opaque strings; PR numbers and repository IDs are positive
integers. Timestamps use UTC RFC 3339; Git IDs use full hexadecimal object IDs;
artifact digests use 64 lowercase hexadecimal SHA-256 characters.

### Common envelope

| Required request field | Type and meaning |
| --- | --- |
| `schema_version` | Integer `1` |
| `request_id` | Unique request ID within the run; reuse cannot replay a mutation |
| `operation` | `collect`, `validate`, `wait`, `execute` or `inspect`, matching the invoked command |
| `run_id`, `round_id` | Launch-registered run and round IDs |
| `target` | Object with `host`, `repository_id`, `owner`, `repo`, `pr_number`, `head_repository_id`; all required and checked against launch authorization |
| `revision` | Object with `head_sha`, `base_ref`, `base_sha`, `worktree_sha`; all required, with `worktree_sha: null` only when no local measurement is claimed |
| `predecessor_ref` | Registered prior record reference; `null` only for a launch-authorized first record |
| `resume_ref` | Registered trusted resume binding or `null` for an uninterrupted run |
| `payload` | Operation-specific object below |

A reference is exactly `{"artifact_id":"receipt-17","sha256":"…"}`: an opaque
registry ID plus its exact-byte digest, never a caller-selected path. The ellipsis
here is type notation only; actual references require a full digest. Every result
echoes the request envelope identity fields, including `operation`, but replaces
`payload` and adds required `status`, `record_ref` and `errors` fields. `record_ref`
identifies the separately persisted immutable operation record; it does not hash
the result wrapper that contains the reference. `errors` is an array, empty on
unqualified success. Error entries have required `code`, `scope`, `message`,
`operation_id` and `artifact_ref`; the last two may be `null`. `message` is bounded
and sanitized; consumers branch on `code`, never its text.

### Operation payloads and result statuses

| Operation | Required request `payload` fields | Required result `payload` fields | `status` enum |
| --- | --- | --- | --- |
| `collect` | `operations`: nonempty array of typed collection operations | `receipt_refs`, `completeness` | `COMPLETE`, `PARTIAL`, `UNAVAILABLE` |
| `validate` | `assessment_ref`, `receipt_refs`, `supersessions` | `bundle_ref` (nullable), `merge_ready_verdict` (nullable), `discrepancies` | `ACCEPTED`, `REJECTED` |
| `wait` | `wake`, `reason`, `recollect_operation_ids` | `elapsed_ms`, `wake_reason`, `event_ref` (nullable) | `WOKEN`, `INTERRUPTED` |
| `execute` | `bundle_ref`, `evaluator_ref`, `revalidation_plan` | `attempt_ref` (nullable before mutation), `mutation_sent`, `platform_state`, `merge_commit_sha` (nullable), `discrepancies` | `REASSESSMENT_REQUIRED`, `REFUSED`, `NO_MERGE`, `DRY_RUN`, `CONFIRMED_MERGED`, `CONFIRMED_NOT_MERGED`, `AMBIGUOUS` |
| `inspect` | `attempt_ref` | `attempt_ref`, `mutation_sent`, `platform_state`, `merge_commit_sha` (nullable), `discrepancies` | `CONFIRMED_MERGED`, `CONFIRMED_NOT_MERGED`, `AMBIGUOUS` |

`completeness` is `COMPLETE`, `PARTIAL` or `UNAVAILABLE`. `platform_state` is
`OPEN`, `CLOSED`, `MERGED` or `UNKNOWN`. `mutation_sent` is `YES`, `NO` or
`UNKNOWN`, accounting for crashes between persisting intent and observing a send.
`wake_reason` is `DEADLINE`, `EVENT`, `CANCELLED` or `EXECUTION_CONTROL`.
`ACCEPTED` means factual support was validated; its verdict may still be
`NOT_MERGE_READY`. `REJECTED` returns `bundle_ref: null` and
`merge_ready_verdict: null`. Each discrepancy uses the error-entry shape above.

`NO_MERGE`, `DRY_RUN`, `REFUSED` and `REASSESSMENT_REQUIRED` require
`mutation_sent: "NO"`. After mutation may have been sent, uncertainty requires
`AMBIGUOUS`, never `REFUSED`. `CONFIRMED_NOT_MERGED` requires a definitive
rejection/no-mutation outcome and authoritative platform confirmation with no
unresolved in-flight attempt. A snapshot of an open PR alone is insufficient.
`CONFIRMED_MERGED` records an authoritative platform merge and its actual commit;
it does not claim the executor caused an already-completed merge.

### Typed records

The common schema and identity bindings also apply to registered records.

| Record | Required fields in its payload |
| --- | --- |
| Collection operation | `operation_id`, `api` (`REST` or `GRAPHQL`), `resource`, `parameters`, `pagination` |
| Receipt | `operation_id`, `api`, `resource`, `parameters`, `collected_at`, `response_ref` (nullable on transport failure), `http_status` (nullable), `response_headers`, `process_exit_code` (nullable), `pagination`, `completeness`, `facts`, `errors` |
| Assessment proposal | `merge_ready_verdict` (`MERGE_READY` or `NOT_MERGE_READY`), `criteria`, `next_action` |
| Criterion | `criterion_id` from the configured criteria set, `disposition` (`SATISFIED`, `BLOCKED`, `UNKNOWN` or `NOT_APPLICABLE`), `explanation`, `fact_refs`, `applicability_fact_refs` |
| Accepted bundle | `assessment_ref`, `receipt_refs`, `supersessions`, `merge_ready_verdict`, `criteria` |
| Evaluator | `loop_verdict` (`CONTINUE`, `DONE` or `STUCK`), `assessment_ref`, `explanation`, `next_action`, `recommended_intervention` (nullable except for `STUCK`) |
| Revalidation plan | `operations`, `coverage`, `reuse_refs`; each coverage entry has `criterion_id` and `operation_ids` |
| Merge attempt | `attempt_id`, `bundle_ref`, `evaluator_ref`, `revalidation_ref`, `expected_head_sha`, `created_at`, `mutation_sent`, `status`, `confirmation_ref` (nullable while ambiguous) |

REST `resource` names are allowlisted typed operations: `PR_METADATA`,
`CHECK_RUNS`, `COMMIT_STATUSES`, `CHECK_SUITES`, `SUITE_CHECK_RUNS`,
`BRANCH_PROTECTION`, `ACTIVE_RULES`, `REVIEWS` and `REVIEW_COMMENTS`.
Parameters supply typed identifiers such as `sha`, `suite_id` or `base_ref`,
never URLs. GraphQL resources identify trusted pinned read-only documents,
including authoritative review-thread collection; agents cannot supply query text.
The same operation shape is used in collection and revalidation plans.

| REST resource | Required `parameters` keys |
| --- | --- |
| `PR_METADATA`, `ACTIVE_RULES`, `REVIEWS`, `REVIEW_COMMENTS` | Empty object `{}`; PR, repository and active-rule target come from the envelope |
| `CHECK_RUNS`, `COMMIT_STATUSES`, `CHECK_SUITES` | `sha`, equal to the selected current-head subject |
| `SUITE_CHECK_RUNS` | `sha`, `suite_id` (positive integer) |
| `BRANCH_PROTECTION` | `base_ref`, matching the envelope |

Pinned GraphQL documents declare their own typed variable schema in the trusted
allowlist; the request's `parameters` must match it. Selecting a document does
not grant access to arbitrary GraphQL operations.

Requested pagination has `scope` (`SINGLE` or `ALL`), `per_page` and `max_pages`.
Receipts additionally record `pages_received`, `next_page` (nullable),
`reported_total` (nullable) and `source_limit` (nullable). Hitting `max_pages`
with pages remaining yields incomplete evidence, not success. Facts have
`fact_id`, `kind`, `value` and `response_pointer`, a JSON Pointer into the
registered response bytes. A fact reference has `receipt_ref` and `fact_id`.
Supersession entries have `prior_receipt_ref`, `replacement_receipt_ref` and
`fact_ids`; their scope cannot silently erase a failed check.

`next_action` is an object with `kind` (`COLLECT`, `WAIT`, `REMEDIATE`,
`ADVANCE` or `STOP`), `reason` and `request_ref` (nullable for actions without
a tool request). Waiting `CONTINUE` names a registered wait request; active
`CONTINUE` names its selected collection or remediation. `DONE` uses `ADVANCE`;
`STUCK` uses `STOP`. The evaluator's `assessment_ref` binds to the accepted
bundle, not an unsupported proposal. Attempt `status` uses the execution status
enum, plus `PREPARED` for persisted intent before sending mutation.

## Request and result examples

**[PLANNED]** These JSON design fixtures illustrate the intended issue-2333
interface. They are not live observations of PR #2333. Numeric repository IDs,
revisions, registered IDs and digests below are fixture bindings; copying them
does not create a trusted request or authorize a merge. A real run must use the
identities and references produced by its trusted launcher and registry.

### Complete collection envelope

Request for a single REST PR observation:

```json
{
  "schema_version": 1,
  "request_id": "collect-pr-round-4",
  "operation": "collect",
  "run_id": "issue-2333-run-1",
  "round_id": "round-4",
  "target": {
    "host": "github.com",
    "repository_id": 2333001,
    "owner": "rysweet",
    "repo": "amplihack-rs",
    "pr_number": 2333,
    "head_repository_id": 2333001
  },
  "revision": {
    "head_sha": "1111111111111111111111111111111111111111",
    "base_ref": "main",
    "base_sha": "2222222222222222222222222222222222222222",
    "worktree_sha": null
  },
  "predecessor_ref": {
    "artifact_id": "round-3-evaluator",
    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  },
  "resume_ref": null,
  "payload": {
    "operations": [{
      "operation_id": "read-pr",
      "api": "REST",
      "resource": "PR_METADATA",
      "parameters": {},
      "pagination": {"scope": "SINGLE", "per_page": 1, "max_pages": 1}
    }]
  }
}
```

Expected result shape (the receipt proves only PR metadata, not merge readiness):

```json
{
  "schema_version": 1,
  "request_id": "collect-pr-round-4",
  "operation": "collect",
  "run_id": "issue-2333-run-1",
  "round_id": "round-4",
  "target": {
    "host": "github.com",
    "repository_id": 2333001,
    "owner": "rysweet",
    "repo": "amplihack-rs",
    "pr_number": 2333,
    "head_repository_id": 2333001
  },
  "revision": {
    "head_sha": "1111111111111111111111111111111111111111",
    "base_ref": "main",
    "base_sha": "2222222222222222222222222222222222222222",
    "worktree_sha": null
  },
  "predecessor_ref": {
    "artifact_id": "round-3-evaluator",
    "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  },
  "resume_ref": null,
  "status": "COMPLETE",
  "record_ref": {
    "artifact_id": "collection-round-4",
    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  },
  "payload": {
    "receipt_refs": [{
      "artifact_id": "receipt-pr-round-4",
      "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
    }],
    "completeness": "COMPLETE"
  },
  "errors": []
}
```

### Validation, wait, execution and inspection payloads

The remaining examples show **payloads**, not standalone request files. For each
request, retain the common envelope above, set a unique `request_id`, set the
matching `operation`, advance `predecessor_ref` to the registered prior record,
and replace `payload`. Results echo those identity fields and include the shown
`status`, `payload`, a registered `record_ref` and `errors`.

Validation request payload for a supported but blocked assessment:

```json
{
  "assessment_ref": {
    "artifact_id": "assessment-round-4",
    "sha256": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
  },
  "receipt_refs": [{
    "artifact_id": "receipt-pr-round-4",
    "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
  }],
  "supersessions": []
}
```

Expected validation result fields:

```json
{
  "status": "ACCEPTED",
  "payload": {
    "bundle_ref": {
      "artifact_id": "blocked-bundle-round-4",
      "sha256": "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
    },
    "merge_ready_verdict": "NOT_MERGE_READY",
    "discrepancies": []
  },
  "errors": []
}
```

The registered assessment must enumerate every configured criterion, even where
this metadata-only collection leaves its disposition `UNKNOWN`. Such an accepted
bundle cannot authorize merge execution.

Wait request payload, chosen by the agent rather than a transport backoff:

```json
{
  "wake": {"kind": "DEADLINE", "deadline": "2026-11-11T08:00:00Z"},
  "reason": "Current-head CI is pending; recollect both CI families after wake.",
  "recollect_operation_ids": ["read-check-runs", "read-commit-statuses"]
}
```

Expected wait result fields for a wake at that deadline:

```json
{
  "status": "WOKEN",
  "payload": {"elapsed_ms": 30000, "wake_reason": "DEADLINE", "event_ref": null},
  "errors": []
}
```

The recollection IDs must name operations in the registered agent-selected
collection plan. The elapsed time above is a fixture outcome, not a default wait.
An event-based `wake` instead has exactly `kind: "EVENT"` and `event_ref`;
a deadline-based `wake` has exactly `kind: "DEADLINE"` and `deadline`.

Execution request payload (the ready bundle below is a later fixture, distinct
from the blocked bundle above):

```json
{
  "bundle_ref": {
    "artifact_id": "ready-bundle-round-9",
    "sha256": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
  },
  "evaluator_ref": {
    "artifact_id": "done-evaluator-round-9",
    "sha256": "9999999999999999999999999999999999999999999999999999999999999999"
  },
  "revalidation_plan": {"operations": [], "coverage": [], "reuse_refs": []}
}
```

This deliberately incomplete plan demonstrates a refusal to mutate. Its common
envelope must use `round_id: "round-9"` and that round's actual bindings. Even a
ready bundle plus `DONE` cannot skip mutable-criterion revalidation.

```json
{
  "status": "REASSESSMENT_REQUIRED",
  "payload": {
    "attempt_ref": null,
    "mutation_sent": "NO",
    "platform_state": "UNKNOWN",
    "merge_commit_sha": null,
    "discrepancies": [{
      "code": "REVALIDATION_INCOMPLETE",
      "scope": "revalidation",
      "message": "The selected plan does not cover mutable criteria.",
      "operation_id": null,
      "artifact_ref": null
    }]
  },
  "errors": []
}
```

Inspection request payload after a different, persisted ambiguous attempt:

```json
{
  "attempt_ref": {
    "artifact_id": "merge-attempt-round-10",
    "sha256": "8888888888888888888888888888888888888888888888888888888888888888"
  }
}
```

Inspection uses the attempt's authorized target and expected revision, or a
trusted resume binding to them. If REST still cannot establish the outcome, the
expected result fields remain ambiguous and authorize no mutation:

```json
{
  "status": "AMBIGUOUS",
  "payload": {
    "attempt_ref": {
      "artifact_id": "merge-attempt-round-10",
      "sha256": "8888888888888888888888888888888888888888888888888888888888888888"
    },
    "mutation_sent": "UNKNOWN",
    "platform_state": "UNKNOWN",
    "merge_commit_sha": null,
    "discrepancies": []
  },
  "errors": [{
    "code": "MERGE_OUTCOME_AMBIGUOUS",
    "scope": "merge",
    "message": "Platform confirmation is unavailable; inspect before any new attempt.",
    "operation_id": null,
    "artifact_ref": {
      "artifact_id": "merge-attempt-round-10",
      "sha256": "8888888888888888888888888888888888888888888888888888888888888888"
    }
  }]
}
```

Command synopses above describe how the future CLI will consume these files.
These fixtures are not runnable against the current CLI and do not constitute
behavioral tests of collection, validation, waiting or merging.

## Identity and artifact references

Every request, receipt, assessment, bundle, evaluator record, wait and merge result
uses `schema_version: 1`. Unsupported versions and duplicate JSON keys are rejected.

| Binding | Meaning |
| --- | --- |
| Run and round identity | The launch-authorized run and the round producing the record |
| Target | Pinned host, repository ID, PR number, and head repository identity, including fork identity |
| Revision | Current PR head SHA and applicable base ref/SHA; local QA also records its actual worktree revision |
| Predecessor | Registered prior record or bundle, preventing unrelated history from being substituted |
| Artifact reference | Registered artifact ID and SHA-256 digest of its exact bytes |

Git object identifiers and artifact SHA-256 digests are distinct types. A Git SHA
does not authenticate an evidence file. References must resolve through the launch
registry; a matching path and hash alone do not establish tool provenance.

The launch side registers tool-produced receipts separately from agent proposals
and finalizes immutable bundles using the existing private manifests. Collection
errors remain in that history when a later receipt supplies usable evidence.
No database, HTTP service, or separate workflow engine is involved.

## Trusted resume and cross-run replay

**[PLANNED]** A process restart may reopen the original run ID, registry and
predecessor chain through trusted launch tooling. A new run ID may import history
only when that tooling registers a resume record with `source_run_id`,
`destination_run_id`, `source_tip_ref`, `target`, `revision`, `imported_refs` and
`authorization_ref`. The new run supplies that record as `resume_ref`; agents
cannot create or approve it by writing a JSON file.

The launcher verifies source registry ownership, immutable artifact hashes,
target/fork identity, predecessor continuity and destination authorization before
registering the binding. Imported receipts keep their original run/round IDs;
they are not relabeled as fresh. An unchanged head permits historical reuse only
for supported immutable/local evidence. Mutable CI, reviews, policy, base and PR
state must be recollected, reassessed and revalidated under current authorization.
A resume binding never widens merge permission or replays an attempted mutation.

Matching PR/task strings, copying a state directory, editing `run_id` or matching
hashes alone is rejected cross-run replay (`CROSS_RUN_REPLAY`). Broken history
returns `PREDECESSOR_MISMATCH`; unauthorized or mismatched imports return
`RESUME_BINDING_INVALID`. Without a valid resume binding, a fresh run recollects
evidence. Preserve unresolved attempts and inspect them before any new mutation,
even when resuming with a new run ID.

## Collection requests and receipts

The agent explicitly selects REST operations or pinned, read-only GraphQL
documents and the pagination needed for each collection. The transport performs
that request once. It does not switch APIs, retry, poll CI, or sleep on its own.
Subsequent collection or waiting requires an agent decision.

| Receipt data | Required record |
| --- | --- |
| Provenance | Operation/API, target, revision, collection time, run/round and registration identity |
| Response | Original response bytes and their hash, HTTP status and allowlisted response headers |
| Process | Subprocess exit or execution failure, with sanitized diagnostic summary |
| Pagination | Requested scope, pages received, next-page information, totals where supplied, and completeness |
| Facts | Typed response-derived observations that the validator checks against the registered bytes |
| Limitations | Partial GraphQL data/errors, malformed bodies, endpoint limits, truncation, cancellation or storage limits |

A GraphQL refusal is unavailable evidence. It is neither failed CI nor successful
CI. The agent may request REST evidence or choose a wait. REST collection includes
both current-head check runs and commit statuses:

| Observation | REST resource |
| --- | --- |
| PR identity, head, base and platform mergeability | `GET /repos/{owner}/{repo}/pulls/{number}` |
| Check runs | `GET /repos/{owner}/{repo}/commits/{sha}/check-runs` |
| Commit statuses | `GET /repos/{owner}/{repo}/commits/{sha}/status`, with complete context accounting |
| Suite-specific checks where needed | Explicitly selected check-suite and suite check-run resources |
| Rules, reviews and inline review activity | Allowlisted branch protection, active rules, reviews and review-comment resources |

These are resource templates, not commands. The transport constructs encoded
endpoints from typed identifiers. Pagination stays on the authenticated HTTPS host
and within the requested scope, including legitimate repository-ID paths.
Cross-host links, credential-bearing links and scope-expanding links are refused.

The requested revision is an expected binding, not a claim about the response.
When PR metadata reports a different head or base, retain the original bytes,
record the observed revision and return `REVISION_MISMATCH` for reassessment.
Never relabel moved-head evidence as support for the requested revision.

## Assessments and accepted bundles

An assessment lists every configured merge-ready criterion exactly once. Each
entry records its disposition, explanation, cited receipt facts and applicability.
The agent selects collection, waiting, remediation or advancement as its next
action. A `MERGE_READY` proposal requires supported satisfaction of every
applicable criterion; an unknown requirement cannot be treated as not applicable.

The support validator checks factual support and integrity. It returns
discrepancies for omitted or duplicated criteria, stale revisions, incomplete
evidence, unsupported applicability or unsupported success. It does not select
an API, request a rerun, set a retry delay or decide convergence. A supported
`NOT_MERGE_READY` assessment is valid and remains available to the evaluator.

Supersession is explicit and scoped: the proposal names the earlier receipt,
the replacement and the facts being replaced. Complete current-head REST evidence
can supersede an unreadable GraphQL measurement. The original refusal remains
visible, and an initial `UNREADABLE` never permanently vetoes a supported later
assessment. An actual failed check cannot disappear through supersession; its
applicable resolution, rerun or replacement must be supported by evidence.

Local evidence retains the existing QA hash chain, private manifests, actual
worktree-head binding and Crusty reviewed-range checks. A description-only fix
can satisfy a finding without a code commit, but still needs a confirming
assessment. A permitted `MINOR_ONLY` Crusty result retains its per-finding
follow-up and parent-task obligations.

## CI support requirements

Collection success alone does not establish CI success. Accepted CI evidence:

- accounts for both check runs and commit statuses on the current PR head,
  including pagination, malformed pages and partial responses;
- preserves check-run, suite, app, status-context, commit and rerun identities;
- identifies applicable required checks, contexts and apps from current policy,
  including required commit subjects where applicable;
- distinguishes completed successful results from pending, failed, cancelled,
  stale, absent or unknown results according to the applicable requirement;
- explains resolution of applicable failures without hiding them behind a newer
  unrelated success.

Both CI families empty, an absent required context, the wrong app identity or an
unreadable requirement cannot prove green CI. One empty family may be legitimate
only when complete enumeration and applicable requirements support that conclusion.
Skipped or neutral conclusions satisfy a check only where the applicable policy
supports them. Pending checks require further evidence or waiting.

The commit check-run listing has a source limit on suites. A receipt records that
limitation; when it affects completeness, the agent explicitly requests suite-level
collection. Following all returned pages alone does not erase an endpoint limit.

## Reviews and repository protections

The assessment accounts for classic branch protection and applicable active rules,
required approvals, code-owner and last-push requirements, requested changes,
conversation resolution and merge-queue requirements. Unknown policy or inaccessible
requirements prevent an unsupported readiness claim. A protection endpoint's `404`
does not by itself establish that no protection exists.

REST `mergeable_state=clean` alone proves neither readiness nor approval. An empty
GraphQL review decision also proves neither that reviews are required nor that
they are satisfied. Supported absence of applicable review requirements is valid;
an unreadable approval measurement is not evidence of a missing review.

Review comments do not expose authoritative thread-resolution state. A REST-only
assessment can establish absence of inline review activity through complete
review/comment enumeration consistent with PR metadata. Existing inline activity
requires authoritative resolution evidence; comments or replies alone cannot
establish it. Outdated threads still require resolution where policy requires it.

No assessment or merge plan bypasses server protections. A required merge queue
prevents a direct squash merge through this executor.

## Evaluator records and waits

An evaluator record binds its exact `CONTINUE`, `DONE` or `STUCK` decision to the
current accepted assessment and its predecessor history. The evaluator receives
current and prior receipts, refusals, CI state, head SHA, actions, discrepancies,
and waiting records. Missing, malformed or stale evaluator output is an evaluation
failure, never an invented `STUCK` judgment.

`CONTINUE` distinguishes active work from external waiting and identifies the
selected action. A wait carries an agent-selected time or an existing registered
readiness event, the reason, and the evidence to recollect after wake. The tool
validates and executes that interruptible wait without a default backoff or
repetition threshold. Its record contains elapsed time and wake reason, including
deadline, event, cancellation or execution-control interruption.

Waking does not prove readiness. Fresh collection and assessment follow every
wait. Repeated pending CI or rate-limit recovery at the same head remains waiting
when the evaluator judges it so. See the [loop-health contract](loop-health-evaluator.md).

## Readiness-event registration

**[PLANNED]** An event is a reference to a trusted source already available to
the launch environment, not a path, shell command or webhook supplied by an
agent. A launch-side adapter registers an immutable event descriptor containing
`schema_version`, `run_id`, `round_id`, `target`, `revision`, `predecessor_ref`,
`source_id`, `subscription_id`, `cursor` and `expires_at`. The registry returns
its `event_ref` (artifact ID and digest) in the bounded evidence context supplied
to the agent. `source_id` must identify an allowlisted existing event source;
`subscription_id` and `cursor` bind a subscription and prevent replayed wakes.

The wait tool resolves `event_ref`, validates registration, target/revision,
run or trusted-resume ownership, expiry and cursor, then waits through that
adapter. Agents can select a delivered event but cannot register one by inventing
an ID, copying a file or naming an executable. A valid notification produces a
registered wait record with `wake_reason: "EVENT"`, its `event_ref` and elapsed
time. Notification bytes remain observations; recollection is still required.

No new event service or public registration command is required. If the launcher
has no supported event adapter, it offers no event references. An unregistered,
expired or unavailable event returns `EVENT_NOT_REGISTERED`, `EVENT_EXPIRED` or
`EVENT_SOURCE_UNAVAILABLE`; the tool does not silently switch to a timed wait.
The agent may explicitly select a deadline instead. Cancellation or execution
controls interrupt an active wait and record the corresponding wake reason.

## Final revalidation and merge execution

Execution requires trusted launch authorization, accepted `MERGE_READY`, matching
evaluator `DONE` and an agent-selected revalidation plan covering every mutable
criterion. The plan names its operations, covered criteria and supported local
artifact reuse. Missing coverage returns discrepancies for reassessment.

The executor runs the selected plan once. It compares relevant facts and coverage,
not incidental timestamps or request IDs. Head/base movement, changed CI, reviews,
rules or other unsupported conditions return to the agent. Neither assessment nor
final validation requires a successful GraphQL reread when accepted evidence
establishes the conditions through another supported source.

| Execution boundary | Required behavior |
| --- | --- |
| Authorization | Bound to host, repository, PR, run and permitted operations; requests cannot widen it |
| `no_merge=true` | Leave the PR open; invoke neither merge nor automatic-merge enablement |
| Dry run | Report validation results without a merge action |
| Attempt | Serialize access and persist attempt state before mutation |
| Merge | At most one `PUT /repos/{owner}/{repo}/pulls/{number}/merge`, with squash strategy and accepted expected head SHA |
| Confirmation | Read platform state through REST and record the actual merge outcome |
| Ambiguous outcome or restart | Inspect persisted attempt and platform state before any further attempt; never automatically replay |
| Cleanup | Delete a branch only after confirmed merge and existing ownership checks |

Attempt updates append immutable registered records under a stable `attempt_id`,
linked through predecessors. Inspection resolves the latest registered state
under the same serialization boundary even when given an earlier `attempt_ref`.
An older `PREPARED` reference cannot reset attempt state or authorize replay.
If a crash leaves sending uncertain, inspection retains `AMBIGUOUS` until it has
authoritative support for a terminal outcome.

Expected-head protection does not atomically freeze reviews, CI, rules or the base.
Fresh revalidation and GitHub's protections both remain necessary. Only the trusted
Rust executor is authorized through the merge guard; a binary name, request flag
or environment variable cannot grant an ordinary agent merge permission.

## Errors and trust limits

Results carry stable error codes with scope, provenance and bounded diagnostic
details. Collection errors, validation discrepancies, evaluation failures,
execution-control stops and ambiguous merge attempts remain distinct. None is
converted into green readiness or an agent `STUCK` judgment.

### Planned stable error codes

These are v1 machine-readable values, not current CLI diagnostics. `scope` is
one of `request`, `collection`, `validation`, `evaluation`, `wait`,
`authorization`, `revalidation`, `merge`, `artifact` or `execution`.

| Code | Meaning and caller handling |
| --- | --- |
| `INVALID_REQUEST`, `UNSUPPORTED_SCHEMA`, `DUPLICATE_JSON_KEY` | Nonzero input failure; repair the request, never infer a verdict |
| `ARTIFACT_UNSAFE`, `ARTIFACT_UNREGISTERED`, `ARTIFACT_HASH_MISMATCH` | Reject unsafe, untrusted or edited evidence |
| `CROSS_RUN_REPLAY`, `RESUME_BINDING_INVALID`, `PREDECESSOR_MISMATCH` | Reject unauthorized history or broken continuity |
| `TARGET_MISMATCH`, `REVISION_MISMATCH` | Request or record does not match its authorized target/revision |
| `API_REFUSED`, `RATE_LIMITED`, `TRANSPORT_FAILED`, `RESPONSE_MALFORMED` | Evidence unavailable; agent chooses further collection or waiting |
| `PAGINATION_INCOMPLETE`, `SOURCE_LIMIT_REACHED`, `GRAPHQL_PARTIAL` | Incomplete evidence; cannot support affected success claims |
| `CRITERIA_INCOMPLETE`, `EVIDENCE_UNSUPPORTED`, `APPLICABILITY_UNSUPPORTED` | Assessment rejected with discrepancies |
| `EVALUATION_FAILED`, `EVALUATOR_INVALID`, `DONE_WITH_NONCLEAN_ROUND` | Invocation failure, invalid decision or inconsistent `DONE`; never synthetic `STUCK` |
| `EVENT_NOT_REGISTERED`, `EVENT_EXPIRED`, `EVENT_SOURCE_UNAVAILABLE` | Requested event cannot be used; no tool-selected fallback wait |
| `AUTHORIZATION_REFUSED` | Requested execution exceeds trusted launch authorization; no mutation |
| `REVALIDATION_INCOMPLETE`, `REVALIDATION_CHANGED` | Return to agent assessment before mutation |
| `MERGE_PROTECTION_REFUSED` | Definitive server refusal; record outcome and reassess without bypassing protections |
| `ATTEMPT_IN_PROGRESS`, `MERGE_OUTCOME_AMBIGUOUS` | Inspect the existing attempt; no automatic replay |
| `RESULT_PERSIST_FAILED` | Nonzero storage failure; if mutation may have occurred, inspect durable attempt and platform state |
| `BUDGET_EXHAUSTED`, `USAGE_LIMIT`, `CANCELLED`, `ORPHANED`, `POLICY_REFUSED`, `EXECUTION_LIMIT_REACHED` | Distinct execution-control stops; no agent convergence judgment |

Domain outcomes with a safely written result use the operation's `status` and
exit zero; e.g. a collection `PARTIAL` can carry `RATE_LIMITED`, and validation
`REJECTED` can carry `EVIDENCE_UNSUPPORTED` in `discrepancies`. Invalid input,
unsafe request access, inability to persist output, cancellation and execution
controls use nonzero exits. When a normal result cannot be produced, the tool
emits a bounded structured diagnostic with `schema_version` and an `error`
entry using the same error shape. No default operation status is synthesized.
A nonzero exit after mutation may have started requires attempt inspection,
not blind resubmission. Terminal policy refusal retains the existing exit-79
contract. Human-readable messages may change without changing these codes.

Artifacts use directories `0700` and files `0600`, atomic writes and immutable
finalized receipts. Readers open by descriptor, hash and parse the same bytes,
and reject unsafe ownership/permissions, traversal, symlinks, unexpected hard
links and special files. Cross-run replay and broken predecessor bindings fail
validation. Response size, JSON depth, subprocess time, pagination and storage
are bounded by execution controls; reaching a limit records incomplete evidence.

Credentials, authorization headers, cookies and secret-bearing environment values
are excluded. Diagnostics sanitize stderr and display control characters. Public
reports use bounded summaries instead of private response bodies.

Registration, ownership and hashes protect against accidental edits, replay and
unregistered proposals. They do not isolate another deliberately malicious process
with the same user privileges. Branch code runs as that user. Stronger privilege
isolation is outside this contract.

Related: [Auto-drive reference](auto-drive-to-merge.md),
[auto-drive light](auto-drive-light.md), and
[operating a waiting run](../howto/handle-auto-drive-external-waits.md).
