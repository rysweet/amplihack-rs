---
name: auto-drive-to-merge
title: Auto Drive To Merge Skill
owner: workflow-team
review_schedule: quarterly
description: goal-driven agent loop that gets a pull request merged under the owner's conditions. Each turn reads live state and new guidance, then picks one action; nothing in code ends a run for slow progress.
version: 2.0.0
allowed-tools: ["Read", "Grep", "Glob", "Bash", "Agent", "Edit", "Write"]
user-invocable: true
last_updated: 2026-11-23
doc_type: reference
auto-activation:
  priority: 4
  keywords: ["auto drive to merge", "drive to merge", "drive it to merge", "take it to merge", "land this pr", "all the way to merge", "until crusty is happy", "until it merges"]
---

# Auto Drive To Merge

The goal is to get the pull request merged under the owner's conditions. The
full specification is [Auto-drive Goal Loop](../../../docs/reference/autodrive-goal-loop.md).

Each turn you get a fresh reading: the PR's head, base and merge state, each
check, the review verdicts, whether the base conflicts or changed the PR's
files, the repository's settings, the task text and guidance. You do any
work the turn needs yourself (fix and push, bring the base in, install a host
input, carry out a later task step), then pick one action: the goal's
`review` or `merge`, or a shared `continue`, `wait-owner`, `handoff`,
`ready` or `stop`. End your reply with one JSON line:
`{"action": "<word>", "args": {...}, "why": "..."}`.

## Rules

- Read guidance at the start of every turn and before any action expected to
  take more than about five minutes. Later guidance governs over the task text.
  Guidance can narrow what you do, never widen merge authority.
- Bring the base in only when the PR conflicts, the task text asks, or the
  repository requires an up-to-date branch. Use a merge commit. Never rebase,
  amend, squash or force-push. Push with plain `git push`, through the hooks.
- Use the round's one shared CI observation from the reading. Agents make no
  extra status queries, watches, polling loops or timed waits. Pending CI
  stays unresolved: finish actionable work and required outputs, then choose
  `continue` for the next reading. Do not choose the legacy `wait` action,
  which starts a CI watcher. Missing or stale CI is unverified. New commits,
  base merges and reruns need evidence for their head and execution.
- Install a missing host input by the repository's own instructions, from
  `origin/<base>`. No sudo, no system package manager, never touch `~/.claude`
  or `~/.amplihack`.
- Merge authority comes only from `no_merge` and the captured invocation. Only
  the loop's merge action merges, with `--match-head-commit`.
- With merging off, carry out the task's later steps, then take `ready`.
- Hand off only for a decision that belongs to a named owner, or for a failure
  that keeps repeating that nothing you can do will change, saying which owner
  and what decision. Write it in the journalistic style with literal words.
- When the owner asks the run to stop, commit and push, then take `stop` with
  the continuation note.

## Invocation

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=3129
amplihack recipe run auto-drive-to-merge -c pr_number=3129 -c no_merge=true
```

The `autodrive_budget_*` keys are accepted and ignored.

`autodrive_minor_findings` sets how minor review findings are treated:
`fix_all` (the default), `judge` or `file_all`. Any other value runs as
`fix_all`, and the reading reports it. See [Minor review findings](../../../docs/reference/autodrive-goal-loop.md#minor-review-findings).

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=3129 -c autodrive_minor_findings=judge
```
