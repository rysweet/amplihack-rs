---
title: Verify a PR with Auto-drive Without Merging
description: verify-only usage after invocation-wide authorization is implemented and tested.
last_updated: 2026-11-13
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
crusty review and repair, and applies all merge-ready criteria. Each
merge-ready round posts its write-up as a PR comment naming the head SHA it
assessed. The final report (`autodrive_outcome`) gives the requested action
`verify_readiness`, the loop result, the merge result and the stop reason,
with the finalization statement as its summary.

Expected successful outcome: loop result `DONE`, merge result
`not-authorised`, stop reason `completed`, PR still open and not a draft. If
a blocker cannot be cleared within the run, expect a hand-off instead: the PR
becomes a draft with a comment naming the blocker and head SHA.
`no_merge=true` never permits a merge, including after re-review or
light-to-full escalation.

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

Expected behavior: zero merge requests, no follow-up issue creation, and no
PR comment or draft change; the write-up and any hand-off are printed in the
step output instead. If minor findings need follow-ups that do not already
exist, readiness remains blocked and the write-up names those obligations.
Dry-run does not waive them.

## Resume incomplete work

Correct the blocker named in the hand-off comment, mark the PR ready for
review again if it was drafted, and rerun with `no_merge=true` again. The
full-composer default is `false`; omitting the restriction on a new
invocation permits merging once ready. Historical authorization does
not carry over, including through `recipe run --fresh` or a durable recovery
resume.

Each run keeps its records in its own state directory unless you pass one.
To continue a run that exhausted its per-loop budget, pass the same absolute
`autodrive_state_dir` that the exhausted run used:

```bash
# Explicitly restrict this invocation to readiness verification.
amplihack recipe run auto-drive-to-merge \
  -c repo_path=. \
  -c existing_branch="$(git branch --show-current)" \
  -c autodrive_state_dir="$HOME/.amplihack/state/auto-drive/pr-2340" \
  -c no_merge=true \
  -c autodrive_budget_loop_rounds=20
```

Expected behavior: retain consumed usage, and start additional work only if
every applicable budget permits it.
A relative path, or one containing `..` or a control character, stops the run
at once with `autodrive_state_dir must be an absolute path`. See
[state directory](../reference/auto-drive-to-merge.md#state-directory).
Raise another exhausted budget separately; a higher limit is not readiness.

A failed GraphQL command or missing client may be recovered through a complete
alternative interface. If recovery cannot supply all facts, the run reports
the cause and stops incomplete. A rate limit is not waited out: the run
records the status and any `Retry-After` or reset time, stops incomplete, and
a later invocation continues the work. Pending approvals require the authorized
reviewer; policy bypasses and hook skipping are prohibited.

## Inspect readiness and publish evidence

Read the last merge-ready comment on the PR. Check that it names the head SHA
you expect, that all 13 criteria have concrete, current evidence, and that
its closing sentence says ready. The PR should still be open. Distinguish
controlled tests, actual agent recovery, live verify-only execution, and
unavailable OS/runtime coverage in the PR description. Verify-only execution
does not demonstrate a real merge.

See [workflow options](../reference/auto-drive-to-merge.md),
[merge-ready round, merge and hand-off](../reference/autodrive-merge-ready-round.md),
[evidence and outcomes](../reference/auto-drive-evidence.md), and
[QA configuration](../reference/auto-drive-qa.md).
