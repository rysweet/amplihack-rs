---
title: Auto-drive Goal Loop
description: The shared goal-driven agent loop behind auto-drive-to-merge and auto-drive-light. Turns, readings, actions, guidance, turn records, run endings, goal contract v1, and the limits of the design.
last_updated: 2026-11-23
review_schedule: quarterly
owner: workflow-team
doc_type: reference
---

# Auto-drive Goal Loop

`auto-drive-to-merge` and `auto-drive-light` run one loop. The goal is to get
the pull request merged under the owner's conditions. Each turn, an agent reads
a fresh reading of the live state and the task text, then chooses the next
action. Fixed code does only mechanical work: reading state, pushing, merging
and waiting for an event. It never decides that a run ends because of how the
run is going.

Issue #3129 describes the design. This page is the specification. It closes
#1924, #3094, #3109 and #3035.

## Contents

- [How a run goes](#how-a-run-goes)
- [A turn](#a-turn)
- [The reading](#the-reading)
- [Actions](#actions)
- [Guidance](#guidance)
- [Waiting](#waiting)
- [How a run ends](#how-a-run-ends)
- [Failures inside a turn](#failures-inside-a-turn)
- [Turn records](#turn-records)
- [Reported outcomes](#reported-outcomes)
- [Trust boundaries that stay](#trust-boundaries-that-stay)
- [Removed behaviour](#removed-behaviour)
- [Retired keys and aliases](#retired-keys-and-aliases)
- [Goal contract v1](#goal-contract-v1)
- [Files](#files)
- [Known limits](#known-limits)
- [Related](#related)

## How a run goes

```
auto-drive-to-merge
  step-recovery-init, autodrive-pr-input, autodrive-build   (unchanged)
  autodrive-loop  ->  autodrive-goal-loop recipe
                        autodrive_goal_loop.sh  (the driver)
                          repeat:
                            final?   one gh pr view: merged, closed, refused
                            read     reading.json + guidance
                            turn     autodrive-goal-turn recipe (one agent step)
                            act      review | wait | merge (the goal's actions)
                                     continue | wait-owner | handoff | ready | stop (shared)
                            record   record.md
  autodrive-report                                             (short)
```

`auto-drive-light` is the same up to its eligibility measurement. It then runs
the same loop with that measurement given to the agent as information. It no
longer escalates to `auto-drive-to-merge`, because both drives run one loop.
Its `no_merge` default stays `true`, so it is merging-off unless the operator
says otherwise.

## A turn

One turn is one fresh `amplihack recipe run` of `autodrive-goal-turn`. The
driver starts every turn at the same recipe depth, so the loop has no depth
limit.

1. Read. Mechanical tools take a reading and the guidance and write them to
   the turn folder.
2. Decide and act. One agent step. The agent does any agentic work itself:
   fix and push through the hooks, bring the base in, install a host input, or
   carry out a later task step. It ends by naming one next action and its
   arguments.
3. Mechanical action. The action the agent named runs: one of the goal's
   actions (`review`, `wait`, `merge`) or a shared one (`continue`,
   `wait-owner`, `handoff`, `ready`, `stop`).
4. Record. The turn record is written.

Each turn takes one action. If the action word is missing or unknown, no
mechanical action runs, the turn record says so, and the next turn starts.

A fresh reading between actions is what keeps the loop from judging a stale
state (#3090). An agent's tool call also cannot block for a 30-minute check,
which is why waiting is an action and not part of the agent step.

## The reading

`reading.json` (schema 1) holds, for this turn:

| Section | Contents |
|---------|----------|
| `repo` | Repository, worktree path, current branch, whether the worktree is clean, commits not yet pushed |
| `merge_authority` | `no_merge`, dry run, and the effective authority |
| `pr` | Head, base, `mergeable`, `mergeStateStatus`, draft, state |
| `checks` | Each check at the head with its status, counts, and whether `required` could be read |
| `reviews` | GitHub review verdicts and Crusty round verdicts, each with the head it was given at and the round's minor findings, which the agent files as follow-up issues before it merges; a round whose record or minor findings could not be read is listed with its `error` |
| `base` | Merge base, how far behind the PR is, conflicting files from `git merge-tree --write-tree`, and the files the base changed since the merge base that the PR also touches |
| `repository` | Allowed merge methods; whether strict status checks are required (`strict_status_checks`, `strict_source`, `strict_unreadable`); `merge_state_behind` as evidence only |
| `guidance` | Name, time, caller, source, new flag and size of each entry; `guidance_error` when guidance could not be read |
| `outcomes` | Every required outcome an agent has named in this run, from the run's ledger, with its latest status, evidence, reason, the turn that last judged it and whether a rejection was answered; `outcomes_error` when the ledger could not be read |
| `previous` | The last turn's number, action and mechanical result; after a failed turn run also its `recovery` and `failure` (class, whether it was retryable, whether the agent finished deciding, the completed steps and the turn log's path) |
| `goal_file` | Path to the goal text |
| `turns_dir` | Path to the turn records |

Strict status checks come from the rules endpoint, then classic branch
protection where it can be read. A policy that was read wins, whether it is
`true` or `false`. If neither can be read, `strict_status_checks` is `null` and
`strict_unreadable` gives the reason. `merge_state_behind` reports whether
GitHub's `mergeStateStatus` is `BEHIND`, beside the policy and as evidence only:
it never sets the policy, never overrides one that was read, and never fills in
one that could not be read (#3141). GitHub can report `BEHIND` under a policy
that does not require an up-to-date branch.

Fetching the base is allowed. Reading never changes the worktree. A read that
fails records its error in its own section and never ends the turn.

## Actions

The agent picks one. The word is used only to dispatch the mechanical step.

| Action | What runs |
|--------|-----------|
| `review` | One Crusty round (`autodrive-crusty-round`). Its verdict is evidence in the next reading. Nothing requires the reviewed head to equal the current head. |
| `wait` | Waits for an event: the checks at the head to complete (`gh pr checks <pr> --watch`, with `--required` when asked), or the owner's answer. |
| `merge` | Merges with an allowed merge method and `--match-head-commit`. Authority comes only from the captured authorisation. Before it merges, guidance must have been read again after the decision, and every [required outcome](#required-outcomes) is checked by name. |
| `continue` | Nothing mechanical; the next turn reads the live state again. The agent chooses it after it pushed, installed something or did a task step. |
| `wait-owner` | Waits for the owner's answer: `amplihack recipe guidance wait` with no command. args: `owner`, `question`. |
| `handoff` | Posts one PR comment naming the owner, the decision and the evidence. The run ends. args: `owner`, `decision`, `text`. |
| `ready` | Posts one PR comment that the PR is ready. The run ends. args: `text`, and `owner` when someone other than the operator decides. |
| `stop` | The owner asked the run to stop. Posts the continuation note. The run ends as `stopped`. args: `owner`, `text`. |

The agent ends its reply with one JSON object on its own line:
`{"action": "<word>", "args": {...}, "outcomes": [...], "why": "..."}`. The turn tool saves it as
`decision.json`; `args` is at most 64 KiB and `why` at most 16 KiB. `outcomes`
is optional on every action except `merge`; its shape is under
[Required outcomes](#required-outcomes), and a list of the wrong shape makes the
decision invalid. A reply
with no such object, or a word outside `^[a-z][a-z0-9-]{0,31}$`, runs no
mechanical action.

Work the agent does inside its own step, before it names the action:

- Fix what a review or CI found and push with plain `git push`, through the
  hooks.
- Bring the base in only when the PR conflicts, when the task text asks, or when
  the repository requires an up-to-date branch. Use a merge commit, resolve any
  conflict, and push. Never rebase, amend, squash or force-push. A branch that
  is only behind, with no strict checks, gets no base merge.
- Install a host input the repository needs and the run lacks, such as a
  `.venv` or a pinned tool, by the repository's own instructions. Prefer the
  worktree or the run's folder. Never use sudo or a system package manager and
  never touch `~/.claude` or `~/.amplihack`. An input that needs privileges is a
  hand-off to the host's owner.
- Carry out the task's later steps. A merging-off run does not end at the
  readiness assessment.

Setup instructions come from `origin/<base>`. If the PR changed a setup script,
a hook or a CI-tooling file, the agent follows the PR's version only on explicit
operator guidance. Otherwise it hands off.

### Hand-off

The owner comes from the task text or guidance. If neither names one, it is the
operator who started the run. A hand-off is for a decision that belongs to that
owner, or for a failure that keeps repeating when nothing the agent can do will
change it, such as GitHub being unreachable. It does not change the PR's draft
or ready state. The text is written in the journalistic style with literal
words. It never quotes guidance, task text or secrets.

### Merging off

Merging is off when the captured authorisation has `no_merge` or a dry run. Once
the PR is ready and the task's later steps are done, the agent takes `ready`.
The comment says three things: the PR is ready, merging is off for this run, and
who decides the merge.

With merging on, the run ends at merged. The report lists any task steps written
for after the merge.

## Guidance

Guidance is what an operator sends with
[`amplihack recipe guide`](./recipe-cli-reference.md#amplihack-recipe-guide).
The loop reads it:

- at the start of every turn, into the reading;
- by the agent, before any action it expects to take more than about five
  minutes;
- by the wait step, before it starts;
- at the start of the next turn after a wait.

Later guidance governs over earlier guidance and over the task text.

Guidance can narrow what the run does, for example "hold", "do not merge" or
"stop". It can never widen merge authority or relax a trust boundary. Merge
authority comes only from `no_merge` and the captured invocation.

Guidance that arrives between the turn's reading and the action makes the action
skip, with outcome `skipped-guidance`. The next turn decides again.

Guidance that cannot be read is unknown, never absent. A failed read at the
start of a turn puts `guidance_error` in the reading, so the agent judges with
that in view. A failed read after the decision tells the goal's action that
guidance is unknown (`AUTODRIVE_GUIDANCE_UNKNOWN=true`). The auto-drive `merge`
then refuses with `guidance-unknown`, and the next turn reads guidance again
and decides. The other actions still run, so a run whose guidance cannot be
read can still wait, review, hand off or stop.

### Required outcomes

An outcome that the task text or guidance requires is one of the run's
conditions (#3129, the guidance case of 7115122327). The agent names each one
in the decision's `outcomes` list:

```json
{"name": "<short name in the agent's own words>", "status": "met|unmet|rejected",
 "evidence": "<what meets it>", "reason": "<why it is rejected>"}
```

The list has at most 64 entries with unique names of at most 200 bytes on one
line. `evidence` and `reason` are at most 2000 bytes each, and a `rejected`
entry needs a `reason`.

- The ledger. After each turn's decision the driver runs
  `autodrive_goal_turn.sh outcomes`, which merges the list into the run's
  ledger, `<autodrive_state_dir>/outcomes.json`. Every name ever listed is
  kept, with its latest status, up to 256 names. A later turn cannot drop one
  silently. Each reading lists the ledger under `outcomes`.
- The answer. A `rejected` outcome is answered in one PR comment that names it
  and gives the agent's reason. It is posted once for each reason. A comment
  that fails the secret screen or cannot be posted leaves the outcome
  unanswered, and the next turn tries again.
- The check by name. The auto-drive `merge` refuses with `outcomes-unmet`,
  naming each outcome, unless the merge decision lists `outcomes` (an empty
  list when none is required) and every name in the ledger is in it as `met`,
  or as `rejected` with that same reason already answered. A ledger that
  exists and cannot be read refuses the merge with `outcomes-unreadable`.
- The end. A `ready`, `handoff` or `stop` comment lists every outcome in the
  ledger with its status.

The mechanical check makes sure that no named outcome is forgotten at the
merge. Whether an outcome is met stays the agent's judgement, with its
evidence on record.

To print the guidance a run has received:

```
amplihack recipe guidance show --pr 3129 --pr-repo rysweet/amplihack-rs
```

Entries that arrived after the previous reading are marked `NEW`. With `--json`
the command prints each entry's metadata and no text. Guidance text is never put
into JSON, context, argv or logs. See
[recipe guidance](./recipe-cli-reference.md#amplihack-recipe-guidance).

### Stopping gracefully

The owner can stop a run with `amplihack recipe stop`, or with guidance asking
it to stop. For guidance, the agent commits and pushes, then ends with a
hand-off. The hand-off text is the continuation note and names the owner who
asked.

## Waiting

Nothing times a wait out. The loop has no timer, no `sleep`, no `--interval` and
no poll of its own, and never sets `AMPLIHACK_STEP_TIMEOUT`.

- Checks completing. `gh pr checks <pr> --watch` blocks until the checks
  complete. A required check that is still running makes the run wait, and the
  run merges once it completes. If `gh` rejects `--required`, the reading lists
  every check with `required_known: false` and the wait watches without it.
- A login with room. The runner's usage-limit wait (#2992) handles this, and
  limit-resume (#3112) restarts the run. The loop adds nothing.
- The owner's answer. The waiting step arms a FIFO in the run's
  `guidance-wake/` folder. `amplihack recipe guide` writes one byte to every
  armed FIFO after it stores an entry. The waiter reads guidance again after
  arming, so an entry that arrived just before the wait is not missed. The same
  mechanism ends a check wait when new guidance arrives. If the FIFO cannot be
  armed (exit 3 from `guidance wait`), the question for the owner ends in a
  hand-off that names the owner and the question.

## How a run ends

After each turn the driver ends the run when:

- the fresh PR reading shows `MERGED` or `CLOSED`;
- the turn carried out a `handoff`, `ready` or `stop` action;
- step recovery recorded a hand-off during the turn (the report's exit 81);
- the owner stopped the run.

Trust-boundary refusals also end the run: exit 79 from any child, or the
runner's `structural_refusal` failure-class marker on a turn run's log
(`terminal_refusal` in `autodrive_refusal.sh`; nothing reads the transcript's
wording). Nothing reads the turn
record to decide, and nothing counts turns, rounds or time.

## Failures inside a turn

These never end the run. They go into the next turn's reading:

- a push rejected by the hooks;
- a merge refused by GitHub (its reason is returned as information);
- a Crusty round that fails;
- a `gh` or `git` error while reading;
- a wait that returns an error.

A turn run that fails does not end the run either (#3129 R1). The runner's own
handling runs first: the usage-limit wait, limit-resume and step recovery. If
the turn run still fails, the driver resets nothing. The agent may have edited,
committed or pushed before a later step failed, and all of that is kept. The
turn's `result.json` has outcome `turn-failed`, the `recovery` chosen and the
`failure`: the runner's class from its last `failure_class` marker, whether
it was retryable, whether the agent finished deciding, the completed steps and
the turn log's path. The next turn's reading carries them as `previous`, so a
fresh agent judges the failure. A decision the failed turn saved is not
carried out. The guidance mark does not move, so guidance the failed turn was
given shows as new again.

What runs before the next turn depends on the failure:

| Failure | Recovery |
|---------|----------|
| The agent finished deciding (`decide` completed, or `decision.json` was saved) and a later step failed | The next turn starts at once |
| A usage limit | The next turn starts at once. The runner's supervisor in its agent step moves to a login with room, or waits for the reset or for a login with room ([usage-limit waits](./usage-limit-waits.md)). No timer. |
| `transient_transport` | The next turn starts at once. The runner has already spent its own retry budget on the call. |
| Anything else before the agent decided: `environmental`, `work`, `agent_api`, `indeterminate`, or a usage limit the runner could not supervise (`usage_limit_wait skipped reason=not_supervised`) | The run waits for the owner's word, `amplihack recipe guidance wait`, an event with no timer. Guidance that arrived during the turn wakes it at once. Then the next turn starts. If no wake can be armed, the question goes to the operator in a hand-off, as `wait-owner` does. |
| A usage limit when the operator turned the runner's limit wait off (`recipe.usage_limit_wait = false`, logged `usage_limit_wait skipped reason=disabled`) | The run ends as `usage_limit`. That setting asks for the limit result at once, and the same command resumes the run. |
| Any failure of a `--once` turn | The driver exits 1. The caller asked for one turn. |

The wait in the fourth row is there because a fault that stopped the agent
before it could decide would meet the next turn at once, and a loop that
started turns with no agent able to judge would spin and fill the disk with
turn folders.

## Turn records

Each turn has a folder, `<autodrive_state_dir>/turns/NNN/`, mode 0700, holding
`reading.json` and `record.md`. The record has three parts:

1. a mechanical header: turn number, UTC time, head, merge state, a summary of
   the checks, and the guidance seen;
2. the agent's paragraph saying what it chose and why, in literal words;
3. the mechanical result line.

A sample:

```
Turn 004 · 2026-11-23T18:42:07Z
Head 9f3c1d2 · mergeStateStatus BLOCKED · checks: 11 passed, 1 running (required: scripts/check fast and full)
Guidance seen: none new

The required check is still running at the current head. The branch is
behind dev, but dev changed none of this PR's files and the repository does
not require an up-to-date branch, so I did not bring dev in. I chose to wait
for the check.

Result: wait returned after the check completed (passed).
```

The prompt for the next turn includes the previous record in full. The report
lists the records. No code parses `record.md`. A paragraph that matches the
secret screen is replaced by a line naming the pattern class.

## Reported outcomes

`report.rs` builds the report from the merge tool's result
(`merge-result.json`), the driver's `loop-end.json`, and a final fresh PR
reading. It makes no network call, never fails and never uses the turn records.
Each file must be a regular file, not a symlink, within the size cap, in the
bound state folder, with a matching run ID.

| `stop_reason` | Meaning |
|---------------|---------|
| `completed` | Merged at the expected head, or the `ready` end with merging off |
| `handoff` | A hand-off to a named owner, or `ready` with merging on |
| `closed` | The PR was closed |
| `stopped` | The owner stopped the run |
| `terminal_refusal` | A trust boundary refused |
| `failed` | A `--once` turn run failed after the runner's recovery |
| `usage_limit` | A usage limit, with the operator's limit wait turned off |
| `orphaned` | Unchanged |

Only `completed` counts as success. A merge counts only when `loop-end.json`
says `merged`, which only the driver writes, after its own reading of GitHub.
A PR merged by someone else counts as `completed`, and the report says the merge
was not by this run's merge action. The values `waiting_for_checks`,
`no_progress`, `budget_exhausted`, `unreadable` and the `base-behind` form of
`not_merged` no longer occur.

## Trust boundaries that stay

These are trust boundaries, not run-health gates, and they stay as strict as
they were:

- Merge authority comes only from the operator's inputs (`no_merge`) and the
  captured invocation: effective = allows merge, PR selected, not `no_merge`, not
  dry run. The run's own context can narrow it and never grant it: `no_merge`
  and `autodrive_dry_run` must also be exactly `false` there. An unreadable
  record gives `authorization-unreadable` and no merge; exit 79 from the
  authorisation helper passes up.
- The pinned launch bundle. Tools, recipes and goal text resolve only inside
  the bundle, as regular files and not symlinks. Edits in the worktree are
  ignored.
- `--match-head-commit` with an allowed merge method. The merge action requires
  the head the agent names to equal this turn's reading and to be 40 or 64
  lowercase hex characters. Required checks must be green.
- Never rebasing or force-pushing. Never `--admin` or any bypass.
- Never reading a token. The text the loop publishes passes a secret screen, and
  `@mentions` and remote images are neutralised. A match gives outcome
  `refused-secret`, nothing is published and the run carries on.
- Refusing a cross-repository PR.
- The pane-ownership proof in limit-resume.

## Removed behaviour

| Removed | Was in | Closes |
|---------|--------|--------|
| Automatic base merge | `autodrive_merge.sh`, merge loop, finalize, merge-evidence step-01 | #1924, #3094 (and fixes #2342) |
| End on pending checks (`checks-pending`) and the hand-back to draft | `autodrive_merge.sh`, `autodrive-handoff.yaml` | #3109 |
| Loop-health STUCK stop and no-progress stop | The auto-drive loop and Crusty loop | #3035 |
| Round and wall-clock budgets as stopping rules | `autodrive_budget.sh` | |
| Parsed-record readiness gates | Merge-evidence and merge-round records | |
| End of a merging-off run at its readiness assessment | Merge round | ui-platform #695, #705 |
| Eligibility escalation to auto-drive-to-merge | `auto-drive-light.yaml` | |

Hook repair, the issue-claim check and the build stage's other ends are in the
build recipes, which a later change covers. `loop-health-evaluator.yaml` stays,
because hook repair still uses it. The loop never calls hook repair or
`loop-health-evaluator`.

## Retired keys and aliases

The `autodrive_budget_*` keys are still declared and are accepted, and nothing
reads them. The run log has one plain note that they are retired. An unknown
`autodrive_budget_*` key no longer fails the run. `light_eligibility` is accepted
and ignored.

For one release (#3049), renamed or removed recipes and tools keep a working
alias. Runs in flight keep their pinned bundle.

| Alias | Now |
|-------|-----|
| `autodrive-merge-loop` recipe | Runs the goal loop |
| `autodrive-crusty-loop` recipe | Runs one review action |
| `autodrive-merge-round`, `autodrive-merge-evidence`, `autodrive-finalize` recipes | Each runs one turn (`--once`) |
| `autodrive_loop.sh`, `autodrive_merge_loop_run.sh` | Forward to the driver where the old call maps onto it |
| `autodrive_budget.sh`, `autodrive_progress.sh`, `autodrive_round_word.sh`, `autodrive_escalate.sh`, `autodrive_done_override.sh`, `autodrive_crusty_rereview.sh` | Print one line naming the replacement and exit 0 |

Every `autodrive_budget.sh` subcommand exits 0, so a call to `check` means
"carry on". No alias brings back a stop. Aliases are real files, because the
bundle copy skips symbolic links.

## Goal contract v1

The shared module holds no pull request fields. A later change supplies its own
goal, reading tool and actions without editing the shared files. The driver
takes a goal text, the task text, a goal tool, the state folder and the
worktree.

The goal tool is a script in the bundle's `tools/` folder. The environment
variable `AUTODRIVE_GOAL_CONTRACT=1` selects the contract; any other value makes
the tool exit 2.

| Call | Does |
|------|------|
| `actions` | Prints the action words it supports, separated by spaces |
| `final` | Prints one word if the goal has ended (for example `merged`), nothing otherwise. `refused` is reserved. Exit 79 passes through. |
| `read <turn_dir>` | Writes `goal.json` |
| `act <word> <turn_dir>` | Carries out the action and writes `result.json` |
| `publish <body_file>` | Posts the hand-off or ready text |

`goal.json` has schema 1 and a required `summary`. Three optional fields are
read by the driver:

| Field | Use |
|-------|-----|
| `subject` | A short name for the goal, copied into `loop-end.json` |
| `guidance_target` | A target `amplihack recipe guidance show|wait` accepts (an `https://` pull request URL). The driver reads that folder's guidance into the reading and gives the turn run the folder in `AMPLIHACK_RUN_GUIDANCE_DIRS`. |
| `worktree` | The absolute folder the agent works in, when it is not the run's checkout |

`act <word> <turn_dir>` reads the agent's `decision.json` and this turn's
`goal.json` and `reading.json` from the turn folder, and
`AUTODRIVE_GUIDANCE_UNKNOWN`, which is `true` when guidance could not be read
again after the decision. `result.json` has
`outcome` and `summary`. The shared words `continue`, `wait-owner`, `handoff`,
`ready` and `stop` are reserved. The driver rejects a goal tool whose action
words collide with them before turn 1, and exits 2.

The goal tool runs with `AUTODRIVE_GOAL_CONTRACT=1`, `AUTODRIVE_STATE_DIR`,
`AUTODRIVE_REPO`, `AUTODRIVE_WORKTREE`, `AUTODRIVE_LOOP_HOME`,
`AUTODRIVE_INVOCATION_FILE` and `AUTODRIVE_TASK_FILE`, and the run's own
`RECIPE_VAR_*` context. The turn run gets none of the `AUTODRIVE_*` variables.

The driver writes `loop-end.json` with `end`, `run_id`, `subject`, `detail`,
`turn` and `time`; a `merged` end adds `after_merge` and a `ready` end adds
`verify_readiness`. Only the driver writes it. The driver removes any
`loop-end.json` left by an earlier run when it starts, and any the agent leaves
in a turn.

The driver is started by the `autodrive-goal-loop` recipe:

```
autodrive_goal_loop.sh --goal-tool <name.sh> --goal-file <bundle-relative path>
  --state-dir <dir> --repo <dir> [--task-file <f>] [--notes-file <f>]
  [--invocation <invocation.json>] [--once]
```

`--once` runs one turn and returns; the one-release recipe aliases use it.
The driver's exit codes: 0 for an end (or one `--once` turn), 1 for an error, a
failed `--once` turn, or a usage limit the operator asked to receive at once, 2
for a usage error or a refused contract (no turn runs), 75 and 79 passed up
unchanged. It prints exactly one JSON line on
stdout.

The auto-drive goal tools are `autodrive_pr_goal.sh` (actions `review wait merge`)
and `autodrive_pr_reading.sh`. The goal text is
`amplifier-bundle/skills/auto-drive-to-merge/goal-merge-pr.md`. The outcomes of
its actions:

| Action | Outcomes |
|--------|----------|
| `review` | `reviewed`, `review-failed` |
| `wait` | `checks-complete`, `checks-pending`, `guidance-arrived`, `wait-error` |
| `merge` | `merged`, `not-authorised`, `authorization-unreadable`, `guidance-unknown`, `outcomes-unmet`, `outcomes-unreadable`, `checks-not-green`, `refused`, `not-open`, `message-unavailable`, `invalid-head`, `head-mismatch` |

The shared outcomes are `continued`, `no-action`, `skipped-guidance`,
`turn-failed`, `ended`, `invalid-args`, `refused-secret`, `guidance-arrived`
and `wait-error`.

## Files

Shared module:

- `autodrive_goal_loop.sh`, the driver
- `autodrive_goal_turn.sh`, the turn tool
- `autodrive_goal_lib.sh`, the validators, atomic writes and text screens
- `autodrive-goal-loop.yaml`, the entry recipe
- `autodrive-goal-turn.yaml`, the turn recipe

Auto-drive:

- `autodrive_pr_goal.sh`, `autodrive_pr_reading.sh`, `autodrive_merge.sh`
- `goal-merge-pr.md`
- `amplifier-bundle/skills/auto-drive-to-merge/SKILL.md` and its second copy,
  `docs/claude/skills/auto-drive-to-merge/SKILL.md`. The two are kept identical.
- `auto-drive-to-merge.yaml`, `auto-drive-light.yaml`, `autodrive-crusty-round.yaml`

Rust:

- `run_guidance/wake.rs` and `commands/recipe/guidance.rs`
- `commands/orch/autodrive/report.rs` and `trusted.rs`

## Trade-offs

- Nothing in code stops a loop whose agent keeps choosing to continue. Each
  turn costs one agent call.
- A pull request someone else merges ends the loop as `completed`; the report
  says the merge was not by this run's merge action.
- The merge action does not tell a pending required check from a failing one:
  both are `checks-not-green`. The goal text tells the agent to `wait` on
  pending checks before it chooses `merge`.

## Known limits

- The turn run gets no `AUTODRIVE_*` variables. The driver passes the goal
  contract only to the goal tool, so the agent step cannot read or steer the
  driver's own state through the environment.

- The agent step can run `gh pr merge` itself. The guard that blocked it (#1551)
  was removed in #2340. The launch stamp tells the agent only the loop's merge
  action merges, but that is a directive and not a mechanical block.
  `--match-head-commit` and GitHub's branch protection remain the real boundary.
  #3144 tracks restoring enforcement.
- There is no code stop. An agent that keeps choosing to continue, or that waits
  on a PR with no checks (where `gh pr checks --watch` returns at once), costs
  one agent call per turn. The agent is told to hand off when a failure keeps
  repeating. The operator can use `amplihack recipe stop`.
- `gh pr checks --watch` polls GitHub inside `gh`. The loop adds no poll of its
  own.
- An operator who passes `--step-timeout` times out the whole loop step.
- A pending required check is reported by the merge action as `checks-not-green`.
- Guidance newness is by entry name order. An entry stamped before the mark but
  published after it loses its `NEW` flag and its wake, but still reaches later
  agent prompts.
- Same-user processes can read and forge state files. The `completed` outcome is
  protected by the driver-only `loop-end.json`.

## Related

- [Auto Drive To Merge Reference](./auto-drive-to-merge.md)
- [Auto-Drive-Light Recipe](./auto-drive-light.md)
- [recipe command](./recipe-command.md#recipe-guidance)
- [recipe CLI reference](./recipe-cli-reference.md#amplihack-recipe-guidance)
- [Loop Health Evaluator](./loop-health-evaluator.md)
