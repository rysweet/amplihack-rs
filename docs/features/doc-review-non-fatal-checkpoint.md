---
title: Nonfatal documentation review
description: Preserve durable work while making documentation follow-up visible.
type: explanation
updated: 2026-10-08
---

# Nonfatal documentation review

Documentation review provides a quality signal while the workflow preserves
already created work. A failed or incomplete review remains visible as follow-up;
it does not erase commits, pull requests or review threads.

## Review status and workflow completion

`workflow-design` writes documentation, reviews it, refines it and records a
checkpoint. Review and refinement use `continue_on_error: true`. The checkpoint
normalizes the review object's `status`: only `OK` passes; other, missing or
unusable feedback produces `NEEDS_ATTENTION` with fixed warnings.

The checkpoint does not inspect English approval phrases or turn a successful
Bash exit into proof of completed implementation. Its own exit is always zero.
The runner still owns the recipe outcome, and strict terminal-state/finalization
gates still assess their required evidence.

## Preserving durable work

The checkpoint carries available branch, PR, commit and review references into
the summary. It omits missing references. A new design phase may have no PR or
commit yet; nonfatal documentation behavior applies regardless of those refs.
Existing durable work remains visible alongside the documentation follow-up.

This separates a documentation quality issue from the workflow's authoritative
completion decision. `NEEDS_ATTENTION` requests review or refinement; it never
certifies implementation, verification, CI or publication.

## Structured transport across providers

Codex, Claude and Copilot use the same canonical object transport for review
feedback. A supplied context file takes priority over namespaced environment
input; legacy scalars remain supported when canonical input is absent. Invalid,
empty or negative authoritative feedback cannot be replaced by stale legacy
success. The reader handles authority while the checkpoint handles status.

Feedback stays data. It is never evaluated or printed as arbitrary checkpoint
instructions, and only the existing non-sensitive reference allowlist is emitted.

See [checkpoint configuration](../howto/configure-doc-review-checkpoint.md),
[checkpoint tutorial](../tutorials/doc-review-non-fatal-checkpoint.md),
[checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md) and
[workflow context transport](../reference/workflow-context-transport.md).
