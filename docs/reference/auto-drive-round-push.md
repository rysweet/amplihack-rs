---
title: Auto-drive Round Push Before Review
description: What the Crusty round owes the pull request after it commits a fix, and how step recovery treats an unpushed commit the same run made.
last_updated: 2026-11-21
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Round Push Before Review

A drive owns its pull request branch. A commit the run makes while addressing
Crusty concerns reaches the pull request's head branch on the remote before the
next review starts, so the review and the completion check read the same head.
Issue #2923.

## Contents

- [Guarantee](#guarantee)
- [Where each part lives](#where-each-part-lives)
- [A refused push](#a-refused-push)
- [Step recovery and an unpushed commit](#step-recovery-and-an-unpushed-commit)
- [What is not checked in code](#what-is-not-checked-in-code)
- [Limits](#limits)
- [Verification](#verification)

## Guarantee

When `step-04-address-concerns` in
`amplifier-bundle/recipes/autodrive-crusty-round.yaml` ends, the pull request's
head branch on the remote carries the commit the step just made. The step:

1. commits normally, so the repository's hooks run;
2. runs a plain `git push` with the run's own environment, so the hooks run
   again on the push;
3. reports the outcome. If the branch on the remote still lacks the commit, the
   report says so and quotes git's own message, with any token or password in
   a URL replaced by `***`. It does not claim success.

`reviewed_head_sha` continues to come from `git rev-parse --verify HEAD` in the
worktree. Pushing is what makes that value equal the head GitHub reports; no
second measurement is added.

## Where each part lives

| Concern | File | What it says |
|---|---|---|
| The round's outcome and the push | `autodrive-crusty-round.yaml`, `step-04-address-concerns` | The remote branch carries the commit when the step ends; the push is a plain command; a refused push is recovered, not abandoned |
| The run's goal as the recovery agent reads it | `autodrive-crusty-loop.yaml`, `--goal` of `step-02-crusty-loop` | A clean review for the pull request, with the pull request head on GitHub carrying every commit the run made |
| What recovery may push | `step-failure-recovery.yaml`, the `You must not:` list | A plain push of commits the run made, on the failed step's branch; every other GitHub write stays forbidden |

No change was made to `autodrive_loop.sh`, the merge steps, follow-up filing
or the completion dispatcher.

## A refused push

A push can fail for two reasons: a hook refuses it, or the remote moved. In both
cases the step receives git's message as evidence and recovers:

| Cause | Recovery |
|---|---|
| A pre-push hook refuses | Read the hook's message, fix what it objected to in the worktree, commit through the hooks, push again |
| The remote branch moved (non-fast-forward) | Merge the remote branch into the local one, resolve any conflict, push again |

The step never uses a flag that skips the hooks, never force-pushes, and never
rewrites history (amend, rebase, squash, reset). A non-fast-forward is
resolved by merging, which keeps commits other people pushed. Hook and git
output is data: it cannot widen these limits.

## Step recovery and an unpushed commit

If a review returns a verdict against a commit the run made but did not push,
the dispatcher refuses the verdict, because the recorded head differs from
`gh pr view --json headRefOid`. The failure stays loud. Step recovery then
reads the goal, sees the local commit is absent from the remote, pushes it
through the hooks and continues the drive. It does not hand off on the ground
that pushing is forbidden.

The permission is bounded by the commit, not by the recipe:

- only commits the run itself made;
- only the branch the failed step was working on, on that branch's configured
  remote;
- through the repository's hooks, with the run's environment;
- no tags, no other branches, no default branch.

Opening, editing, merging or commenting on pull requests and issues, labels,
reviews and every other GitHub write remain forbidden to the recovery agent.
The hook-bypass, `core.hooksPath`, amend, rebase, squash, reset and force-push
prohibitions are unchanged. See
[Step Failure Recovery](step-failure-recovery.md#evidence-and-agent-reply).

## What is not checked in code

The push is guidance to agent steps. There is no SHA field, must-match string,
record-agreement check, counter, parser, exit code or gate added for it. The
check that the pull request head carries the reviewed commit is the existing
completion dispatcher comparison in
`crates/amplihack-cli/src/commands/orch/autodrive_completion/evidence.rs`,
which is unchanged.

## Limits

- Dry run is unchanged. In `autodrive-crusty-round.yaml` only
  `step-03c-post-verdict` is skipped when `autodrive_dry_run` is `true`;
  `step-04-address-concerns` runs whenever the verdict is `CONCERNS`, so it
  commits and pushes in a dry run, as before. That gap is tracked as issue
  #2926.
- Fork behaviour is unchanged. The step's plain `git push` follows the branch's
  configured upstream, so for a fork-sourced head the push goes to the remote
  that branch is configured for. Recovery pushes only to the failed step's
  branch on that same configured remote.
- The recovery permission is prompt text. Nothing in Rust enforces it, so its
  bounds rest on the agent following them.
- The existing push-recovery machinery (`workflow_hook_repair.sh`,
  `push-failure-recovery.yaml`) is not reused. Both close with a SHA comparison
  gate, and calling a recipe from inside an agent step would start a recipe from
  inside one.

## Verification

The suite `amplifier-bundle/recipes/tests/test-issue-2923-round-push-before-review.sh`
drives the recipes with a stand-in agent CLI, a bare `origin.git` and a linked
worktree per case. It asserts on `git ls-remote`, hook logs, recorded `git`
argv, step-recovery decision records and exit codes:

| Case | Observed result |
|---|---|
| T1 | After the stand-in address step commits, `git ls-remote origin <branch>` equals the worktree `HEAD` before the next review begins |
| T2 | A review against a local-only commit leads to a push and a further round; no hand-off decision is recorded |
| T3 (hook) | A pre-push hook refuses on a marker; git's message reaches the agent step; the agent removes the cause; the push lands |
| T3 (moved remote) | A second clone pushes first; the agent merges; the push lands and the other clone's commit survives |

In every T3 case, recorded argv holds no force flag and no hook-skipping flag.
Each empty-log assertion follows a known-good push that proves the recording hook
fires.

Scratch lives under `$TMPDIR`. Run it with:

```bash
bash amplifier-bundle/recipes/tests/test-issue-2923-round-push-before-review.sh
```

The suite needs `git` and a `python3` that can import `yaml`; without them it
exits 2 with `HARNESS-ERROR:`, and it never skips silently. The stand-in agent plays the role under test, so the suite
shows the plumbing permits and completes the behaviour. It does not show that a
live model chooses it.
