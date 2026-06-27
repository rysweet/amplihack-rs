# Tutorial: Degraded-Success After a Failed Documentation Review

> [Home](../index.md) > Tutorials > Degraded-Success After a Failed Documentation Review

This walkthrough reproduces the scenario from issue
[#834](https://github.com/rysweet/amplihack-rs/issues/834): a round that has
already pushed a commit, opened a PR, and posted a review thread, but whose
`step-06b-documentation-review` then fails. You will see the workflow end in a
clearly-labelled degraded-success state instead of a generic `FAILURE`.

---

## What You Will Learn

- Why a documentation-review failure used to hide completed work.
- How the checkpoint converts that failure into a visible follow-up.
- How to read and resolve the `NEEDS_ATTENTION: doc-review` marker.

---

## Step 1: Start a Run That Produces Durable Side Effects

Run `default-workflow` on a task that implements and documents a feature:

```bash
amplihack recipe run default-workflow \
  -c "task_description=Add a token-bucket rate limiter and document it" \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "branch_prefix=feat"
```

By the time the workflow reaches the documentation phase, earlier steps have
typically produced durable output for the round:

- a working branch, for example `feat/issue-834-non-fatal-doc-review`,
- a pushed commit, for example `8fb46865...`,
- an opened or merged PR, for example `rysweet/amplihack-rs#841`,
- a posted review thread, for example `1987654321`.

---

## Step 2: Observe the Documentation-Review Failure

Suppose `step-06b-documentation-review` exits non-zero (the architect agent
errored, or the feedback came back empty).

Before #834, the run stopped here with a generic hard failure. The completed
commit, PR, and review thread were invisible in the result, and you had to
reconcile by hand.

With the checkpoint, the run does **not** abort. `continue_on_error: true` on
the review step records the failure and lets the workflow continue.

---

## Step 3: Read the Checkpoint Output

The follow-on checkpoint step inspects the review feedback, sees it failed, and
emits a `WARNING` to stderr plus a `NEEDS_ATTENTION` summary to stdout:

```text
WARNING: step-06b-documentation-review exited non-zero (review reported failure).
NEEDS_ATTENTION: doc-review
  branch:        feat/issue-834-non-fatal-doc-review
  pr:            rysweet/amplihack-rs#841 (https://github.com/rysweet/amplihack-rs/pull/841)
  commit:        8fb46865fb4412038b9313a62c02cc5aa0693132
  review_thread: 1987654321
  follow_up:     Re-run documentation review for the listed PR before close.
```

Inspect just the checkpoint output with the CLI:

```bash
amplihack recipe run default-workflow \
  -c task_description="Add a token-bucket rate limiter and document it" \
  -c repo_path="/home/user/src/amplihack-rs" \
  -c branch_prefix="feat" \
  --output json | jq -r '.context.doc_review_checkpoint'
```

Every durable reference the workflow knew about is listed. Any reference that
did not exist for this round is simply omitted — the checkpoint never prints a
placeholder or fails on a missing value.

---

## Step 4: Confirm the Run Ended Degraded, Not Failed

The run reaches its reconciliation/summary phase. The summary lists:

- what succeeded: the branch, commit, PR, and review thread, and
- what needs follow-up: the `NEEDS_ATTENTION: doc-review` item.

This is the degraded-success state. The completed implementation, verification,
and PR work are intact and visible; only the documentation quality signal is
outstanding.

---

## Step 5: Resolve the Follow-Up

1. Open the PR under `pr:` (`rysweet/amplihack-rs#841`).
2. Verify the commit under `commit:` is on the branch under `branch:`.
3. Re-run documentation review for that PR.
4. When review passes, resolve the review thread under `review_thread:` and
   close the follow-up.

You did not redo any implementation — you only cleared the documentation review.

---

## Step 6: Compare Against the Out-of-Scope Path

To confirm the boundary, picture a failure that happens *before* any durable
side effect — no commit, no PR, no review thread. There is no completed work to
protect, so that path may still surface as a hard failure and is intentionally
**not** softened by this feature. Terminal-state and finalization gates likewise
remain fail-closed.

---

## What You Did

You ran a workflow whose documentation review failed after durable work landed,
saw the checkpoint preserve and surface that work as a degraded-success with a
`NEEDS_ATTENTION` follow-up, and resolved the follow-up without redoing the
implementation.

---

## Related Documentation

- [Non-fatal documentation review checkpoint overview](../features/doc-review-non-fatal-checkpoint.md)
- [How to configure the documentation-review checkpoint](../howto/configure-doc-review-checkpoint.md)
- [Documentation-review checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md)
