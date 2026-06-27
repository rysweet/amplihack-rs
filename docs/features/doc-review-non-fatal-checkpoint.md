# Non-Fatal Documentation Review Checkpoint

**A failed `step-06b-documentation-review` no longer destroys completed
implementation, verification, and PR work. Documentation review is a quality
signal, not a release gate. When it fails after durable side effects already
landed, the workflow checkpoints the partial success, surfaces the durable
artifacts, and ends in a clearly-labelled degraded-success state.**

> [Home](../index.md) > [Features](README.md) > Non-Fatal Documentation Review Checkpoint

## Quick Navigation

- [How to configure the documentation-review checkpoint](../howto/configure-doc-review-checkpoint.md)
- [Tutorial: degraded-success after a failed doc review](../tutorials/doc-review-non-fatal-checkpoint.md)
- [Documentation-review checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md)

## What This Feature Does

`smart-orchestrator` and `default-workflow` run a documentation phase inside
`workflow-design`:

1. `step-06a` writes retcon documentation.
2. `step-06b-documentation-review` reviews that documentation.
3. `step-06c-documentation-refinement` revises it.

Before issue [#834](https://github.com/rysweet/amplihack-rs/issues/834), a
non-zero exit from `step-06b-documentation-review` propagated upward as a
generic hard failure. That happened even when earlier rounds had already
produced **durable** output — a pushed hardening commit, a merged follow-up PR,
and a posted review thread. The generic `FAILURE` hid the completed work and
forced manual reconciliation to discover what actually shipped.

This feature changes the failure semantics of this phase:

| Guarantee | Behavior |
| --- | --- |
| Non-fatal review | `step-06b-documentation-review` runs with `continue_on_error: true`. A non-zero exit no longer aborts the parent workflow. |
| Checkpointed partial success | A follow-on checkpoint step records the completed work and lets the workflow continue to its reconciliation/summary phase. |
| Surfaced artifacts | The checkpoint records the durable references it knows about — branch, PR id/url, review thread/comment id, and commit sha — so the summary shows them instead of only `failed`. |
| Visible, not swallowed | The failure is reported as a `WARNING` on stderr **and** a `NEEDS_ATTENTION` marker in the run summary. It is never silently discarded. |
| Degraded-success, not green | The run ends in a partial state that lists what succeeded and what needs follow-up. The doc-review item remains an explicit open action. |

Documentation-review failure is now a quality follow-up item, not an event that
deletes verified implementation and published PR work.

## Scope Boundary

This feature softens **only** the documentation-review failure path that runs
*after* durable side effects exist. It does not change:

- the documentation review logic itself,
- terminal-state / finalization gates (those remain fail-closed),
- any pre-side-effect failure path. A documentation-review failure that occurs
  before any commit, PR, or review thread exists may still surface as a hard
  failure, because there is no completed work to protect.

In other words: the checkpoint annotates and continues; it never overrides the
authoritative terminal-state arbiter that gates dirty worktrees, PR state,
diffs, and CI.

## Quick Start

No new required input. The behavior is on by default for every
`smart-orchestrator` and `default-workflow` run, because both compose
`workflow-design`.

```bash
amplihack recipe run smart-orchestrator \
  -c "task_description=Implement and document the rate-limiter" \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "branch_prefix=feat"
```

If `step-06b-documentation-review` fails after the round has already pushed a
commit, opened or merged a PR, or posted a review thread, the run continues and
the summary contains a checkpoint block. The exact `WARNING` text depends on
why the review failed.

When the review step produced **no usable feedback** (it exited non-zero with
empty output):

```text
WARNING: step-06b-documentation-review exited non-zero (documentation review feedback unavailable).
NEEDS_ATTENTION: doc-review
  branch:        feat/issue-834-non-fatal-doc-review
  pr:            rysweet/amplihack-rs#841 (https://github.com/rysweet/amplihack-rs/pull/841)
  commit:        8fb46865fb4412038b9313a62c02cc5aa0693132
  review_thread: 1987654321
  follow_up:     Re-run documentation review for the listed PR before close.
```

When the review step returned feedback that **explicitly reports a failure**:

```text
WARNING: step-06b-documentation-review exited non-zero (review reported failure).
NEEDS_ATTENTION: doc-review
  branch:        feat/issue-834-non-fatal-doc-review
  pr:            rysweet/amplihack-rs#841 (https://github.com/rysweet/amplihack-rs/pull/841)
  commit:        8fb46865fb4412038b9313a62c02cc5aa0693132
  review_thread: 1987654321
  follow_up:     Re-run documentation review for the listed PR before close.
```

Both produce the same `NEEDS_ATTENTION: doc-review` follow-up; only the
parenthetical reason on the `WARNING` line differs. The exit signal you act on
is the `NEEDS_ATTENTION: doc-review` marker plus the references under it — not a
bare `FAILURE`.

## What Happens When Documentation Review Fails

1. `step-06b-documentation-review` exits non-zero. Because of
   `continue_on_error: true`, the workflow does not abort.
2. The follow-on checkpoint step inspects the review feedback. When the feedback
   indicates failure or is empty, it:
   - writes a `WARNING:` diagnostic line to stderr naming the failed step and
     the reason,
   - writes a machine-consumable summary to stdout containing the
     `NEEDS_ATTENTION` marker and every durable artifact reference that is
     present in context, each guarded so absent references are simply omitted,
   - exits `0` so the workflow proceeds.
3. The checkpoint output is threaded into the parent context and the run
   summary, so the degraded-success state lists both the work that succeeded and
   the doc-review follow-up.

When documentation review succeeds, the checkpoint records an `OK` result and
the workflow proceeds exactly as before. The checkpoint adds a follow-up item
only when review actually failed.

## Why Degraded-Success Instead of a New Status

The recipe-runner exit status is owned by the external `recipe-runner-rs`
binary and is not redefined by this feature. Partial state is expressed the way
the rest of the codebase already expresses it: with `WARNING` and
`NEEDS_ATTENTION` markers in the summary output. There is no new status enum to
learn, and `--locked` CI is unaffected because no Rust or dependency change is
required — the fix is YAML-only and relies on `continue_on_error` and `output`,
which the runner already honors.

## Related Documentation

- [How to configure the documentation-review checkpoint](../howto/configure-doc-review-checkpoint.md)
- [Tutorial: degraded-success after a failed doc review](../tutorials/doc-review-non-fatal-checkpoint.md)
- [Documentation-review checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md)
- [Workflow execution guardrails](workflow-execution-guardrails.md)
