---
title: Read documentation review checkpoints
description: Inspect structured review status and act on nonfatal documentation follow-up.
type: howto
updated: 2026-10-08
---

# Read documentation review checkpoints

Recipes composing `workflow-design` record documentation review automatically.
No feature flag or additional required context enables the checkpoint.

## Inspect the checkpoint

Read `doc_review_checkpoint` in the saved recipe result or run summary. Look for
`DOC_REVIEW_CHECKPOINT: OK` or `DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION`. The latter
includes a fixed reason and follow-up, plus any available durable references.

| Marker | Meaning | Action |
| --- | --- | --- |
| `OK` | Selected feedback has normalized status `OK`. | Continue assessing the workflow's other gates. |
| `NEEDS_ATTENTION` | Feedback is missing, unusable or non-OK. | Inspect review/refinement output and address the documentation gaps. |
| Missing checkpoint | The checkpoint did not produce a result. | Inspect the run's actual failure and completed steps. |

A checkpoint does not establish that implementation, publication or finalization
succeeded. References may be absent, especially during a fresh design phase.

## Resolve follow-up

1. Inspect the documentation-review object and the refinement output.
2. Address the gaps or rerun the documentation review in the owning workflow.
3. If present, use `pr_url`, `branch`, `commit_sha` and `review_thread` to locate
   the relevant durable work. Confirm those references before acting on them.
4. Confirm the new review's structured status is `OK` and inspect the new
   checkpoint. Preserve earlier failure records.

## Diagnose missing feedback

The reader uses `AMPLIHACK_CONTEXT_FILE` when supplied, then relevant canonical
`RECIPE_VAR_doc_review_feedback` input, then legacy `DOC_REVIEW_FEEDBACK` only
when canonical input is absent. A supplied invalid file or empty canonical root
cannot revive legacy success. Correct the producer or authoritative transport;
do not add an `OK` alias to mask missing feedback.

The producer returns an object with `status` and `feedback`. Prose such as
“looks good” does not replace structured status. See
[workflow context transport](../reference/workflow-context-transport.md).

## Fixed behavior

Review and refinement remain nonfatal. The Bash checkpoint always exits zero,
emits fixed warnings for follow-up and includes only allowlisted non-sensitive
references. It does not soften implementation verification, publication or
strict finalization. There is no checkpoint-specific configuration knob.

See the [checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md)
and [checkpoint tutorial](../tutorials/doc-review-non-fatal-checkpoint.md).
