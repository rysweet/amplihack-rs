---
title: Verify a PR with Auto-drive Without Merging
description: verify-only usage after invocation-wide authorization is implemented and tested.
last_updated: 2026-11-11
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Verify a PR with Auto-drive Without Merging

The runner captures `no_merge=true` before child execution and preserves it through finalization. Controlled tests cover authorization and zero-request outcomes. Live verify-only execution and the OS/runtime matrix require their own evidence; no live merge is part of validation.

Use `no_merge=true` to review, repair, and verify an existing PR while keeping
it open. This allows commits, pushes, description updates, and required
follow-up issues. Dry-run additionally suppresses follow-up issue creation;
it is not a general read-only mode.

## Establish the target

Start in the repository's PR branch worktree. You need an installed
`amplihack`, an available agent runtime, authenticated platform access,
and the repository's required test tools. Commands shown here use Git to
identify the branch; the agent may use other available platform clients for
observations and recovery.

```bash
git branch --show-current
git status --short
```

The first command prints the source branch; the second lists local changes
or prints nothing for a clean worktree. Preserve unrelated local work.
Ensure that the branch has the PR you intend to verify.

## Run with merging disabled

```bash
# Explicitly restrict this invocation to readiness verification.
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c existing_branch="$(git branch --show-current)" \
  -c no_merge=true
```

Auto-drive reuses the branch's open PR, resolves its linked worktree, runs
crusty review and repair, and applies all merge-ready criteria. Its final
report identifies the PR, verified head, criterion evidence, requested action,
observed platform state, and blockers.

Expected successful outcome: merge-ready, no merge requested, PR observed
open. If evidence or approval is missing, expect the named blocker or
incomplete stop reason instead. `no_merge=true` never permits a merge,
including after re-review or light-to-full escalation.

For explicit selection, supply the actual PR number through `-c pr_number`;
it takes precedence over branch discovery. Do not name a different branch.

## Suppress follow-up creation too

```bash
# Explicitly restrict this invocation to readiness verification.
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c existing_branch="$(git branch --show-current)" \
  -c no_merge=true \
  -c autodrive_dry_run=true
```

Expected behavior: zero merge requests and no follow-up issue creation.
If minor findings need follow-ups that do not already exist, readiness remains
blocked and the report lists those obligations. Dry-run does not waive them.

## Resume incomplete work

Correct the reported cause and rerun with `no_merge=true`
again. The full-composer default is `false`; omitting the restriction
on a new invocation permits merging once ready. Historical authorization does
not carry over.

If the run exhausted its per-loop budget and continued work is appropriate:

```bash
# Explicitly restrict this invocation to readiness verification.
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c existing_branch="$(git branch --show-current)" \
  -c no_merge=true \
  -c autodrive_budget_loop_rounds=20
```

Expected behavior: reuse validated progress for the same PR, retain consumed
usage, and start additional work only if every applicable budget permits it.
Raise another exhausted budget separately; a higher limit is not readiness.

A failed GraphQL command or missing client may be recovered through a complete
alternative interface. If recovery cannot supply all facts, the run reports
the cause and stops incomplete. Pending approvals require the authorized
reviewer; policy bypasses and hook skipping are prohibited.

## Inspect readiness and publish evidence

Check that all 13 criteria have concrete, current evidence, the PR description
was read back, and the final platform observation says open. Distinguish
controlled tests, actual agent recovery, live verify-only execution, and
unavailable OS/runtime coverage in the PR description. Verify-only execution
does not demonstrate a real merge.

See [workflow options](../reference/auto-drive-to-merge.md),
[evidence and outcomes](../reference/auto-drive-evidence.md), and
[QA configuration](../reference/auto-drive-qa.md).
