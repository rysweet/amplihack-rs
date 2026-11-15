---
title: Read a Drive That Waited on a GitHub Read
description: Tell an unreadable GitHub read apart from a moved head or a failing check in an auto-drive run, and decide what to do next.
last_updated: 2026-11-15
review_schedule: quarterly
owner: workflow-team
doc_type: howto
---

# Read a Drive That Waited on a GitHub Read

> [Home](../index.md) > [Auto Drive To Merge](../reference/auto-drive-to-merge.md) > Waiting on a GitHub read

A drive can stop short of merging because GitHub did not answer a read, not
because anything is wrong with the pull request. This guide shows how to tell
the two apart from the run's own output, and what each case asks of you.

## Find the dispatch line

Each Crusty round prints one JSON line. Read its `head_read` field:

```sh
grep -o '{"loop":"crusty"[^}]*}' <run log> | tail -1
```

| `head_read` | What happened | What to do |
| --- | --- | --- |
| `matches` | The PR's head on GitHub is the head that was reviewed. | Nothing. The round completed. |
| absent, with a dispatch error naming two commits | The head moved after the review. | Nothing. The next round reviews the new head. |
| `unavailable:<token>` | The head read produced no usable answer. | Nothing is wrong with the pull request. For `read-failed`, run the drive again. For the other tokens, clear the configuration fault named below first. |

The value is always `matches` or `unavailable:<token>`, with `<token>` one of
`read-failed`, `origin-not-github`, `pr-number-unresolved`, `ambiguous-ref`
and `bad-sha`, and nothing else: no exit status and no message text. Only
`read-failed` is a platform fault. `origin-not-github` means the worktree's
`origin` is not a GitHub remote in a form the read accepts, and
`pr-number-unresolved`, `ambiguous-ref` and `bad-sha` are local or repository
configuration faults, which another drive run will not clear on its own. The
full grammar is in
[The grammar of the two fields](../reference/autodrive-crusty-completion.md#the-grammar-of-the-two-fields).

An unavailable read does not end the round as `INVALID_DECISION`, and does
not stop the merge loop with `crusty-rereview-not-done`. The round goes to
the loop-health evaluator with the fault stated in its context, which
normally continues to another round.

## Find the merge step's result

The merge step prints one JSON line with a `result` and a `reason`:

```sh
jq -r '"\(.result): \(.reason)"' <state folder>/finalize-merge.json
```

The result is one of nine values. Read the `reason` with it: two of them can
follow either a decision GitHub made or a read that did not answer, and only
the reason tells you which.

| `result` | Meaning | What to do |
| --- | --- | --- |
| `merged` | GitHub reads the PR merged at the expected head. | Nothing. |
| `checks-pending` | A check is still running, **or** a read did not answer. The reason says which. | Start a drive again once the checks have finished or the allowance window has reset. |
| `checks-not-green` | A check failed. The reason names the failure, not a read. | Fix the failing check. |
| `not-open` | GitHub answered, and the PR was not open. A read that did not answer never gives this. | Reopen the PR, or accept that it is closed or already merged. |
| `base-behind` | The branch is behind or conflicts with its base. | The merge loop merges the base in and assesses once more; after a recovery re-run of finalisation it hands off instead, and you merge the base yourself. |
| `message-unavailable` | The PR title or body could not be read into a squash message. | Check the PR's title and body, then start a drive again. |
| `refused` | Either GitHub declined the merge request, or the merge request returned and no route could read the PR back. The reason distinguishes them. The finalize step also emits it before any merge request when its own guards refuse: the wrong PR, no recorded assessed head, or a worktree head that moved after the assessment. | After a read fault, read the PR yourself: the merge may have completed. Start a drive again either way, which observes the merge if it happened and makes no second merge request. After a decline, clear what GitHub named. |
| `not-authorised` | The expected end of a verify-only run (`no_merge` or a dry run). The PR stays open. | Nothing. |
| `authorization-unreadable` | A merge run could not read its captured authorization. The PR did not merge and is handed off. | Start the drive again from an invocation that captures its authorization. |

`checks-pending`, `refused`, `message-unavailable` and `authorization-unreadable`
hand the PR off: it becomes a draft, and the hand-off comment names what it is
waiting for.

## What not to do

- Do not add a wait, a retry or a polling loop. Nothing in auto-drive sleeps
  on a GitHub read, by design: the drive ends and the next run reads again.
- Do not treat `unavailable` as a finding against the work. It means the read
  produced no answer, not that the review found something. A token other than
  `read-failed` repeating across runs is a configuration fault to clear, not a
  reason to stop the work either.
- Do not read the head with `gh pr view --json headRefOid` when reproducing a
  case by hand. That is a GraphQL call, and it is the read that fails while
  REST still answers. Use `git ls-remote origin refs/pull/<n>/head` or
  `gh api repos/{owner}/{repo}/pulls/<n> --jq .head.sha`.

## Confirm the allowance, if you want to know why

```sh
gh api rate_limit --jq '.resources.graphql, .resources.core'
```

A `remaining` of `0` with a `reset` in the future on `graphql`, while `core`
still has budget, is the condition this behaviour exists for.

## Related

- [Head check outcomes](../reference/autodrive-crusty-completion.md#head-check-outcomes)
- [Auto-drive REST Reads](../reference/autodrive-rest-reads.md)
- [Merge-ready Round, Merge and Hand-off](../reference/autodrive-merge-ready-round.md)
