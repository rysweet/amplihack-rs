---
title: Auto-drive GitHub Reads and Waits
description: How auto-drive reads PR facts when GraphQL is refused, how a failed read differs from a finding, and how waits end a round as WAITING instead of STUCK.
doc_type: reference
last_updated: 2026-11-13
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

Every auto-drive read of GitHub now goes through one Rust helper, which tries
GraphQL, then REST, then (for the head only) `git ls-remote`. A read that
fails on every path is reported as **unavailable**, which is a **wait** and
never a finding. A round whose only blockers are waits ends with the
resumable loop result `WAITING` (issue #2472, waits slice). At the top level
the run reports `readiness: incomplete` with the blocker `waiting: <target>`.

## Contents

- [The read cascade](#the-read-cascade)
- [`amplihack orch helper autodrive-gh-read`](#amplihack-orch-helper-autodrive-gh-read)
- [Output format](#output-format)
- [Exit codes](#exit-codes)
- [How REST values map to GraphQL values](#how-rest-values-map-to-graphql-values)
- [Rate limits and the reset time](#rate-limits-and-the-reset-time)
- [Which reads use the helper](#which-reads-use-the-helper)
- [What the agent steps are told](#what-the-agent-steps-are-told)
- [Waits and fixable findings](#waits-and-fixable-findings)
- [`amplihack orch helper autodrive-round-waits`](#amplihack-orch-helper-autodrive-round-waits)
- [The WAITING result](#the-waiting-result)
- [How the run report shows WAITING](#how-the-run-report-shows-waiting)
- [The `.wait` sidecar](#the-wait-sidecar)
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
| 3 | `git ls-remote origin refs/pull/{n}/head` | `head` only, and only when `origin` is the repository being read |

The fallback runs on **any** GraphQL failure, not only on a recognised
rate-limit message. A GraphQL answer that fails validation (for example a
head SHA that is not 40 hex characters) also falls through.

The `ls-remote` route reads `origin`, whatever `--repo` says. It is therefore
used only when `--repo` is not given, or when `origin` resolves to the same
`owner/name` as `--repo`. Otherwise the route reports itself unavailable
(`ls-remote: origin is not <owner>/<name>`) and makes no `git ls-remote`
call, so a pull request with the same number in another repository is never
read as the head. The comparison resolves `origin` with
`git remote get-url origin`, which reads the local configuration and contacts
no server.

Review threads have no REST path. When the GraphQL thread read fails,
`unresolved_threads` is `"unavailable"`, which is a wait. It is never read as
`0` and never counts as resolved.

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
| `reviews <n>` | PR number; optional `--required <count>` (`1`–`100`, default `1`) | `review_decision`, `approvals`, `changes_requested`, `unresolved_threads` |
| `pr-for-branch <branch>` | branch name | `number` of the open PR for the branch, or `null` when there is none |

These five are the only subcommands. Merge state, mergeability and the base
branch all come from `pr <n>`; there is no `merge-state` or `base`
subcommand, and the PR number is always positional, never `--pr`.

Every subcommand takes an optional `--repo <owner>/<name>`. Without it the
repository comes from the working directory's `origin`, as `gh` resolves it.
`GH_REPO` from the environment is ignored.

The shell tools and the agent steps call this subcommand. The completion
dispatch calls the same library functions directly (`autodrive_gh::head`,
`autodrive_gh::pr_for_branch`). The code lives in
`crates/amplihack-cli/src/commands/orch/autodrive_gh/` (`cli.rs`, `reads.rs`,
`rest_map.rs`, `ratelimit.rs`).

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
| `2` | Usage error: unknown subcommand or flag, a PR number that is not `1`–`9999999999`, a SHA that is not 40 hex characters, a `--repo` that is not `owner/name`, or a branch name that fails the branch rules. Nothing is run. |

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
| Checks at the head | `autodrive_platform_facts.sh` | `checks` | `read@checks` wait |
| Reviews and approvals | `autodrive_platform_facts.sh` | `reviews` | `approval-facts-unreadable` wait |
| Review threads | `autodrive_platform_facts.sh` | GraphQL only | `unresolved_threads: "unavailable"`, `read@threads` wait |
| PR head | `autodrive_pr_head.sh`, completion dispatch | `head` | `crusty-pr-head-unreadable` wait; dispatch `head-unavailable` |
| PR head (agent step) | `autodrive-crusty-round.yaml` step-01-round-context | `head` | Wait recorded in the round's `.wait` sidecar; see [What the agent steps are told](#what-the-agent-steps-are-told) |
| Merge state and base (agent step) | `autodrive-merge-evidence.yaml` step-01-sync-with-base | `pr` | `base-unavailable`; a wait, and no merge from a guessed `main` |
| Checks at the head (agent step) | `autodrive-merge-evidence.yaml` step-03-wait-for-ci | `checks <sha>` | `ci_status` `UNREADABLE`, `read@checks` wait |

`autodrive_pr_head.sh` exits `4` only when every path is unavailable, and
prints the helper's reason. `gh` state values from REST are upper-cased
before use.

Finalize (`autodrive-finalize.yaml`) keeps its own checks before it merges.
An unavailable read is never treated as satisfied there, so finalize refuses
while any read it needs is unavailable.

## What the agent steps are told

Since #2340 the crusty round, merge-evidence and merge-loop steps are agent
steps with prompts, not bash. The #2333 and #2472 rules are therefore in the
prompt text. Each prompt names the helper as the first way to read GitHub and
keeps "an equivalent authenticated interface" as the second. The agent records
the helper's JSON line in its result, not raw `gh` output.

| Step | What the prompt requires |
| --- | --- |
| `autodrive-crusty-round.yaml` step-01-round-context | Read the head with `amplihack orch helper autodrive-gh-read head <n>`. Exit `3` (head unavailable) is a wait: write the [`.wait` sidecar](#the-wait-sidecar) next to `RECIPE_VAR_autodrive_round_record` and report the round incomplete. A head that answers with another SHA than the local head is a refusal, as before. The two are never confused. |
| `autodrive-merge-evidence.yaml` step-01-sync-with-base | Read merge state, mergeable and base with `autodrive-gh-read pr <n>`. An unavailable base is `base-unavailable`; never merge a guessed base. On an `UNKNOWN` merge state, when the branch is not behind locally, call `autodrive-gh-update-branch <n> --expected-head-sha <head read this round> --prior-wait "$RECIPE_VAR_autodrive_prior_wait"` and record its JSON line, then write this round's [`.wait` sidecar](#the-wait-sidecar). The prompt never reads the previous sidecar itself and never decides whether the write is allowed; the helper does. See [Recomputing an UNKNOWN merge state](#recomputing-an-unknown-merge-state). |
| `autodrive-merge-evidence.yaml` step-03-wait-for-ci | Read the checks once with `autodrive-gh-read checks <head sha>`. Pending or queued checks at the head are a wait on that SHA (`ci_status` `PENDING`). Never sleep, never read the checks again in the same step. A `GREEN` rollup is still confirmed with `autodrive_expected_checks.sh`, as before (#2186). |
| `autodrive-merge-loop.yaml` step-01-merge-loop-preflight | Reads `<state dir>/crusty-result.json`. A crusty result of `WAITING` is passed up as `should_run` `"false"` and `crusty_waiting` `"true"`; the merge loop does not run. Step-02b runs only when `'WAITING' not in merge_loop_result`, and step-03 (finalize) only when that holds and `crusty_waiting` is not `"true"`. |

Step-03 replaces main's earlier instruction to "poll pending work to its
natural terminal state". That instruction needed a timer or a polling loop,
which #2472 and the no-timers rule forbid. It is the one place where main's
behaviour was replaced rather than kept.

The prompts can only be checked with an agent, so no test reads prompt text.
The behaviour the prompts point to is tested in the helpers, in
`autodrive_waits.rs` and in the run report.

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
| `recompute` | Optional. The step-01 recompute outcome, `{action, result, pushed_sha, rate_limited, reset_epoch}`, when one ran this round. `action` is `merge-push`, `update-branch`, `noop-up-to-date` or `none`; `result` is `ok`, `refused`, `unavailable` or `first-read`. It sets `wait_target` by the table in [Recomputing an UNKNOWN merge state](#recomputing-an-unknown-merge-state) when the round waits on mergeability and no read was rate-limited. Any other `action` or `result` value is refused with exit `2`. |

Exit `0` on success, `2` on input that is not valid JSON or has the wrong
shape. Unknown input fields are refused with exit `2`, so a renamed field
cannot be silently ignored. A unit test in `autodrive_waits.rs` pins the
input and output field names above and the `recompute` values. A field or
value added for step-01 is added to the pinned list and to the accepted set
in the same change, so the unknown-field rule never refuses a value step-01
is told to send.

## The WAITING result

`WAITING` is a `loop_result` alongside `DONE`, `STUCK`, `USAGE_LIMIT`,
`BUDGET_EXHAUSTED` and `ORPHANED`. It is resumable and is not a failure.

```json
{"loop_result":"WAITING","loop":"merge-ready","round_label":"round-4","reason":"every blocker this round is a wait (ci-pending)","wait_reason":"ci-pending","wait_target":"ci@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"}
```

The loop-result JSON is one compact line with `loop_result` as its first
field. `escalate WAITING` in `autodrive_escalate.sh` writes `wait_reason` and
`wait_target` only after both pass `wait_valid`, so free text never reaches
them.

How a loop reaches it:

1. After a round, `autodrive_loop.sh` calls `wait_stop` **before**
   `progress_stop`. `wait_stop` reads `round_waiting`, `wait_target` and
   `wait_reason` from the round record, or from the round's `.wait` sidecar
   when crusty step-01 could not read the head. When `round_waiting` is
   `true`, it calls `escalate WAITING` and the loop exits `0`.
2. `autodrive_progress.sh check` takes `round_waiting` as an optional 7th
   argument and returns "no stop" when it is `true`. This covers `QUEUED`,
   `UNREADABLE` and `UNKNOWN` as well as `PENDING`. Callers that pass five or
   six arguments keep the old `PENDING`-only rule.
3. A wait-only round therefore never runs the loop-health evaluator and never
   reaches the "no progress (decided in code)" stop. The `STUCK` count does
   not move.

### What the recipes do with WAITING

| Recipe | Behaviour |
| --- | --- |
| `autodrive-merge-loop.yaml` step-02-merge-ready-loop | `autodrive_loop.sh` exits `0` on `WAITING` as it does on `DONE`, so the step reads `loop_result` from the output, never the exit code. |
| `autodrive-merge-loop.yaml` step-02b-file-followups, step-03-finalize | Each has the condition `merge_loop_preflight.should_run == 'true' and 'WAITING' not in merge_loop_result`. On `WAITING` no follow-up issue is filed and finalize does not run, so nothing merges. |
| `autodrive-merge-loop.yaml` step-01-merge-loop-preflight | A crusty `WAITING` is passed up; the merge loop is skipped. |
| `auto-drive-to-merge.yaml` | Unchanged. The run report comes from the reducer; see [How the run report shows WAITING](#how-the-run-report-shows-waiting). |

The condition is a substring test, so a finding whose text contains `WAITING`
also skips step-02b and step-03. That fails safe: the round makes no progress
and nothing merges, and the next drive reads the facts again.

### Resuming

Run the same drive again with the same `pr_number`. Durable state and round
labels continue (`autodrive_state.sh`, `autodrive_last_round.sh`), so the next
round is labelled after the last one and reads the facts fresh:

```bash
amplihack recipe run auto-drive-to-merge -c pr_number=2333
```

Resume after the event in `wait_target`: when CI at that SHA has finished,
when the rate-limit reset epoch has passed, or when GitHub shows a merge
state other than `UNKNOWN`. Nothing in auto-drive waits for the event itself;
a later drive or sweep picks it up. Budget limits and the `STUCK` rule on
fixable findings still apply across resumes.

## How the run report shows WAITING

The top-level reducer (`orch/autodrive/report.rs`) knows the outcomes
`completed`, `incomplete`, `dependency_failure` and `budget_exhausted`. There
is no separate waiting outcome. A loop that ended `WAITING` is reported as:

| Field | Value |
| --- | --- |
| `readiness` | `incomplete` |
| `action` | `not_requested` |
| `stop_reason` | `waiting` |
| `blockers` | includes `waiting: <wait_target>` |

```json
{"schema_version":1,"readiness":"incomplete","action":"not_requested","platform_state":"unknown","stop_reason":"waiting","blockers":["accepted finalization evidence unavailable: …","waiting: ci@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"]}
```

`stop_reason` `waiting` is set in the reducer's error path, beside
`budget_exhausted`, `usage_limit`, `orphaned`, `terminal_refusal` and
`no_progress`. The typed `StopReason` enum and the accepted-outcome path do
not change.

`wait_target` is read as a JSON string and checked against the same closed
pattern as `autodrive_round_evidence.sh`:

```text
^((ci|mergeability)@[0-9a-f]{40}|ratelimit-reset@([0-9]{1,12}|unknown)|read@(pr|pr-head|pr-number|checks|reviews|threads))$
```

| `wait_target` in the loop result | Blocker added |
| --- | --- |
| matches the pattern | `waiting: <wait_target>` |
| present but not a string, or does not match | `waiting: <invalid target>`; the value itself is not repeated |
| absent | none |

This follows `evidence-contract.md`: a rate limit is `incomplete` with the
reset recorded, and a later drive picks it up.

## The `.wait` sidecar

A step that cannot finish its round record, such as crusty step-01 when the
head is unavailable, records the wait in `<round record>.wait`, where
`<round record>` is `RECIPE_VAR_autodrive_round_record`. Merge-evidence
step-01 records an `UNKNOWN` merge state there too.

The round record is `<state dir>/<loop>-round-<N>.json`, so its sidecar is
`<state dir>/<loop>-round-<N>.json.wait`. For the previous round's sidecar,
`autodrive_loop.sh` passes one more context value to the round recipe:

| Context value | Value |
| --- | --- |
| `autodrive_prior_wait` | `<state dir>/<loop>-round-<N-1>.json.wait`, the sidecar of the previous round of the same loop. Empty when `N` is `1`. |

`N` continues from `autodrive_last_round.sh`, so on a resumed drive round
`N-1` is the last round of the earlier drive, the one that ended `WAITING`.
`autodrive-merge-round.yaml` declares `autodrive_prior_wait` and passes it to
`autodrive-merge-evidence.yaml` unchanged. Step-01 hands the path to the
update-branch helper and does not open it.

The two writers use two shapes. Crusty step-01, when the head is
unavailable, writes the wait for `wait_stop`:

```json
{"round_waiting":true,"wait_reason":"crusty-pr-head-unreadable","wait_target":"read@pr-head"}
```

The target is `ratelimit-reset@<epoch>` or `ratelimit-reset@unknown` when
the head read was rate-limited. Merge-evidence step-01, on an `UNKNOWN`
merge state, writes the shape `--prior-wait` reads in the next round:

```json
{"wait":"mergeability@3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","merge_state":"UNKNOWN"}
```

| Rule | Detail |
| --- | --- |
| Writing | To a temporary file in the same directory, then renamed over `<round record>.wait`, under `RECIPE_VAR_autodrive_state_dir`, with umask `022` or stricter. |
| Clearing | `autodrive_loop.sh` runs `rm -f -- "${RECORD}.wait"` at the start of each round. This removes only the **current** round's sidecar, left by an earlier drive that used the same label, so it cannot stop this round. The previous round's sidecar is kept; it is what `autodrive_prior_wait` names. |
| Content | Only the keys of one of the two shapes above. It holds SHAs, state names and closed wait targets, nothing else. |
| Reading | Untrusted, and read by code only; no agent prompt opens a sidecar. `wait_stop` in `autodrive_escalate.sh` reads the **current** round's sidecar: a regular file owned by the user, not a symlink, at most 4096 bytes, with `round_waiting` true and `wait_target` and `wait_reason` checked by `wait_valid`; the merge-evidence shape has no `round_waiting`, so it never stops a loop there. `autodrive-gh-update-branch --prior-wait` reads the **previous** round's sidecar with the strict reader in `autodrive_waits.rs`: a regular file, not a symlink, at most 4 KiB, only the three keys above, and a `head_sha` of 40 lowercase hex characters. Anything else is treated as if there were no previous read. |
| Effect | It can stop a loop as `WAITING`. It can never complete a round, is never sealed and is never read by the completion dispatch. |

## Recomputing an UNKNOWN merge state

GitHub sometimes leaves `mergeStateStatus` at `UNKNOWN` until the branch
changes. Merge-evidence step-01 handles it across two rounds, and the
decision to write is made in code by `autodrive-gh-update-branch`, not by
the prompt.

On an `UNKNOWN` merge state, step-01 does the following:

1. **If the branch is behind its base locally**, step-01's existing
   merge-and-push runs, as on main. `recompute_action` is `merge-push`.
2. **Otherwise** it calls
   `autodrive-gh-update-branch <n> --expected-head-sha <head read this round>
   --prior-wait "$RECIPE_VAR_autodrive_prior_wait"`. The helper reads the
   previous round's sidecar with the strict reader and makes the write only
   when the sidecar is valid, records `merge_state` `UNKNOWN`, and has a
   `head_sha` equal to `--expected-head-sha`. In every other case (no
   previous round, no file, a refused file, another merge state, another
   head) it answers `first-read` and makes no `gh` call.
3. In both cases step-01 then writes this round's sidecar with
   `merge_state` `UNKNOWN` and the head read this round, so the next round
   can qualify.

The first `UNKNOWN` at a head is therefore always a wait with no write, and
the second at the same head is the earliest point a write can happen.

The outcome is recorded in `merge_sync`:

| `merge_sync.recompute_action` | When |
| --- | --- |
| `merge-push` | The branch was behind; base merged locally and pushed. |
| `none` | The helper answered `first-read`; no write was made. |
| `update-branch` | The helper sent the update-branch request. |
| `noop-up-to-date` | GitHub answered 422 "no new commits"; the branch is already up to date. |

`merge_sync.recompute_result` holds `ok`, `refused`, `unavailable` or
`first-read`. The round's `wait_target` comes from `recompute_target` in
`autodrive_waits.rs`:

| Action / result | `wait_target` |
| --- | --- |
| `merge-push` / `ok` | `mergeability@<pushed sha>`: the SHA step-01 pushed. |
| `none` / `first-read` | `mergeability@<expected_head_sha>`. |
| `update-branch` / `ok` | `mergeability@<expected_head_sha>`. GitHub answers `202` and makes the new head later, so its SHA is not known in this round. |
| `noop-up-to-date` / `ok` | `mergeability@<expected_head_sha>`. |
| any action / `unavailable` | `ratelimit-reset@<epoch>` or `ratelimit-reset@unknown` when the call was rate-limited, otherwise `read@pr`. |
| `update-branch` / `refused` | `read@pr-head`. The head has probably moved, so the next round reads it again and starts the two-round count from the new head. |

None of these is `STUCK`. The recompute runs at most once per round, is
never retried and never merges.

## `amplihack orch helper autodrive-gh-update-branch`

```bash
amplihack orch helper autodrive-gh-update-branch 2333 \
  --expected-head-sha 3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d \
  --prior-wait "$RECIPE_VAR_autodrive_prior_wait"
```

```json
{"pr":2333,"action":"update-branch","result":"ok","expected_head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d"}
```

When the previous sidecar does not qualify:

```json
{"pr":2333,"action":"none","result":"first-read","expected_head_sha":"3f9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3d","reason":"prior wait: head differs"}
```

It sends `PUT repos/{owner}/{repo}/pulls/{n}/update-branch` with
`expected_head_sha`, which GitHub checks atomically. It is the only write
auto-drive makes through these helpers, and it is made only after the
`--prior-wait` check passes.

| `action` / `result` | Meaning |
| --- | --- |
| `none` / `first-read` | The previous sidecar is missing, empty-path, refused by the strict reader, not `UNKNOWN`, or at another head. No `gh` call was made. `reason` is one of `prior wait: none`, `prior wait: invalid`, `prior wait: not unknown`, `prior wait: head differs`. |
| `update-branch` / `ok` | GitHub accepted the request (202). |
| `noop-up-to-date` / `ok` | GitHub answered 422 with no new commits. |
| `update-branch` / `refused` | GitHub refused: a 403, a 422 for an expected SHA mismatch, or any other error that is not transient. Never turned into a wait, so a forbidden write is never repeated. |
| `update-branch` / `unavailable` | Network failure or rate limit; `reset_epoch` is included. |

| Argument | Rule |
| --- | --- |
| `<n>` | PR number, positional. |
| `--expected-head-sha <sha>` | Required. 40 lowercase hex characters. |
| `--prior-wait <path>` | Required. The previous round's sidecar path. An empty value means there is no previous round and gives `first-read`. A missing file gives `first-read`. |
| `--repo <owner>/<name>` | Optional, as for `autodrive-gh-read`. |

There is **no base argument**: GitHub updates the branch from the PR's own
base. A `--base` flag, a missing `--expected-head-sha` or `--prior-wait`, or
any other unknown flag is a usage error.

| Exit | Meaning |
| --- | --- |
| `0` | An outcome was recorded, including `first-read`, `refused` and `unavailable`. |
| `2` | Usage error. No `gh` call is made. |

The `--prior-wait` check runs before any `gh` call, so a `first-read`
outcome costs no GitHub request and is not affected by rate limits.

## Security rules

- **Fail closed.** `head-unavailable` and `head-differs` never reach `DONE`.
  Unavailable reads, `rate_limited`, `unresolved_threads: "unavailable"`, an
  `UNKNOWN` merge state and pending checks never count as passing. A merge
  still needs the evaluator's `DONE` and finalize's own checks. The
  completion dispatch checks the record before the remote read, so a rate
  limit cannot turn an invalid record into a wait.
- **Review decision.** The REST `review_decision` is computed locally and is
  evidence only. GitHub's answer at merge time decides.
- **One write.** `autodrive-gh-update-branch` is the only write. The helper
  itself reads the previous round's sidecar (`--prior-wait`) with the strict
  reader and makes the write only when that sidecar is valid, records
  `UNKNOWN`, and names the same head as `--expected-head-sha`. The rule is
  enforced in code, not in prompt text. It takes no base.
- **Head of the right repository.** The `ls-remote` route is used only when
  `origin` is the repository being read. Otherwise no `git ls-remote` call
  is made; resolving `origin` reads only local configuration.
- **Arguments.** PR numbers, SHAs, branch names, `--required` and `--repo`
  are validated before any command is built, and passed as separate
  arguments. The branch in `pulls?head=` is sent with `-X GET -f head=…`.
  `head.sha` and `base.ref` returned by GitHub are validated again before use.
- **Environment.** Each `gh` call runs with `GH_REPO` removed, `GH_PAGER=cat`,
  `GH_PROMPT_DISABLED=1`, `GH_NO_UPDATE_NOTIFIER=1` and stdin from
  `/dev/null`; `git` runs with `GIT_TERMINAL_PROMPT=0`. The host `timeout` does
  not end a read from a terminal, so no call may prompt. Authentication is the
  user's existing `gh` login: token variables are inherited, never read or
  set, and no credential file is read.
- **Untrusted state.** The `.wait` sidecar is size-capped, limited to known
  keys and SHA-checked before use. `wait_target` in the run report is checked
  against a closed pattern and an invalid value is never repeated.
- **Recipe bash.** Any bash in these recipes reads only `RECIPE_VAR_*`
  variables, never a bare `REPO_PATH`, `AUTODRIVE_STATE_DIR` or
  `AUTODRIVE_ROUND_RECORD` (#2545). Every expansion is quoted, paths follow
  `--`, and nothing read is passed to `eval` or `sh -c`.
- **Output.** Error excerpts that reach JSON, records, the sidecar or a PR
  body pass through `excerpt()`: one line, at most 200 bytes, printable ASCII,
  with `ghp_…`, `github_pat_…`, `Authorization`, `Bearer` and `token` values
  redacted. All JSON is written with `serde_json`. Nothing logs the
  environment, the authentication status or request headers.
- **No timers.** `reset_epoch` is clamped and recorded, never slept on. No
  retry, poll or interval is involved anywhere in this path.

## Tests

| Behaviour | Test file |
| --- | --- |
| Each of `pr`, `head`, `checks`, `reviews` falls back to REST (and `head` to `ls-remote`) when the fake `gh` refuses GraphQL with the secondary-limit message; REST output equals the GraphQL output | `crates/amplihack-cli/tests/issue_2333_gh_fallback_reads.rs` |
| With `--repo` different from `origin`, the head read makes no `ls-remote` call and that route is unavailable; with `--repo` equal to `origin`, `ls-remote` is still used | `crates/amplihack-cli/tests/issue_2333_gh_fallback_reads.rs` |
| A read failing over GraphQL and REST reports unavailable, with text distinct from a finding | `crates/amplihack-cli/tests/issue_2333_gh_fallback.rs` |
| `autodrive-gh-read pr <n>` returns base and merge state over REST when GraphQL is refused | `crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs` |
| `autodrive-gh-update-branch` with `--base`, or without `--prior-wait`, exits `2` and makes no `gh` call; a GitHub 422 for a mismatched `--expected-head-sha` gives `refused` | `crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs` |
| `autodrive-gh-update-branch --prior-wait` gives `first-read` and makes no `gh` call when the path is empty, the file is missing, a symlink, over 4 KiB, has an unknown key, has an invalid `head_sha`, records a merge state other than `UNKNOWN`, or records another head; a valid `UNKNOWN` sidecar at the same head makes exactly one `PUT` | `crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs` |
| `autodrive_loop.sh` passes `autodrive_prior_wait` naming round `N-1` (empty for round `1`, and the last round of the earlier drive after a resume), and the round-start `rm -f` leaves the previous round's sidecar in place | `crates/amplihack-cli/tests/issue_2333_gh_fallback_loops.rs` |
| `autodrive-gh-read checks <sha>` makes exactly one `gh` call and returns `pending` | `crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs` |
| `head <n>` unavailable is a wait; a differing head is a refusal | `crates/amplihack-cli/tests/issue_2333_gh_fallback_steps.rs` |
| A CI-pending round and a rate-limited-read round end `WAITING`, the `STUCK` count does not move, and the evaluator context has `loop_round_wait_only=true`; an unchanged failing check still gives `STUCK` | `crates/amplihack-cli/tests/issue_2333_gh_fallback_loops.rs`, `issue_2333_gh_fallback_waits.rs` |
| A `CLEAN` dispatch with GraphQL refused and a matching REST head is accepted; a head unavailable on every path gives `WAITING`, exit `76`, not `INVALID_DECISION` | `crates/amplihack-cli/tests/issue_2333_gh_fallback_dispatch.rs` |
| REST mapping rows, rate-limit classing (same answer as `classify_gh_error` on every sample), reset bounds, argument rejection and redaction | `crates/amplihack-cli/src/commands/orch/autodrive_gh_tests.rs` |
| Wait classing, every downgrade token row, the `ci_signal` mapping, `wait_target` order, recompute targets and the pinned `autodrive-round-waits` field names | `crates/amplihack-cli/src/commands/orch/autodrive_waits_tests.rs` |
| Strict sidecar reader: a valid sidecar at the same head qualifies; a different head, a file over 4 KiB, an unknown key or an invalid `head_sha` each count as a first read; the `none`/`first-read` recompute target is `mergeability@<expected_head_sha>` | `crates/amplihack-cli/src/commands/orch/autodrive_waits_tests.rs` |
| `WAITING` with a valid target gives `readiness` `incomplete`, `stop_reason` `waiting` and the blocker `waiting: ci@<sha>`; a missing target adds no waiting blocker; an invalid target adds `waiting: <invalid target>` without the value; `unresolved_threads: "unavailable"` blocks readiness and is never read as `0` | unit tests in `crates/amplihack-cli/src/commands/orch/autodrive/report.rs` |

Each test uses a temporary `HOME`, `XDG_CONFIG_HOME` and `GH_CONFIG_DIR`,
removes `GH_TOKEN`, `GITHUB_TOKEN`, `GH_ENTERPRISE_TOKEN` and `GH_HOST` from
the environment, and puts a fake `gh` and `git` first on `PATH`
(`tests/support/autodrive_gh_fake.rs`). No test reaches GitHub, reads
`~/.claude`, `~/.amplihack` or a profile root, reads the clock or waits on a
timer. The rate-limit reset epoch is a fixed fixture value.

## Related references

- [Auto Drive To Merge Reference](auto-drive-to-merge.md#waits-are-not-stuck-2333-2472)
- [Crusty Completion Contract](autodrive-crusty-completion.md#head-check-outcomes)
- [Loop-Health Evaluator Reference](loop-health-evaluator.md#waits-are-not-lack-of-progress)
