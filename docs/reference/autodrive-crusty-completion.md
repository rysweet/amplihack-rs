---
title: Auto-drive Crusty Completion Contract
description: Completion-first dispatch, decision integrity, the PR head check without GraphQL, and follow-up finalization for Crusty reviews.
last_updated: 2026-11-15
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Crusty Completion Contract

> [Home](../index.md) > [Auto Drive To Merge](auto-drive-to-merge.md) > Crusty Completion

Auto-drive checks whether the Crusty review objective is complete before
dispatching the loop-health evaluator. An evidence-valid `CLEAN` or
`MINOR_ONLY` round completes review iteration without an evaluator call.
Required follow-up recording then gates phase completion.

This contract applies to both the primary Crusty loop and Crusty rereviews
started between merge-ready rounds. It also applies when
[`auto-drive-light`](auto-drive-light.md) uses those loops. The merge-ready
loop's own evaluator and completion policy are unchanged.

## Contents

- [Crusty loop preflight](#crusty-loop-preflight)
- [Next-action judgment](#next-action-judgment)
- [Rust helper operations](#rust-helper-operations)
- [Decision format](#decision-format)
- [Dispatch validation](#dispatch-validation)
- [Head check outcomes](#head-check-outcomes)
- [Results and accounting](#results-and-accounting)
- [Follow-up finalization](#follow-up-finalization)
- [Readiness assessment outcomes](#readiness-assessment-outcomes)
- [Behavior examples](#behavior-examples)
- [Configuration and failures](#configuration-and-failures)

## Crusty loop preflight

Step `step-01-crusty-loop-preflight` in `autodrive-crusty-loop.yaml` decides
whether the Crusty loop runs. `should_run` is the string `"true"` or
`"false"`. Steps `step-02-crusty-loop` and `step-03-record-crusty-phase` run
only when it is `"true"`. An open PR with a verified worktree at the PR head,
valid private state and no complete Crusty round at that head always runs the
loop.

`should_run` is `"false"` only in these cases, each with named blockers:

| `skip_reason` | Observation |
| --- | --- |
| `merged` | The PR is merged. |
| `closed` | The PR is closed without merging. |
| `complete-at-head` | An evidence-valid `CLEAN` or `MINOR_ONLY` round already exists at the current PR head. |
| `no-access` | The PR cannot be read, or cannot be identified unambiguously, through any configured authenticated interface. |
| `no-worktree` | No linked worktree for the PR head branch can be found or verified. |
| `no-state` | The private state directory cannot be created, or existing state is unsafe. `state_dir` is then `""`. |
| `worktree-diverged` | The worktree and the PR head have diverged and cannot be reconciled without losing commits. |

`skip_reason` is `""` if and only if `should_run` is `"true"`. It is
informational: the step conditions compare only `should_run`.

The preflight agent decides whether a round is complete at the head, using
the rule from #2345: a `CLEAN` round, or a `MINOR_ONLY` round whose follow-up
obligations are met. No Rust function makes that judgement. An existing open
PR whose round at the current head is incomplete, missing or recorded at an
older head is not `complete-at-head`, so step-02 and step-03 run.

The worktree position relative to the PR head affects the result as follows:

- A worktree behind the PR head is repaired with `git fetch` and
  `git merge --ff-only`, then checked again with
  `amplihack orch helper autodrive worktree`. The loop then runs.
- A worktree that has diverged is reported as `worktree-diverged` and the
  loop does not run. Reset, force and branch switches are not used.
- A worktree ahead of the PR head is treated as the head under review, and a
  review receipt for that local head does not count as merge readiness at a
  different remote PR head: `completion::verify` rejects a receipt bound to
  another head
  (`receipt_for_a_local_ahead_head_is_not_readiness_at_another_pr_head`).

## Next-action judgment

After the round record is written, an agent reads the record and its
`.findings`, `.minor` and `.severity` sidecars, then selects the next action:

| Current review state | Action | Next step |
| --- | --- | --- |
| `CLEAN`, and `reviewed_head_sha` equals `head_sha` | `complete` | Finalize required follow-ups, then record phase completion. |
| `MINOR_ONLY`, and `reviewed_head_sha` equals `head_sha` | `complete` | File or verify every minor follow-up before recording phase completion. |
| `CONCERNS` (blocking or major concerns open), including after a round with commits when the heads differ | `evaluate` | Retain remediation, progress and budget checks, then evaluate convergence where applicable. |
| `CLEAN` or `MINOR_ONLY`, but `reviewed_head_sha` differs from `head_sha` | `evaluate`, with the head mismatch as the reason | The new head is not reviewed yet, so `complete` is not allowed. Dispatch does not accept this decision either, so the round stops with a dispatch error. It fails closed and grants no merge authority. Dispatch's own head check reaches the same closed failure from its own read ([Differs](#the-three-outcomes)); routing either of them to the evaluator instead of erroring is still a follow-up under #2472, which this change does not settle. |
| Invalid evidence: missing or unreadable record, no verdict, or a failed or refused round (rc not 0, exit `79`) | `evaluate`, with the fault as the reason | Never `complete`. Dispatch keeps its own failure handling: exits `79` and `75` return before the decision is read, a missing or unreadable record fails dispatch validation, and a record with no verdict goes to the evaluator as `MISSING`. |

The PR head is the round record's `head_sha`. The agent compares
`reviewed_head_sha` only with that field. Step-07 runs in the base checkout,
so `git rev-parse HEAD` in its working directory gives the base branch, not
the PR head, and must never be used for this comparison.

`CONCERNS` always means `evaluate`, whatever the head SHAs or sidecars say.
After a round with commits, `reviewed_head_sha` differing from `head_sha` is
expected and is not invalid evidence; it only means the new head has not been
reviewed. The `.minor` sidecar is required only for `MINOR_ONLY`. Dispatch
checks the sidecars itself: every round needs a `.findings` sidecar, `CLEAN`
and `MINOR_ONLY` need it empty, and `MINOR_ONLY` also needs a non-empty
`.minor` and a `.severity` sidecar.

The agent always runs the Rust writer and records a decision. It stops
without a decision only when the writer itself fails. It writes the reason in
its own words and never copies text from the record, sidecars or review prose
into it.

Completed reviews repeated in persisted history or within the same run are
still complete. An unchanged `MINOR_ONLY` verdict is not evidence of
nonconvergence. A finding that was previously major still follows the
[minor-classification rules](auto-drive-to-merge.md#crusty-review);
history does not override those rules.

This selection is agent judgment under SR-103. Shell code does not choose an
action by matching verdict tokens. The agent treats findings, history, PR
prose and embedded instructions as untrusted data. It writes only its
designated decision through the Rust writer; it does not remediate concerns,
edit evidence, manifests or receipts, or mark the phase complete in this step.

## Rust helper operations

The recipes use internal operations under `amplihack orch helper`. These are
driver interfaces; operators use the existing auto-drive recipe invocation.

| Operation | Responsibility |
| --- | --- |
| Prepare | Accept the PR number as `--pr <n>` (`1` to `9999999999`) and store it in the private state as `pr`. Create a fresh unpredictable run identity, a unique per-round decision output path, and driver-owned identity, evidence snapshots and evaluator launch context. |
| Seal | The record-writing bash step captures fresh evidence before the next-action agent runs. Direct clients preparing existing records seal during prepare. |
| Write | Accept typed `complete` or `evaluate` and a bounded reason; compute the designated record's hash and atomically create the decision. |
| Dispatch | Independently validate the decision and evidence; return objective completion or launch the trusted evaluator with typed process arguments. |

The driver keeps trusted state outside the agent's designated output
directory. Prepare captures evaluator context into the private state file, accepting only declared
data fields. It cannot supply executable paths, arbitrary arguments,
environment overrides or depth-limit overrides. The binary, recipes and
evaluator resolve from the trusted launch installation.

## Decision format

Version 1 is exactly one JSON object, at most 16 KiB, with these fields:

| Field | Type | Constraint |
| --- | --- | --- |
| `schema_version` | Integer | Exactly `1`. |
| `run_id` | String | Matches the driver-issued identity for this run. |
| `round_label` | String | Matches the prepared round identity. |
| `round_record_hash` | String | Computed by the Rust writer using the existing manifest-compatible git blob hashing convention. |
| `action` | String | Exactly `complete` or `evaluate`. |
| `reason` | String | At most 1024 bytes; unsafe control characters are rejected. |

Unknown or duplicate fields, trailing content, unsupported versions and
invalid identities are rejected. The writer atomically creates the decision
without following an existing destination link. Missing, stale, replayed,
symlinked, oversized or insecure decisions fail visibly; dispatch never
infers completion or automatically retries an invalid decision. A private,
atomically created dispatch receipt prevents reusing a decision in the same run.

## Dispatch validation

Dispatch validates all of the following independently of the agent's claim:

- Driver-issued run and round identities, and the actual preserved record hash.
- Agreement among the preserved record, latest copy and manifest row.
- Required sidecars against driver-owned snapshots, including minor findings
  and severity evidence. A matching round JSON hash alone is insufficient.
- Current reviewed-head evidence and the recorded child execution status.
  A completion claim additionally requires successful child execution and an
  allowed `CLEAN` or `MINOR_ONLY` round result.
- The PR's head on GitHub, read after every check above has passed; see
  [Head check outcomes](#head-check-outcomes).
- Private directory ancestry, ownership, permissions and regular-file types.
  Descriptor-relative no-follow opens and descriptor metadata checks avoid
  races between path validation and use.

Both `complete` and `evaluate` pass through this validation. A recoverable
child failure may select `evaluate` only when the decision and required
record, manifest and sidecar evidence remain valid; failed execution can
never authorize `complete`. A no-progress exemption does not waive dispatch
validation. Missing or unreadable required records, invalid decisions or
evidence-integrity failures stop visibly before evaluation. Terminal
refusals, orphaning, usage-limit and review-directory failures retain their
own stop classifications and cannot use this recovery path.

Private directories and files normally use modes `0700` and `0600`.
Hashes check integrity against trusted state; they do not authenticate the
agent. Instructions and same-user permissions do not isolate the decision
step from arbitrary code running as that user.

## Head check outcomes

Dispatch reads the PR's head on GitHub and compares it with the head the
round reviewed. No GraphQL call is made on this path: `gh pr view --json
headRefOid` is not used, so an exhausted GraphQL allowance cannot decide a
clean round (#2333). The merge step's own reads are separate and are
described in [Auto-drive REST Reads](autodrive-rest-reads.md).

### Where the PR number comes from

`autodrive_completion_context.sh` passes the merge loop's existing `AD_PR` to
Prepare as `--pr <n>` when it matches `^[1-9][0-9]{0,9}$`, and Prepare stores
it in the private state as `pr`. Nothing else supplies it. It is never read
from the round record, from the evaluator context, or from any file the
round's agent can write. A state file written before this field existed has
no `pr`, and a `pr` of `0` counts as absent; dispatch then resolves the head
from the PR worktree's current branch, whose name is validated before it is
composed into a ref.

### The two read routes

| Order | Read | Used when |
| --- | --- | --- |
| 1 | `git ls-remote <canonical origin URL> refs/pull/{n}/head` | Always first. It is correct for fork PRs as well as same-repository ones, and uses the git protocol, so neither API allowance applies to it. |
| 2 | `gh api repos/{owner}/{repo}/pulls/{n} --jq .head.sha` | Route 1 answered with no line. |

These are two routes to one fact, not a retry schedule. Neither is repeated,
nothing sleeps between them, and no further read follows the second.

`origin` is resolved from `remote.origin.url` and has to read as
`https://github.com/<owner>/<repo>` or `git@github.com:<owner>/<repo>`, with
or without a `.git` suffix. Any other value is `origin-not-github` and gives
the unavailable outcome, never `Matches` and never `Differs`. The verified
`<owner>/<repo>` pair is then used on both routes, rather than letting `gh`
resolve the repository from the worktree's remotes, and `git ls-remote` is
invoked with the canonical HTTPS URL as a literal argument, from a directory
outside the PR worktree, with `GIT_CONFIG_NOSYSTEM=1`,
`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_COUNT`
removed, `GIT_TERMINAL_PROMPT=0`, and `GIT_ASKPASS` and `SSH_ASKPASS`
removed. The round's own agent writes inside that worktree, so a remote, an
`insteadOf` rewrite or a credential helper configured there must not reach
this read; and a remote that asks for credentials must fail rather than wait
for an answer that will not come.

Both SHAs are validated as 40 or 64 lowercase hex characters and compared as
whole strings. Exactly one `ls-remote` line with a valid first field counts
as an answer: zero lines falls through to route 2, and more than one line is
unavailable rather than a choice among them.

### The three outcomes

| Outcome | When | What dispatch does |
| --- | --- | --- |
| Matches | A route answered with the reviewed head SHA. | The decision proceeds as before: `complete` writes the `.dispatched` receipt and returns `loop_result` `DONE` with `evaluator_invoked` `false`. |
| Differs | A route answered with another SHA. | An error, naming both commits: the reviewed head is no longer the PR's head, so the completion claim is stale. It fails closed and grants no merge authority. The message says only that the head differs; it never says "unavailable". |
| Unavailable | No route answered, the PR number could not be resolved, or `origin` is not a GitHub remote. | Not an error. Dispatch writes the `.dispatched` receipt and runs the loop-health evaluator with the fault stated in its context, so the round is judged by the agentic evaluation with the evidence in front of it. |

Before #2333 the two read failures and the moved head shared one message,
and an unavailable read ended the round as `INVALID_DECISION`, which stopped
the merge loop with `crusty-rereview-not-done`. Neither is reachable from an
unavailable read now. Dispatch has no `WAITING` result and no exit code of
its own for this case: the exit codes `79` and `75` keep
their existing meanings, and nothing new enters that space.

### The dispatch line

A `Matches` completion is unchanged except for an additive `head_read` field:

```json
{"loop":"crusty","loop_result":"DONE","round_label":"round-14","round_verdict":"CLEAN","source":"objective-completion","evaluator_invoked":false,"head_read":"matches"}
```

An unavailable read runs the evaluator, so the line is the evaluator's, with
`evaluator_invoked` `true` and `head_read` naming the fault:

```json
{"loop":"crusty","loop_result":"CONTINUE","round_label":"round-14","source":"loop-health-evaluator","evaluator_invoked":true,"head_read":"unavailable:read-failed"}
```

### What the evaluator is told

An unavailable read inserts `loop_head_read` into the evaluator's context,
alongside `loop_ci_signal` and the other declared keys. Its default is the
empty string, so every other caller is unaffected, and the step-02 prompt
states that a non-empty value is a fault in reading GitHub rather than
evidence about the branch, and that the round's own verdict still stands.

`loop_head_read` is not a member of `evaluator::FIELDS`, so a context key of
that name supplied through the shell is still rejected as an unknown field.
Dispatch inserts it directly, and applies the same constraints at that point:
one line, no NUL, no control characters, and a bounded length.

### The grammar of the two fields

Both fields carry program-generated tokens and nothing else. The token set is
closed:

| Token | Meaning |
| --- | --- |
| `read-failed` | Neither route answered. |
| `origin-not-github` | `remote.origin.url` is not a GitHub remote in an accepted form. |
| `pr-number-unresolved` | No PR number was supplied and none could be resolved from the worktree's branch. |
| `ambiguous-ref` | `git ls-remote` returned more than one line for the ref. |
| `bad-sha` | A route answered, but the SHA did not validate as 40 or 64 lowercase hex characters. |

`head_read`, on the dispatch line, matches:

```
^(matches|unavailable:(read-failed|origin-not-github|pr-number-unresolved|ambiguous-ref|bad-sha))$
```

`loop_head_read`, in the evaluator's context, carries the bare token and is
empty when the head read did not fail:

```
^(read-failed|origin-not-github|pr-number-unresolved|ambiguous-ref|bad-sha)?$
```

No exit status, no text from `gh` or `git` standard error, and no check name,
PR title, branch name, path or URL appears in either field. All of those can
be set by anyone who can open a pull request or name a workflow, and both
values are read by an agent.

There is no `differs` value in either field. A `Differs` outcome stops the
round with a dispatch error instead of writing a dispatch line, so the only
record of it is that error, which names both commits.

### Why an unreadable head cannot complete a round

The unavailable outcome is reachable by anything that breaks a read, so it
does not fail closed on its own. Three properties together keep it from
becoming a completion:

1. The head check runs only after `evidence::validate` has accepted the
   record, and only for a `CLEAN` or `MINOR_ONLY` verdict. An invalid record
   with an unreadable head is still `INVALID_DECISION`.
2. The record is bound by hash to its manifest row, with the review
   directory stamp compared.
3. The evaluator's `DONE` is downgraded to `CONTINUE` for an unresolved
   review, so an unavailable read costs one further Crusty round rather than
   completing one.

The third property is not special-cased for this path. The cost is one round
on a rare fault, against a clean round discarded every time a read failed.

## Results and accounting

Accepted objective completion returns these structured fields:

| Field | Value |
| --- | --- |
| `source` | `objective-completion` |
| `loop_result` | `DONE` |
| `evaluator_invoked` | `false` |
| `head_read` | `matches` |

The real round verdict remains `CLEAN` or `MINOR_ONLY`; it is not rewritten
to persuade an evaluator. Round records, reviewed-head evidence, sidecars,
manifests, history, progress rows and actual round usage remain recorded.
There is no evaluator verdict line, evaluator log or evaluator usage entry
for this dispatch. `DONE` here ends review iteration; it does not certify
that follow-up recording or phase finalization succeeded.

For `evaluate`, Rust launches the trusted evaluator as a sibling at the
existing session depth, preserving inherited policy ceilings, working
directory checks, usage accounting and evaluator result parsing. Unresolved
reviews retain no-progress detection and budget checks. Evaluator `DONE`
cannot advance an unresolved review: it triggers re-review or the existing
inconsistent-`DONE` stop when nothing changes. Genuine nonconvergence remains
`STUCK`.

Exit `79` is terminal and never retried. Orphaning and usage failures retain
their own classifications; neither becomes objective completion or an
evaluator-sourced `STUCK`.

## Follow-up finalization

Both Crusty entry points finalize in this order:

1. Collect the terminal round's required minor findings using validated
   review evidence.
2. File or verify each required follow-up issue and any required parent-task
   comment using the existing idempotent follow-up tools.
3. Validate receipts against the finding, task, PR, repository and review
   identities.
4. Record resolved concern ids and a phase-completion marker bound to the
   current review evidence. Only then permit advancement to merge-ready or
   return successfully from a rereview.

A filing failure, missing dependency, missing receipt or dry run with
unrecorded required follow-ups leaves the phase incomplete. An older marker
cannot authorize a newer unfinished review. Existing valid receipts remain
available for recovery without duplicate issues. Later merge-loop filing
remains idempotent, and finalization refuses to report readiness while a
required follow-up is unrecorded.

## Readiness assessment outcomes

> **Retired (#2575).** This section, down to
> [Behavior examples](#behavior-examples), describes the parsed readiness
> record (`CandidateRecord`), `helper autodrive assess` and `round`, and the
> finalisation steps `refresh-readiness`, `remeasure-readiness` and
> `accept-remeasured-readiness`. All of them were removed when merge
> readiness became an agentic step. Criterion 3 is now judged from the PR's
> own Crusty verdict comment, not from a record. See
> [Merge-ready round, merge and hand-off](autodrive-merge-ready-round.md),
> in particular [Crusty verdict at the head](autodrive-merge-ready-round.md#crusty-verdict-at-the-head).
> The text below is kept only to explain older runs' logs.

`amplihack orch helper autodrive assess` and `round` read the agent's
`CandidateRecord`. A parse fault, an unknown value in a tolerant string field
or stale evidence is reported as a recoverable, named blocker with exit 0 and
never produces readiness. The record rules are in the skill's
`evidence-contract.md`.

Faults found by `validate_candidate` after these checks remain hard errors
and exit non-zero, for example a criterion that is incomplete and has no
cause. Making those recoverable is tracked separately in #2539.

| Fault | Blocker |
| --- | --- |
| `null`, missing or `"unknown"` value in a tolerant string field | `unknown-field:<path>`, for example `unknown-field:attempts[0].cause` |
| Any other parse failure, such as a bad enum value or an unknown key | `evidence-parse-error:<class> at line <n> column <n>` |
| Evidence head differs from the worktree `HEAD` | `stale-evidence-head` |
| Evidence base differs from the current base | `stale-evidence-base` |
| Evidence merge-base differs from the current merge-base | `stale-evidence-merge-base` |
| The worktree `HEAD` cannot be read, for example in a repository with no commits | `current-head-unreadable` |

The field at line 1 column 26 in the failure reported in #2556 is
`attempts[0].cause`: in `{"attempts":[{"cause":null` the `null` token ends at
column 26. The writer rule in `evidence-contract.md` and in the refresh,
remeasure and merge-round prompts is `"cause":"none"` for an attempt that
succeeded and `"unknown"` for any value that could not be observed. The
reader treats `null`, a missing key and `"unknown"` the same way, so a writer
that breaks the rule produces a blocker, not a parse failure that ends the
round. No output ever contains serde's message text, such as
`invalid type: null, expected a string`.

Unknown-field and parse blockers come first, followed by drift or
`current-head-unreadable` blockers.

The merge-base is compared only when the head and base match. Once the head
or base has drifted, the merge-base check is skipped and only the drift
blockers are reported. When the worktree `HEAD` cannot be read, no
comparison is made: the only drift blocker is `current-head-unreadable`, and
both `current_head` and `current_base` are printed as `""`.

Integrity faults stay hard errors and exit non-zero, because refreshing the
evidence cannot fix them: oversize input, an invalid binding, an invalid
branch name, a head branch that does not match, grafts, and every Git
failure other than reading `HEAD`.

For any of these faults, `assess` prints the following, writes no snapshot
and changes nothing in the state directory:

```json
{"status":"refresh_required","accepted":null,"readiness":"incomplete","refresh_required":true,"evidence_head":"<40 or 64 hex, or empty>","current_head":"<40 or 64 hex, or empty>","current_base":"<40 or 64 hex, or empty>","blockers":["stale-evidence-head"]}
```

Each SHA field holds 40 or 64 hexadecimal characters, or `""` when the value
does not have that form.

An accepted candidate prints the existing result with `"status":"accepted"`
and `"refresh_required":false` added.

For the same faults, `round` writes a round record with
`merge_ready_verdict` `NOT_MERGE_READY`, `test_signal` `BLOCKED`,
`ci_signal` `UNVERIFIED`, `accepted_evidence` null and the `blockers`
array, and writes the blockers one per line to `<record>.findings`. The
loop's progress check reads that file, so a writer fault that repeats at the
same head stops the loop as no progress with the blocker named.
Authorisation, the state directory, a record path whose parent is not the
state directory and an existing record path remain hard errors, checked
before anything is written.

`autodrive-finalize.yaml` measures stale evidence again once:

1. `accept-refreshed-readiness` assesses the refreshed candidate.
2. When its `status` is `refresh_required`, `remeasure-readiness` builds a
   new `CandidateRecord` at the current head, base and merge-base, and
   `accept-remeasured-readiness` assesses it through the full pipeline. The
   second result replaces `accepted_readiness`.
3. `finalize-observed-action` and `reduce-observed-outcome` run only when
   `status` is `accepted`.
4. Otherwise `report-unaccepted-readiness` reports
   `{"outcome":{"readiness":"incomplete","blockers":[...]},"accepted_snapshot":null}`.

There is no further loop and no timer. A remeasured candidate gains no trust
from the first assessment. `validate_reduction` still requires the evidence
head to equal the worktree head, so a merge is never reduced against a head
that moved after acceptance; an unreadable worktree `HEAD` also fails
reduction. Evidence measured at a newer head still needs a Crusty
review covering that head: `review::verify` reports `crusty-review-required`
for substantive commits after `reviewed_head_sha`, and `completion::verify`
rejects a receipt bound to another head.

## Behavior examples

| Input and evidence | Observable result |
| --- | --- |
| Six persisted `MINOR_ONLY` rounds, then another valid `MINOR_ONLY` round | `source=objective-completion`, `loop_result=DONE`, `evaluator_invoked=false`; file or verify follow-ups before advancing. No evaluator-sourced `STUCK`. |
| Six legacy minor-only results accumulated within one process, then a valid completion decision | The same result at the dispatch boundary. Corrected iteration exits on its first valid completion rather than generating six extra rounds. |
| Valid `MINOR_ONLY`, but issue creation or receipt verification fails | Review iteration ends; phase completion and advancement are refused. |
| Valid `CLEAN`, with no outstanding follow-up obligation | Objective completion, no evaluator call, then phase finalization. |
| Major findings recur with no measured progress | Existing no-progress detection or evaluator judgment stops as `STUCK`. |
| Unresolved `CONCERNS`, evaluator says `DONE` | No phase advancement; require re-review and retain inconsistent-`DONE` safeguards. |
| Completion decision from another run, altered sidecar, changed reviewed head, or failed child | Reject completion; no phase marker. |
| Valid `CLEAN`, head reviewed and head on GitHub both `b5864181`, every GraphQL read unavailable | `head_read=matches` from `git ls-remote`; objective completion as usual. |
| Valid `CLEAN`, `git ls-remote` fails and the REST read is unavailable | The evaluator runs with `loop_head_read` set; `evaluator_invoked=true`. Not `INVALID_DECISION`, and the merge loop does not stop with `crusty-rereview-not-done`. |
| Valid `CLEAN`, a route answers with a SHA other than the reviewed head | A dispatch error naming both commits. No completion, no merge authority. |
| Open PR, worktree behind the PR head, no complete round at that head | Preflight fast-forwards the worktree; `should_run="true"`, `skip_reason=""`; the Crusty loop runs. |
| Open PR, worktree diverged from the PR head | `should_run="false"`, `skip_reason="worktree-diverged"`; the worktree is left unchanged. |
| Merge-ready verdict with `"cause":null` in an attempt that did not succeed | `round` exits 0 with `NOT_MERGE_READY` and `unknown-field:attempts[0].cause`. |
| Merge-ready verdict with the `cause` key removed from an attempt that did not succeed | `assess` exits 0 with `status=refresh_required` and `unknown-field:attempts[0].cause`. |
| Refreshed candidate bound to commit A while the worktree is at commit B | `assess` exits 0 with `status=refresh_required`, `stale-evidence-head` and `current_head` B; finalize remeasures once. The candidate re-bound to B is `accepted` with `binding.head_sha` B, and only then does `finalize-observed-action` run. |
| Remeasured candidate still refused | `report-unaccepted-readiness` records `readiness: incomplete` with the blockers and exits 0; recovery takes over. |
| Repository with no commits, where the binding names the unborn current branch | `assess` exits 0 with `status=refresh_required`, `current_head` and `current_base` `""`, and `current-head-unreadable`. |
| Candidate whose binding names another head branch | Hard error, non-zero exit; no refresh is attempted. |

## Configuration and failures

Completion-first dispatch requires no new operator setting and applies in
every repository. Existing `plan_task_id`, `autodrive_state_dir`,
`autodrive_dry_run` and `autodrive_budget_*` settings retain their roles; see
the [auto-drive skill](../claude/skills/auto-drive-to-merge/SKILL.md#invocation-and-authorization).
Pre-round budget checks still run before spending another review session.

An installation without the Rust helper fails preflight with an explicit
compatibility error before review starts. Invalid decision or evidence
errors require correction of the installation or evidence source; replaying
a decision or editing a phase marker cannot resume the run safely. Follow-up
failures leave existing receipts available for a rerun after the recording
problem is resolved. No completion decision authorizes a merge; the existing
merge-ready checks and finalization remain authoritative.
