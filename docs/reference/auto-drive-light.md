---
title: Auto-Drive-Light Recipe
description: auto-drive-light builds a small PR, then runs the same goal loop as auto-drive-to-merge, with merging off by default.
last_updated: 2026-11-23
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-Drive-Light Recipe

`auto-drive-light` is for small changes. Its build stage is unchanged: it
resolves or opens the PR and repairs hooks. It then measures how large the
change is. After that it runs the same [goal loop](./autodrive-goal-loop.md) as
[auto-drive-to-merge](./auto-drive-to-merge.md).

A cross-repository PR is refused twice: by the eligibility step when the build
result names the PR and its `is_cross_repository` is not exactly `false`
(#2538), and by the goal loop from GitHub's own reading before every turn. When
the build result does not name the PR, its head and its base, the change is not
measured (`decision: unmeasured`), and the run carries on; the measurement is
information, never a reason to end the run.

## Differences from auto-drive-to-merge

- `no_merge` defaults to `true`. The run is merging-off unless the operator
  passes `-c no_merge=false`. A merging-off run carries on with the task's
  remaining steps, then reports the PR ready, with the owner who decides the
  merge named in the comment.
- The measurement of the change (`light_eligibility`) goes to the agent as
  information. It no longer escalates the run to `auto-drive-to-merge`. A large
  change is reviewed in the same loop, and how deeply is the agent's judgement.
  The `light_eligibility` key is accepted and ignored.
- The `autodrive_budget_*` keys are accepted and ignored.

Merge authority, guidance, hand-off and the reported outcomes are the same as
for the other drive. The build stage and hook repair still use their current
behaviour and are covered by a later change.

## How to

```bash
amplihack recipe run auto-drive-light -c pr_number=3129
```

Set `checks_watch_interval` to change the 120-second interval between CI reads
during `wait`. [CI check interval](./autodrive-goal-loop.md#ci-check-interval)

## Related

- [Auto-drive Goal Loop](./autodrive-goal-loop.md)
- [Auto Drive To Merge Reference](./auto-drive-to-merge.md)
