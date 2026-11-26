---
title: Auto Drive To Merge Reference
description: How auto-drive-to-merge runs as one goal-driven agent loop, its inputs, authorization, endings and operator how-to.
last_updated: 2026-11-23
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto Drive To Merge Reference

`auto-drive-to-merge` builds a pull request if needed, then runs one
goal-driven agent loop. The goal is to get the pull request merged under the
owner's conditions. On each turn an agent reads the live state and the task
text and picks the next action. The loop is specified in
[Auto-drive Goal Loop](./autodrive-goal-loop.md); this page covers what an
operator sets and sees.

## Contents

- [Stages](#stages)
- [Inputs](#inputs)
- [Merge authority](#merge-authority)
- [What the agent decides](#what-the-agent-decides)
- [Minor review findings](#minor-review-findings)
- [How a run ends](#how-a-run-ends)
- [Crusty round fix evidence](#crusty-round-fix-evidence)
- [Retired inputs](#retired-inputs)
- [How to](#how-to)
- [Related](#related)

## Stages

1. `step-recovery-init`, `autodrive-pr-input` and `autodrive-build` resolve or
   open the pull request.
2. `autodrive-loop` runs the [goal loop](./autodrive-goal-loop.md) until the PR
   is merged, closed, handed off or ready, or the owner stops the run. It is
   skipped when the PR was already merged before the run.
3. `autodrive-report` writes the run report, including the step-recovery report.

## Inputs

| Context key | Meaning |
|-------------|---------|
| `pr_number`, `repo_path` | The pull request and its checkout. A cross-repository PR is refused. |
| `task_description` | The task text. Later guidance governs over it where they differ. |
| `no_merge` | `true` turns merging off. Defaults to `false` for this recipe. |
| `merge_method` | `merge`, `squash` or `rebase`; empty takes the first of squash, merge commit and rebase the repository allows. A method the repository does not allow is never used. |
| `autodrive_dry_run` | `true` reads and decides but never merges or writes to GitHub. |
| `checks_watch_interval` | Seconds between CI reads during `wait`. Defaults to 120. See [CI check interval](./autodrive-goal-loop.md#ci-check-interval). |
| `autodrive_budget_*` | Accepted and ignored. See [Retired inputs](#retired-inputs). |

## Merge authority

Merge authority comes only from the operator's own inputs (`no_merge`) and the
invocation the runner captured at launch. The agent's own context, the task
text and guidance cannot widen it. Guidance can narrow it, for example "do not
merge". The merge action always passes `--match-head-commit` and an allowed
merge method, and never rebases or force-pushes.

## What the agent decides

Each turn the agent chooses one of: review the PR (a Crusty round), wait for an
event, merge, hand off to a named owner, or report ready. Before it names the
action, it can fix findings and push through the hooks, bring the base in, or
install a host input.

- The base is brought in only when the PR conflicts, when the task text asks,
  or when the repository requires an up-to-date branch. It uses a merge commit.
  A branch that is only behind, with no strict checks, gets no base merge.
- A required check that is still running means the run waits. It merges when
  the check completes. It does not end as "checks pending".
- A missing host input, such as a `.venv`, is installed by the repository's own
  instructions and the run carries on.
- A decision that belongs to an owner, such as whether to accept a risk, ends
  in a hand-off naming that owner and the decision.
- Each required outcome in the task text or guidance is named by the agent and
  checked by name before a merge. One the agent rejects is answered in a PR
  comment with its reason
  ([required outcomes](./autodrive-goal-loop.md#required-outcomes)).
- A turn run that fails does not end the run. Its work is kept and the next
  turn's agent reads the failure
  ([failures inside a turn](./autodrive-goal-loop.md#failures-inside-a-turn)).

## How a run ends

| Ending | Report `stop_reason` |
|--------|----------------------|
| Merged at the expected head | `completed` |
| Merging off, PR ready, later task steps done | `completed` |
| Hand-off for an owner's decision | `handoff` |
| PR closed | `closed` |
| Owner stopped the run | `stopped` |
| A trust boundary refused | `terminal_refusal` |
| The operator's limit wait is off and a turn met a usage limit | `usage_limit` |

With merging off, the run does not end at the readiness assessment. It carries
on with the task's remaining steps, then posts a comment saying the PR is ready,
merging is off, and who decides the merge. Nothing ends a run for slow progress,
number of rounds or elapsed time.

## Minor review findings

A Crusty round sorts its findings by severity (`autodrive_followups.sh
classify`). The reading lists each round's minor findings, and the agent files
each as its own issue with the `follow-up` label before it merges.

#### Cell sanitising

Finding identifiers are exact stable tokens. The shared finding rules
(`autodrive_finding_rules.sh`) remove control characters from display cells and
escape HTML or table delimiters. Summaries are data; they cannot create new
records or change a finding's identifier or severity.

## Crusty round fix evidence

In a round that ended with concerns, `step-05-verify-concerns-addressed` runs
`autodrive_crusty_concerns_evidence.sh` to measure the fix (#2519). It enters
`repo_path`, the PR's linked worktree where step-04 commits, clears the
repository-selecting `GIT_*` variables through
[`git-repo-env.sh`](git-repo-env-helper.md) and runs
`autodrive_worktree_guard.sh commit-dir <pr>`. It never reads `REPO_PATH` and
never measures in the folder it started in.

The base is `reviewed_head_sha`, the commit the loop measured with
`git rev-parse` before the round started, as the round record uses it (#2814).
The round context's `head_sha`, which an agent writes, is read only when no
full lowercase commit id arrived as `reviewed_head_sha`; it must then be a hex
commit id, never a git expression such as `HEAD~1`. Either way the base must
resolve to a commit in that worktree. The step reports `base_sha`, `head_sha`
(the worktree's `HEAD`), `commits`, `dirty_worktree`, `git_observed` and
`head_descends_from_base`. The last is `"false"` when `HEAD` no longer
descends from the base. `commits` counts the commits in `HEAD` that are not
reachable from the base, which is 0 after a plain rewind.

When it cannot measure, it exits 1 and prints nothing on stdout, rather than an
unmeasured `commits: 0`. On stderr it writes a `step05_refusal=<reason>` line
and a `STOP: step-05` line that gives `repo_path` and the start folder.
Warnings take the same form, `step05_warning=<reason>`. The reason codes are
the stable interface, and tests assert on them; the prose beside them may be
reworded.

| Marker | Meaning |
| --- | --- |
| `step05_refusal=no-repo-path` | `repo_path` is empty |
| `step05_refusal=no-pr-number` | `pr_number` is missing or not numeric |
| `step05_refusal=repo-path-unreachable` | `repo_path` cannot be entered |
| `step05_refusal=helper-missing` | `git-repo-env.sh` could not be loaded |
| `step05_refusal=not-a-git-repo` | `repo_path` is not a git work tree |
| `step05_refusal=not-pr-worktree` | the worktree guard refused `repo_path` |
| `step05_refusal=no-usable-base` | no `reviewed_head_sha`, and the context `head_sha` is not hex |
| `step05_refusal=base-not-a-commit` | the base does not resolve to a commit in `repo_path` |
| `step05_refusal=head-unreadable` | `HEAD` cannot be read |
| `step05_refusal=count-failed` | `git rev-list --count` failed |
| `step05_refusal=ancestry-unreadable` | `git merge-base --is-ancestor` failed |
| `step05_refusal=worktree-state-unreadable` | `git diff --quiet` failed |
| `step05_warning=no-commit` | `commits` is 0 |
| `step05_warning=head-not-descendant` | `head_descends_from_base` is `"false"` |

The failed step fails the round, and a failed
round does not end the run. The goal loop's `review` action
(`do_act_review` in `autodrive_pr_goal.sh`) records the turn as
`review-failed`, with the round's whole output, the `STOP` line included, in
the turn folder's `review.log`. The next turn's agent reads that result as
`previous` and judges the cause (see
[Failures inside a turn](autodrive-goal-loop.md#failures-inside-a-turn)). The
test `step05_stop_reaches_the_next_turn_as_a_failed_review` in
`crates/amplihack-cli/tests/issue_2519_crusty_step05_worktree.rs` runs that
path with the real script.

## Retired inputs

The `autodrive_budget_*` keys are still accepted, so older launch commands keep
working. They have no effect and the run log says so once. The loop-health STUCK
stop, the no-progress stop, `checks-pending` and the automatic base merge are
removed. See [Removed behaviour](./autodrive-goal-loop.md#removed-behaviour).

## How to

Drive a pull request to merge:

```bash
amplihack recipe run auto-drive-to-merge \
  -c pr_number=3129 \
  -c task_description="Merge once CI passes at the head."
```

Verify without merging:

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=3129 -c no_merge=true
```

Send guidance while it runs. The next turn reads it:

```bash
echo "Hold: wait for the owner's answer on #3109." \
  | amplihack recipe guide https://github.com/rysweet/amplihack-rs/pull/3129 -
amplihack recipe guidance show --pr 3129 --pr-repo rysweet/amplihack-rs
```

Stop it gracefully: send guidance asking it to stop (the agent commits, pushes
and ends with a continuation note), or run `amplihack recipe stop <session>`.

Read what each turn chose and why: `<autodrive_state_dir>/turns/NNN/record.md`.

## Related

- [Auto-drive Goal Loop](./autodrive-goal-loop.md)
- [Auto-Drive-Light Recipe](./auto-drive-light.md)
- [Merge-ready Round (retired)](./autodrive-merge-ready-round.md)
- [Crusty Completion (retired)](./autodrive-crusty-completion.md)
