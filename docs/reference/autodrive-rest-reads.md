---
title: Auto-drive REST Reads
description: autodrive_rest.sh and the merge path's REST routes for the PR state and its checks, with the rules that keep an unanswered read a wait rather than a failed criterion.
last_updated: 2026-11-15
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive REST Reads

> [Home](../index.md) > [Auto Drive To Merge](auto-drive-to-merge.md) > REST Reads

GitHub counts GraphQL calls against a different allowance from REST calls,
and its secondary limit stops answering `gh pr view` and `gh pr checks` while
`gh api repos/…` still answers. Auto-drive reads the facts that decide a
round through a route that does not depend on the GraphQL allowance, and
treats a read that nothing answered as waiting rather than as a failed
criterion (#2333).

## Contents

- [What reads what](#what-reads-what)
- [`autodrive_rest.sh`](#autodrive_restsh)
- [The PR read on the merge path](#the-pr-read-on-the-merge-path)
  - [The read-back after a merge request returns](#the-read-back-after-a-merge-request-returns)
- [The checks read on the merge path](#the-checks-read-on-the-merge-path)
- [Scope parity between GraphQL and REST](#scope-parity-between-graphql-and-rest)
- [Waiting is not a failed criterion](#waiting-is-not-a-failed-criterion)
- [Out of scope](#out-of-scope)

## What reads what

| Read | Transport | Where it is documented |
| --- | --- | --- |
| The PR's head, in the Crusty completion dispatch | `git ls-remote`, then `gh api repos/{owner}/{repo}/pulls/{n}` | [Head check outcomes](autodrive-crusty-completion.md#head-check-outcomes) |
| The PR's state, title, body and merge state, in the merge step | `gh pr view`, then `GET /repos/{owner}/{repo}/pulls/{n}` | [The PR read on the merge path](#the-pr-read-on-the-merge-path) |
| The checks at the head, in the merge step | `gh pr checks`, then check-runs and combined status | [The checks read on the merge path](#the-checks-read-on-the-merge-path) |
| Expected checks at the head | `gh api` only, already REST | [Expected checks at the head](autodrive-expected-checks.md) |

The merge mutation itself, `gh pr merge --squash --match-head-commit`, is
unchanged. It is a GraphQL mutation, and no REST route replaces it: every
recorded fault was a failed read, and a merge request that does not complete
because the platform did not answer is recorded as waiting.

## `autodrive_rest.sh`

`amplifier-bundle/tools/autodrive_rest.sh` defines two functions and nothing
else. Sourcing it has no other effect: it sets no global `umask`, exports no
variable and runs no command.

```bash
rest_get <out> <paginate 0|1> <endpoint>
rest_json <out> <paginate 0|1> <endpoint> <jq filter over the pages>
```

`rest_get` writes the answer's pages to `<out>` as one JSON array. `rest_json`
applies the filter to that array and writes the result to `<out>`. The caller
supplies `<out>`; neither function picks a path of its own.

| Exit | Meaning |
| --- | --- |
| `0` | The endpoint answered and the result was written. |
| `1` | The call failed for any other reason, or the answer did not parse. |
| `4` | HTTP 404, read from `gh`'s standard error only. |
| `79` | `gh` exited `79`; passed through unchanged, and the drive still stops on it. |

The shape follows the `get`/`read_into` pair already proven in
`autodrive_expected_checks.sh`; that file is left alone, because rewriting it
would change which checks condition the merge. The duplication is about a
dozen lines and is recorded in the pull request rather than resolved here.

`<endpoint>` must match a closed form, `repos/{owner}/{repo}/…` with an
optional query string, and returns exit `1` when it contains `://`, a
leading `/`, a leading `-`, an `@` before the first `/`, `..`, whitespace or
control characters. Without that rule, an endpoint of the caller's choosing
would send the `Authorization` header to a host of the caller's choosing.

`rest_get` writes `<out>.raw` and `<out>.err` beside the path it was given. It
requires that path to sit in a private directory (`mktemp -d`, `umask 077`),
does not accept an `<out>` that is already a symbolic link, and removes both side
files before returning.

## The PR read on the merge path

`_merge_pr_json` in `autodrive_merge.sh` serves the three places that read the
PR: once before the merge request, once after GitHub declines one, and once
after a merge request returns. It tries `gh pr view` first. On any failure
other than exit `79` it reads `GET /repos/{owner}/{repo}/pulls/{n}` and
reshapes the answer with `jq` into the same field names, so
`_autodrive_merge_at_head`, `autodrive_merge_subject` and
`autodrive_merge_body` are unchanged:

| Field | REST source |
| --- | --- |
| `state` | `.state` and `.merged`, as `OPEN`, `CLOSED` or `MERGED` |
| `headRefOid` | `.head.sha` |
| `title` | `.title` |
| `body` | `.body` |
| `mergeStateStatus` | `.mergeable_state`, upper-cased |

The three calls do not share one outcome, because they are asked after
different things have happened.

| Where | Neither route answers |
| --- | --- |
| Before the merge request | **Waiting**, not `not-open`: the step returns `checks-pending` (tool exit `7`) with a reason naming the unreadable read. Before #2333 the same case read as "the PR state read before the merge was 'unreadable'" and ended the drive as `not-open`, which drafts the PR and reports a condition that was never observed. |
| After GitHub declines a merge request | `refused` (tool exit `4`), as before. The decline is an observation of the platform's answer, not a read fault, and the read-back only chooses between `base-behind` and `refused`. |
| After a merge request returns | `refused` (tool exit `4`), as before, with a reason naming the unreadable read-back rather than a state. |

### The read-back after a merge request returns

`gh pr merge` returned success and then no route could read the PR back, so
the merge is unverified: it may have completed. The result stays `refused`,
with hand-off, and the reason says that the read-back was unreadable. It no
longer reads "state 'unreadable'", which named a state the run never
observed.

`refused` is kept here rather than replaced because the alternatives assert
more than the run saw. `merged` would claim a merge no route confirmed.
`checks-pending` would name checks as the blocker when the checks were green
and the merge request was made. `refused` with hand-off is the only existing
result that stops the run without asserting the PR's state, and it is correct
as a statement about this run: this run did not observe a merge.

A merge that did complete is then observed by the next run rather than
re-attempted blindly. The read before the merge request goes through the same
two routes, finds the PR `MERGED` at the expected head and emits `merged` with
no second merge request; and `--match-head-commit` keeps any second request
from merging a head other than the assessed one.

## The checks read on the merge path

The existing `gh pr checks <pr> --required` read, its fallback to all checks,
and its reading of `no required checks reported` from standard error are
unchanged. `_merge_checks_rest` is called only from the arm that previously
turned an unreadable read into `checks-not-green`. It reads two endpoints at
the head SHA:

- `GET /repos/{owner}/{repo}/commits/{sha}/check-runs?per_page=100&filter=all`
- `GET /repos/{owner}/{repo}/commits/{sha}/status`

| Observation | Result |
| --- | --- |
| Either route reports a check queued, in progress or pending | `checks-pending` (tool exit `7`); no merge request. Pending is tested before failing, so a suite still running is never reported as a failure. |
| Nothing pending, and either route reports a failure | `checks-not-green` (tool exit `3`); no merge request. |
| Both routes answered, nothing pending and nothing failing | The merge request goes ahead, and GitHub's branch protection decides. |
| Either route did not answer | `checks-pending` (tool exit `7`). An unreadable check is not an absent check. |

Both routes must have answered before the merge request is reached. One route
answering can only move the outcome towards waiting or towards not-green, so
the pending and failing branches are safe with a single answer; the
fall-through to the merge request is not.

A 404 on either endpoint is unreadable, never an empty list, and therefore
never "nothing is failing". A fine-grained token without the checks
permission is answered with 404 where `gh pr checks` succeeded.

Reasons carry no check names from this path. The existing
`gh pr checks` wait message keeps its own treatment, which removes control
characters and cuts the text at 300 bytes.

## Scope parity between GraphQL and REST

GraphQL and REST do not see the same thing for every token, and the
difference is not always an error:

- A token may read a PR through one and not the other.
- The combined status endpoint reports commit statuses; the check-runs
  endpoint reports check runs. A repository may use either or both.
- `mergeable_state` is computed, and GitHub may answer `unknown` while it
  computes. The merge step reads it twice, and neither comparison turns an
  uncomputed value into a refusal of its own. Before the merge request it
  tests for `DIRTY` alone, so `unknown` falls through and the merge request
  itself decides. After GitHub has declined a merge request it acts on
  `BEHIND` and `DIRTY`, and everything else, `unknown` and an unread value
  included, is `refused`: the decline has already been observed there, so
  that arm reports the decline rather than a wait.

Where the two could disagree, the REST route is only consulted after the
GraphQL route has failed. Its unreadable outcome is waiting everywhere the
step has not yet made a merge request; after one has been made, it is
`refused` ([The read-back after a merge request returns](#the-read-back-after-a-merge-request-returns)).

## Waiting is not a failed criterion

A round that produces no commit because it is waiting on an external system,
whether a platform allowance window or a check still running, is recorded as
waiting. The no-progress rule in `autodrive_progress.sh` is unchanged, and
the merge-ready loop remains exempt from it. What changed is narrower: an
unavailable read and an external wait are not counted as an absence of
progress, and nothing sleeps, retries or polls to make that so. The next
drive run reads again.

## Out of scope

- The merge mutation's own transport.
- `autodrive_expected_checks.sh`, which already reads through REST.
- Which checks condition the merge, and reconstructing branch protection or
  rulesets from REST.
- Any backoff, retry schedule or sleep-based mitigation.
- `autodrive_pr_head.sh`, which reads the head through `gh pr view` and ends
  `pr-lookup-failed` when that read fails. It has the same fault and is not
  on the merge path; it is named in the pull request for a follow-up issue.
