---
title: Nonfatal documentation review checkpoint
description: Structured review status, durable references and nonfatal checkpoint output.
type: reference
updated: 2026-10-08
---

# Nonfatal documentation review checkpoint

`workflow-design.yaml` records documentation-review follow-up without discarding
durable work. Review and refinement retain `continue_on_error: true`; the
checkpoint always exits zero. Strict implementation and finalization gates keep
their existing policies.

## Contents

- [Step order](#step-order)
- [Feedback input](#feedback-input)
- [Status classification](#status-classification)
- [Checkpoint output](#checkpoint-output)
- [Durable references](#durable-references)
- [Propagation and security](#propagation-and-security)

## Step order

| Order | Step | Output / responsibility |
| --- | --- | --- |
| 1 | `step-06a-documentation` | Writes retcon documentation. |
| 2 | `step-06b-documentation-review` | Produces structured `doc_review_feedback`; nonfatal. |
| 3 | `step-06c-documentation-refinement` | Produces `final_documentation`; nonfatal. |
| 4 | `step-06b-checkpoint-doc-review` | Produces `doc_review_checkpoint`; Bash, exit `0`. |
| 5 | `step-06d-goal-already-met-probe` | Performs its independent goal probe. |

The checkpoint's `06b` prefix associates it with the review. It executes after
refinement. Recipes composing `workflow-design` receive this behavior through
the design phase.

## Feedback input

The review uses `parse_json: true` and returns one object. For example:

```json
{"status":"OK","feedback":"The Codex guide preserves explicit hook trust and user configuration."}
```

The producer's `status` is exactly `OK` or `NEEDS_ATTENTION`; `feedback` contains
review guidance. The checkpoint consumes status as data through `orch helper
extract-json` and `extract-field --field status`.

The shared reader selects the documentation cohort: authoritative
`AMPLIHACK_CONTEXT_FILE` when supplied, otherwise relevant
`RECIPE_VAR_doc_review_feedback` root/nested presence, otherwise legacy
`DOC_REVIEW_FEEDBACK`. File input is a complete top-level context containing
`doc_review_feedback`, whereas the namespaced root contains the complete feedback
object. Empty, malformed, negative or missing authoritative feedback never falls
back to a stale legacy `OK`. Nested status aliases cannot manufacture a missing
object. See [workflow context transport](workflow-context-transport.md) for
snapshot, decoding and error contracts.

## Status classification

Status normalization uppercases the extracted value and removes whitespace.
Only normalized `OK` passes. English prose, success keywords and unknown status
values do not approve the review.

| Selected feedback | Checkpoint | Fixed reason |
| --- | --- | --- |
| Usable object with normalized status `OK` | `OK` | `documentation-review passed` |
| Absent or unusable feedback | `NEEDS_ATTENTION` | `documentation-review produced no feedback (step may have failed)` |
| Present feedback with missing, unknown or non-OK status | `NEEDS_ATTENTION` | `documentation-review reported issues requiring follow-up` |

A valid empty object is present feedback with missing status. Invalid canonical
input yields no usable feedback and retains canonical authority. Legacy-only
text retains the existing extractor's behavior.

## Checkpoint output

The Bash checkpoint emits a String into `doc_review_checkpoint`. With an OK
review and no durable references, stdout is:

```text
DOC_REVIEW_CHECKPOINT: OK
  reason: documentation-review passed
  durable_artifacts:
```

With no usable feedback and no durable references, stdout is:

```text
DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION
  reason: documentation-review produced no feedback (step may have failed)
  marker: NEEDS_ATTENTION
  follow_up: re-run documentation-review or address the noted gaps
  durable_artifacts:
```

A follow-up emits three warnings on stderr: the review did not cleanly pass,
documentation review is a quality signal while execution continues, and durable
work is preserved. Reasons and follow-up messages are fixed text. All checkpoint
outcomes exit zero; an OK checkpoint alone does not establish workflow completion.

## Durable references

Only these existing non-sensitive metadata references are included. Each line is
omitted when neither source has a value.

| Output label | Primary source | Fallback |
| --- | --- | --- |
| `branch` | `BRANCH_NAME` | `WORKTREE_SETUP_BRANCH` |
| `pr_number` | `PR_NUMBER` | `PULL_REQUEST_NUMBER` |
| `pr_url` | `PR_URL` | `PULL_REQUEST_URL` |
| `commit_sha` | `COMMIT_SHA` | `HEAD_SHA` |
| `review_thread` | `REVIEW_THREAD_ID` | `REVIEW_COMMENT_ID` |

The checkpoint never fabricates a reference or emits placeholders. Repairing
feedback transport does not expand this reference allowlist.

## Propagation and security

The checkpoint String passes through the parent recipe context and summary.
`NEEDS_ATTENTION` makes documentation follow-up visible while preserving already
created commits, pull requests and review threads. Address the feedback or rerun
the review before treating documentation quality as settled.

Feedback is untrusted data. It is piped through fixed extraction commands,
never evaluated, sourced, interpolated into commands or printed verbatim in the
checkpoint. Diagnostics do not dump context, credentials or the environment.
Nonfatal documentation behavior does not soften HOLLOW rejection, ownership
checks, publication requirements or strict finalization.

See [checkpoint configuration](../howto/configure-doc-review-checkpoint.md),
[implementation evidence](workflow-implementation-evidence.md) and
[workflow context transport](workflow-context-transport.md).
