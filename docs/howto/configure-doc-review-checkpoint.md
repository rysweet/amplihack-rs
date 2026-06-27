# How to Configure the Documentation-Review Checkpoint

> [Home](../index.md) > How-To > Configure the Documentation-Review Checkpoint

This guide shows how the non-fatal documentation-review checkpoint behaves in a
run, how to read its output, and what is and is not configurable.

The behavior ships enabled. There is no feature flag to turn it on, and there is
no new required input.

---

## Before You Start

- You are running `smart-orchestrator` or `default-workflow` (both compose
  `workflow-design`).
- `repo_path` points at a writable git repository.
- You are comfortable letting the workflow create branches, commits, and PRs
  when it reaches those steps.

---

## 1. Run the Workflow Normally

No extra context is required. The checkpoint activates automatically.

```bash
amplihack recipe run default-workflow \
  -c "task_description=Implement and document the rate-limiter" \
  -c "repo_path=/home/user/src/amplihack-rs" \
  -c "branch_prefix=feat"
```

If `step-06b-documentation-review` succeeds, the checkpoint records `OK:
doc-review` and the run proceeds with no follow-up item.

---

## 2. Read the Degraded-Success Summary

When documentation review fails *after* durable side effects exist, the run
continues and the summary contains a checkpoint block:

```text
WARNING: step-06b-documentation-review exited non-zero (review reported failure).
NEEDS_ATTENTION: doc-review
  branch:        feat/issue-834-non-fatal-doc-review
  pr:            rysweet/amplihack-rs#841 (https://github.com/rysweet/amplihack-rs/pull/841)
  commit:        8fb46865fb4412038b9313a62c02cc5aa0693132
  review_thread: 1987654321
  follow_up:     Re-run documentation review for the listed PR before close.
```

To extract just the checkpoint output via the CLI:

```bash
amplihack recipe run default-workflow \
  -c task_description="Implement and document the rate-limiter" \
  -c repo_path="/home/user/src/amplihack-rs" \
  -c branch_prefix="feat" \
  --output json | jq -r '.context.doc_review_checkpoint'
```

Act on the `NEEDS_ATTENTION: doc-review` marker and the references under it. The
listed PR, branch, commit, and review thread are the durable work that already
landed.

---

## 3. Distinguish the Three Outcomes

| Summary contains | Meaning | Action |
| --- | --- | --- |
| `OK: doc-review` | Review passed. | None. |
| `NEEDS_ATTENTION: doc-review` with refs | Review failed after work landed; run is degraded-success. | Re-run documentation review for the listed PR, then close. |
| A hard `FAILURE` with no checkpoint block | Failure occurred before any durable side effect, or in a fail-closed terminal-state gate. | Investigate the failing step; this path is intentionally not softened. |

---

## 4. Resolve a Documentation-Review Follow-Up

When you see `NEEDS_ATTENTION: doc-review`:

1. Open the PR listed under `pr:`.
2. Confirm the commit under `commit:` is present on the branch under `branch:`.
3. Re-run documentation review against that PR (re-run the workflow or run the
   documentation phase manually).
4. Once review passes, resolve the review thread under `review_thread:` and
   close the follow-up.

The implementation, verification, and PR work are already complete — you are
only clearing the documentation quality signal.

---

## 5. Know What Is Not Configurable

The checkpoint is a safety/visibility contract, not a tuning surface. These are
fixed:

- `step-06b-documentation-review` is non-fatal (`continue_on_error: true`).
- The checkpoint always exits `0` and never aborts the workflow.
- Failure is always reported — `WARNING` on stderr and `NEEDS_ATTENTION` in the
  summary. It is never silently swallowed.
- Only the allow-listed artifact refs (branch, PR id/url, commit sha, review
  thread/comment id) are surfaced. No tokens or environment dumps.
- Pre-side-effect failures and terminal-state gates remain fail-closed.

If you need different behavior, change the workflow contract and its tests
together. Do not override the checkpoint in local wrapper scripts.

---

## Related Documentation

- [Non-fatal documentation review checkpoint overview](../features/doc-review-non-fatal-checkpoint.md)
- [Documentation-review checkpoint reference](../reference/doc-review-non-fatal-checkpoint.md)
- [Tutorial: degraded-success after a failed doc review](../tutorials/doc-review-non-fatal-checkpoint.md)
