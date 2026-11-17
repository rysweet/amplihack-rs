---
title: Auto-drive Merge-ready Round, Merge and Hand-off
description: how the merge-ready agent writes up the 13 criteria in the PR, how it reads the Crusty verdict from the PR, how the loop reads its outcome and waits for pending checks, the GitHub-only merge conditions, the base sync, the hand-off recipe, the run report, and how the round and loop steps run on private review checkouts with step recovery, and which invocations the outcome rule judges (#2575, #2628, #2663, #2096, #2539, #2769).
last_updated: 2026-11-17
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Merge-ready Round, Merge and Hand-off

Merge readiness in [auto-drive](auto-drive-to-merge.md) is an agentic step.
The merge-ready agent assesses the 13 criteria of the trusted
[merge-ready skill](../../amplifier-bundle/skills/merge-ready/SKILL.md) at the
PR head and posts its write-up as a PR comment for people to read. Nothing
parses that write-up as a condition of continuing or merging. A second agent
reads the outcome in one word, and the merge itself depends only on the
captured authorisation, the assessed head and GitHub.

This replaces the parsed readiness record, its strict validation, the
finalisation observation and the loop receipt (#2575), in the same way that
#2340 retired the shell merge gate. A write-up of any shape no longer stops
the drive.

## Contents

- [Round steps](#round-steps)
- [Review checkout and step recovery](#review-checkout-and-step-recovery)
- [Crusty verdict at the head](#crusty-verdict-at-the-head)
- [Outcome word](#outcome-word)
- [Round files](#round-files)
- [How the loop ends](#how-the-loop-ends)
- [Finalisation and merge conditions](#finalisation-and-merge-conditions)
- [Base sync](#base-sync)
- [Hand-off](#hand-off)
- [Run report](#run-report)
- [Tool reference](#tool-reference)
- [Security properties](#security-properties)
- [What was retired](#what-was-retired)

## Round steps

`autodrive-merge-round.yaml` runs these steps in order for each round, after
`step-00-tools-dir`, `step-00-review-dir-guard` and the evidence steps. Every
agent step that judges the PR (`step-01`, `step-01b`, `step-02` and `step-03`)
starts in `{{review_launch_dir}}` and reads the PR from the private review
checkout by path and SHA (see
[Review checkout and step recovery](#review-checkout-and-step-recovery)).
`step-04` alone works in the PR worktree, because it commits and pushes.

| Step | Kind | Output | Responsibility |
| --- | --- | --- | --- |
| `step-01b-crusty-evidence` | agent | `crusty_evidence` | Reads the newest authorised `Crusty verdict:` comment on the PR and judges whether it covers the current head. See [Crusty verdict at the head](#crusty-verdict-at-the-head). |
| `step-01a` | bash | `merge_ready_assessed_head` | Captures `git rev-parse HEAD` in the PR worktree before the assessment and checks it is 40 or 64 lowercase hex characters; any other value exits `2`. |
| `step-02` | agent, `parse_json: false` | `merge_ready_writeup` | Reads the pinned merge-ready skill, assesses all 13 criteria at `merge_ready_assessed_head`, posts one PR comment naming that SHA and ends with one sentence stating the outcome. Under `autodrive_dry_run=true` it prints the write-up instead of posting it. |
| `step-03` | agent | `merge_ready_outcome` | Answers one question about the write-up: did the assessment conclude ready, not ready, waiting, or hand-off? It replies with one word on its last line and, on the line before, `ci: pass`, `ci: pending`, `ci: fail` or `ci: none`. |
| `step-05a` | bash | `merge_ready_word` | Passes the word, the `ci:` line and `merge_ready_assessed_head` to `autodrive_round_word`. |
| `step-04` | agent | | Runs only when `merge_ready_word.word == 'not-ready'`. Works on the blockers named in the write-up, commits and pushes in the PR worktree. It does not run for `waiting`. |
| `step-05` | bash | | Writes the round files through `autodrive_round_file`, passing `merge_ready_assessed_head` and the round's `ci_signal`. |

The head is captured before `step-02` so that the `.head` round file names
the commit the agent assessed, not a commit made afterwards. `step-02` may
still commit or push by mistake, and `step-04` commits before `step-05`
runs; in both cases the worktree head no longer equals the captured head,
so finalisation refuses the merge and hands the PR off. The write-up is not
searched for the SHA; the comparison at finalisation is the check.

From round two onwards, `step-02` receives the previous round's write-up as
`prior_writeup` and works on the blockers it names. Round one receives an
empty value. The previous and current write-ups reach `step-02`, `step-03`
and `step-04` inside marked fences labelled as untrusted data, and the
existing untrusted-input and no-bypass instructions stay in each prompt.

The write-up is prose. Its shape does not matter to the drive: unknown keys,
a notice printed before a JSON block, a `null` field, a missing heading or
no JSON at all all lead to the same next step, which is decided by the
outcome word. What the write-up should contain is set out in the bundled
[evidence contract](../../amplifier-bundle/skills/auto-drive-to-merge/evidence-contract.md).

The round's output template names the outcome word and the commit the review
checkout held, as `**Reviewed commit:** {{reviewed_head_sha}}`. It carries no
parsed verdict and no `**Record:**` line.

## Review checkout and step recovery

The merge round and the merge loop run on the private review generations of
[checkout isolation](recipe-checkout-isolation.md#private-review-generations)
(#2096) and under [step failure recovery](step-failure-recovery.md) (#2539).
Both apply to the steps that remain after #2575; the retired steps listed in
[What was retired](#what-was-retired) are not wrapped and do not return.

### Context keys set by the loop

`autodrive_loop.sh` sets these round keys itself and passes them as `-c`
values after the caller's context, so the loop's values win. `repo_path`
comes from the loop's `--repo` argument; the other keys come from the Rust
helpers `internal review-next`, `review-launch` and `review-head`.

| Key | Value |
| --- | --- |
| `repo_path` | The PR worktree. |
| `review_dir` | The private review checkout, detached at the recorded PR head. |
| `review_launch_dir` | The trusted launch directory that judging agents start in. |
| `reviewed_head_sha` | The commit the review checkout holds. |
| `autodrive_launch_tools` | The launch install's tools directory. |

`step_recovery_dir` is not one of these keys. `autodrive_merge_loop_run.sh`
passes it to the loop as a `--context` value; the loop reads it and passes it
on to each round in the round context.

`--context` refuses `review_dir`, `review_launch_dir`, `reviewed_head_sha`,
`autodrive_launch_tools` and `autodrive_completion_state`. The round order of
context is the completion context, then `prior_writeup` (from the state
directory), then the round context, then the loop's own `-c` values.

### Where recovery applies

| Recipe | Step | Flags |
| --- | --- | --- |
| `autodrive-merge-round` | `step-00-review-dir-guard`, `step-02b-review-dir-unchanged` | `--guard` |
| `autodrive-merge-loop` | `step-02-merge-ready-loop` | `--essential --stop-on-handoff` |
| `autodrive-merge-loop` | `step-03-finalize` (sub-recipe, `continue_on_error: true`), recovered by `step-03r-finalize-recovery` | `--already-failed --essential` |

`step-02-merge-ready-loop` reads its short body (`exec bash "$RUN"`, which
runs `autodrive_merge_loop_run.sh`) from a quoted heredoc into `STEP_BODY` and
runs it under recovery. A run with no `step_recovery_dir` runs the body
directly.

```bash
[ -n "${RECIPE_VAR_step_recovery_dir:-}" ] || exec bash -c "$STEP_BODY" step-02-merge-ready-loop
exec amplihack orch helper step-recovery run --step-id step-02-merge-ready-loop --essential --stop-on-handoff \
  --dir "$RECIPE_VAR_step_recovery_dir" \
  --goal "Run the merge-ready loop to a merge-ready verdict for PR ${RECIPE_VAR_pr_number:-unknown}" \
  -- bash -c "$STEP_BODY" step-02-merge-ready-loop
```

`step-03r-finalize-recovery` runs immediately after `step-03-finalize`, only
when `merge_loop_outcome.finalise == 'true' and not autodrive_finalization`.
A loop that handed the PR off never reaches finalisation, so recovery never
runs finalisation without a `DONE` or `WAITING` loop result. The other loop
steps (`step-02a`, `step-02c` and `step-03b` to `step-03f`) are not wrapped.

### Review checkout lifecycle in `autodrive_merge_loop_run.sh`

The tool allocates the review generation itself:

1. As soon as it creates the task file, it installs
   `trap 'rm -f -- "$TASK_IN"' EXIT`, so that every exit after that point
   removes the task text: `task_description` unavailable, the Crusty
   re-review tool missing, no repository, the round recipe missing, the tool
   root unresolved, or no PR worktree or review checkout. The round-count,
   tools-directory and state-directory checks run earlier and exit before the
   task file exists.
2. It checks the PR worktree, then runs `amplihack internal review-allocate`
   on it.
3. At allocation it replaces the early trap with one that releases the
   review generation (`release_review_dir`, through `internal review-release`)
   and removes the task file.
4. It calls `autodrive_loop.sh` with `--context step_recovery_dir=<dir>`
   after `repo_path`, and with the allocated checkout as `--review-dir`.

The loop never receives `--invocation`; merge permission is read only by
`autodrive-finalize` through `helper autodrive authorization --invocation`.

## Crusty verdict at the head

Criterion 3 inside auto-drive is met by a Crusty verdict posted on the PR
itself, not by a record file in the state directory (#2628). A drive that
starts with an empty or missing state directory still finds the verdict, and
a missing record never blocks the drive.

### Writer: the Crusty round posts its verdict

After `step-03-extract-crusty-verdict`, `autodrive-crusty-round.yaml` runs the
agent step `step-03c-post-verdict`. It posts one PR comment whose first line
is exactly:

```text
Crusty verdict: MINOR_ONLY at 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
```

The verdict is `CLEAN`, `MINOR_ONLY` or `BLOCKING`, and the SHA is the full
40- or 64-character head the round reviewed. The verdict summary follows on
the next lines.

| Rule | Detail |
| --- | --- |
| Transport | The body goes in on standard input: `gh pr comment <pr> --body-file -`. If GraphQL refuses, the step makes one REST call instead: `gh api repos/{owner}/{repo}/issues/<pr>/comments -f body=@-`. |
| Nested lines | Any summary line that starts with `Crusty verdict:` is prefixed with `> `, so only the first line can be read as a verdict. |
| Duplicates | The step reads the comments back once and does not post again when a comment with the same head and verdict already exists. |
| Approvals | It never calls `gh pr review --approve`. The verdict is a comment, not a review. |
| Content | No tokens, environment values or home-directory paths. |
| Dry run | Under `autodrive_dry_run=true` it posts nothing. |
| Failure | A failed post does not change the round's result. The next merge round then finds no verdict at the head and criterion 3 is not met, so the PR does not merge. Step-06 records no posted line, so the next before-round step runs Crusty again (see [Running Crusty when there is no verdict](#running-crusty-when-there-is-no-verdict)). |

#### The verdict form

The drive counts only a comment whose first line is exactly
`Crusty verdict: <CLEAN|MINOR_ONLY|BLOCKING> at <full sha>`, with a full 40-
or 64-character lowercase SHA. A verdict in any other form, including a
shortened SHA or a verdict on a later line, counts as no verdict. A person or
lane that reviews outside the drive and wants the drive to count the verdict
posts it in this form, as the PR owner, a member or a collaborator.

#### The posted line

`step-03c-post-verdict` ends its reply with one JSON line,
`{"verdict_posted":"true|false","head_sha":"<full sha>"}`: `true` only when it
posted the comment, or found its own comment with that exact first line.
After writing the round record, step-06 runs `autodrive_crusty_posted.sh`.
When that line says `true` for the head the round reviewed, the tool appends
`<WORD> <reviewed sha>` to `crusty-verdict-posted.txt` in the state
directory; otherwise it records nothing. No tool reads PR comments (#2628).
The agent's word decides only whether Crusty runs again, never a merge: a
wrong `true` leaves criterion 3 unmet, as before this change. Under
`autodrive_dry_run=true` the line is recorded without a post, because nothing
was posted and Crusty would only repeat itself.

### Reader: `step-01b-crusty-evidence`

`step-01b-crusty-evidence` in `autodrive-merge-round.yaml` is an agent
judgement. No tool parses or gates on the comment. The agent:

1. Reads the drive's own login once with `gh api user`.
2. Lists the PR's comments with
   `gh api repos/{owner}/{repo}/issues/<pr>/comments --paginate`, reading
   `.user.login`, `.author_association`, `created_at` and `updated_at`.
3. Keeps only comments from an authorised author: the drive's own login, or
   an author whose association is `OWNER`, `MEMBER` or `COLLABORATOR`.
   Comments from anyone else are ignored and named in `crusty_reason`.
4. Takes the newest authorised comment whose first line starts with
   `Crusty verdict:`, ordered by `updated_at`, then `created_at`.
5. Judges criterion 3 from that comment.

| Newest authorised verdict comment | Criterion 3 | Reason |
| --- | --- | --- |
| `CLEAN` or `MINOR_ONLY` at the full current head SHA | met | |
| `CLEAN` or `MINOR_ONLY` at an earlier full SHA, and the range check below prints `ok` | met | |
| `BLOCKING` at the full current head SHA | not met | `quality-audit-convergence-crusty-not-done-clean` |
| Names another head that the range check does not cover, or a shortened SHA | not met | `crusty-review-required` |
| None | not met | `crusty-review-required` |

Only the full SHA counts. A comment written by a person with a 7-character
SHA counts as no verdict; the cost is at most one extra Crusty pass.

A [base sync](#base-sync), or a commit that changes only the PR description
or evidence files, moves the head without needing a new review. For a verdict
at an earlier head, the agent runs the trusted, read-only range check. It is
the only tool step-01b runs. The tool comes from the round's pinned tools
directory, the `autodrive_tools_dir` output of `step-00-tools-dir`, never
from `$AMPLIHACK_HOME` directly and never from the PR worktree. The prompt
gives the agent the path as `{{autodrive_tools_dir}}`:

```bash
bash "{{autodrive_tools_dir}}/autodrive_round_evidence.sh" crusty-covers 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
# ok
```

It walks the commits from the verdict's SHA to `HEAD` with the same rules as
the before-round re-review check (`autodrive_crusty_range`): `ok` when every
later commit is a clean base merge or an allowed description or evidence
change; otherwise `crusty-unreviewed-commits` or `crusty-range-unreadable`,
and criterion 3 is not met. The SHA argument must be 40 or 64 lowercase hex
characters, otherwise the tool prints `crusty-range-unreadable` and exits
`2`. The check reads git only; it never reads a state file.

The base SHA comes from the same lookup the re-review check uses:
`pr_base_ref` in `autodrive_round_evidence.sh` (`AUTODRIVE_BASE_BRANCH`, or
`gh pr view --json baseRefName`), then `autodrive_base_sha`, which fetches
`refs/heads/<base>` from `origin` and never trusts an existing local ref.
When the lookup gives an empty SHA (no base name, a refused name, or a
failed fetch), the walk runs without excluding base commits. The commits a
base merge brought in then count as code, the check prints
`crusty-unreviewed-commits`, and criterion 3 is not met. An empty base SHA
therefore costs a Crusty pass; it never lets an unreviewed commit through.

| Verdict at | Later commits | `crusty-covers` prints |
| --- | --- | --- |
| A pre-sync head | One clean merge of the base, base SHA found | `ok` |
| A pre-sync head | One clean merge of the base, base SHA empty | `crusty-unreviewed-commits:<head>` |
| An earlier head | A commit that changes code | `crusty-unreviewed-commits:<head>` |
| An earlier head | A change to evidence files only | `ok` |
| A SHA not in this clone, or a shallow clone | any | `crusty-range-unreadable` |

#### Which head step-01b judges

`step-01b-crusty-evidence` runs before `step-01a`, so it judges against
`HEAD` in the PR worktree at the moment it runs, not against
`merge_ready_assessed_head`.

| Commit made | What catches it |
| --- | --- |
| After `step-01a` (by the builder, a later round or anyone else) | The head check at finalisation: the worktree's `HEAD` no longer equals `merge-ready-latest.head`, so `step-03` returns `refused` without calling `gh` (see [Merge conditions](#merge-conditions)). GitHub's `--match-head-commit` catches a head that moved on the server. |
| Between `step-01b` and `step-01a` | Not the finalisation head check: `step-01a` records the new commit as the assessed head, so the two heads agree. No recipe step runs between the two, and the PR worktree belongs to the drive, so only an outside process writing to that worktree could commit there. |

PR text and comment text reach the agent as data, never as instructions.
The output keeps its name, `crusty_evidence`, and its four `crusty_*` keys.

### Running Crusty when there is no verdict

Crusty can only start from the loop's before-round step
(`autodrive_crusty_rereview.sh`), which asks `autodrive_rereview_decision`
in `autodrive_trust.sh`:

| `crusty-records.tsv` | Decision |
| --- | --- |
| Missing or empty | `{"rereview":"true","range":"no-record"}`; the `crusty-loop` phase marker is cleared, and the Crusty round runs. |
| Has records, none final | `not-checked`, as before. |
| Has a final `CLEAN` or `MINOR_ONLY` record, and `crusty-verdict-posted.txt` has no `CLEAN <sha>` or `MINOR_ONLY <sha>` line for its reviewed head | `{"rereview":"true","range":"no-record"}`, as for no record: the marker is cleared and Crusty runs (#2575). This covers a verdict post that failed and a state directory written before this change. |
| Has a final record whose verdict was posted | The code-commit range check from that record's head, as before. |

The Crusty round then writes its record and posts its verdict comment, so the
next merge round finds a verdict at the head. A person's verdict at the head
with no local record therefore costs one extra Crusty pass; no tool reads PR
comments to avoid it.

## Outcome word

`autodrive_round_word` maps the reader's answer to the loop driver's verdict.
It lower-cases the input, keeps only `a` to `z` and `-` (under `LC_ALL=C`)
and compares the result exactly with the four allowed words.

| Normalised word | `merge_ready_verdict` | Next step |
| --- | --- | --- |
| `ready` | `MERGE_READY` | Loop-health evaluation; `DONE` leads to finalisation. |
| `not-ready` | `NOT_MERGE_READY` | `step-04` repairs the named blockers; another round follows. |
| `waiting` | `WAITING` | Everything is ready except one or more required checks still running at the head. The loop ends with `WAITING` and finalisation runs. No builder, no evaluator. |

`WAITING` goes to finalisation on the reader's one word. `MERGE_READY` also
needs the loop-health evaluator's `DONE` before the loop ends; `WAITING` does
not. This is not a safety gap: finalisation still requires green or absent
required checks and GitHub's acceptance of the merge, and a pending check ends
`checks-pending` without a merge request.
| `hand-off` | `HANDOFF` | The loop stops and the PR is handed off. |
| anything else, including empty | `NOT_MERGE_READY` | Treated as not ready, so another round runs. |

An unclear answer therefore costs one more round. It never ends the drive
with `STUCK` and never leads to a merge.

The `ci:` line from `step-03` becomes `ci_signal` only when, lower-cased and
with the `ci:` prefix removed, it is exactly `pass`, `pending`, `fail` or
`none`, and the head is 40 or 64 lowercase hex characters. The value is
`<state>@<head>`, for example `pending@15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605`.
Otherwise the `ci_signal` key is left out.

## Round files

`autodrive_round_file` writes three files per round into the run's
[state directory](auto-drive-to-merge.md#state-directory):

| File | Content |
| --- | --- |
| `merge-ready-<label>.json` | `{"merge_ready_verdict":"MERGE_READY"}`, `NOT_MERGE_READY`, `WAITING` or `HANDOFF`, plus `ci_signal` (for example `"ci_signal":"pending@15a7f6f4…"`) when the round reported a valid one. |
| `merge-ready-<label>.writeup` | The write-up, cut at 256 KiB with a final truncation line. |
| `merge-ready-<label>.head` | The head captured by `step-01a` before the assessment: 40 or 64 lowercase hex characters. |

The loop's record step copies the last `.writeup` and `.head` to
`merge-ready-latest.writeup` and `merge-ready-latest.head`. Finalisation uses
`merge-ready-latest.head` as the expected head of the merge.

The `.json` round file is loop plumbing, not a readiness record. The loop
driver reads `merge_ready_verdict` and `ci_signal` from it only when
`LOOP_NAME=merge-ready`. `ci_signal` is written only when it matches
`^(pass|pending|fail|none)@([0-9a-f]{40}|[0-9a-f]{64})$`; the record step
checks it against the same pattern again and uses `''` when it does not match.
The former fields `blocker_count`, `head_sha`, `test_signal` and
`<record>.findings` are no longer written for the merge-ready loop. The crusty loop keeps its own round records,
written by `autodrive_crusty_record.sh`.

## How the loop ends

`autodrive-merge-loop.yaml` `step-02` calls the trusted tool
`autodrive_merge_loop_run.sh`, which runs `autodrive_loop.sh` with
`LOOP_NAME=merge-ready`. The step's values reach the tool as quoted
`RECIPE_VAR_*` environment variables, never by templating into the command.
An optional first argument asks for that many further rounds; it must
match `^[0-9]+$` and be at least `1`, and the tool turns it into a budget of
the rounds already used plus that number (see
[`autodrive_merge_loop_run.sh`](#autodrive_merge_loop_runsh)). The step keeps its default `fatal`
handling and the tool maps the driver's exit status itself, so that exit `79`
is never hidden by `continue_on_error` (#2490).

| Driver exit | Loop result (read from `merge-ready-result.json`) | Step exit |
| --- | --- | --- |
| `0` | `DONE` or `WAITING` | `0` |
| `1` | `USAGE_LIMIT` | `1`: the run stops; a later run continues the work. |
| `1` | `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF` | `0`, with the loop line printed. |
| `1` | result file missing or unreadable, or `DONE` or `WAITING` (the driver writes those only when it exits `0`) | `0`, with loop result `UNREADABLE`. |
| `0` | anything other than `DONE` or `WAITING`, or no readable result | `0`, with loop result `UNREADABLE`. |
| `75` | `ORPHANED` | `75` |
| `79` | `TERMINAL_REFUSAL`, set by the step; the result file is not consulted | `79` |
| `2` or any other value | | passed through unchanged |

The loop result comes from the driver's own result file, read with `jq`.
Text on standard output, including JSON an agent printed, does not decide it.

Exit `79` means a `gh` call refused for a terminal reason, such as an
authentication or account refusal. The step prints a `TERMINAL_REFUSAL` loop
line as its output and then exits `79`, so the run stops there and no
hand-off or finalisation runs. The driver's own standard output goes to the
step's standard error, for the log; the step's output is only the line it
builds from the result file.

A `HANDOFF` round writes a `HANDOFF` line through `record_result` before the
evaluator runs, then the driver exits `1`. A round that writes no write-up or
head leaves none behind, so no earlier assessment reaches the next round or
the hand-off.

### A pending check is a wait, not a lack of progress

A `WAITING` round is handled in `autodrive_loop_record.sh`. It calls
`record_result` with
`{"loop_result":"WAITING","round_label":"...","round_verdict":"WAITING","ci_signal":"pending@<head>"}`
and the loop exits `0` before the progress stop, the budget charge and the
evaluator. The builder (`step-04`) does not run. A round whose only open
item is a required check still running at the head can therefore never be
counted as no progress and never leads to `STUCK`.

Other rounds pass their `ci_signal` to the
[loop-health evaluator](loop-health-evaluator.md) as `loop_ci_signal`. The
evaluator's rule: when `loop_ci_signal` is `pending@<head>` and the head has
not changed, the verdict is `CONTINUE`, never `STUCK`. Repeated rounds with no
movement and no pending signal, including `fail@<head>`, are still `STUCK`.
The budget stop still bounds the loop.

### Loop outcome

`step-02a` turns the loop result into
`{"outcome":"done"|"handoff","finalise":"true"|"false","reason":"...","loop_result":"...","stop_comment_posted":"true"|"false","head_sha":"..."}`,
with the reason cut to 1000 bytes of printable ASCII:

| Loop result | Outcome | `finalise` | Next |
| --- | --- | --- | --- |
| `DONE` | `done` | `true` | `step-03` runs [`autodrive-finalize.yaml`](#finalisation-and-merge-conditions). |
| `WAITING` | `done` | `true` | `step-03` runs finalisation, which merges if the checks have passed by then, or ends `checks-pending` and hands the PR off. |
| `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF`, `UNREADABLE`, anything else | `handoff` | `false` | `step-02c` runs [`autodrive-handoff.yaml`](#hand-off). |

`step-03` runs only when `merge_loop_outcome.finalise == 'true'`. When
finalisation returns `base-behind`, the loop runs the [base sync](#base-sync).

`step-02a` reads `head_sha` with `git rev-parse HEAD` in the PR worktree and
sets `stop_comment_posted=true` for `BUDGET_EXHAUSTED`, and `step-02c`
passes both to the hand-off, because the driver has already posted a stop
comment for a budget stop, so the PR receives one comment rather than two.

## Finalisation and merge conditions

`autodrive-finalize.yaml` has four steps.

| Step | Kind | Output | Responsibility |
| --- | --- | --- | --- |
| `observe-finalization-entry` | agent, `parse_json: false` | | Re-observes the PR at entry and states its state and head SHA. |
| `merge` | bash, `parse_json: true` | `finalize_merge` | Applies the merge conditions below. |
| `state-outcome` | agent | `autodrive_finalization` | States in prose whether the PR merged and, if not, why. |
| `handoff` | sub-recipe | | Runs `autodrive-handoff.yaml` when `finalize_merge.handoff == 'true'`. |

The recipe's output is `{{autodrive_finalization}}`.

### Merge conditions

The `merge` step requests a merge only when every condition holds, checked in
this order:

1. `amplihack orch helper autodrive authorization` succeeds for this run's
   captured invocation and state directory and prints `"allows_merge":true`
   and, for `--pr <pr_number>`, `"pr_selected":true`: when the operator
   selected a PR at launch by number (`pr_number`) or URL (`pr_url`), only that
   PR is merged. Any other PR gives `refused` and is handed off.
2. The context values `no_merge` and `autodrive_dry_run` are both exactly
   `false`. They can narrow the captured permission, never grant it.
3. `merge-ready-latest.head` holds a well-formed SHA and the PR worktree's
   `HEAD` still equals it. A different head means the PR changed after the
   assessment; the step returns `refused` without calling `gh`.
4. `autodrive_merge` finds the PR open, its checks green or absent (see
   [Checks](#checks)) and its `mergeStateStatus` not `DIRTY`, then GitHub
   accepts `gh pr merge --squash --match-head-commit <expected sha>`.

GitHub's acceptance covers required reviews, branch protection and a head
that moved on the server. No agent-written text is a merge condition: an
agent can stop the drive, hand it off or start another round, but cannot
cause a merge.

### Checks

`autodrive_merge` reads `gh pr checks <pr> --required` once.

| Observation | Result |
| --- | --- |
| Exit `0`: required checks all passed | Merge is attempted. |
| Exit `8`: a required check is still running | `checks-pending` (tool exit `7`); no merge attempt. The PR is handed off. The reason names the pending checks. |
| `gh` reports `no required checks reported` on stderr, with nothing on stdout | Falls back to `gh pr checks <pr>` (all checks): exit `0` continues, exit `8` is `checks-pending`, `no checks reported` on stderr with nothing on stdout continues, anything else is `checks-not-green`. The message is never matched in stdout, where check names from the PR's own workflows appear. |
| Any other non-zero exit, including a failing check | `checks-not-green` (tool exit `3`); no merge attempt. |

**Pending checks are handed back, not waited on.** A `checks-pending`
result ends the drive with `not merged: waiting for <check> at <sha>`. No
drive is left running to wait for the checks, so the finalisation hands the
PR off: it becomes a draft, and the hand-off comment names the pending checks
and the head. Once the checks pass, a person marks the PR ready and starts a
drive, which reads the check runs again and merges. A PR is never left ready
for review with no drive. There is no sleep, no `gh pr checks --watch` (which
polls on an interval) and no counter.

**A PR with no checks at all is not refused for that reason.** GitHub does not
require checks; when none are reported, the merge request goes ahead and
GitHub's branch protection decides. A repository that relies on checks must
make them required in branch protection.

Check names in the reason have control characters removed and are cut to 300
bytes before they go into the JSON, which is built with `jq --arg`.

### Merge state

`autodrive_merge` reads `mergeStateStatus` in the same `gh pr view` call as
the PR state, title and body. It compares the value only with `case`
statements whose default arm is the safe result.

| When | `mergeStateStatus` | Result |
| --- | --- | --- |
| Before the merge request | `DIRTY` (conflicts with the base) | `base-behind` (tool exit `8`); `gh pr merge` is not called. |
| Before the merge request | anything else | The merge request goes ahead. `BEHIND` alone does not stop it; GitHub decides whether an out-of-date branch may merge. |
| After GitHub refuses the merge, read once more | `BEHIND` or `DIRTY` | `base-behind` (tool exit `8`). |
| After GitHub refuses the merge | anything else, including empty or unknown | `refused` (tool exit `4`). |

`--squash --match-head-commit` is always used, so the merge can only land the
assessed head. A `gh` exit `79` stays terminal.

### Merge message

The squash subject and body come from the trusted
`$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh`, as before
(#2056; see [the squash commit message](auto-drive-to-merge.md#the-squash-commit-message-2056)).
The helper is sourced only after the absolute-path and non-symlink checks, and
the body reaches `gh` on standard input:

```bash
printf '%s' "$MERGE_BODY" | gh pr merge "$pr" --squash --match-head-commit "$sha" --subject "$subject" --body-file -
```

### `finalize_merge` values

| `result` | `autodrive_merge` exit | Step exit | `handoff` |
| --- | --- | --- | --- |
| `merged` | `0` | `0` | `false` |
| `not-authorised` | not called | `0` | `false` |
| `authorization-unreadable` | not called | `0` | `true` |
| `checks-not-green` | `3` | `0` | `true` |
| `refused` | `4`, or head moved before the call | `0` | `true` |
| `not-open` | `5` | `0` | `true` |
| `message-unavailable` | `6` | `0` | `true` |
| `checks-pending` | `7` | `0` | `true` |
| `base-behind` | `8` | `0` | `false`; the merge loop runs the [base sync](#base-sync). `true` after a recovery re-run, which no base sync follows |
| | `2` (bad arguments or unsafe helper) | `2` | |
| | `79` | `79` | |

A `not-authorised` result is the expected outcome of a verify-only run
(`no_merge=true` or `autodrive_dry_run=true`), which stops before the
authorization helper is called, or of a captured invocation that does not
allow a merge. The PR stays open and is not drafted. On a merge run, a helper
that exits non-zero or prints no object with a boolean `allows_merge` gives
`authorization-unreadable`, and the PR is handed off. Its reason names only
the helper's exit status. A helper exit `79` passes through as `79`.

`autodrive-finalize.yaml` emits `base-behind` with `handoff false`: the
merge loop syncs the base instead, except after a recovery re-run (below). It emits `checks-pending` with
`handoff true`, so the PR is drafted with a comment naming the checks.

### The merge result in the state folder

The `merge` step prints its JSON line and also writes the same line, with a
`run_id` field, to `finalize-merge.json` in the runner-bound state folder
(mode `0600`, through a new file renamed over the old). The `run_id` is the
calling run's: `step-03r-finalize-recovery` passes it to the separate
`amplihack recipe run autodrive-finalize` process as
`autodrive_parent_run_id`; otherwise the step takes it from
`autodrive_authorization`. With no well-formed run id, or a state folder that
is missing or a symbolic link, nothing is written.

`report()` and the report step read this file only when the context holds no
`finalize_merge`, which happens when finalisation was re-run by recovery in
another process (#2575). They use it only when its `run_id` is this run's, so
a record another run left in a reused folder is ignored. `report()` reads it
through the private evidence store and adds the note
`finalize_merge: read from the state folder`.

A re-run by recovery is not followed by the merge loop's base sync. When it
finds the branch behind its base, its `merge` step emits `base-behind` with
`handoff true`, so the PR is drafted with a comment rather than left ready
with no drive. The run ends `not_merged`.

The runner writes `invocation.json` again once it has bound the state folder
to the invocation, so the authorization read below finds the bound folder.
Before this, the record held no state folder and every merge run's
finalisation returned `authorization-unreadable`.

## Base sync

When finalisation returns `base-behind`, `autodrive-merge-loop.yaml` brings
the base into the branch and assesses once more. These are steps written out
in sequence, each run once; there is no loop and no counter.

| Step | Kind | Runs when | Responsibility |
| --- | --- | --- | --- |
| `step-03b-base-sync` | agent (builder) | `finalize_merge.result == 'base-behind'` | Merges the base into the PR branch and pushes. Outputs `{"synced":"true"\|"false","reason":"..."}`. |
| `step-03c-sync-round` | bash | `synced == 'true'` | Runs `autodrive_merge_loop_run.sh 1`: the merge-ready loop for one more round (see [the round budget](#autodrive_merge_loop_runsh)). |
| `step-03d-sync-outcome` | bash | after `step-03c` | Maps the loop result as `step-02a` does, giving `sync_loop_outcome`. |
| `step-03e-finalize-again` | sub-recipe | `sync_loop_outcome.finalise == 'true'` | Runs `autodrive-finalize.yaml` once more. |
| `step-03f-sync-handoff` | sub-recipe | `finalize_merge.result == 'base-behind'` and `synced != 'true'` (including a base sync that returned no JSON), or `sync_loop_outcome.outcome == 'handoff'`, or a second finalisation that returned `base-behind` again | Runs [`autodrive-handoff.yaml`](#hand-off), naming the moving base. |

After the second finalisation, any result other than `merged` ends the drive
with `not merged: <reason>` or a hand-off. A second `base-behind` is not
synced again: the base moved while the drive assessed it, and the PR is
handed off with the reason `the branch is out of date with its base: the base
sync did not finish, or the base moved again after it`, so it is never left
ready with no drive.

### What the sync step does

1. Reads `baseRefName`, `headRefName` and `isCrossRepository` with
   `gh pr view`.
2. Checks both branch names against `^[A-Za-z0-9._/-]+$` with no leading
   `-`. A name that fails gives `synced:false`.
3. A PR from a fork (`isCrossRepository == true`) gives `synced:false`; the
   drive does not push to another repository.
4. Checks `git status --porcelain` in the PR worktree. Unrelated staged or
   modified files give `synced:false`.
5. Runs `git fetch origin -- <base>` and then `git merge origin/<base>`.
6. On any conflict, runs `git merge --abort` and gives `synced:false` with the
   reason `conflicts need review`. Conflict resolution is never pushed
   without review.
7. Pushes with `git push origin HEAD:<headRefName>`, with hooks on.

The step never uses `rebase`, `--force`, `--force-with-lease`, `--no-verify`,
`reset --hard`, `git add -A` or `core.hooksPath`. It stages only the merge's
own result.

A worked case: PR #2575 is `CLEAN` and ready at `15a7f6f4…`, but `main`
moved and GitHub refuses the merge. The read-back says `BEHIND`, so
finalisation returns `base-behind`. The sync step merges `origin/main` into
the branch as a new merge commit and pushes. One more merge-ready round
assesses the new head, and the second finalisation merges it at that head.

## Hand-off

`autodrive-handoff.yaml` is the single place where a drive gives a PR back to
people. It replaces every bare `STUCK` ending of the merge-ready loop.

| Input | Meaning |
| --- | --- |
| `pr_number` | PR to hand off; checked to be a positive integer. |
| `head_sha` | Head the drive last saw; checked against the SHA format before use. |
| `handoff_reason` | The loop result or merge result that caused the hand-off. |
| `handoff_blocker` | The blocker sentence from the last write-up or finalisation statement. |
| `stop_comment_posted` | `true` when the loop driver already posted a stop comment. |
| `autodrive_state_dir` | State directory; when `handoff_blocker` is empty the agent reads the blocker from `merge-ready-latest.writeup` there. |

The agent step runs `gh pr ready --undo` and posts one comment. When
`stop_comment_posted` is `true`, it still runs `gh pr ready --undo` and
skips only the comment. The comment states the reason, quotes the
blocker sentence in a fenced block, so that any `@name` or markdown in it
notifies nobody, and names the head SHA and PR number. Under
`autodrive_dry_run=true` it makes no GitHub write and lists what it would do.

A closing bash step reads `isDraft` back, makes no write call and exits `79`
if `gh` returned `79`. The recipe's output, `handoff_result`, is prose and is
never parsed.

Issue #2592 extends the hand-off. Any write it adds must sit behind the same
dry-run check; the recipe header states this.

## Run report

At the end of `auto-drive-to-merge`, the runner's `report()` builds
`autodrive_outcome`. It never fails and never panics; a badly shaped agent
output cannot fail the run report.

```json
{
  "schema_version": 2,
  "run_id": "a3f9c2e1-7b44-4d0e-9a51-2c6f0e8d1b37",
  "requested_action": "merge",
  "merge": {"result": "merged", "pr": 2575, "expected_head": "15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605", "handoff": "false"},
  "loop_result": "DONE",
  "stop_reason": "completed",
  "summary": "PR #2575 merged at 15a7f6f4 after all 13 criteria were met."
}
```

| Field | Source |
| --- | --- |
| `schema_version` | Always `2`. |
| `run_id` | The captured invocation. |
| `requested_action` | `merge` or `verify_readiness`, from the captured invocation. |
| `merge` | The `finalize_merge` JSON written by the merge step, reduced to `result`, `pr`, `expected_head` and `handoff`, or `null` when finalisation did not run or its output is unreadable. A PR merged before the run gives `{"result":"already-merged","pr":N}` with `stop_reason` `completed`. |
| `loop_result` | `merge_loop_result`: from the driver's result file, or `TERMINAL_REFUSAL` set by the loop step on exit `79`. |
| `stop_reason` | Derived from `loop_result` and `merge` (table below). |
| `summary` | `autodrive_finalization` verbatim, cut on a character boundary at `MAX_RECORD_BYTES`. |
| `summary_truncated` | Present and `true` only when the summary was cut. |
| `note` | Present only when something could not be read; a fixed sentence naming the context key, never file contents. |

The first matching row applies.

| Loop result | Merge result | `stop_reason` |
| --- | --- | --- |
| any value, missing or unreadable | `merged`, with `pr` and `expected_head` | `completed`; when `loop_result` is not `DONE` or `WAITING`, the `note` says `loop_result not consulted` |
| `TERMINAL_REFUSAL` or `TERMINAL_POLICY_REFUSAL` | | `terminal_refusal` |
| `DONE` or `WAITING` | `merged` | `completed` |
| `DONE` or `WAITING` | `not-authorised`, `requested_action` `verify_readiness` | `completed` |
| `DONE` or `WAITING` | `not-authorised`, `requested_action` `merge` | `not_merged` |
| `DONE` or `WAITING` | `checks-pending` | `waiting_for_checks` |
| `DONE` or `WAITING` | `base-behind` | `not_merged` |
| `DONE` or `WAITING` | any other result, including `authorization-unreadable` and unknown values | `handoff` |
| `DONE` or `WAITING` | `null`: finalisation did not run, or its output is missing or unreadable, with no exit `79` | `unreadable`, with a `note` naming the error kind and path |
| `HANDOFF`, `STUCK` | | `handoff` |
| `BUDGET_EXHAUSTED` | | `budget_exhausted` |
| `USAGE_LIMIT` | | `usage_limit` |
| `ORPHANED` | | `orphaned` |
| `UNREADABLE` or absent | | `unreadable` |

When the loop step exited `79`, `report()` sees `merge_loop_result` as
`TERMINAL_REFUSAL`. `report()` receives only the run context, not the exit
status of each step, so when the merge step exits `79` after a `DONE` loop,
`merge` is `null` and the report says `unreadable` with a `note`; the run
itself has already stopped with exit `79`, which is the terminal signal.

When `merge_loop_result` is absent, for example because the crusty loop
stopped first, `loop_result` is read from `crusty_loop_result` with the same
table, except that a crusty `STUCK` is reported as `no_progress`, as it was
before #2575 (crusty endings are not handed off), and a crusty `DONE` alone is
`unreadable`.

When the runner bound a state directory and the context or the build result
names a different one, `merge` is ignored (`null`) and the `note` says so.
When a PR is expected, a `finalize_merge` record without a numeric `pr`, or
with another PR's number, is also ignored.

A PR already merged at entry is still reported through the `already_merged`
path, which keeps its binding, selection, `run_id`, provenance and PR checks.

When a [base sync](#base-sync) ran, `merge` is the second finalisation's
`finalize_merge`.

### Success means merged

The rule in this section judges the drive's top-level invocation: a recipe run
named exactly `auto-drive-to-merge` or `auto-drive-light`. The nested
`autodrive-*` sub-recipes a drive runs, such as `autodrive-crusty-round` and
`autodrive-merge-round`, are not judged by it. Their success is the success of
their own steps. See
[Which invocations the rule judges](#which-invocations-the-rule-judges).

A top-level drive succeeds only when `stop_reason` is `completed`: the PR
merged in this run, was already merged at entry, or the run was verify-only
and reached `DONE` or `WAITING` (merge result `not-authorised`). The runner
decides this with the pure function `autodrive_run_succeeded` in
`crates/amplihack-cli/src/commands/recipe/run/execute.rs`, an allow-list of
the single value `completed`. A missing key, an empty value or any other
value is a failure.

GitHub's merge state decides, in both directions (#2663). A PR that GitHub
reads merged at the expected head is `merged`, and so `completed`, even when
`gh pr merge` reported an error or the merge has moved the base so that the
branch now reads `BEHIND`. No later base or readiness comparison is made
after the merge. A PR that GitHub does not read merged is never `merged`,
whatever `gh` printed or exited.

| `stop_reason` | Top-level drive result |
| --- | --- |
| `completed` | ✓ Success |
| `not_merged`, `waiting_for_checks`, `handoff`, `unreadable`, `budget_exhausted`, anything else | not successful |

After `report()` adds `autodrive_outcome`, the runner sets `success` to
`autodrive_run_succeeded(autodrive_outcome)` in both directions
(`apply_autodrive_outcome` in `execute.rs`), and only when the invocation is a
top-level drive:

- A top-level drive the runner reported as a success, but which did not merge,
  becomes a failure.
- A top-level drive that merged, and then failed at a later step (the
  state-outcome agent, the hand-off, the base-sync steps or the report agent),
  becomes a success. It carries no failure class, and admission records it as
  a success. Only a `merged` record with the expected PR, a valid
  `expected_head` and the runner-assigned state directory raises the status;
  exit `79` and other terminal refusals end the run with an error and are
  never raised.

This happens after `run_with_transient_retry` has returned, so a run that
did not merge never starts the whole-run re-attempt (#1267). The runner
prints one `INFO` line whenever it changes the status.

A verify-only run (`no_merge=true` or `autodrive_dry_run=true`) that reaches
finalisation did what it was asked: it ends `completed` and is ✓ Success, as
[Verify a PR without merging](../howto/verify-pr-with-auto-drive.md) states.
A merge run that ends `not-authorised` stays `not_merged`.

#### Which invocations the rule judges

Two predicates in
`crates/amplihack-cli/src/commands/orch/autodrive/trusted.rs` name auto-drive
invocations, and they are deliberately different widths (#2769).

| Predicate | True for | Decides |
| --- | --- | --- |
| `trusted::applies(name)` | `auto-drive-to-merge`, `auto-drive-light`, and any name beginning `autodrive-` | Whether the run captures a merge authorisation, launches from a private bundle copy, stamps its recipes and steps, and infers auto-drive context |
| `trusted::is_top_level_drive(name)` | `auto-drive-to-merge` and `auto-drive-light`, by exact match | Whether the outcome rule above sets the run's `success` flag |

`is_top_level_drive` compares the invocation name with two literals. It has no
prefix rule, no case folding and no configuration, so a sub-recipe cannot
claim or disclaim the outcome rule by renaming itself into or out of a prefix.
The predicate reads the name, not the depth, despite what its own name
suggests: a run named `auto-drive-to-merge` is judged by the rule wherever it
is invoked from. Nothing invokes one that way today. `auto-drive-light`
escalates through an in-process `type: recipe` step, which never re-enters the
run entry point that applies the rule.

`applies` keeps its own width: authorisation capture, bundle selection and
stamping are unchanged, and every `autodrive-*` sub-recipe still runs under a
captured authorisation.

A nested round keeps all of its outcome data. Every invocation `applies`
accepts is written the same five keys, in the same places:

| Key | Result `extra` | Context |
| --- | --- | --- |
| `autodrive_outcome` | ✓ | |
| `autodrive_summary` (the same object) | | ✓ |
| `autodrive_authorization` | ✓ | ✓ |
| `no_merge` | | ✓ |
| `autodrive_dry_run` | | ✓ |

`tools/autodrive_loop.sh` and the completion helpers read them there. Only the
`success` flag, and with it the process exit status, is left to the run's own
steps.

`autodrive-finalize` is the one nested invocation whose exit status a caller
observes directly: `step-03r-finalize-recovery` runs it as a separate
`amplihack recipe run autodrive-finalize` process. Under the old rule that
child's exit status tracked the PR's merge state rather than its own work: a
finalisation that returned `base-behind`, `checks-pending` or `not-authorised`
exited non-zero with `stop_reason` `not_merged`, or `unreadable` where it
wrote no merge result at all, having done exactly what was asked of it. It now
exits on its own steps. The change is inert for the caller, which reads the
result from `finalize-merge.json` keyed by `run_id` and never from the child's
exit status (see
[The merge result in the state folder](#the-merge-result-in-the-state-folder)).
Step recovery's exit `81` is unchanged, for a reason worth stating precisely,
because the obvious reading of it is wrong. `failed_run_exit_code` returns
`81` only when a step both did not complete and carries `81` as its own
termination code; otherwise a failed run exits `1`. Recording a hand-off does
not by itself fail the step that recorded it, so the outcome rule never
produced `81` on this path: it turned a finalisation that had completed its
steps into a plain exit `1`. Narrowing the rule therefore cannot suppress a
hand-off exit, because `81` was never the code it produced. What does change
is that such a finalisation now exits `0` rather than `1`, which is the
1.2.162 behaviour and is what the state folder, not the exit status, is read
for.

```bash
# A Crusty round, as tools/autodrive_loop.sh starts it: its own process.
amplihack recipe run "$ROUND_RECIPE_PATH" \
  --working-dir "$REVIEW_LAUNCH" \
  -c "autodrive_round_record=${RECORD}" \
  -c "autodrive_round_label=${ROUND_LABEL}"
echo $?
# 0 when the round's own steps succeeded, whatever the PR's merge state.
```

```bash
# The drive itself.
amplihack recipe run auto-drive-to-merge -c pr_number=402
echo $?
# 0 only when stop_reason is completed; non-zero when the drive did not merge.
```

A round's exit status is what the loop acts on. `autodrive_loop.sh` records it
as `ROUND_RC`, passes it to `autodrive-completion-dispatch` as
`--child-exit-code` and to the loop-health evaluator as
`loop_child_exit_code`, and dispatch does not accept `complete` from a round
whose child execution failed (see
[Dispatch validation](autodrive-crusty-completion.md#dispatch-validation)).
Up to 1.2.164 a CLEAN round exited `0` and the loop went on. #2637 added
`autodrive_run_succeeded` and `apply_autodrive_outcome`, which first shipped
in 1.2.165, and applied the outcome rule to every name `applies` accepts. From
1.2.165 until #2769, therefore, a round was judged by whether it had merged,
which a round never does: a CLEAN round exited non-zero with `stop_reason
unreadable`, dispatch would not accept `complete`, and the drive ended with no
evaluator fallback. A CLEAN round exits `0` again, and the loop proceeds to
merge-ready and merge. #2663 is the rationale the code comment cites for
applying the rule in both directions, not the change that introduced it; trace
the regression to #2637.

### Report step message

The last step of `auto-drive-to-merge.yaml` reads JSON, never prose: the
`finalize_merge` value (or, when the context has none, this run's
`finalize-merge.json` in the state folder), then `sync_loop_outcome` or `merge_loop_outcome`,
then `handoff_result`, each parsed with `jq 'fromjson?'`. The first matching
row applies.

| Observed | Message | Step exit |
| --- | --- | --- |
| `finalize_merge.result == "merged"` | `completed` | `0` |
| `not-authorised` on a verify-only run (`no_merge` or `autodrive_dry_run` not `false`) | `completed: verified; no merge was requested` (`completed`) | `0` |
| PR merged before the run | as for `already_merged` | `0` |
| `checks-pending` | `not merged: waiting for <check> at <sha>` (`waiting_for_checks`) | `0` |
| any other `finalize_merge.result` | `not merged: <reason>` (`not_merged`) | `0` |
| a hand-off ran | `handed off: <reason>` (`handoff`) | `0` |
| nothing parseable | `dependency_failure` | `1` |

`<reason>` comes from the JSON and is passed in with `jq --arg`. A deliberate
loop hand-off is reported as a hand-off, not as `dependency_failure`.

The step begins with the step-recovery preamble: `SRD` is the run's
`step_recovery_dir`, and `report()` prints the recovery report when `SRD` is
set. On `merged`, the step calls `report --remove-if-created-by
auto-drive-to-merge`, so it removes only a recovery directory this recipe
created, then exits `0`. When there is no merge evidence (the `dependency_failure` row) it calls `report`,
records the failure with `amplihack orch helper step-recovery record --step-id
autodrive-report`, calls `report` again and exits `1`.

## Tool reference

All tools live in `amplifier-bundle/tools/`, are sourced from the pinned
launch bundle, and are listed in `REQUIRED_ASSETS` so that the launch-bundle
digest covers them. Where a step finds them depends on its recipe:

| Recipe | Tools directory |
| --- | --- |
| `autodrive-merge-round.yaml` | `{{autodrive_tools_dir}}`, the output of `step-00-tools-dir`. No later round step searches again. |
| `autodrive-merge-loop.yaml`, `autodrive-finalize.yaml` | `${AMPLIHACK_HOME}/amplifier-bundle/tools`, the launch install that `step-02` pins `AMPLIHACK_HOME` to. |

No step takes a tool from `REPO_PATH`, the git toplevel or the PR worktree.
The examples below are written as an operator would run them by hand.

### `autodrive_round_word.sh`

Defines functions only; sourcing it has no other effect.

```bash
source "{{autodrive_tools_dir}}/autodrive_round_word.sh"

autodrive_round_word "Ready."
# {"word":"ready","merge_ready_verdict":"MERGE_READY"}

autodrive_round_word "Probably ready"
# {"word":"not-ready","merge_ready_verdict":"NOT_MERGE_READY"}

autodrive_round_word "waiting" "ci: pending" 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
# {"word":"waiting","merge_ready_verdict":"WAITING","ci_signal":"pending@15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605"}

autodrive_round_word "waiting" "ci: passed" 15a7f6f
# {"word":"waiting","merge_ready_verdict":"WAITING"}

printf '%s' "$RECIPE_VAR_merge_ready_writeup" |
  autodrive_round_file "$RECIPE_VAR_autodrive_state_dir" round-3 WAITING \
    "$RECIPE_VAR_merge_ready_assessed_head" \
    "pending@$RECIPE_VAR_merge_ready_assessed_head"
# writes merge-ready-round-3.json, .writeup and .head
```

`autodrive_round_word <answer> [<ci_answer>] [<head>]` adds `ci_signal` only
when both optional arguments are valid (see [Outcome word](#outcome-word));
otherwise the key is left out. All JSON is built with `jq -n --arg`.

`autodrive_round_file <state_dir> <round_label> <verdict> <assessed_head> [<ci_signal>]`
reads the write-up from standard input, never from an argument, and writes
`<assessed_head>` to the `.head` file. It does not run `git` itself.

| Check | Refusal |
| --- | --- |
| More than 5 arguments | exit `2` |
| `round_label` matches `^[A-Za-z0-9._-]{1,64}$` and is not `.` or `..` | exit `2` |
| `state_dir` passes `autodrive_private_dir` | exit `2` |
| `verdict` is `MERGE_READY`, `NOT_MERGE_READY`, `WAITING` or `HANDOFF` | exit `2` |
| `assessed_head` is 40 or 64 lowercase hex characters | exit `2` |
| `ci_signal`, when given, matches `^(pass\|pending\|fail\|none)@([0-9a-f]{40}\|[0-9a-f]{64})$` | not written; the round file has no `ci_signal` key |
| Each file written under `umask 077` to a temporary file in the same directory, then moved into place with `mv -f` | exit `1` on a write failure |

A file is never written through an existing path, so a symbolic link planted
at a round file's name is replaced, not followed.

### `autodrive_merge.sh`

```bash
source "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge.sh"
autodrive_merge 2575 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
# {"result":"merged","pr":2575,"expected_head":"15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605","reason":"merged at the expected head; ..."}
```

The function prints one JSON line built with `jq -cn`. It calls `gh` once for
each of these, in order: PR state with title, body and `mergeStateStatus` (one
read), `checks --required` (and `checks` when no required checks are
reported), merge, PR state after the merge. It adds no retries or waits.

```bash
autodrive_merge 2575 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
# {"result":"checks-pending","pr":2575,"expected_head":"15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605","reason":"waiting for test (ubuntu-latest) at 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605"}
echo $?
# 7
```

| Exit | Meaning |
| --- | --- |
| `0` | `merged`: GitHub reads the PR `MERGED` with `headRefOid` equal to the expected head. This holds whether the PR already read so before the request (no request is made), after the request, or after `gh pr merge` exited non-zero. |
| `2` | Bad arguments, or the merge-message helper failed its path checks. |
| `3` | `checks-not-green`; no merge attempted. |
| `4` | `refused`: GitHub declined the merge, for example on a moved head, a missing review or branch protection, and the read-back `mergeStateStatus` is not `BEHIND` or `DIRTY`. |
| `5` | `not-open`: the PR was closed, or merged at a head other than the expected one, before the merge. |
| `6` | `message-unavailable`: the merge-message helper refused the title or body. |
| `7` | `checks-pending`: a required check is still running at the head; no merge attempted. |
| `8` | `base-behind`: `DIRTY` before the merge request, or `BEHIND` or `DIRTY` after GitHub refused it. |
| `79` | A `gh` call exited `79`; passed through at once. |

### `autodrive_merge_loop_run.sh`

Holds the command body of `autodrive-merge-loop.yaml` `step-02`, moved into
a trusted tool so that the base-sync round can run it again. The body keeps
its behaviour, including the review-generation allocation and release of
[the review checkout lifecycle](#review-checkout-lifecycle-in-autodrive_merge_loop_runsh);
the tool adds only the round-count argument and the budget sum described
below.

`autodrive-merge-loop.yaml` has no `step-00-tools-dir`. Both steps take the
tool from the directory `step-02` already uses for its loop helper,
`autodrive_loop.sh`: `${AMPLIHACK_HOME}/amplifier-bundle/tools` in the launch
install, never `REPO_PATH` or the PR worktree. When the file is not there the
step stops with `autodrive-tools-not-found` and exit `1`. Inside the tool,
every other tool and the round recipe are found next to the tool itself
(`dirname` of its own path), as `step-02` does today:

```bash
TOOLS="${AMPLIHACK_HOME}/amplifier-bundle/tools"
bash "$TOOLS/autodrive_merge_loop_run.sh"     # step-02
bash "$TOOLS/autodrive_merge_loop_run.sh" 1   # step-03c: one round
```

It reads its inputs from the step's `RECIPE_VAR_*` environment variables and
prints the same loop line and exit statuses as described in
[How the loop ends](#how-the-loop-ends).

The optional argument is a number of further rounds. It must match
`^[0-9]+$` (at most 9 digits) and be at least `1`, otherwise the tool exits `2`. The round
budget counts rounds across runs that share the state directory, so the tool
counts the merge-ready rounds already recorded in `autodrive-usage.tsv` (the file `autodrive_budget.sh` writes) and
passes `autodrive_budget_loop_rounds=<used + N>`, never more than the
operator's own `autodrive_budget_loop_rounds` when one is set. When the
operator's budget leaves no room, the round stops as `BUDGET_EXHAUSTED` and
the PR is handed off. `autodrive_budget_total_rounds` applies unchanged.

### `amplihack orch helper autodrive authorization`

```bash
amplihack orch helper autodrive authorization \
  --invocation "$RECIPE_VAR_autodrive_invocation_file" \
  --directory "$RECIPE_VAR_autodrive_state_dir" \
  --pr "$RECIPE_VAR_pr_number"
# {"allows_merge":true,"run_id":"a3f9c2e1-7b44-4d0e-9a51-2c6f0e8d1b37","pr_selected":true}
```

Reads the captured invocation with `InvocationAuthorization::read_private`,
checks that `--directory` is the bound state directory, and prints whether
the invocation allows a merge. With `--pr`, `pr_selected` says whether that PR
is the one selected at launch (always `true` when none was selected). The
head branch selection (`existing_branch`) is not checked here. Any error exits `2` and prints nothing on
standard output; on a merge run the merge step reports that as
`authorization-unreadable` and hands the PR off.

The launcher writes the invocation to `invocation.json` inside a fresh
`autodrive-invocation-*` folder. It creates the folder with mode `0700` and
the file with mode `0600` (`create_new`), whatever the caller's umask. The
private read requires a folder outside the auto-drive state root to have no
group or other permission bits (mode `0700`). The default folder mode under
`umask 022` (`0755`) or `umask 002` (`0775`) would make this check fail and
leave the drive `STUCK` (#2656). The tests
`autodrive::trusted::umask_tests` cover both umasks.

## Security properties

- A merge requires the captured `allows_merge()`, the PR selected at launch
  (when one was), the head the `DONE` round
  assessed (captured before the assessment and format-checked), green checks
  (or no checks reported at all) and GitHub's acceptance of
  `--match-head-commit`.
- Merge permission comes only from the captured invocation. Recipe context
  can narrow it, never grant it.
- Narrowing the outcome rule to the top-level drive moves no authorisation
  boundary. `allows_merge()` is `!no_merge && !dry_run` and does not read the
  run's `success` flag, and `trusted::applies` still decides authorisation
  capture for every `autodrive-*` sub-recipe. A round that exits `0` still
  needs matching run, round and record identities, valid evidence and a
  `CLEAN` or `MINOR_ONLY` verdict before dispatch accepts `complete`.
- Loop results are read from the driver's result file, not from text an
  agent printed.
- Judging agents start in `{{review_launch_dir}}`, never in the PR worktree,
  so the PR's `AGENTS.md`, `CLAUDE.md` and `.claude/` files cannot instruct
  its own judge. A caller cannot set `review_dir`, `review_launch_dir` or
  `reviewed_head_sha`.
- Step recovery cannot turn a failed merge-ready loop or finalisation into a
  silent success: both are `--essential`, and the review-dir guards are
  `--guard`.
- Exit `79` is checked after every `gh` call and passes through the merge
  step, the finalisation recipe and the hand-off recipe. No status defaults
  to `0`.
- Command bodies contain no `{{var}}` substitutions. Agent and context values
  are read through quoted `RECIPE_VAR_*` variables or `write_context_value`,
  and all JSON is built with `jq -cn --arg`.
- Public comments contain no tokens, environment or authentication output,
  home-directory paths or unrelated logins. Quoted PR text goes in fenced
  blocks.
- Under dry run, the merge path is enforced in code. Comment posting and
  drafting rely on the agent following the dry-run instruction, and the
  hand-off's closing step makes no writes.
- A Crusty verdict comment counts only from the drive's own login or an
  `OWNER`, `MEMBER` or `COLLABORATOR`, and only with a full SHA. Only the
  author or a repository admin can edit a comment, so the newest verdict by
  `updated_at` cannot be rewritten by an outside contributor.
- The Crusty verdict is posted as a comment, never as a review approval.
- `ci_signal`, verdicts and heads are checked against fixed sets and
  patterns before they are stored. Finalisation decides only from `gh` exit
  codes, `mergeStateStatus` literals and `--match-head-commit`, never from an
  agent's words or `ci_signal`. An unknown merge state gives `refused`.
- `WAITING` sends the drive to finalisation, which merges only on green
  required checks at the assessed head. It cannot pass over a failing check.
- The base sync pushes only `HEAD:<headRefName>` with the drive's existing
  credentials, refuses fork PRs, checks branch names, puts `--` before
  values taken from the PR, never forces, rebases, resets or skips hooks, and
  pushes only conflict-free merges.
- Run success is an allow-list of `stop_reason == "completed"`. Unknown merge
  results map to `handoff`, never to `completed`.

### Accepted risk: readiness is an agent's judgement

Since #2575 the merge-ready verdict is an agent's judgement of the PR. The
hash-checked QA evidence that used to gate the merge is gone, and the QA
evidence and the 13 criteria are checked only by the agent. Text planted in
the PR, its comments or its files could persuade the agents to answer
`ready`. That answer alone cannot merge: the captured authorisation, the
assessed-head match, green checks and GitHub's acceptance of
`--match-head-commit` still apply. An injected or mistaken `ready` on a PR
with green checks and no required review would merge at the assessed head.

This risk is accepted. A repository that uses auto-drive to merge must have
branch protection on its base branch, with the required status checks and
required reviews that it relies on. Without them, the agent's judgement is
the only gate before a merge. This includes checks: a PR with no checks
reported merges when the agents judge it ready and GitHub accepts the merge.

## What was retired

| Retired | Replacement |
| --- | --- |
| The readiness record, finalisation observation and loop-completion record, with the 13-criterion schema | The PR comment write-up and the one-word outcome |
| Strict record validation (`validation.rs`, `records.rs`) | None; the drive does not parse the write-up |
| The record operations of `helper autodrive` and the loop-completion call | `helper autodrive authorization` and the round tools |
| Accepted snapshots, the loop receipt (`completion.rs`), `outcome.rs`, `review.rs`, `followups.rs` | GitHub's `--match-head-commit` and the run report |
| `autodrive_merge_ready_files.sh` and round `step-00-merge-ready-files` | The prompt names the skill files under `$AMPLIHACK_HOME/amplifier-bundle/skills/merge-ready/` |
| `round-record`, `merge_ready_verdict` in `autodrive_round_evidence.sh`, and `autodrive_qa_trusted` | `autodrive_round_file` |
| Finalisation steps `refresh-readiness`, `accept-refreshed-readiness`, `finalize-observed-action`, `reduce-observed-outcome` | `merge` and `state-outcome` |
| Bare `STUCK` endings of the merge-ready loop | [Hand-off](#hand-off) |
| Criterion 3 from `crusty-records.tsv` and the `crusty-evidence` subcommand of `autodrive_round_evidence.sh` | The PR's own [Crusty verdict comment](#crusty-verdict-at-the-head) and the read-only `crusty-covers` range check |
| `checks-not-green` and a hand-off for a pending check | `checks-pending` and the next wake |
| ✓ Success for a top-level drive that did not merge | [Success means merged](#success-means-merged) |

`Binding`, `validate_binding` and `MAX_RECORD_BYTES` remain, in
`orch/autodrive/identity.rs`, because authorisation and the already-merged
report still use them. The crusty loop's `STUCK` endings are unchanged.

## Related documentation

- [Auto Drive To Merge Reference](auto-drive-to-merge.md)
- [Auto-drive evidence and outcomes](auto-drive-evidence.md)
- [Verify a PR without merging](../howto/verify-pr-with-auto-drive.md)
- [Loop-health evaluator](loop-health-evaluator.md)
- [Crusty completion contract](autodrive-crusty-completion.md)
- [Checkout isolation](recipe-checkout-isolation.md)
- [Step failure recovery](step-failure-recovery.md)
