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
