---
title: Auto-drive Merge-ready Round, Merge and Hand-off
description: how the merge-ready agent writes up the 13 criteria in the PR, how the loop reads its outcome, the GitHub-only merge conditions, the hand-off recipe and the run report (#2575).
last_updated: 2026-11-14
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
- [Outcome word](#outcome-word)
- [Round files](#round-files)
- [How the loop ends](#how-the-loop-ends)
- [Finalisation and merge conditions](#finalisation-and-merge-conditions)
- [Hand-off](#hand-off)
- [Run report](#run-report)
- [Tool reference](#tool-reference)
- [Security properties](#security-properties)
- [What was retired](#what-was-retired)

## Round steps

`autodrive-merge-round.yaml` runs these steps in order for each round.

| Step | Kind | Output | Responsibility |
| --- | --- | --- | --- |
| `step-01a` | bash | `merge_ready_assessed_head` | Captures `git rev-parse HEAD` in the PR worktree before the assessment and checks it is 40 or 64 lowercase hex characters; any other value exits `2`. |
| `step-02` | agent, `parse_json: false` | `merge_ready_writeup` | Reads the pinned merge-ready skill, assesses all 13 criteria at `merge_ready_assessed_head`, posts one PR comment naming that SHA and ends with one sentence stating the outcome. Under `autodrive_dry_run=true` it prints the write-up instead of posting it. |
| `step-03` | agent | `merge_ready_outcome` | Answers one question about the write-up: did the assessment conclude ready, not ready, or hand-off? It replies with one word. |
| `step-05a` | bash | `merge_ready_word` | Normalises the word through `autodrive_round_word`. |
| `step-04` | agent | | Runs only when `merge_ready_word.word == 'not-ready'`. Works on the blockers named in the write-up, commits and pushes in the PR worktree. |
| `step-05` | bash | | Writes the round files through `autodrive_round_file`, passing `merge_ready_assessed_head`. |

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

## Outcome word

`autodrive_round_word` maps the reader's answer to the loop driver's verdict.
It lower-cases the input, keeps only `a` to `z` and `-` (under `LC_ALL=C`)
and compares the result exactly.

| Normalised word | `merge_ready_verdict` | Next step |
| --- | --- | --- |
| `ready` | `MERGE_READY` | Loop-health evaluation; `DONE` leads to finalisation. |
| `not-ready` | `NOT_MERGE_READY` | `step-04` repairs the named blockers; another round follows. |
| `hand-off` | `HANDOFF` | The loop stops and the PR is handed off. |
| anything else, including empty | `NOT_MERGE_READY` | Treated as not ready, so another round runs. |

An unclear answer therefore costs one more round. It never ends the drive
with `STUCK` and never leads to a merge.

## Round files

`autodrive_round_file` writes three files per round into the run's
[state directory](auto-drive-to-merge.md#state-directory):

| File | Content |
| --- | --- |
| `merge-ready-<label>.json` | `{"merge_ready_verdict":"MERGE_READY"}`, `NOT_MERGE_READY` or `HANDOFF`. This is the only field. |
| `merge-ready-<label>.writeup` | The write-up, cut at 256 KiB with a final truncation line. |
| `merge-ready-<label>.head` | The head captured by `step-01a` before the assessment: 40 or 64 lowercase hex characters. |

The loop's record step copies the last `.writeup` and `.head` to
`merge-ready-latest.writeup` and `merge-ready-latest.head`. Finalisation uses
`merge-ready-latest.head` as the expected head of the merge.

The `.json` round file is loop plumbing, not a readiness record. The loop
driver reads `merge_ready_verdict` from it only when `LOOP_NAME=merge-ready`. The former fields `blocker_count`, `head_sha`,
`test_signal`, `ci_signal` and `<record>.findings` are no longer written for
the merge-ready loop. The crusty loop keeps its own round records,
written by `autodrive_crusty_record.sh`.

## How the loop ends

`autodrive-merge-loop.yaml` `step-02` runs `autodrive_loop.sh` with
`LOOP_NAME=merge-ready`. The step keeps its default `fatal` handling and maps
the driver's exit status itself, so that exit `79` is never hidden by
`continue_on_error` (#2490).

| Driver exit | Loop result (read from `merge-ready-result.json`) | Step exit |
| --- | --- | --- |
| `0` | `DONE` | `0` |
| `1` | `USAGE_LIMIT` | `1`: the run stops; a later run continues the work. |
| `1` | `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF` | `0`, with the loop line printed. |
| `1` | result file missing or unreadable, or `DONE` (the driver writes `DONE` only when it exits `0`) | `0`, with loop result `UNREADABLE`. |
| `0` | anything other than `DONE`, or no readable result | `0`, with loop result `UNREADABLE`. |
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
evaluator runs, then the driver exits `1`. The no-progress rule does not
apply to the merge-ready loop: its round file has no CI signal, so a round
that waits for pending CI at an unchanged head would otherwise stop the loop.
The budget stop bounds the loop instead. A round that writes no write-up or
head leaves none behind, so no earlier assessment reaches the next round or
the hand-off.

`step-02a` turns the loop result into
`{"outcome":"done"|"handoff","reason":"...","loop_result":"...","stop_comment_posted":"true"|"false","head_sha":"..."}`,
with the reason cut to 1000 bytes of printable ASCII:

| Loop result | Outcome | Next |
| --- | --- | --- |
| `DONE` | `done` | `step-03` runs [`autodrive-finalize.yaml`](#finalisation-and-merge-conditions). |
| `STUCK`, `BUDGET_EXHAUSTED`, `HANDOFF`, `UNREADABLE` | `handoff` | `step-02c` runs [`autodrive-handoff.yaml`](#hand-off). |

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
4. `autodrive_merge` finds the PR open and its checks green, then GitHub
   accepts `gh pr merge --squash --match-head-commit <expected sha>`.

GitHub's acceptance covers required reviews, branch protection and a head
that moved on the server. No agent-written text is a merge condition: an
agent can stop the drive, hand it off or start another round, but cannot
cause a merge.

### Checks

`autodrive_merge` reads `gh pr checks <pr> --required`.

| Observation | Result |
| --- | --- |
| Required checks all passed | Merge is attempted. |
| Any required check failing | `checks-not-green`; no merge attempt. |
| Any required check pending | `checks-not-green`; no merge attempt and no waiting. |
| `gh` reports `no required checks reported` | Falls back to all checks: exit `0` counts as green; anything else, including no checks at all, is `checks-not-green`. |

The fallback fails closed. If a later `gh` release changes that message, the
result is a hand-off, not a merge.

Pending checks are not polled. The merge-ready criteria already require green
CI, so a ready round normally finds the checks complete. If they are still
running at finalisation, the result is `checks-not-green` and a hand-off;
the drive does not wait, retry or sleep, in line with the rule against
timer-based fixes. A later run picks the PR up once people mark it ready.

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
| | `2` (bad arguments or unsafe helper) | `2` | |
| | `79` | `79` | |

A `not-authorised` result is the expected outcome of a verify-only run
(`no_merge=true` or `autodrive_dry_run=true`), which stops before the
authorization helper is called, or of a captured invocation that does not
allow a merge. The PR stays open and is not drafted. On a merge run, a helper
that exits non-zero or prints no object with a boolean `allows_merge` gives
`authorization-unreadable`, and the PR is handed off. Its reason names only
the helper's exit status. A helper exit `79` passes through as `79`.

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
| `TERMINAL_REFUSAL` or `TERMINAL_POLICY_REFUSAL` | | `terminal_refusal` |
| `DONE` | `merged` or `not-authorised` | `completed` |
| `DONE` | any other result, including `authorization-unreadable` | `handoff` |
| `DONE` | `null`: finalisation did not run, or its output is missing or unreadable, with no exit `79` | `unreadable`, with a `note` naming the error kind and path |
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

A PR already merged at entry is still reported through the `already_merged`
path, which keeps its binding, selection, `run_id`, provenance and PR checks.

## Tool reference

All tools live in `amplifier-bundle/tools/`, are sourced from the pinned
launch bundle, and are listed in `REQUIRED_ASSETS` so that the launch-bundle
digest covers them.

### `autodrive_round_word.sh`

Defines functions only; sourcing it has no other effect.

```bash
source "$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_round_word.sh"

autodrive_round_word "Ready."
# {"word":"ready","merge_ready_verdict":"MERGE_READY"}

autodrive_round_word "Probably ready"
# {"word":"not-ready","merge_ready_verdict":"NOT_MERGE_READY"}

printf '%s' "$RECIPE_VAR_merge_ready_writeup" |
  autodrive_round_file "$RECIPE_VAR_autodrive_state_dir" round-3 NOT_MERGE_READY \
    "$RECIPE_VAR_merge_ready_assessed_head"
# writes merge-ready-round-3.json, .writeup and .head
```

`autodrive_round_file <state_dir> <round_label> <verdict> <assessed_head>`
reads the write-up from standard input, never from an argument, and writes
`<assessed_head>` to the `.head` file. It does not run `git` itself.

| Check | Refusal |
| --- | --- |
| `round_label` matches `^[A-Za-z0-9._-]{1,64}$` and is not `.` or `..` | exit `2` |
| `state_dir` passes `autodrive_private_dir` | exit `2` |
| `verdict` is `MERGE_READY`, `NOT_MERGE_READY` or `HANDOFF` | exit `2` |
| `assessed_head` is 40 or 64 lowercase hex characters | exit `2` |
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
each of these, in order: PR state with title and body (one read), `checks --required`,
merge, PR state after the merge. It adds no retries or waits.

| Exit | Meaning |
| --- | --- |
| `0` | Merged; the read-back state is `MERGED`. |
| `2` | Bad arguments, or the merge-message helper failed its path checks. |
| `3` | `checks-not-green`; no merge attempted. |
| `4` | `refused`: GitHub declined the merge, for example on a moved head, a missing review or branch protection. |
| `5` | `not-open`: the PR was not open before the merge. |
| `6` | `message-unavailable`: the merge-message helper refused the title or body. |
| `79` | A `gh` call exited `79`; passed through at once. |

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
  assessed (captured before the assessment and format-checked), green checks and
  GitHub's acceptance of `--match-head-commit`.
- Merge permission comes only from the captured invocation. Recipe context
  can narrow it, never grant it.
- Loop results are read from the driver's result file, not from text an
  agent printed.
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
the only gate before a merge.

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

`Binding`, `validate_binding` and `MAX_RECORD_BYTES` remain, in
`orch/autodrive/identity.rs`, because authorisation and the already-merged
report still use them. The crusty loop's `STUCK` endings are unchanged.

## Related documentation

- [Auto Drive To Merge Reference](auto-drive-to-merge.md)
- [Auto-drive evidence and outcomes](auto-drive-evidence.md)
- [Verify a PR without merging](../howto/verify-pr-with-auto-drive.md)
- [Loop-health evaluator](loop-health-evaluator.md)
- [Crusty completion contract](autodrive-crusty-completion.md)
