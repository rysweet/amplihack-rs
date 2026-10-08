---
title: Inspect a documentation review checkpoint
description: Practice reading documentation follow-up without inferring workflow completion.
type: tutorial
updated: 2026-10-08
---

# Inspect a documentation review checkpoint

This exercise uses the checkpoint's actual no-feedback output shape to practice
reading follow-up. It creates a private temporary text fixture and does not run
agents or create repository changes.

You need a POSIX shell and `mktemp`.

## Create a practice checkpoint

Run these commands in one shell:

```bash
checkpoint_dir=$(mktemp -d)
cat > "$checkpoint_dir/checkpoint.txt" <<'CHECKPOINT'
DOC_REVIEW_CHECKPOINT: NEEDS_ATTENTION
  reason: documentation-review produced no feedback (step may have failed)
  marker: NEEDS_ATTENTION
  follow_up: re-run documentation-review or address the noted gaps
  durable_artifacts:
CHECKPOINT
cat "$checkpoint_dir/checkpoint.txt"
```

The output is the five-line checkpoint above. No references appear because this
fixture has none. A real checkpoint adds only the durable references available
in its context.

## Identify the follow-up

Read the first two lines: documentation needs attention because no usable
feedback was selected. In a real run, inspect the structured review object and
the refinement result. A canonical transport error may suppress a stale legacy
`OK`; restoring that alias would mask the problem.

The follow-up line identifies the action: rerun the review or address its gaps.
The checkpoint's zero exit lets reconciliation continue. It does not establish
that implementation, verification, publication or finalization passed.

## Compare a passing checkpoint

Replace the fixture with the OK output shape:

```bash
cat > "$checkpoint_dir/checkpoint.txt" <<'CHECKPOINT'
DOC_REVIEW_CHECKPOINT: OK
  reason: documentation-review passed
  durable_artifacts:
CHECKPOINT
cat "$checkpoint_dir/checkpoint.txt"
```

The output is the three-line checkpoint above. In a real run, this means the
selected feedback has normalized status `OK`. Continue assessing the other
workflow gates using their own evidence.

## Remove the practice file

```bash
rm -rf -- "$checkpoint_dir"
```

Apply the same reading process to `doc_review_checkpoint` in a saved recipe
result. Keep earlier failed results when obtaining a corrected review.

See [checkpoint configuration](../howto/configure-doc-review-checkpoint.md),
[checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md) and
[workflow context transport](../reference/workflow-context-transport.md).
