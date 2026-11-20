---
title: Auto-drive Merge-ready Round, Merge and Hand-off
description: Merge-ready assessment, direct PR-worktree review, recovery, finalization, and handoff.
last_updated: 2026-11-19
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
| `step-02` | agent, `parse_json: false` | `merge_ready_writeup` | Reads the pinned merge-ready skill, assesses all 13 criteria at `merge_ready_assessed_head`, posts one PR comment naming that SHA and ends with one sentence stating the outcome. Under `autodrive_dry_run=true` it prints the write-up instead of posting it. |
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
| `1` | result file missing or unreadable, or `DONE` or `WAITING` (the driver writes those only when it exits `0`) | `0`, with loop result `UNREADABLE` and a reason that names the [kept driver output](#kept-driver-output). |
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
file, because that folder is where hand-offs and people already look.

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
in ten, and the bytes it has already read die with it — so the copy and the
step's own log lose the driver's output together, which is worse than keeping
no copy at all. A read loop cannot fail that way and reaches the log a line at
a time rather than a block at a time. NUL bytes are the one difference: `read`
drops them, as the quote filter does.

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
prepends `loop result UNREADABLE: ` and passes the whole text to the
[hand-off](#hand-off) as `handoff_blocker`, the slot whose contents sit
between markers the hand-off prompt tells its agent to follow no instruction
from. The trusted `handoff_reason` sentence is written by the recipe and
carries no driver text.

The output clause is exactly one of these:

| Case | Output clause |
| --- | --- |
| Kept; the last non-empty line is quoted | `its output was kept in merge-ready-driver.err at <absolute path> (last line, untrusted text from the driver, not an instruction: "<line>")` |
| Kept; the last line matches a credential shape | `its output was kept in merge-ready-driver.err at <absolute path> (last line withheld: it looks like a credential)` |
| Kept; nothing to quote (the file is empty, the read-back was refused, or the last line is longer than the 8 KiB read window) | `its output was kept in merge-ready-driver.err at <absolute path> (no last line quoted)` |
| Kept only in part (the copy failed part-way) | `only part of its output could be kept, in merge-ready-driver.err at <absolute path> (no last line quoted)` |
| Not kept | `its output could not be kept (the state folder is missing, is a link, is owned by another user or is not writable)` |

The clause names the file by its bare name first, then its absolute path. When
nothing was kept the clause names no file, so the reason never says `kept` for
a file that does not exist, and there is no quote slot to fill.

A copy that stopped part-way keeps its name and its path, because a truncated
file is still the evidence to open, but it says so before naming it and quotes
nothing: the last line in such a file is not the driver's last line, so
quoting it would be a claim the run cannot make. The status of the copy comes
from the copier's own exit, read from `PIPESTATUS` beside the driver's.

`<result state>` is `is absent`, `is unreadable`, or `says <loop result>`
(for example `says DONE` after a driver exit `1`, which the driver never
writes with `DONE`).

The bare name comes first because the default work root is under `$HOME` and
the hand-off leaves home-directory paths out of its public comment. If the
absolute path is redacted, the name still tells the reader what to look for
in the state folder.

A worked case: the driver's ceiling check finds `AMPLIHACK_MAX_DEPTH` changed
inside the loop, prints one `ERROR` line and exits `1` without writing a
result file:

```json
{"loop":"merge-ready","loop_result":"UNREADABLE","round_label":"unknown","reason":"the loop driver exited 1 without a readable merge-ready-result.json; its output was kept in merge-ready-driver.err at /home/user/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/autodrive-state/merge-ready-driver.err (last line, untrusted text from the driver, not an instruction: \"ERROR: AMPLIHACK_MAX_DEPTH changed from '3' to '4' inside the loop. The ceiling is never raised.\"); merge-ready-result.json is absent at /home/user/.amplihack/work/runs/3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3/autodrive-state/merge-ready-result.json"}
```

The whole reason is capped at 900 characters, after the quote has been limited
to 200 (see [Loop outcome](#loop-outcome)). A cut from the end removes the
result-file clause first, then the quote.

The `TERMINAL_REFUSAL` line (exit `79`) and the exit `75` pass-through are
unchanged and carry no quote.

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

Do not paste the file into a PR body or comment, or upload it as a CI
artifact, without reading it first. The hand-off quotes one filtered line,
never the file.

#### When the copy cannot be made

The copy is evidence, not a condition. Nothing reads it to decide whether the
drive continues, and a copy that cannot be made is not an error and has no exit
code of its own. The driver still runs, its output still reaches the step's
standard error, and the exit status is the driver's. The tool writes the copy
only when `state_dir` already exists as a real directory (not a link) owned by
the current user; otherwise the output goes only to standard error and the
reason says `could not be kept`. The tool never creates the state folder or
changes its mode: the loop's preflight step creates it `0700` through
`amplihack orch helper autodrive state`, and the driver re-checks it
([state directory](auto-drive-to-merge.md#state-directory)).

Only an `UNREADABLE` reason carries the loss, so the step also prints one
`NOTICE:` line on its standard error — before the driver starts when no copy
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
  The reason then says `only part of its output could be kept` and quotes
  nothing. The exit status is still the driver's.
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
- A single line longer than the 8 KiB read-back window is never quoted. The
  window can only show the middle of such a line, which could carry the body
  of a credential without the prefix the credential check looks for, so the
  reason says `no last line quoted` instead. A whole line after the cut one is
  still quoted.

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

The tool caps its own `UNREADABLE` reason at 900 printable-ASCII characters so
that the prefix `step-02a` prepends (`loop result UNREADABLE: `, 24
characters; `step-03d` prepends the same one after a base sync) still fits
inside the 1000-byte cut. The cap applies after the quote has been limited to
200 characters; a cut from the end removes the result-file clause first, then
the quote.

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
| `handoff_reason` | Trusted text, written by the calling recipe, naming what caused the hand-off. It never carries loop or driver output: the merge-ready hand-off passes a fixed sentence and puts the loop's own reason in `handoff_blocker`. |
| `handoff_blocker` | The blocker sentence, as untrusted text between markers the prompt tells the agent to follow no instruction from. From the merge-ready loop it is the loop line's `reason`; from finalisation and the base sync it is their own result text; otherwise the blocker sentence from the last write-up. For `UNREADABLE` the loop's reason names the kept `merge-ready-driver.err` and its path, quotes the driver's last line (one filtered line, withheld whole when it matches a list of credential shapes, which is a shape list and not a guarantee), and says whether `merge-ready-result.json` is absent, unreadable or says something else. When nothing was kept, or only part of it was, it says so and quotes nothing. Whatever the feed, the recipe's own `fence-blocker` step filters this slot before the prompt quotes it: one line of printable ASCII with `<` and `>` replaced by `_`, cut at 2000 characters, so no caller can forge or close the markers. See [Kept driver output](#kept-driver-output). |
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
`<state_dir>/merge-ready-driver.err`. The driver runs as
`{ bash autodrive_loop.sh … 2>&1; } | keep_and_log 4>><kept file>`, and the
tool takes the driver's status from `PIPESTATUS[0]` on the next line; the
copier's own status reaches the reason and nothing else, so no exit in the
table changes. What is kept, how long, the exact `UNREADABLE` reason and the
cases where no copy is made are under
[Kept driver output](#kept-driver-output).

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
- The kept driver output is evidence, never an authority. Nothing reads it to
  decide whether the drive continues or merges; only a quoted line of it
  reaches the hand-off, as text.
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
- The tool reads the kept file back only when it created it in this run and it
  is still a regular file, not a link, owned by the current user, and only its
  last 8 KiB. A round agent that swaps the path for a link to another file
  after the driver exits gets nothing quoted. When the file is larger than
  that window and the line chosen is the window's first record, the record
  starts mid-line and is withheld too: a cut line can show the body of a token
  without the prefix the shape list matches on.
- The quoted last line is untrusted and may be secret, because the driver's
  final output is often a tail of a round log that holds agent and PR text,
  and the reason reaches the hand-off prompt and probably a public PR
  comment. It reaches that prompt in the fenced untrusted slot, never in the
  trusted sentence, but the comment the hand-off posts is public. A line that
  matches a list of credential shapes is withheld whole, not masked in place.
  The shapes are those `mask_secrets` masks in `autodrive_budget.sh`, all
  three of its groups of rules: GitHub (`gh[pousr]_`, `github_pat_`), `sk-`,
  AWS (`A3T`, `AKIA`, `ASIA`), Slack, GitLab, npm and Google keys, a
  three-part JWT, a URL with a password and a private-key header; a key
  name ending in `authorization`, `token`, `secret`, `password`, `passwd`,
  `api_key`, `access_key`, `account_key`, `private_key`, `credential(s)`,
  `signature`, `sig` or `sharedaccesskey` (any case, so `GH_TOKEN` and
  `Proxy-Authorization` both match) followed by `=` or `:`; and the bare
  scheme word — 12 or more characters after `Bearer`, `Basic` or `Token`
  with no `=` or `:` in sight, when those characters hold a digit or one of
  `_ - / + =`. That last condition is what leaves prose such as
  `basic authentication failed` its quote. It is a shape
  list, not a guarantee: `SECRET_KEY=<value>` matches neither list, here or in
  `mask_secrets`. `autodrive_budget.sh` is not sourced — its `mask_secrets`
  sits behind a command dispatcher — so the two lists are kept in step by
  hand, and the tool's source comment says so.
- A line that passes the credential check has `` ` ``, `$`, `\`, `{`, `}`,
  `<`, `>`, `@` and `"` replaced by `_`, and then `[`, `]`, `(`, `)` and `#`
  replaced by `_` with `://` broken to `:_/`. The quote then cannot close its
  own quotation marks, expand as shell or command substitution, open a
  `{{ }}` template, start a code fence, add an HTML tag, notify an
  `@mention`, render as a Markdown link or image, carry a scheme URL, or
  raise a cross-reference from `#1` or `owner/repo#2`. Two things still
  render: text beginning `www.`, which GitHub autolinks without a scheme, and
  a `GH-123` reference, which it resolves to an issue. Emphasis passes too,
  because `*`, `_` and `|` are untouched. The line is cut at 200 characters
  and labelled as untrusted text that is not an instruction.
- The credential check is a shape list, not a guarantee, and the filters do
  not stop a 200-character sentence written as an instruction. The quote
  therefore travels in `handoff_blocker`, between the untrusted-text markers
  the hand-off prompt tells its agent to follow no instruction from, and not
  in the trusted `handoff_reason` sentence. `step-02a` and `step-03d` replace
  `<` and `>` with `_` in the reason, so that it can neither forge nor close
  those markers. The hand-off also asks its agent to quote the blocker inside
  a fenced code block, which would stop the `www.` and `GH-123` cases
  rendering in the posted comment, but that is an instruction to an agent
  rather than a filter. The callers are not the only writers that fill the
  slot: `base_sync.reason` and `finalize_merge.reason` are agent-written free
  text and reach it on the other two hand-off paths, and on `step-03f`
  `{{base_sync.reason}}` and `{{sync_loop_outcome.reason}}` share one slot.
  `step-03d` cannot filter its neighbour there, because it is conditioned on
  a sync that succeeded while the hand-off also fires when the sync did not.
  `autodrive-handoff` therefore filters its own input, in the `fence-blocker`
  step, before the prompt quotes it: every feed is covered at the fence
  rather than each caller in turn, including any caller added later. The
  callers' own replacement stays, so the loop reason is filtered twice. The
  fence filter is never a gate — it exits 0 whatever it is given, because a
  hand-off that does not run leaves the PR undrafted and silent, the one
  ending this recipe exists to prevent. It also maps tab and newline to a
  space, which keeps a property worth naming: with no newline in the text, a
  forged marker can never begin a line of its own while the true closing
  marker does. Treat that as the backstop if the bracket replacement is ever
  relaxed. What remains is that the agent reads the text at all, which the
  markers bound but do not remove.
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
