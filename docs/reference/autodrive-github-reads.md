---
title: Auto-drive GitHub Reads and Waits
description: How auto-drive reads PR facts when GraphQL is refused, how a failed read differs from a finding, and how waits end a round as WAITING instead of STUCK.
doc_type: reference
last_updated: 2026-11-12
review_schedule: quarterly
owner: workflow-team
---

# Auto-drive GitHub Reads and Waits

> [Home](../index.md) > [Auto Drive To Merge](auto-drive-to-merge.md) > GitHub Reads and Waits

Auto-drive reads pull request facts (head, state, merge state, checks,
reviews) on every round. GitHub limits GraphQL calls separately from REST
calls, and its secondary limit refuses `gh pr view` and `gh pr checks` while
`gh api repos/…` still answers. Before issue #2333 a refused GraphQL read
was reported as a finding, such as `crusty-pr-head-unreadable` or a head that
"differs", and the round counted toward `STUCK`.

Now every read goes through one Rust helper that tries GraphQL, then REST,
then (for the head only) `git ls-remote`. A read that fails on every path is
reported as **unavailable**, which is a **wait**, never a finding. A round
whose only blockers are waits ends with the resumable result `WAITING`
(issue #2472, waits slice).

## Contents

- [The read cascade](#the-read-cascade)
- [`amplihack orch helper autodrive-gh-read`](#amplihack-orch-helper-autodrive-gh-read)
- [Output format](#output-format)
- [Exit codes](#exit-codes)
- [How REST values map to GraphQL values](#how-rest-values-map-to-graphql-values)
- [Rate limits and the reset time](#rate-limits-and-the-reset-time)
- [Which reads use the helper](#which-reads-use-the-helper)
- [Waits and fixable findings](#waits-and-fixable-findings)
- [`amplihack orch helper autodrive-round-waits`](#amplihack-orch-helper-autodrive-round-waits)
- [The WAITING result](#the-waiting-result)
- [Recomputing an UNKNOWN merge state](#recomputing-an-unknown-merge-state)
- [`amplihack orch helper autodrive-gh-update-branch`](#amplihack-orch-helper-autodrive-gh-update-branch)
- [Security rules](#security-rules)
- [Tests](#tests)

## The read cascade

Each read tries these sources in order and stops at the first that answers:

| Order | Source | Used for |
| --- | --- | --- |
| 1 | GraphQL: `gh pr view --json …`, `gh pr checks` | every read |
| 2 | REST: `gh api repos/{owner}/{repo}/pulls/{n}`, `…/commits/{sha}/check-runs`, `…/commits/{sha}/status`, `…/pulls/{n}/reviews`, `…/pulls?head={owner}:{branch}&state=open` | every read |
| 3 | `git ls-remote origin refs/pull/{n}/head` | `head` only |

The fallback runs on **any** GraphQL failure, not only on a recognised
rate-limit message. A GraphQL answer that fails validation (for example a
head SHA that is not 40 hex characters) also falls through.

Review threads have no REST path. When the GraphQL thread read fails,
`unresolved_threads` is `"unavailable"`, which is a wait.

Nothing in the cascade sleeps, retries on a timer or polls. Each source is
tried once per read.

## `amplihack orch helper autodrive-gh-read`

```bash
amplihack orch helper autodrive-gh-read pr 2333
amplihack orch helper autodrive-gh-read head 2333
amplihack orch helper autodrive-gh-read checks 2333
amplihack orch helper autodrive-gh-read checks 3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d
amplihack orch helper autodrive-gh-read reviews 2333 --required 1
amplihack orch helper autodrive-gh-read pr-for-branch feat/issue-2333-fix-amplihack-rs-issue
```

| Subcommand | Argument | Returns |
| --- | --- | --- |
| `pr <n>` | PR number | `state`, `is_draft`, `head_sha`, `head_ref`, `base_ref`, `merge_state`, `mergeable`, `merged_at` |
| `head <n>` | PR number | `head_sha` (and `head_ref` when the source has it) |
| `checks <n\|sha>` | PR number or a 40-hex commit SHA | `head_sha` and `checks`: a list of `{name, bucket}`; `bucket` is `pass`, `fail`, `pending` or `skipping` |
| `reviews <n>` | PR number; optional `--required <count>` (default `1`) | `review_decision`, `approvals`, `changes_requested`, `unresolved_threads` |
| `pr-for-branch <branch>` | branch name | `number` of the open PR for the branch, or `null` when there is none |

Every subcommand takes an optional `--repo <owner>/<repo>`. Without it the
repository comes from the working directory's `origin`, as `gh` resolves it.
`GH_REPO` from the environment is ignored.

The bash tools call this subcommand. The completion dispatch calls the same
library functions directly (`autodrive_gh::head`, `autodrive_gh::pr_for_branch`).

## Output format

The helper prints exactly one JSON line on stdout. Diagnostics go to stderr.

A successful read names the source that answered:

```json
{"read":"pr","pr":2333,"source":"rest","state":"OPEN","is_draft":false,"head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","head_ref":"feat/issue-2333-fix-amplihack-rs-issue","base_ref":"main","merge_state":"BLOCKED","mergeable":"MERGEABLE","merged_at":null}
```

```json
{"read":"head","pr":2333,"source":"ls-remote","head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"}
```

A read that failed on every path carries `unavailable` and no values:

```json
{"read":"head","pr":2333,"source":null,"unavailable":{"reason":"graphql: You have exceeded a secondary rate limit; rest: You have exceeded a secondary rate limit; ls-remote: exit 128","rate_limited":true,"reset_epoch":1794528000}}
```

| Field | Meaning |
| --- | --- |
| `source` | `graphql`, `rest`, `ls-remote`, or `null` when unavailable. |
| `unavailable.reason` | One short line per path tried. Each excerpt is the first stderr line, at most 200 bytes, printable ASCII only, with token values redacted. |
| `unavailable.rate_limited` | `true` when any path was refused by a rate limit. |
| `unavailable.reset_epoch` | Unix time GitHub gave for the limit's reset, or `null` when unknown. See [Rate limits and the reset time](#rate-limits-and-the-reset-time). |

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | A source answered. The JSON holds the values. |
| `3` | Unavailable on every path. The JSON holds `unavailable`. This is a wait, never a finding. |
| `2` | Usage error: unknown subcommand, a PR number that is not `1`–`9999999999`, a SHA that is not 40 hex characters, or a branch name that fails the branch rules. Nothing is run. |

`autodrive-gh-read pr-for-branch` exits `0` with `"number":null` when the
branch has no open PR. That is an answer, not an unavailable read.

## How REST values map to GraphQL values

REST answers are converted to the GraphQL spelling the tools already read,
by a pure function (`autodrive_gh::rest_map`). A REST read and a GraphQL read
of the same PR give equal output apart from `source`.

**Head, base and state**

| REST | GraphQL field | Value |
| --- | --- | --- |
| `head.sha` | `headRefOid` | unchanged |
| `head.ref` | `headRefName` | unchanged |
| `base.ref` | `baseRefName` | unchanged |
| `merged_at` not null | `state` | `MERGED` |
| `state: closed` | `state` | `CLOSED` |
| `state: open` | `state` | `OPEN` |
| `draft` | `isDraft` | unchanged |

**Mergeable**

| REST `mergeable` | GraphQL `mergeable` |
| --- | --- |
| `true` | `MERGEABLE` |
| `false` | `CONFLICTING` |
| `null` | `UNKNOWN` |

**Merge state**

| REST `mergeable_state` | GraphQL `mergeStateStatus` |
| --- | --- |
| `clean` | `CLEAN` |
| `dirty` | `DIRTY` |
| `behind` | `BEHIND` |
| `blocked` | `BLOCKED` |
| `unstable` | `UNSTABLE` |
| `has_hooks` | `HAS_HOOKS` |
| `draft` | `DRAFT` |
| `unknown` or any other value | `UNKNOWN` |

**Review decision**, from `pulls/{n}/reviews`. The latest `APPROVED`,
`CHANGES_REQUESTED` or `DISMISSED` review per user counts; `COMMENTED`
reviews are ignored.

| Condition | `review_decision` |
| --- | --- |
| Any user's latest review is `CHANGES_REQUESTED` | `CHANGES_REQUESTED` |
| Otherwise, approvals ≥ `--required` | `APPROVED` |
| Otherwise | `REVIEW_REQUIRED` |

**Checks**, from `commits/{sha}/check-runs` plus the legacy contexts in
`commits/{sha}/status`. Results are read up to 10 pages and 8 MiB; a larger
answer is unavailable, not truncated.

| Check run | Bucket |
| --- | --- |
| `status` `queued` or `in_progress` | `pending` |
| `conclusion` `success` or `neutral` | `pass` |
| `conclusion` `skipped` | `skipping` |
| any other conclusion (`failure`, `cancelled`, `timed_out`, `action_required`, `stale`) | `fail` |
| status context `pending` | `pending` |
| status context `success` | `pass` |
| status context `failure` or `error` | `fail` |

## Rate limits and the reset time

Each failed path's stderr is classed by the same rules, in the same order,
as `classify_gh_error` in `amplifier-bundle/tools/workflow_gh_retry.sh`. All
matches ignore case. The first rule that matches wins:

| Order | Class | Stderr matches |
| --- | --- | --- |
| 1 | auth | `bad credentials`, `HTTP 401`, a bare `401`, `unauthorized`, `authentication failed`, `requires authentication`, `gh auth login`, `not logged in`, `token … expired` or `token … invalid`, `invalid … token` |
| 2 | rate limit | `api rate limit exceeded`, `rate limit already exceeded`, `secondary rate limit`, `abuse detection`, `retry-after`, `x-ratelimit-reset`, `rate limit`, `HTTP 429`, a bare `429` |
| 3 | rate limit | `HTTP 403` or a bare `403`, **unless** it also contains `resource not accessible`, `must have admin`, `not authorized`, `insufficient` or `permission` (then it is class "other") |
| 4 | transient | `HTTP 5xx`, a bare `500`, `502`, `503` or `504`, `timed out`, `timeout`, `temporar`, `connection reset`, `connection refused`, `TLS handshake`, `network`, `server error`, `i/o timeout`, `no such host`, `EOF` |
| 5 | other | anything else |

A "bare" code is the number with no digit directly before or after it.
`was submitted too quickly` matches none of these rules, so on its own it is
not a rate limit. `unavailable.rate_limited` is `true` only when some path's
class was "rate limit". Every class still moves the read on to the next path;
the class only decides `rate_limited` and the reset lookup.

The Rust list is a copy, not a call into the shell script. A unit test in
`autodrive_gh.rs` runs both over the same stderr samples, one per rule above
plus `was submitted too quickly`, and fails if they disagree.

When a read is unavailable and any path was rate-limited, `reset_epoch` comes
from, in order:

1. a `Retry-After` (seconds, added to now) or `X-RateLimit-Reset` (epoch)
   value in the stderr or headers of the refused call;
2. `gh api rate_limit`: `.resources.graphql.reset` when GraphQL was refused,
   `.resources.core.reset` when REST was refused;
3. `null`.

A value more than one day before or after now becomes `null`. Only the
numeric value is kept; no other header text is stored.

A `null` reset gives `wait_target` `ratelimit-reset@unknown` and
`wait_reason` `rate-limited-reset-unknown`. Nothing sleeps until the reset.
The loop ends `WAITING` and names the reset time, and the next drive run
resumes from there.

## Which reads use the helper

| Read | Where | Subcommand | When every path fails |
| --- | --- | --- | --- |
| PR number for a branch | `autodrive_platform_facts.sh`, completion dispatch | `pr-for-branch` | `read@pr-number` wait |
| State, merge state, mergeable, base | `autodrive_platform_facts.sh`, `autodrive_round_evidence.sh`, `autodrive_state.sh` | `pr` | `read@pr` wait; `autodrive_state.sh` reports `UNKNOWN`, never "not merged" |
| Checks at the head | `autodrive_platform_facts.sh`, `autodrive-merge-evidence.yaml` step-03 | `checks` | `read@checks` wait |
| Reviews and approvals | `autodrive_platform_facts.sh` | `reviews` | `approval-facts-unreadable` wait |
| Review threads | `autodrive_platform_facts.sh` | GraphQL only | `unresolved_threads: "unavailable"`, `read@threads` wait |
| PR head | `autodrive_pr_head.sh`, `autodrive-crusty-round.yaml` step-01, completion dispatch | `head` | `crusty-pr-head-unreadable` wait; dispatch `head-unavailable` |
| Base branch | `autodrive-merge-evidence.yaml` step-01 | `pr` | `merge_sync.reason: base-unavailable`; no merge from a guessed `main` |
| Gate re-reads | `autodrive_merge_gate.sh` | none: still `gh pr view` | The gate **refuses** to merge, as before. Its reads need title, body and URL, which the REST mapping does not carry, so they are unchanged. |

`autodrive_pr_head.sh` exits `4` only when every path is unavailable, and
prints the helper's reason. `gh` state values from REST are upper-cased
before use.

`autodrive-merge-evidence.yaml` step-03 reads CI once. Pending or queued
checks at the head end the step with exit `0` and record the wait; the
former `while :; … sleep 60` loop is gone.

The merge gate's logic is unchanged. An unavailable read is never treated as
satisfied, so the gate blocks while any read is unavailable.

## Waits and fixable findings

Each round's blockers are classed in code by `autodrive-round-waits`. Only
fixable findings count toward `STUCK`.

| Class | Blocker |
| --- | --- |
| **Wait** | CI `PENDING` or `QUEUED` at the head |
| **Wait** | Any read unavailable or rate-limited: `approval-facts-unreadable`, `expected-checks-unreadable`, threads unavailable, head unavailable (`crusty-pr-head-unreadable`), PR or base unavailable |
| **Wait** | `merge_state` or `mergeable` `UNKNOWN` |
| **Fixable** | Failing checks |
| **Fixable** | Conflicts (`DIRTY`, `CONFLICTING`) |
| **Fixable** | Crusty concerns |
| **Fixable** | `CHANGES_REQUESTED` |
| **Fixable** | Unresolved review threads |
| **Fixable** | Required checks `MISSING` at the head |
| **Fixable** | QA failures |
| **Fixable** | Any id the classifier does not recognise |

Waits come from measured facts in the round evidence. A finding id written
by an agent becomes a wait only when its whole id ends with one of the fixed
wait ids above; anything else stays fixable. Errors in classing therefore
lean toward `STUCK`, never toward a silent wait.

### The measured-evidence downgrade

`round_findings` adds the id `measured-evidence-disagrees` whenever
`measured_downgrade` in `autodrive_round_evidence.sh` returns a non-empty
list. The list is space-separated `key=value` tokens, one per measurement
that is not at its passing value. The classifier drops
`measured-evidence-disagrees` only when every token is a wait token, after
the neutral tokens are removed. When that leaves no tokens, or the list was
empty, the id stays fixable.

| Token | Class | Why |
| --- | --- | --- |
| `ci_status=PENDING` | wait (`ci@<sha>`) | CI is still running at the head. |
| `ci_status=UNREADABLE` | wait (`read@checks`) | The checks read failed. |
| `ci_status=RED` | fixable | A check failed. |
| `ci_status=MISSING` | fixable | No CI evidence was written this round. |
| `conflict=unknown` | wait (`mergeability@<sha>`) | GitHub has not computed mergeability. |
| `conflict=true` | fixable | The branch conflicts with its base. |
| `unresolved_threads=unreadable` | wait (`read@threads`) | The thread read failed. |
| `unresolved_threads=<n>` | fixable | Threads are open. |
| `approval_status=UNREADABLE` | wait (`read@reviews`) only when the round's findings include `approval-facts-unreadable`; fixable otherwise | `UNREADABLE` also covers `approval-state-unreadable`, a `BLOCKED` state the facts cannot explain, which is not a failed read. |
| `approval_status=PENDING` | neutral: removed before the check | `autodrive_platform_facts.sh` gives `PENDING` only when another blocker (checks not passing, a conflict, `BEHIND`, open threads, an `UNKNOWN` merge state) hides the approval. That blocker has its own token, which decides. When it is the only token left, it is explained by a measured wait (CI `PENDING`/`QUEUED`, or merge state or mergeable `UNKNOWN`) when the round has one, and the id is dropped; otherwise it stays fixable. |
| `approval_status=NOT_MET`, `MISSING`, `OTHER` | fixable | |
| `qa_status=<any>` | fixable | QA did not pass or did not run. |
| `crusty_status=<any>` | fixable | Crusty did not return `DONE_CLEAN` for the head. |
| `qa_evidence=modified` | fixable | The QA evidence changed after step-00d hashed it. |
| `measured_evidence=unreadable` | fixable | Nothing could be compared. |
| any other token | fixable | |

For example, a round with CI still running at the head gives
`ci_status=PENDING approval_status=PENDING`. `approval_status=PENDING` is
removed, `ci_status=PENDING` is a wait, so `measured-evidence-disagrees` is
dropped and the round is wait-only. A round with
`ci_status=PENDING qa_status=FAIL` keeps the id as fixable.

The `ci_signal` input of `autodrive-round-waits` follows the same rule:
`PENDING` and `QUEUED` give `ci@<head_sha>`, `UNREADABLE` gives
`read@checks`, and every other value adds no wait.

`round_record` (in `autodrive_round_evidence.sh`) gains these fields:

| Field | Value |
| --- | --- |
| `merge_state` | The merge state read this round, or `UNKNOWN`. |
| `waits` | List of wait ids, for example `["ci-pending","approval-facts-unreadable"]`. |
| `wait_target` | What the round waits on; see the table below. Empty when there are no waits. |
| `round_waiting` | `true` when `waits` is non-empty and there are no fixable findings. |

`wait_target` names one event. When several waits are present, the first in
this order wins:

| Order | `wait_target` | Meaning |
| --- | --- | --- |
| 1 | `ratelimit-reset@<epoch>` or `ratelimit-reset@unknown` | GitHub's stated reset time for the limit. |
| 2 | `ci@<sha>` | CI completion at the head SHA. |
| 3 | `mergeability@<sha>` | GitHub computing the merge state at the head SHA. |
| 4 | `read@<what>` | A read that failed without a rate limit: `read@pr`, `read@pr-head`, `read@pr-number`, `read@checks`, `read@reviews`, `read@threads`. |

The `.findings` file that `autodrive_progress.sh` hashes holds **fixable
ids only**. The full classifier result is written next to it as `.waits`.
Both are written atomically with mode `0600`. If the classifier fails, every
id is treated as fixable and a `WARNING` line is printed.

A **mixed round** (waits and fixable findings) counts toward `STUCK` on its
fixable findings only. A failing check that did not change since the last
round still stops the loop as `STUCK`.

## `amplihack orch helper autodrive-round-waits`

Reads one JSON object on stdin and prints one JSON object. Pure: no `gh`,
no `git`, no files.

```bash
printf '%s' '{"head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","ci_signal":"PENDING","merge_state":"BLOCKED","mergeable":"MERGEABLE","unavailable":[],"findings":["ci-pending"],"measured_downgrade":[]}' \
  | amplihack orch helper autodrive-round-waits
```

```json
{"waits":["ci-pending"],"fixable":[],"wait_target":"ci@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","round_waiting":true}
```

| Input field | Meaning |
| --- | --- |
| `head_sha` | Head SHA measured this round. |
| `ci_signal` | `PASS`, `FAIL`, `PENDING`, `QUEUED`, `UNREADABLE` or empty. `PENDING` and `QUEUED` give `ci@<head_sha>`; `UNREADABLE` gives `read@checks`. |
| `merge_state`, `mergeable` | As read this round. |
| `unavailable` | List of `{read, rate_limited, reset_epoch}` from helper reads that exited `3`. |
| `findings` | All finding ids the round produced. |
| `measured_downgrade` | The `measured_downgrade` tokens as a list, for example `["ci_status=PENDING","approval_status=PENDING"]`. See [The measured-evidence downgrade](#the-measured-evidence-downgrade). |
| `recompute` | Optional. The step-01 recompute outcome, `{action, result, pushed_sha, rate_limited, reset_epoch}`, when one ran this round. It sets `wait_target` by the table in [Recomputing an UNKNOWN merge state](#recomputing-an-unknown-merge-state) when the round waits on mergeability and no read was rate-limited. |

Exit `0` on success, `2` on input that is not valid JSON or has the wrong
shape. Unknown input fields are refused with exit `2`, so a renamed field
cannot be silently ignored. A unit test in `autodrive_waits.rs` pins the
input and output field names above.

## The WAITING result

`WAITING` is a `loop_result` alongside `DONE`, `STUCK`, `USAGE_LIMIT`,
`BUDGET_EXHAUSTED` and `ORPHANED`. It is resumable and is not a failure.

```text
AUTO_DRIVE_LOOP: WAITING — loop 'merge-ready' stops at round label 'round-4'.
  Reason: every blocker this round is a wait (ci-pending). Resume the drive after ci@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d.
```

```json
{"loop":"merge-ready","loop_result":"WAITING","round_label":"round-4","reason":"every blocker this round is a wait (ci-pending)","wait_reason":"ci-pending","wait_target":"ci@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"}
```

How a loop reaches it:

1. After a round, `autodrive_loop.sh` calls `wait_stop` **before**
   `progress_stop`. `wait_stop` reads `round_waiting`, `wait_target` and
   `wait_reason` from the round record, or from the `${RECORD}.wait` sidecar
   when crusty step-01 could not read the head. When `round_waiting` is
   `true`, it calls `escalate WAITING` and the loop exits `0`.
2. `autodrive_progress.sh check` takes `round_waiting` as an optional 7th
   argument and returns "no stop" when it is `true`. This covers `QUEUED`,
   `UNREADABLE` and `UNKNOWN` as well as `PENDING`. Callers that pass five or
   six arguments keep the old `PENDING`-only rule.
3. A wait-only round therefore never runs the loop-health evaluator and never
   reaches the "no progress (decided in code)" stop.

`wait_reason` and `wait_target` are checked against closed patterns before
they are written; free text never reaches them.

The `${RECORD}.wait` sidecar is mode `0600`, written to a temporary file
then moved. It is never sealed, never read by the completion dispatch, and
deleted at the start of each round, so an old one cannot stop a later round.
It can only stop a loop; it can never complete one.

### What the recipes do with WAITING

| Recipe | Behaviour |
| --- | --- |
| `autodrive-merge-loop.yaml` step-02 | `autodrive_loop.sh` exits `0` on `WAITING` as it does on `DONE`, so step-02 reads `loop_result` from its output and never treats exit `0` as `DONE`. On `WAITING` it passes the result through unchanged. |
| `autodrive-merge-loop.yaml` step-02b, step-03 | Run only when step-02's `loop_result` is `DONE`. On `WAITING` no follow-up issue is filed and the merge gate does not run, so `merge_result` is never `MERGED`. |
| `autodrive-merge-loop.yaml` step-04 | Records the merge phase complete only on `MERGED` or `ALREADY_MERGED`, as before. A `WAITING` loop leaves it not complete, so the next drive runs the merge-ready loop again. |
| `auto-drive-to-merge.yaml` | When the crusty loop ended `WAITING`, the merge loop is skipped (`crusty-waiting`). The report step has a `WAITING)` case beside `STUCK)` and `BUDGET_EXHAUSTED)`: for either loop ending `WAITING` it prints `AUTO_DRIVE_TO_MERGE: the <loop> loop is WAITING; resume after <wait_target>.`, sets the outcome to `WAITING` and exits `0`. The outcome is never `MERGED`. |
| Every merge step | Requires `loop_result == DONE`. An exit code of `0` alone never merges. |

### Resuming

Run the same drive again with the same `pr_number`. Durable state and round
labels continue (`autodrive_state.sh`, `autodrive_last_round.sh`), so the next
round is labelled after the last one and reads the facts fresh:

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=2333
```

Resume after the event in `wait_target`: when CI at that SHA has finished,
when the rate-limit reset epoch has passed, or when GitHub shows a merge
state other than `UNKNOWN`. Budget limits and the `STUCK` rule on fixable
findings still apply across resumes.

## Recomputing an UNKNOWN merge state

GitHub sometimes leaves `mergeStateStatus` at `UNKNOWN` until the branch
changes. `autodrive-merge-evidence.yaml` step-01 acts when the merge state is
`UNKNOWN` this round **and** the previous round's record showed `UNKNOWN` at
the same head SHA:

1. If the branch is behind its base locally, step-01's existing merge-and-push
   runs.
2. Otherwise it calls `autodrive-gh-update-branch`, which sends
   `PUT repos/{owner}/{repo}/pulls/{n}/update-branch` with
   `expected_head_sha`.

The outcome is recorded in `merge_sync`:

| `merge_sync.recompute_action` | When |
| --- | --- |
| `merge-push` | The branch was behind; base merged locally and pushed. |
| `update-branch` | GitHub accepted the update-branch request. |
| `noop-up-to-date` | GitHub answered 422 "no new commits"; the branch is already up to date. |

`merge_sync.recompute_result` holds `ok`, `refused` or `unavailable`. The
round's `wait_target` depends on the outcome:

| Action / result | `wait_target` |
| --- | --- |
| `merge-push` / `ok` | `mergeability@<pushed sha>`: the SHA step-01 pushed, which it knows. |
| `update-branch` / `ok` | `mergeability@<expected_head_sha>`: the head read this round. GitHub answers `202` and makes the new head later, so its SHA is not known in this round. |
| `noop-up-to-date` / `ok` | `mergeability@<expected_head_sha>`. |
| any action / `unavailable` | `ratelimit-reset@<epoch>` or `ratelimit-reset@unknown` when the call was rate-limited, otherwise `read@pr`. |
| `update-branch` / `refused` | `read@pr-head`. The head has probably moved, so the next round reads the head again and starts the two-round `UNKNOWN` count from the new head, instead of repeating a mergeability wait at the old one. |

None of these is `STUCK`. The recompute runs at most once per round, is
never retried and never merges.

## `amplihack orch helper autodrive-gh-update-branch`

```bash
amplihack orch helper autodrive-gh-update-branch 2333 --expected-head-sha 3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d
```

```json
{"pr":2333,"action":"update-branch","result":"ok","expected_head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"}
```

| `action` / `result` | Meaning |
| --- | --- |
| `update-branch` / `ok` | GitHub accepted the request (202). |
| `noop-up-to-date` / `ok` | GitHub answered 422 with no new commits. |
| `update-branch` / `refused` | GitHub refused for another reason, such as a moved head (422 with an expected SHA mismatch). |
| `update-branch` / `unavailable` | Network failure or rate limit; `reset_epoch` is included. |

It exits `0` whenever it recorded an outcome, including `refused` and
`unavailable`, and `2` on a usage error. `--expected-head-sha` is required.

## Security rules

- **Fail closed.** `head-unavailable` and `head-differs` never reach `DONE`.
  The merge gate blocks when a read is unavailable. The completion dispatch
  checks the record before the remote read, so a rate limit cannot turn an
  invalid record into a wait.
- **Arguments.** PR numbers, SHAs, branch names, `--required` and `--repo`
  are validated before any command is built, and passed as separate
  arguments. The branch in `pulls?head=` is sent with `-X GET -f head=…`.
  `head.sha` and `base.ref` returned by GitHub are validated again before use.
- **Environment.** Each `gh` call runs with `GH_REPO` removed, `GH_PAGER=cat`,
  `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1` and stdin closed; `git`
  runs with `GIT_TERMINAL_PROMPT=0`. Token variables are inherited, never
  read or set, and no credential file is read.
- **Output.** Error excerpts are one line, at most 200 bytes, printable
  ASCII, with `ghp_…`, `github_pat_…`, `Authorization`, `Bearer` and `token`
  values redacted. All JSON is written with `serde_json`.

## Tests

| Behaviour | Test file |
| --- | --- |
| Each of `pr`, `head`, `checks`, `reviews` falls back to REST (and `head` to `ls-remote`) under a fake `gh` that refuses GraphQL with the secondary-limit message; REST output equals the GraphQL output | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| Both paths failing reports unavailable, with text distinct from the finding; dispatch gives `head-unavailable` and `head-differs` as separate outcomes | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| A pending-CI-only round and a rate-limited-read-only round end `WAITING`; no `STUCK`; `wait_target` is `ratelimit-reset@<fixture epoch>`; evaluator context has `loop_round_wait_only=true` | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| `UNKNOWN` at the same head on two rounds sends one `PUT …/update-branch` and records the action; the 422 fixture gives `noop-up-to-date` | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| A `CLEAN` dispatch with GraphQL refused and a matching REST head is accepted; both head reads failing gives `WAITING`, exit `76`, not `INVALID_DECISION` | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| An unchanged failing check still gives `STUCK` | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| REST mapping rows, rate-limit classing (same answer as `classify_gh_error` on every sample), reset bounds, argument rejection and redaction | unit tests in `autodrive_gh.rs` |
| Wait classing, every downgrade token row, the `ci_signal` mapping, `wait_target` order, recompute wait targets, and the pinned `autodrive-round-waits` field names | unit tests in `autodrive_waits.rs` |
| A `WAITING` merge-ready loop runs neither step-02b nor the merge gate, and the run report prints the `WAITING` line with outcome `WAITING` | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |

The fake `gh` and `git` live in the test's temporary directory with a
temporary `HOME` and `AMPLIHACK_HOME`. The rate-limit reset epoch is a fixed
fixture value; no test reads the clock or GitHub.

## Related references

- [Auto Drive To Merge Reference](auto-drive-to-merge.md#waits-are-not-stuck-2333-2472)
- [Crusty Completion Contract](autodrive-crusty-completion.md#head-check-outcomes)
- [Loop-Health Evaluator Reference](loop-health-evaluator.md#waits-are-not-lack-of-progress)
