---
title: Auto-drive Merge-ready Round, Merge and Hand-off
description: Merge-ready assessment, direct PR-worktree review, recovery, finalization, and handoff.
last_updated: 2026-11-21
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
  - [Conditions stated in the task](#conditions-stated-in-the-task)
- [Review checkout and step recovery](#review-checkout-and-step-recovery)
- [Crusty verdict at the head](#crusty-verdict-at-the-head)
- [Outcome word](#outcome-word)
- [Round files](#round-files)
- [How the loop ends](#how-the-loop-ends)
- [Finalisation and merge conditions](#finalisation-and-merge-conditions)
  - [Merge method](#merge-method)
- [Base sync](#base-sync)
- [Hand-off](#hand-off)
- [Run report](#run-report)
- [Tool reference](#tool-reference)
- [Security properties](#security-properties)
- [What was retired](#what-was-retired)

## Round steps

`autodrive-merge-round.yaml` runs its judging agents directly in the actual
linked PR worktree. Trusted skills and tools come from the launch installation.
Immediately before the reviewer, read-only tool steps capture ordinary
pre-review evidence after preceding agents finish. The reviewer is instructed
to leave files, index, branch, `HEAD`, and workflow state unchanged.
A following assessment agent inspects any changes before the round continues.
Fixes, commits, and pushes also use this PR worktree.

| Step | Kind | Output | Responsibility |
| --- | --- | --- | --- |
| `step-01b-crusty-evidence` | agent | `crusty_evidence` | Reads the newest authorised `Crusty verdict:` comment on the PR and judges whether it covers the current head. See [Crusty verdict at the head](#crusty-verdict-at-the-head). |
| `step-01a` | bash | `merge_ready_assessed_head` | Captures `git rev-parse HEAD` in the PR worktree before the assessment and checks it is 40 or 64 lowercase hex characters; any other value exits `2`. |
| `step-02` | agent, `parse_json: false` | `merge_ready_writeup` | Reads the pinned merge-ready skill, assesses all 13 criteria and any [conditions stated in the task](#conditions-stated-in-the-task) at `merge_ready_assessed_head`, posts one PR comment naming that SHA and ends with one sentence stating the outcome: hand-off for a task-stated condition that only an outside party can clear, not ready when a later round could satisfy it. Under `autodrive_dry_run=true` it prints the write-up instead of posting it. |
| `step-02b-assess-review-changes` | agent, no `parse_json` | `review_change_assessment` | Compares status, index, staged/unstaged content, untracked files, and HEAD against pre-review evidence. Preserves existing and uncertain work; differences alone do not fail. |
| `step-03` | agent | `merge_ready_outcome` | Answers one question about the write-up: did the assessment conclude ready, not ready, waiting, or hand-off? It replies with one word on its last line and, on the line before, `ci: pass`, `ci: pending`, `ci: fail` or `ci: none`. |
| `step-05a` | bash | `merge_ready_word` | Passes the word, the `ci:` line and `merge_ready_assessed_head` to `autodrive_round_word`. |
| `step-04` | agent | | Runs only when `merge_ready_word.word == 'not-ready'`. Works on the blockers named in the write-up, commits and pushes in the PR worktree. It does not run for `waiting`. |
| `step-05` | bash | | Writes the round files through `autodrive_round_file`, passing `merge_ready_assessed_head` and the round's `ci_signal`. |

The head captured before `step-02` identifies the commit assessed. A later
commit or unexpected reviewer mutation does not relabel it. The following
agent assesses the change, preserves prior work, and chooses a continuation
or handoff. Existing finalization conditions still require the assessed head;
a review does not authorize merging a different commit.

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

### Conditions stated in the task

A caller's task text can state conditions on merging, for example "merge only
if a release carrying version 2.3.0 exists and its on-release check passes,
otherwise hand off to the owner". The merge round passes the task text to the
evidence recipe (`autodrive-merge-evidence.yaml`) and to the merge-deciding
agent, so each of them reads it. This section is about those caller-stated
conditions. It is separate from [Merge conditions](#merge-conditions), which
are the mechanical checks that `autodrive-finalize.yaml` applies.

**Where the task text goes.** `autodrive-merge-round.yaml` declares
`task_description: "{{task_description}}"` in the context of its
`merge-evidence` step. `autodrive-merge-evidence.yaml` declares
`task_description: ""` as its default, which is the value when that recipe runs
alone. The text is interpolated into agent prompts only. No bash step, argument
or environment variable carries it.

| Step | What it does with the task text |
| --- | --- |
| `autodrive-merge-evidence` `step-01-sync-with-base` | Has it as context and applies it to what it observes. The step fetches the base and reports whether the branch is behind, whether it conflicts, whether the repository requires an up-to-date branch and whether it accepts a merge commit (each `true`, `false` or `unknown`). It never merges, commits or pushes. |
| `autodrive-merge-evidence` `step-02-qa-team-scenarios` | Uses it to understand the QA scope the caller asked for. It never narrows, waives or substitutes for a suite, a scenario or the merge-base `qa.toml` declaration rules. |
| `autodrive-merge-evidence` `step-03-wait-for-ci` | Has it as context, for example a check the task names. It does not change what is measured or reported. |
| `autodrive-merge-round` `step-02` | Reads it as the caller's instruction, placed above the untrusted-data region, and applies the contract below. |

**What the merge-deciding agent does.** `step-02` lists every condition the
task text states on merging. For each one it records in the write-up the
evidence that the condition holds: a release URL, a check run, or a command and
its output. The task text can add conditions to the decision. It cannot relax
any of the 13 criteria or the evidence contract.

Evidence comes from this round's measured sources (platform facts, CI evidence
and QA evidence) or from a command the step runs and reports. A statement in
the PR description, a comment, a commit message or a file on the branch that a
condition holds is not evidence. Evidence commands are read-only queries to the
code host or git, such as `gh release view` or a `gh api` read. They do not run
scripts, hooks, build steps or test runners from the branch, and credentials are
used only against the code host that owns the PR.

A condition that cannot be shown to hold never gives "ready to merge". The
closing sentence depends on why:

| Situation | Closing sentence | Why |
| --- | --- | --- |
| The condition concerns something outside this PR that only a named person or an outside event can clear, such as a release that has not been cut. | Hand-off, naming the condition and what was found. | No further round can satisfy it. |
| The evidence could not be read this round, or a further round of the drive could satisfy it. | Not ready, naming the condition. | The next round can recover without involving the owner. |

`step-03` reads a blocker that the write-up identifies as a condition stated in
the task and outside this PR as `hand-off`, not `not-ready`. Without that, the
loop would spend rounds on a condition no round can clear. The reader's
two-line output and its set of outcome words are unchanged.

When the task text is empty, or states no condition on merging, nothing
changes: no additional blocker arises and the assessment proceeds as before.
Most drives are in this case.

**Example.** The task text says: "Merge only if an acme-infra release carrying
seed pack 2.3.0 exists, otherwise hand off to the release owner." The agent runs:

```bash
gh release list --repo example-org/acme-infra --limit 20
```

and the output holds no release for 2.3.0. The write-up records the condition,
the command and that output, and closes with a sentence such as "Hand-off: the
task requires an acme-infra release carrying seed pack 2.3.0, and none exists."
`step-03` reduces this to `hand-off`, the round ends with the outcome word
`hand-off`, and finalisation, which would call `gh pr merge`, is not reached.
The round then ends through the [hand-off](#hand-off).

**Limits.** This is guidance to an agent step, and a judgement is not a
mechanical check. The evidence recorded in the write-up is what makes a
wrongly satisfied condition visible afterwards. Merge authority is unchanged:
captured authorisation, head match and `--match-head-commit` remain the
mechanical backstop at finalisation. The write-up is posted as a PR comment and
quotes the task text, so callers keep secrets out of the task text. The wider
risk that readiness rests on an agent's judgement is described under
[Accepted risk: readiness is an agent's judgement](#accepted-risk-readiness-is-an-agents-judgement).

## Review checkout and step recovery

Both round types and their relevant evaluators run in the actual linked PR
worktree. The loop resolves it through the named repository using
`amplihack orch helper autodrive worktree`, retaining registration, branch,
and intended-head verification. It does not allocate, advance, bind records
to, or release a private review generation.

### Review evidence and assessment

Immediately before each reviewer invocation, tool steps save repository
identity, branch, full `HEAD`, NUL-delimited status/index entries, the index
and split-index dependencies, binary staged/unstaged diffs, and original dirty
and untracked contents, modes, and symlink targets. Existing artifact storage
keeps evidence private (`0700` directories, `0600` files). Collection does not
change the repository or follow symlinks into external content.

The agent after review compares status, index, working content, untracked
files, and `HEAD` with that baseline. It may safely restore clearly
attributable reviewer edits, or report them and choose further work or handoff.
Staged edits, edits to already-dirty files, and changed pre-existing untracked
files require content comparison. Uncertainty preserves the work. A difference
alone never fails the run; blanket restoration, cleanup, and wholesale index
replacement are forbidden. See the
[assessment contract](recipe-checkout-isolation.md#assessment-after-review).

### Recovery and launch assets

The loop supplies the actual PR worktree, intended reviewed commit, and trusted
launch-tools path to rounds. Private review checkout and `review_launch_dir`
context are removed. Trusted asset selection remains independent of cwd.
`step_recovery_dir` continues through the loop to each round. The loop's own
context values retain precedence over caller context, and existing completion
state remains driver-owned.

The merge-loop recovery contracts remain:

| Step | Recovery flags |
| --- | --- |
| `step-02-merge-ready-loop` | `--essential --stop-on-handoff` |
| `step-03-finalize`, recovered by `step-03r-finalize-recovery` | `--already-failed --essential` |

Finalization recovery runs only when the loop permits finalization and the
finalization result is absent. A loop that handed off never gains merge
authority through recovery.

Recovery retains the original baseline for an interrupted reviewer invocation,
including staged, unstaged, and untracked state. It does not recapture partial
reviewer edits as pre-existing work. Runtime finalization, loop recovery, and
finalization recovery retain their responsibilities under
[step failure recovery](step-failure-recovery.md). The former review-directory
guards and `step-02b-review-dir-unchanged` deterministic gate are removed.

### Loop launch and exit

`autodrive_merge_loop_run.sh` validates its required inputs, resolves the PR
worktree in the named repository, and runs the loop there. Temporary task text
is removed on exit. There are no review-generation release traps. Uncommitted
files, unpushed commits, branches, and worktree registration survive an
interrupted run. Legacy execution/review directories are left for recovery.

The loop does not grant merge authority; `autodrive-finalize` reads captured
invocation authorization through the existing helper.

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

A clean [base sync](#base-sync), or a commit that changes only the PR
description or evidence files, moves the head without needing a new review. A
base merge that needed a hand-resolved conflict is code and does need one. For a verdict
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
later commit is a clean base merge (no hand-resolved conflict) or an allowed description or evidence
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
| A pre-sync head | A base merge with a hand-resolved conflict | `crusty-unreviewed-commits:<head>` |
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
[`autodrive_merge_loop_run.sh`](#autodrive_merge_loop_runsh)). A
`merge_method` the merge tool would not take stops the tool with exit `2`
before the driver starts (see [Merge method](#merge-method)). The step keeps its default `fatal`
handling and the tool maps the driver's exit status itself, so that exit `79`
is never hidden by `continue_on_error` (#2490).

| Driver exit | Loop result (read from `merge-ready-result.json`) | Step exit |
| --- | --- | --- |
| `0` | `DONE` or `WAITING` | `0` |
| `1` | `USAGE_LIMIT` | `1`: the run stops; a later run continues the work. |
| `1` | `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF` | `0`, with the loop line printed. |
| `1` | result file missing or unreadable, or `DONE` or `WAITING` (the driver writes those only when it exits `0`) | `0`, with loop result `UNREADABLE` and a reason that names the [kept driver output](#kept-driver-output) or says it could not be kept. |
| `0` | anything other than `DONE` or `WAITING`, or no readable result | `0`, with loop result `UNREADABLE` and the same kind of reason. |
| `75` | `ORPHANED` | `75` |
| `79` | `TERMINAL_REFUSAL`, set by the step; the result file is not consulted | `79` |
| `2` or any other value | | passed through unchanged |

The loop result comes from the driver's own result file, read with `jq`.
Text on standard output, including JSON an agent printed, does not decide it.

Exit `79` means a `gh` call refused for a terminal reason, such as an
authentication or account refusal. The step prints a `TERMINAL_REFUSAL` loop
line as its output and then exits `79`, so the run stops there and no
hand-off or finalisation runs. The driver's own output, standard output and
standard error together, goes to the step's standard error, for the log, and
a copy is [kept in the state folder](#kept-driver-output); the step's output
is only the line it builds from the result file.

A `HANDOFF` round writes a `HANDOFF` line through `record_result` before the
evaluator runs, then the driver exits `1`. A round that writes no write-up or
head leaves none behind, so no earlier assessment reaches the next round or
the hand-off.

### Kept driver output

The loop driver's output goes to the step's standard error and nowhere else.
A driver that dies before it writes a result file therefore leaves the
hand-off nothing to name and a person nothing near the PR's state to read. The
tool keeps a second copy in the PR's
[state directory](auto-drive-to-merge.md#state-directory), beside the result
file, because that folder is where hand-offs and people already look. The
`UNREADABLE` reason names the file. The hand-off agent is told to read it on
every hand-off, including after a base sync, where the reason reaches the
hand-off in `handoff_blocker` (see [Hand-off](#hand-off)).

Only the run's `runtime/` folder is removed when a run ends. By default the
state directory is `autodrive-state` inside the run's durable folder, which is
never removed (see
[clean up after recipe runs](../howto/clean-up-after-recipe-runs.md)), so the
files below stay there. A dry run (`autodrive_dry_run=true`) with no explicit
`autodrive_state_dir` is the exception: its state directory is in the launch
bundle and goes with it once the report is produced, so the kept file and the
path the reason names no longer exist after that. Pass an absolute
`autodrive_state_dir` to keep them.

| File in the state directory | Content | Lifetime |
| --- | --- | --- |
| `merge-ready-driver.err` | Everything the loop driver wrote to standard output and standard error, in the order it wrote it. Mode `0600`. | Replaced at the start of each run of the tool. Stays in the state directory after the run ends, for every outcome, except in a dry run's default state directory. |
| `merge-ready-result.json` | The driver's own result, or the partial file it left. | Removed at the start of each run of the tool, then written by the driver. Stays on the same terms. |

The file name `merge-ready-driver.err` does not start with `merge-ready-round-`,
so the round-number readers (`autodrive_last_round.sh`) never see it.

`keep_and_log` is a read loop in the tool itself, not an external
duplicator. The copy is the step's own account of a driver that died, so it
cannot rest on a program the host supplies: where the host's coreutils is the
uutils implementation (Ubuntu 25.10 installs it as the default `tee`), `tee`
aborts part-way through any input larger than one pipe buffer, roughly one run
in ten, and the bytes it has already read die with it. The copy and the
step's own log then lose the driver's output together, which is worse than
keeping no copy at all. A read loop cannot fail that way and reaches the log a
line at a time rather than a block at a time. NUL bytes are the one
difference: `read` drops them.

The kept file is opened once, before the driver starts, as a descriptor the
step holds for the rest of the run, and falls back to `/dev/null` when the
open fails. The writer therefore always runs, whatever became of the copy: a
writer that could be skipped would leave the driver writing into a pipe with
no reader, and a driver killed by that would lose both its output and its own
exit status to the copy, which is exactly what the copy must never decide.
POSIX mode is turned off around the open and restored after it, for the same
reason: a redirection error on `exec` ends a POSIX-mode shell outright, and
bash enters that mode on its own when `POSIXLY_CORRECT` is in the
environment, so the step would end before the driver ran.

The step's standard error carries the same bytes in the same order as before,
so the run log is unchanged. The driver's exit status is the step's exit
status; the copy cannot change any exit in the [table above](#how-the-loop-ends).
The copy is made for every outcome, `DONE` and `WAITING` included, because
the output of a run that went well is useful when a later one does not.

A second run of the tool in the same state folder (the
[base sync](#base-sync) round) happens only after `DONE` or `WAITING`, so
within one drive it cannot replace the output of a run that ended
`UNREADABLE`. A later drive that passes the same explicit
`autodrive_state_dir` replaces the file at its first run of the tool, as it
does `merge-ready-result.json`.

#### What the UNREADABLE reason says

When the loop result is `UNREADABLE`, the tool builds the loop line's `reason`
from one template:

```text
the loop driver exited <rc> without a readable merge-ready-result.json; <output clause>; merge-ready-result.json <result state> at <absolute path>
```

The opening words are the text the tool has always used, so `UNREADABLE` and
the exit status in it are unchanged. The output clause and the result-file
clause are new. The outcome step (`step-02a`, and `step-03d` after a base sync)
prepends `loop result UNREADABLE: ` and cuts the text to 1000 bytes of
printable ASCII. `step-02c` passes it to the [hand-off](#hand-off) as
`handoff_reason`, the "Why the drive stopped" sentence, and leaves
`handoff_blocker` empty. After a base sync, `step-03f` passes a fixed sentence
as `handoff_reason` and carries the reason in `handoff_blocker`, joined to the
base-sync reason as `{{base_sync.reason}} {{sync_loop_outcome.reason}}`.

The output clause is exactly one of these:

| Case | Output clause |
| --- | --- |
| Kept | `its output was kept in merge-ready-driver.err at <absolute path>` |
| Kept only in part (the copy failed part-way) | `only part of its output could be kept, in merge-ready-driver.err at <absolute path>` |
| Not kept | `its output could not be kept (the state folder is missing, is a link, is owned by another user or is not writable)` |

The clause names the file by its bare name first, then its absolute path. When
nothing was kept the clause names no file, so the reason never says `kept` for
a file that does not exist.

The reason never contains a line of the file. The tool does not read the kept
file, not even to learn its size: the hand-off agent opens it by the name the
reason gives.

A copy that stopped part-way keeps its name and its path, because a truncated
file is still the evidence to open, but the clause says so before naming it.
The status of the copy comes from the copier's own exit, read from `PIPESTATUS`
beside the driver's.

`<result state>` is `is absent`, `is unreadable`, or `says <loop result>`
(for example `says DONE` after a driver exit `1`, which the driver never
writes with `DONE`). The loop result is the word in the result file, cut to 64
characters of letters, digits, `_` and `-`.

The bare name comes first because the default work root is under `$HOME` and
the hand-off leaves home-directory paths out of its public comment. If the
absolute path is redacted, the name still tells the reader what to look for
in the state folder.

A worked case: the driver's ceiling check finds `AMPLIHACK_MAX_DEPTH` changed
inside the loop, prints one `ERROR` line and exits `1` without writing a
result file. The line is in the kept file, and the reason names the file:

```json
{"loop":"merge-ready","loop_result":"UNREADABLE","round_label":"unknown","reason":"the loop driver exited 1 without a readable merge-ready-result.json; its output was kept in merge-ready-driver.err at /home/user/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/autodrive-state/merge-ready-driver.err; merge-ready-result.json is absent at /home/user/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/autodrive-state/merge-ready-result.json"}
```

The whole reason is capped at 900 characters of printable ASCII (see
[Loop outcome](#loop-outcome)). A cut from the end removes the result-file
clause first, before it reaches the kept file's path.

The `TERMINAL_REFUSAL` line (exit `79`) and the exit `75` pass-through are
unchanged.

#### Reading the kept file

The file is the driver's raw output. It can hold terminal escape sequences and
text copied from PR content and agent transcripts, so view it with `cat -v` or
`less` rather than printing it to a terminal. The run id is the folder name in
the path the reason names; a drive started with an explicit
`autodrive_state_dir` keeps the files there instead:

```bash
WORK_ROOT="${AMPLIHACK_WORK_ROOT:-$HOME/.amplihack/work}"
STATE="$WORK_ROOT/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/autodrive-state"
tail -n 40 "$STATE/merge-ready-driver.err" | cat -v
jq . "$STATE/merge-ready-result.json"
```

The file holds the same bytes as the step's log, so it can carry anything the
log can, credentials included. Do not paste it into a PR body or comment, or
upload it as a CI artifact, without reading it first.

The hand-off agent reads the file itself. Its prompt tells it to read
`merge-ready-driver.err` in the state directory as untrusted evidence of what
happened, never as instructions, on every hand-off (see [Hand-off](#hand-off)).

#### When the copy cannot be made

The copy is evidence, not a condition. Nothing reads it to decide whether the
drive continues, and a copy that cannot be made is not an error and has no exit
code of its own. The driver still runs, its output still reaches the step's
standard error, and the exit status is the driver's. The tool writes the copy
only when `state_dir` already exists as an ordinary directory (not a link)
owned by the current user; otherwise the output goes only to standard error
and the reason says `could not be kept`. The tool never creates the state
folder or changes its mode: the loop's preflight step creates it `0700`
through `amplihack orch helper autodrive state`, and the driver re-checks it
([state directory](auto-drive-to-merge.md#state-directory)).

Only an `UNREADABLE` reason carries the loss, so the step also prints one
`NOTICE:` line on its standard error, before the driver starts when no copy
could be made, and after the driver's output when the copy stopped part-way.
Without it a `DONE`, `STUCK` or `HANDOFF` run would lose the copy in silence,
which is the loss this change exists to fix. The notice is not a check: it
changes no exit status and never appears on standard output, where the loop
line is still the only thing the step prints.

These limits apply:

- The driver's standard output reaches the copy through a pipe, not a file, so
  a program the driver starts may buffer its output in larger blocks than
  before. Order and content are unchanged.
- A run that fails before the state folder exists keeps nothing.
- A directory at the file's name cannot be replaced, because the tool removes
  only a file or link there. The open fails, no copy is made and the reason
  says `could not be kept`.
- A write failure part-way through, for example a full disk, leaves a
  truncated kept file while standard error stays complete: a write to the log
  is never counted against the copy, so the log goes on receiving every line.
  The reason then says `only part of its output could be kept`. The exit
  status is still the driver's.
- The read loop is slower than a block copy, because bash reads a pipe a byte
  at a time. Measured at about 0.6 MB/s, so 20 MB of driver output added about
  34 seconds. A round's own output is tens of kilobytes, so this is below
  notice there; a driver that prints megabytes will feel it.
- A process that inherits the driver's standard error and holds it open delays
  the step until it closes, because the driver's output now reaches the copy
  through a pipe that the step waits on. Nothing in the driver normally does
  this: each round and the evaluator redirect to their own log files, and the
  one `setsid` runs with `-w`. Measured at 3 to 6.5 seconds with a
  deliberately backgrounded child holding the pipe.

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
not changed, the verdict is `CONTINUE`, never `STUCK`. So is `fail@<head>`,
with CI repair named in `recommended_action` (#2966); it is `STUCK` only when the
evidence names a cause the run cannot fix, with that cause and the human step.
Other repeated rounds with no movement are judged as before. The budget stop
still bounds the loop, as the operator's own usage limit, not as a verdict on CI.

### Loop outcome

`step-02a` turns the loop result into
`{"outcome":"done"|"handoff","finalise":"true"|"false","reason":"...","loop_result":"...","stop_comment_posted":"true"|"false","head_sha":"..."}`,
with the reason cut to 1000 bytes of printable ASCII:

| Loop result | Outcome | `finalise` | Next |
| --- | --- | --- | --- |
| `DONE` | `done` | `true` | `step-03` runs [`autodrive-finalize.yaml`](#finalisation-and-merge-conditions). |
| `WAITING` | `done` | `true` | `step-03` runs finalisation, which merges if the checks have passed by then, or ends `checks-pending` and hands the PR off. |
| `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF`, `UNREADABLE`, anything else | `handoff` | `false` | `step-02c` runs [`autodrive-handoff.yaml`](#hand-off). |

The tool caps its own `UNREADABLE` reason at 900 printable-ASCII characters so
that the prefix `step-02a` prepends (`loop result UNREADABLE: `, 24
characters; `step-03d` prepends the same one after a base sync) still fits
inside the 1000-byte cut. A cut from the end removes the result-file clause
first, before it reaches the kept file's path.

The reason reaches the hand-off by one of two routes. `step-02c` passes the
`step-02a` reason as `handoff_reason` and leaves `handoff_blocker` empty. After
a base sync, `step-03d` builds the same kind of reason for `sync_loop_outcome`,
and `step-03f` carries it in `handoff_blocker`, after the base-sync reason,
with a fixed sentence as `handoff_reason` (see [Base sync](#base-sync)).

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
   [Checks](#checks)) and its `mergeStateStatus` not `DIRTY`, settles on a
   merge method the repository allows (see [Merge method](#merge-method)),
   then GitHub accepts
   `gh pr merge --<method> --match-head-commit <expected sha>`.

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
| Before the merge request | `DIRTY` (conflicts with the base) | `base-behind` (tool exit `8`), reason `the branch conflicts with its base (mergeStateStatus DIRTY)`; `gh pr merge` is not called. |
| Before the merge request | anything else | The merge request goes ahead. `BEHIND` alone does not stop it; GitHub decides whether an out-of-date branch may merge. |
| After GitHub refuses the merge, read once more | `DIRTY` | `base-behind` (tool exit `8`), reason `GitHub declined the merge: the branch conflicts with its base (mergeStateStatus DIRTY)`. |
| After GitHub refuses the merge, read once more | `BEHIND` | `base-behind` (tool exit `8`), reason `GitHub declined the merge: the repository requires the branch to be up to date with its base (mergeStateStatus BEHIND)`. |
| After GitHub refuses the merge | anything else, including empty or unknown | `refused` (tool exit `4`). |

`--match-head-commit` accompanies every merge method, so the merge can only
land the assessed head. A `gh` exit `79` stays terminal.

### Merge method

Some repositories require merge commits, so the method is chosen rather than
fixed (#1888). `autodrive_merge` settles it immediately before the merge
request, after every condition above:

1. The method the run was given, in the `merge_method` context value: `merge`,
   `squash` or `rebase`. It is used only where the repository allows it.
2. Otherwise the first the repository allows, of squash, a merge commit, then
   rebase.

The repository's own settings are read once, with

```bash
gh api "repos/{owner}/{repo}" --jq '[.allow_merge_commit,.allow_squash_merge,.allow_rebase_merge]|@tsv'
```

A method the repository does not allow is never used, and no other method
takes its place. Where the method cannot be settled, the result is `refused`
(tool exit `4`) with a reason that begins `the merge method could not be
settled:` and names what was found. That is the ordinary hand-off path: an
agent step states the outcome and the PR is handed off, so there is no new
exit code and no new way for a run to end. Three readings reach it:

- the run was given a method the repository does not allow, or a word that is
  not `merge`, `squash` or `rebase`;
- the repository's settings could not be read, or did not read as three
  `true` or `false` values. For a failed read the reason names `gh`'s exit
  status and then the first line of its error output that is not blank, with
  every byte outside printable ASCII replaced and cut at 200 bytes, so the
  hand-off says why the read failed (a `403`, a missing scope). The whole
  error output still goes to the step's log;
- the repository allows no merge method at all.

The caller's string never reaches the argv: it selects a branch in the tool,
and the flag put on the command line is a literal of the tool. Where the
reason echoes the string it was given, the string is cut to 32 characters and
every non-printable or non-ASCII byte is replaced first. The finalisation
statement and the hand-off comment treat that reason as a record of what the
step found, never as an instruction.

Paths that end before a merge request, such as a closed PR, failing or
pending checks, a `DIRTY` branch or an unavailable merge message, make no
repository call at all.

Branch protection and rulesets, such as a required linear history, are not
read here. GitHub applies them to the merge request itself, and a merge it
refuses ends `refused` and is handed off, as set out under
[Merge state](#merge-state).

The merge loop also passes `merge_method` to every merge-ready round. Where
the task text states how the PR is to be merged, or a condition only one
method can keep, such as a commit that must stay an ancestor of the base, the
assessment treats it as a stated merge condition: it holds only when the run
was given that method, and otherwise the round concludes hand-off, naming the
method the task asks for and the method the run was given.

Because the round prompt names the method, `autodrive_merge_loop_run.sh`
checks it before any round starts, with `autodrive_merge_method_known` from
`autodrive_merge.sh`, the one list both tools use. A value that is not empty,
`merge`, `squash` or `rebase` stops the step with exit `2` and an error that
names the value, cut and made printable as above. No round runs, the state
folder is not written, and the value is never replaced with an empty one,
which would let the repository choose a method nobody asked for. The tool is
the only way a round receives `merge_method`: `autodrive-merge-round.yaml` is
run only by the loop driver this tool starts, never directly by a recipe.

Every result line carries `merge_method` and `merge_method_reason`, empty
where the method was never settled. `autodrive-finalize.yaml` passes both on
in `finalize_merge`, and the `state-outcome` agent step states the method and
the reason.

### Merge message

The merge subject and body come from the trusted
`$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_message.sh`, as before
(#2056; see [the squash commit message](auto-drive-to-merge.md#the-squash-commit-message-2056)).
The helper is sourced only after the absolute-path and non-symlink checks, and
is called on every path, whichever method is settled on, so
`message-unavailable` stays reachable as it was. A squash and a merge commit
both take the composed subject and body; a rebase writes no commit of its own
and takes neither, and no body is piped to it. The body reaches `gh` on
standard input:

```bash
printf '%s' "$MERGE_BODY" | gh pr merge "$pr" --squash --match-head-commit "$sha" --subject "$subject" --body-file -
printf '%s' "$MERGE_BODY" | gh pr merge "$pr" --merge --match-head-commit "$sha" --subject "$subject" --body-file -
printf '' | gh pr merge "$pr" --rebase --match-head-commit "$sha"
```

### `finalize_merge` values

| `result` | `autodrive_merge` exit | Step exit | `handoff` |
| --- | --- | --- | --- |
| `merged` | `0` | `0` | `false` |
| `not-authorised` | not called | `0` | `false` |
| `authorization-unreadable` | not called | `1`; goes to [recovery](#an-unreadable-invocation-record-goes-to-recovery) | `false` |
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
`authorization-unreadable`. The step fails with exit `1` before any merge
command, and the PR is not drafted. A helper exit `79` passes through as `79`
and is terminal.

### An unreadable invocation record goes to recovery

When the record is missing, unreadable or bound to another state folder, the
`merge` step prints one `ERROR:` line to standard error and exits `1`. The
line names the result (`authorization-unreadable`), the expected
`invocation.json` path and whether it is present or missing, the state folder
the step was given, and the helper's exit status. It holds paths and exit
codes only, never the record's contents. The helper's own message goes to the
same standard error and names both the folder it was given and the folder the
runner bound.

The merge loop's `step-03-finalize` continues on error, so
`step-03r-finalize-recovery` runs. It carries the condition
`not autodrive_finalization`, because `continue_on_error` lets the loop go on
after the failed step and recovery must still be entered.

The pinned runner captures a bash step's standard error and keeps it only in
that step's error result; it does not stream it to the drive log, whose
progress lines show the step's status and duration only. The first attempt's
`ERROR:` line and the helper's message therefore stay in the parent run's
result for `step-03-finalize`. Recovery starts with `--already-failed`, so the
recovery agent does not receive that first attempt's standard error either.
What it can read is `finalize-merge.json`, if one exists. When a re-run made
by recovery fails, its `ERROR:` line does reach the recovery agent, because
`step-recovery` passes on and keeps the tail of each attempt's standard error.
It reaches the drive log only if `step-03r-finalize-recovery` itself fails and
the runner reports that failure.

Recovery runs `autodrive-finalize` again as a separate process. Capturing the
invocation again uses only the operator's original inputs (`no_merge`, the dry
run flag and the PR), and `step-03r-finalize-recovery` defaults `no_merge` to
`true`, so a recovery re-run cannot allow a merge the operator did not ask
for. A verify-only or dry run stays verify-only. The recipe tests in
`crates/amplihack-cli/tests/issue_2539_step_recovery_recipes.rs` and
`issue_2539_step_recovery_loop.rs` cover the recipe wiring.

If recovery succeeds, the run report shows the re-run's result, read from
`finalize-merge.json`. If it fails too, finalisation produced no readable
result and the report shows `merge` as `null` with `stop_reason`
`unreadable`.

An explicit refusal (exit `79`) never reaches recovery.

`autodrive-finalize.yaml` emits `base-behind` with `handoff false`: GitHub will
not merge the branch as it stands (it conflicts, or an up-to-date rule is
enforced), and the merge loop's base sync brings the base in where the task
text and the repository allow, resolving any conflict, then assesses once
more. After a recovery re-run (below) it hands the PR back as a draft. It emits `checks-pending` with
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

Every step of a drive reads and writes the one runner-bound
`autodrive_state_dir`. The crusty loop, the merge loop and finalisation each
receive `{{autodrive_state_dir}}` from their parent recipe and nothing else, in
`auto-drive-to-merge.yaml`, `auto-drive-light.yaml` and
`autodrive-merge-loop.yaml`. The recovery re-run in
`step-03r-finalize-recovery` passes the same bound folder, so
`finalize-merge.json` is written to the folder the report reads.

A state folder that an agent step reports, such as `build_result.state_dir` or
a preflight's `state_dir`, is information only. It never decides where records
go. The runner copies every key of a finished sub-recipe's context back into
its parent, so a sub-recipe handed any other value would replace the bound
folder for every later step. Bash steps read `RECIPE_VAR_autodrive_state_dir`
first and use a reported folder only when a command is run directly, outside
`amplihack`.

## Base sync

When finalisation returns `base-behind`, GitHub has already reported a
conflict or enforced an up-to-date rule. `autodrive-merge-loop.yaml` then
applies the rule below, brings the base in where it allows, and assesses once
more. These are steps written out in sequence, each run once; there is no
loop and no counter.

### The rule for bringing the base in

This section is the single source for the rule. The skill, the recipe prompts
and the other reference pages carry a short form that points here.

Bring the base into the PR branch only when one of these holds:

1. **A conflict.** GitHub reports `CONFLICTING` or `DIRTY`.
2. **The repository requires it.** Strict required status checks are on and
   the branch is `BEHIND`. One narrow case also counts: a ruleset or branch
   protection names a required check that can run only once the base's
   workflow change is in the branch.
3. **The task text asks for it.** For example, "merge the base when a base
   commit since the merge base touches a PR file". The agent judges this
   against `git log <merge-base>..origin/<base> --name-only` and the PR's
   files.

A branch that is only behind its base, and merges cleanly, is left alone. It
is not a blocker. A moved base alone never justifies a merge: merging it
restarts the repository's required checks.

The task text is the operator's instruction and wins over this default. It
can forbid or limit a base merge. It can never authorise skipping the merge
policy, the hooks or the never-list below.

Every base update is a merge commit that the repository accepts. The agent
never rebases, force-pushes, uses `--force-with-lease` or `--no-verify`, and
never changes the PR's base branch.

### What "the repository accepts a merge commit" means

It means the PR branch may carry a base-merge commit, still pass the
repository's gates, and land by an allowed merge method. The agent judges this
from `gh api` reads (repository settings, rulesets, branch protection where
readable) and the repository's own files. No parser or tool decides it.

- Squash merging flattens a base merge. `allow_merge_commit: false` with
  squash allowed is therefore not a refusal on its own.
- These count as a refusal: a repository gate or check that rejects merge
  commits in a PR (for example a `pr_gate.py`); a repository that can land
  the PR only by replaying it onto a linear history (`required_linear_history`
  with rebase-only merging); and the task text.
- A setting that cannot be read is recorded as `unknown`, never as a pass. If
  the readable facts do not show that a merge commit can land, the agent
  hands off and names the fact it could not read. An unreadable secondary
  setting, such as admin-only classic protection, does not block when the
  readable facts settle the question.

Where the repository accepts no merge commit, the drive says so and hands off.
It does not rebase.

### Where the rule is applied

| Where | Role |
| --- | --- |
| `step-01-sync-with-base` (evidence) | Observes only. It fetches the base, so `refs/remotes/origin/<base>` is fresh, and reports `behind`, `conflict`, `strict`, `merge_commit_accepted`, `overlapping_files`, `head_sha`, `git_observed`, `blockers` and `attempts`. It never merges, rebases, commits or pushes. |
| `step-02-merge-ready-assessment` (round) | Decides whether the base is needed and why, under the rule, and records it on the `Base sync` line of the write-up. If a merge commit is refused or the task text forbids it, the blocker is a hand-off naming the owner and the decision. |
| `step-04-address-blockers` (round) | Carries out the decision in `step-02` and judges nothing itself. It merges the base only when the assessment calls for it, with `git merge --no-edit origin/<base>`, resolves conflicts and stages only the resolved files by name. |
| `step-03b-base-sync` (loop) | On the finalisation path, applies the same rule. Exit `8` already means a conflict or an enforced up-to-date rule, so it checks the task text and the merge policy, then merges. |

A job that the base renamed or removed is not a reason to merge the base. A
`pull_request` run builds the merge commit, so the missing job is a
measurement artefact. The assessment treats it as one only when the job passed
at the head under its base name. It counts the base as required only in the
narrow case in the rule above.

### The sync steps

| Step | Kind | Runs when | Responsibility |
| --- | --- | --- | --- |
| `step-03b-base-sync` | agent (builder) | `finalize_merge.result == 'base-behind'` | Applies the rule. Merges the base into the PR branch, resolves any conflict, runs the fast checks and pushes. Outputs `{"synced":"true"\|"false","reason":"...","head_sha":"..."}`. |
| `step-03c-sync-round` | bash | `synced == 'true'` | Runs `autodrive_merge_loop_run.sh 1`: the merge-ready loop for one more round (see [the round budget](#autodrive_merge_loop_runsh)). |
| `step-03d-sync-outcome` | bash | after `step-03c` | Maps the loop result as `step-02a` does, giving `sync_loop_outcome`. |
| `step-03e-finalize-again` | sub-recipe | `sync_loop_outcome.finalise == 'true'` | Runs `autodrive-finalize.yaml` once more. |
| `step-03f-sync-handoff` | sub-recipe | `finalize_merge.result == 'base-behind'` and `synced != 'true'` (including a base sync that returned no JSON), or `sync_loop_outcome.outcome == 'handoff'`, or a second finalisation that returned `base-behind` again | Runs [`autodrive-handoff.yaml`](#hand-off), naming the owner and the decision where the sync step gave them. |

After the second finalisation, any result other than `merged` ends the drive
with `not merged: <reason>` or a hand-off. A second `base-behind` is not
synced again: the base moved while the drive assessed it, and the PR is
handed off, so it is never left ready with no drive.

On this route `step-03f` passes `handoff_reason` as `the base sync did not
bring the base in (see the blocker for the owner and the decision), or the
base moved again after it`. The text the sync produced travels in
`handoff_blocker` as `{{base_sync.reason}} {{sync_loop_outcome.reason}}`: the
base-sync agent's reason, then the `step-03d` loop reason. The agent's reason
holds the named owner and the decision, so they reach the hand-off with no
extra field. When the one extra round ended `UNREADABLE`, the loop reason
names the [kept driver output](#kept-driver-output). The prompt's instruction
to read `merge-ready-driver.err` applies on this route as it does on
`step-02c`.

The report step shows the same reason. For a `base-behind` result it prints
`not merged: <reason>; base sync: <base_sync.reason>`, with the base-sync
reason reduced to printable ASCII and capped at 1000 bytes. The stop reason
stays `not_merged`.

### What the sync step does

1. Reads `baseRefName`, `headRefName` and `isCrossRepository` with
   `gh pr view`.
2. Checks both branch names against `^[A-Za-z0-9._/-]+$` with no leading
   `-`. A name that fails gives `synced:false`.
3. A PR from a fork (`isCrossRepository == true`) gives `synced:false`; the
   drive does not push to another repository.
4. Checks `git status --porcelain` in the PR worktree. Unrelated staged or
   modified files give `synced:false`.
5. Runs `git fetch origin -- <base>` and reads the task text. Where the text
   names a condition, the agent judges it against
   `git log <merge-base>..origin/<base> --name-only` and the PR's files. If
   the text forbids or limits a base merge in a way this state does not meet,
   the answer is `synced:false`, quoting the operator's instruction and
   naming the operator as the one who decides.
6. Checks the merge policy (repository settings, rulesets, readable branch
   protection and the repository's PR gates). If the repository accepts no
   merge commit, the answer is `synced:false`, naming the repository's
   maintainers (to allow a merge commit) or the operator (to rebase by hand),
   and the decision.
7. Runs `git merge --no-edit origin/<base>`.
8. On a conflict, resolves each conflicted file so that both sides keep their
   meaning, stages the resolved files by name, confirms no markers remain
   with `git diff --cached --check`, and commits with `git commit --no-edit`
   with hooks on.
9. Runs the fast checks: the repository's pre-commit and pre-push hooks and
   targeted tests for the files the resolution touched. This is not the full
   CI suite. Each shell call that runs tests or hooks starts by loading
   [`git-repo-env.sh`](git-repo-env-helper.md) from
   `$AMPLIHACK_HOME/amplifier-bundle/tools`. A fix commit may touch only the
   conflicted files or files the PR already changes.
10. Pushes with `git push origin HEAD:<headRefName>`, with hooks on. If the
    remote moved, it fetches, merges and pushes once more, never forcing; if
    that is refused too, the answer is `synced:false`.
11. Records in `reason` the method used and why the rule and the repository
    allow it.

A conflict that needs a decision belonging to a named owner (a product or API
choice, where neither side can be dropped silently) ends with `git merge
--abort` and `synced:false`. The reason names the owner, found from the
commits, the PR or CODEOWNERS, and the decision. There is no fixed string.

The step never uses `rebase`, `--force`, `--force-with-lease`, `--no-verify`,
`reset --hard`, `git add -A`, `git add .` or `core.hooksPath`, and never
changes git config.

A resolved conflict is code. Crusty reviews it before the one extra round, and
that round assesses it before the second finalisation. If either finds
something, `step-03d` hands off. CI restarts after a base merge, so the extra
round can end `checks-pending`; that hands off as a draft, as it does today.

### Worked cases

**Behind only, not strict.** PR #2575 is `CLEAN` and ready, but `main` moved.
The repository does not require an up-to-date branch, and the task text does
not ask for the base. The base is not merged, nothing is pushed, and
finalisation merges the PR at the assessed head.

**Strict required checks.** The same PR, but the repository requires an
up-to-date branch. GitHub refuses the merge and the read-back says `BEHIND`,
so finalisation returns `base-behind`. The sync step merges `origin/main` as a
new merge commit and pushes through the hooks. One more round assesses the new
head, and the second finalisation merges it at that head.

**A conflict.** `main` changes the same lines the PR changes after the round
said ready. The merge is refused as `DIRTY`, so finalisation returns
`base-behind`. The sync step merges `origin/main`, resolves the file keeping
both sides' text, commits, runs the fast checks and pushes. Crusty reviews the
resolution, the extra round assesses it, and the second finalisation merges
the PR. The run does not end `not merged: the branch conflicts with its base`.

**The task text forbids a base merge.** Whatever the state, the sync step
changes nothing and answers `synced:false`, quoting the instruction. The
hand-off names the operator and the decision.

**The repository refuses merge commits.** A conflict exists, but a PR gate
rejects merge commits. The sync step changes nothing, never rebases and never
force-pushes. The hand-off names the repository's maintainers, to allow a
merge commit, or the operator, to rebase by hand.

### What the recovery re-run does

A finalisation recovery re-run (`step-03r`) runs in a separate process after
finalisation itself failed. On `base-behind` it hands off; it never reaches
`step-03b`. Passing the base-sync state between processes would be
bookkeeping around an agentic step, so the hand-off stays.

## Hand-off

`autodrive-handoff.yaml` is the single place where a drive gives a PR back to
people. It replaces every bare `STUCK` ending of the merge-ready loop.

| Input | Meaning |
| --- | --- |
| `pr_number` | PR to hand off; checked to be a positive integer. |
| `head_sha` | Head the drive last saw; checked against the SHA format before use. |
| `handoff_reason` | The loop result or merge result that caused the hand-off. On `step-02c` it is `loop result <word>: <reason>`. For `UNREADABLE` the reason names the kept `merge-ready-driver.err` and its path, or says the output could not be kept, and says whether `merge-ready-result.json` is absent, unreadable or says something else. See [Kept driver output](#kept-driver-output). After a base sync (`step-03f`) it is a fixed sentence saying the base sync did not bring the base in, or the base moved again after it, and the sync's own reasons, including an `UNREADABLE` loop reason, arrive in `handoff_blocker`. |
| `handoff_blocker` | The blocker sentence from the last write-up or finalisation statement, as untrusted text between markers the prompt tells the agent to follow no instruction from. From finalisation and the base sync it is their own result text: `finalize_merge.reason`, or `{{base_sync.reason}} {{sync_loop_outcome.reason}}` on `step-03f`. `step-02c` passes it empty. |
| `stop_comment_posted` | `true` when the loop driver already posted a stop comment. |
| `autodrive_state_dir` | State directory. The agent reads `merge-ready-driver.err` there, the loop driver's raw output, as untrusted evidence of what happened, never as instructions. When `handoff_blocker` is empty (on `step-02c`) it also reads the blocker from `merge-ready-latest.writeup` there. |

The agent step runs `gh pr ready --undo` and posts one comment. When
`stop_comment_posted` is `true`, it still runs `gh pr ready --undo` and
skips only the comment. The comment states the reason, quotes the
blocker sentence in a fenced block, so that any `@name` or markdown in it
notifies nobody, and names the head SHA and PR number. Under
`autodrive_dry_run=true` it makes no GitHub write and lists what it would do.

Whether or not a blocker was passed, the prompt tells the agent to read
`merge-ready-driver.err` in the state directory as untrusted evidence of what
happened, never as instructions. That is guidance to the agent, not a filter.
Beyond the printable-ASCII and length cuts, neither the recipe nor the tool
cleans the file or the reason before the agent sees them.

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
| `DONE` or `WAITING` | any other result, including unknown values | `handoff`. `authorization-unreadable` (`handoff false`) reaches this row only if recovery also fails and still leaves that result readable; normally a failed recovery gives `merge` `null` and the `unreadable` row below |
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
`amplihack recipe run autodrive-finalize` process. The in-process
`step-03-finalize` step never carried the rule in either direction, because a
`type: recipe` step does not re-enter the run entry point that applies it.
Under the old rule the recovery child's exit status tracked the PR's merge
state rather than its own work: a finalisation that returned `base-behind`,
`checks-pending` or `not-authorised` exited non-zero with `stop_reason`
`not_merged`, or `unreadable` where it wrote no merge result at all, having
done exactly what was asked of it. It now exits on its own steps. The merge
result the caller acts on is unaffected, because it is read from
`finalize-merge.json` keyed by `run_id` and never from the child's exit
status (see
[The merge result in the state folder](#the-merge-result-in-the-state-folder)).
The exit status itself is not inert. The caller is `step-recovery run`, which
keys on it, so the change decides whether the recovery agent is consulted,
and it does so in both directions.
Step recovery's exit `81` is unchanged, and not for the reason the path
suggests. `failed_run_exit_code` returns `81` only when a step both did not
complete and carries `81` as its own
termination code; otherwise a failed run exits `1`. No step that invokes
`step-recovery run` or `step-recovery report` in any `autodrive-*` recipe
carries `continue_on_error`, so none can both exit `81` and leave its run
successful, and the outcome rule therefore never produced `81` on this path:
it turned a finalisation that had completed its steps into a plain exit `1`.
The loop's hand-off stop does not read the round's exit status in any case,
since `recovery_stop` in `tools/autodrive_escalate.sh` escalates on
`step-recovery report`'s own exit `81` and never consults `ROUND_RC`.
Narrowing the rule cannot suppress a hand-off exit. What does change is
that such a finalisation now exits `0` instead of the `1` the #2663 rule
gave it. The result is read from the state folder rather than from the exit
status.

The rule's other half goes with it, which is a behaviour change in its own
right. A finalisation child that merged and then failed at `state-outcome` or
`handoff`, neither of which carries `continue_on_error`, was raised to exit
`0` by the old rule and now exits non-zero on the step that failed. Both halves of
#2663 belong to the top-level drive, so a sub-recipe keeps neither. The merge
is untouched either way: the `merge` step writes `finalize-merge.json` before
any step that could fail after it, and the drive's own report reads the merge
from that record rather than from a child's status. What this direction does
reach is step recovery: a non-zero child now consults the recovery agent
after a merge has already happened, which can re-run `autodrive-finalize`
against an already-merged PR (#2773). That run issues no second merge
request, because `tools/autodrive_merge.sh` reports `merged` when the PR
already reads MERGED at the expected head. Both directions are the behaviour
of every release up to 1.2.164, restored rather than introduced.

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
| `base-behind` | `not merged: <reason>; base sync: <base_sync.reason>` (`not_merged`). The base-sync part appears only when `base_sync` holds a reason; it is reduced to printable ASCII and capped at 1000 bytes. | `0` |
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
# {"result":"merged","pr":2575,"expected_head":"15a7f6f...","reason":"merged at the expected head; ...","merge_method":"squash","merge_method_reason":"no merge method was given to the run, so 'squash' was taken: ..."}

# A third argument is the merge method the run was given (#1888); an empty
# one, or none, leaves the choice to the repository's allowed methods.
autodrive_merge 1170 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605 merge
```

The function prints one JSON line built with `jq -cn`. It calls `gh` once for
each of these, in order: PR state with title, body and `mergeStateStatus` (one
read), `checks --required` (and `checks` when no required checks are
reported), the repository's allowed merge methods, merge, PR state after the
merge. It adds no retries or waits.

```bash
autodrive_merge 2575 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605
# {"result":"checks-pending","pr":2575,"expected_head":"15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605","reason":"waiting for test (ubuntu-latest) at 15a7f6f47c1e2d0b9a8f7e6d5c4b3a2918070605"}
echo $?
# 7
```

| Exit | Meaning |
| --- | --- |
| `0` | `merged`: GitHub reads the PR `MERGED` with `headRefOid` equal to the expected head. This holds whether the PR already read so before the request (no request is made), after the request, or after `gh pr merge` exited non-zero. |
| `2` | Bad arguments (fewer than two or more than three, a malformed PR number or head), or the merge-message helper failed its path checks. |
| `3` | `checks-not-green`; no merge attempted. |
| `4` | `refused`: GitHub declined the merge, for example on a moved head, a missing review or branch protection, and the read-back `mergeStateStatus` is not `BEHIND` or `DIRTY`; or the merge method could not be settled (#1888), in which case no merge was requested. |
| `5` | `not-open`: the PR was closed, or merged at a head other than the expected one, before the merge. |
| `6` | `message-unavailable`: the merge-message helper refused the title or body. |
| `7` | `checks-pending`: a required check is still running at the head; no merge attempted. |
| `8` | `base-behind`: `DIRTY` before the merge request, or `DIRTY` (conflict) or `BEHIND` (up-to-date rule enforced) after GitHub refused it. The reasons state the fact and prescribe no remedy; the [base sync](#base-sync) decides what to do. |
| `79` | A `gh` call exited `79`; passed through at once. |

### `autodrive_merge_loop_run.sh`

Holds the command body of `autodrive-merge-loop.yaml` `step-02` in a trusted
tool so the base-sync round can run it again. It resolves and runs in the
actual PR worktree, retains task-file cleanup, and passes the round count and
budget sum described below. It allocates no review repository or launch folder.

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

It also keeps the loop driver's combined output in
`<state_dir>/merge-ready-driver.err`. The tool opens the file new as
descriptor 4, then runs the driver as
`{ bash autodrive_loop.sh … 2>&1; } 4>&- | keep_and_log`, which closes the
descriptor for the driver and its agents, and takes the driver's status from
`PIPESTATUS[0]` on the next line. The copier's own status reaches the reason
and nothing else, so no exit in the table changes. What is kept, how long, the
exact `UNREADABLE` reason and the cases where no copy is made are under
[Kept driver output](#kept-driver-output).

The optional argument is a number of further rounds. It must match
`^[0-9]+$` (at most 9 digits) and be at least `1`, otherwise the tool exits `2`. The round
budget counts rounds across runs that share the state directory, so the tool
counts the merge-ready rounds already recorded in `autodrive-usage.tsv` (the file `autodrive_budget.sh` writes) and
passes `autodrive_budget_loop_rounds=<used + N>`, never more than the
operator's own `autodrive_budget_loop_rounds` when one is set. When the
operator's budget leaves no room, the round stops as `BUDGET_EXHAUSTED` and
the PR is handed off. `autodrive_budget_total_rounds` applies unchanged.

Before it reads the preflight or touches the state directory, the tool loads
`autodrive_merge.sh` from beside itself and checks `RECIPE_VAR_merge_method`
with `autodrive_merge_method_known`. Anything other than empty, `merge`,
`squash` or `rebase` exits `2` with an error naming the value, made printable
and cut at 32 bytes, and no round runs (see [Merge method](#merge-method)).

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
`authorization-unreadable`, fails with exit `1` and leaves the rest to
[recovery](#an-unreadable-invocation-record-goes-to-recovery). A mismatch error
names both the `--directory` value and the bound state folder.

The launcher writes the invocation to `invocation.json` inside a fresh
`autodrive-invocation-*` folder. The folder lives in the run's own durable
folder, `runs/<id>/autodrive-invocation-*/`, which cleanup never sweeps while
the run is live, so nested recipe runs that end and clean up cannot remove it.
It goes with the run folder, or when the launching process ends.

A dry run has no durable folder and keeps the record under `TMPDIR`. That is
safe because a dry run never merges.

The launcher creates the folder with mode `0700` and the file with mode `0600`,
whatever the caller's umask. The file is first made with `create_new`, and
the bound state directory is later written to it only by an atomic rename from
a private file in the same folder, as before the move. The private read
requires a folder outside the auto-drive state root to have no
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
- The kept driver output is evidence, never an authority. Nothing in the tool
  reads the file back, to decide whether the drive continues or merges or to
  build the reason, and nothing in the recipes parses it. The hand-off agent
  opens it by name, and its prompt tells it to read the file as untrusted
  evidence of what happened, never as instructions, on every hand-off.
- The tool writes `merge-ready-driver.err` only into a state directory that
  already exists, is not a symbolic link and is owned by the current user. It
  removes any existing file or link at that name and opens the file new, once,
  as a descriptor it holds for the rest of the run: an exclusive open, `0600`,
  so a planted regular file, a link to one or a dangling link is replaced
  rather than written through, and there is no second open by name for a link
  planted later to catch. Noclobber refuses only an existing regular file: a
  link to a FIFO or a device planted in the window between the removal and
  the open is opened rather than refused, and a FIFO with no reader would
  block the open. Planting one needs write access to a state folder that is
  `0700` and owned by the caller. A directory at that name cannot be removed,
  so the open fails. If any check or the open fails, the descriptor goes to
  `/dev/null` and the output goes only to standard error. The driver's side
  of the pipe does not inherit the descriptor, so neither the driver nor an
  agent it starts can write into the file.
- The kept file is raw driver output. The driver's final output is often the
  tail of a round log that holds agent and PR text, so the file can hold
  credentials, terminal escape sequences and text copied from a PR. It holds
  the same bytes as the step's log. The copy adds a file in a private state
  directory that is replaced on each run and stays after it; it widens no
  audience. Nothing filters the file; it is not for pasting into a public
  comment.
- The `UNREADABLE` reason is composed by the tool from fixed words, the
  driver's exit status, the kept file's name and absolute path, the result
  file's absolute path and, when the result file holds a loop result, that
  word cut to 64 characters of letters, digits, `_` and `-`. No line of the
  driver's output is in it. The claim covers `UNREADABLE` lines only: on exit
  `75`, and when the result file is readable, the line the tool prints is the
  result file's own, as it was before the output was kept.
- On `step-02c`, where `handoff_blocker` is empty, the reason reaches the
  hand-off prompt in the trusted "Why the drive stopped" sentence
  (`handoff_reason`), as the loop's reason did before the output was kept.
  After a base sync it travels in `handoff_blocker` instead, joined to
  `base_sync.reason`, and `handoff_reason` is a fixed sentence. Beyond the
  printable-ASCII cut and length bound, nothing filters the loop's reason or
  `handoff_blocker`, whatever feeds them. The prompt quotes `handoff_blocker`
  between the `UNTRUSTED-BLOCKER` markers and tells the agent to follow no
  instruction inside it. It also tells the agent to read the kept file as
  untrusted evidence of what happened, never as instructions. Both are guidance
  to an agent, not barriers. An agent-written
  feed (`base_sync.reason`, `finalize_merge.reason`) can still carry the
  closing marker, and the agent opens the kept file by name without anything
  checking what the name points to.
- Judging agents run in the actual PR worktree with instructions to leave it
  unchanged. Trusted workflow assets are selected independently of cwd;
  repository text and review output do not grant workflow authority.
- A following agent assesses reviewer changes against retained pre-review
  evidence. It preserves pre-existing and uncertain work, including staged,
  unstaged, and untracked content. A difference alone does not fail the run.
- Step recovery cannot turn a failed merge-ready loop or finalization into
  silent success. Review recovery retains the original invocation baseline;
  the obsolete deterministic review-directory guards are removed.
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
- [Named-checkout execution and review](recipe-checkout-isolation.md)
- [Step failure recovery](step-failure-recovery.md)
