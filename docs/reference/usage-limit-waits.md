# Usage-limit waits and limit records

A supervised recipe agent step that meets a usage, spend or credit limit does
not end because of it. It records the limit for the login it ran under, waits,
and runs the step again. No count, budget or clock ends that chain: not the
number of limits, not the time spent waiting, and not the step timeout. What
still ends a step is set out [below](#what-ends-a-step-and-what-does-not).
This page owns the tables for limit kinds, the limit record, which wait a
limit starts and the `waiting_for_capacity` line. The wait mechanics (reset
time, wake conditions, the log grammar) are in the
[usage-limit wait reference](./profile-command.md#usage-limit-wait), which
links here rather than repeating these tables, and task-oriented guidance is in
the [how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

## Contents

- [What ends a step and what does not](#what-ends-a-step-and-what-does-not)
    - [A step that is not supervised](#a-step-that-is-not-supervised)
    - [The step timeout does not cut a wait](#the-step-timeout-does-not-cut-a-wait)
    - [A prompt piped to the step](#a-prompt-piped-to-the-step)
    - [A prompt that could not be read to its end](#a-prompt-that-could-not-be-read-to-its-end)
- [Limit kinds](#limit-kinds)
- [The limit record](#the-limit-record)
- [Which wait a limit starts](#which-wait-a-limit-starts)
- [The waiting_for_capacity line](#the-waiting_for_capacity-line)
- [What max_wait_secs means](#what-max_wait_secs-means)
- [Codex credit exhaustion](#codex-credit-exhaustion)
- [Removed endings](#removed-endings)
- [Limits of this behaviour](#limits-of-this-behaviour)

## What ends a step and what does not

A limit never ends a supervised step. These end a step that has met one:

| Ending | Reason |
|--------|--------|
| A stop signal | Returns promptly, also during a wait. |
| The session ends | Nothing is left to continue. |
| `recipe.usage_limit_wait = false` | The operator turned the wait off, so the limit result is passed up at once (`skipped reason=disabled`). |
| `skipped reason=no_limit_message` | The output carried no message in a harness's limit shape, so it is not a limit. |
| `failed reason=prompt_cut_short` | The step's prompt came on a pipe and could not be read to its end, so its attempt was given part of its task. This is a lost prompt, not a limit ending: the step fails whether or not it meets a limit ([below](#a-prompt-that-could-not-be-read-to-its-end)). |

The last row is the one ending left that is not a stop, the end of the
session or the operator's choice. It used to be `skipped reason=piped_prompt`,
which also ended a step whose attempt had been given its whole prompt, when
the copy kept for a retry failed or could not be set up. Neither can happen
now: the copy is kept in memory, and a step whose copy cannot be set up is not
run at all ([below](#a-prompt-piped-to-the-step)).

There is no retry count and no wall-clock budget. A step that meets a second,
third or tenth limit waits for each, and nothing counts the waits or totals
their time. The step timeout plays no part in a limit wait
([below](#the-step-timeout-does-not-cut-a-wait)).

A step on a single login with no rotator, whose harness keeps reporting a
limit, therefore waits for one `max_wait_secs`, runs the agent again, and
repeats until the limit clears. Each retry runs the agent again, with its cost
and its side effects, and [`max_wait_secs`](#what-max_wait_secs-means) is how
often that happens. The
[waiting_for_capacity line](#the-waiting_for_capacity-line) at the start of
each wait is how an operator tells such a step from a hung one, a stop signal
ends it, and `recipe.usage_limit_wait = false` is how a run opts out of
waiting altogether.

### A step that is not supervised

Everything on this page is done by the supervisor, and a step it never
supervised does none of it. When the supervisor cannot start at all (Docker or
auto mode, no profile store, no private scratch directory, no stop-signal
handler, or a first attempt that never spawned), the step writes
`skipped reason=not_supervised` and runs once as a plain launch, as it did
before steps were supervised. That launch waits for nothing, so its result, a
limit included, is passed up as the harness gave it, and the recipe runner
classes it from the harness's own words. The conditions are listed in the
[profile reference](./profile-command.md#stdin-handling) under standing down.
#2911 does not change them.

A step whose prompt comes on a pipe and whose copy cannot be set up is
different: it is not run at all ([below](#a-prompt-piped-to-the-step)). It
meets no limit, so nothing on this page applies to it.

### The step timeout does not cut a wait

`AMPLIHACK_STEP_TIMEOUT` (`--step-timeout`, `recipe.step_timeout`) does not end
a limit wait, and does not stop the retry after one. A wait ends on its own
terms: its reset, its length, a login with room, a login change, a stop
signal or the end of the session.

It used to. A wait's deadline was pulled back to the step timeout less 60
seconds, the `start` line read `bound=step_timeout`, and the step then failed
with `failed reason=step_timeout: usage limit not lifted` while the limit still
stood. The reason given was that the recipe runner kills a step at that
timeout, so a wait that ran past it would be killed mid-sleep. That is not so:
`recipe-runner-rs` enforces only a step's own YAML `timeout:` field and reads
`AMPLIHACK_STEP_TIMEOUT` for nothing else, and nothing in amplihack enforces
it either (the `--step-timeout` help says as much). The wait was therefore
the only code anywhere that turned the variable into an ending, and the only
steps it ended were steps waiting on a limit: a wall-clock budget for limits,
which [#2911](https://github.com/rysweet/amplihack-rs/issues/2911) removes
([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)).

A recipe step's own YAML `timeout:` is enforced by the runner, which kills the
step when it passes, whether or not the step is waiting on a limit. That is a
bound the recipe's author set on that one step, not a bound on limits, and
none of the bundled recipes sets one on an agent step.

### A prompt piped to the step

A recipe agent step is given its prompt on a pipe. The supervisor keeps a copy
of it in memory as it arrives, so every retry reads the same bytes from the
first one. Nothing is written to disk. The cases that used to end a step on a
limit with `skipped reason=piped_prompt` no longer do
([#3036](https://github.com/rysweet/amplihack-rs/issues/3036), and the review
of [#2992](https://github.com/rysweet/amplihack-rs/pull/2992)):

| Case | What happens now |
|------|------------------|
| stdin is an `--input-format` stream channel | It is copied and replayed like any piped prompt. Every supervised attempt's standard output is held back until the step finishes, and the output of an attempt that is run again is discarded, so the caller has seen nothing of an attempt that met a limit; a retry given the same input from its first byte is the same step from the caller's side. |
| the disk will not take the copy | No disk is involved. The copy is kept in memory from its first byte, so there is no store to refuse it. |
| the copy fails part way: a panic, a final flush, or a part on disk that cannot be read back | Every byte the copier reads is kept in memory before it is passed to the attempt, so whatever the attempt was given, a retry is given too. There is no flush and no part on disk. A copier that panics closes the attempt's pipe where it stopped, so the attempt may have been given part of its task: that is a prompt cut short ([below](#a-prompt-that-could-not-be-read-to-its-end)), not a lost copy. |
| the copy cannot be set up, which happens only to a process out of file descriptors or threads | The step is not run. Nothing is read from the pipe, and the step fails at once with `this step was not run: its prompt comes on a pipe, and the copy of it that a retry would read could not be set up (…)`. No attempt runs that a retry could not repeat, so no limit can end it. The line carries no limit marker, because no limit was met. |
| the prompt is still arriving when the retry is due | The retry starts at once. It is fed the copy already held, from its first byte, and then each further byte as the caller writes it, and it reaches the end of its input when the caller closes the pipe, as the first attempt did. Nothing waits on the caller's pipe: a caller that holds it open can hold up only an attempt that reads it, and an attempt that does not read its input runs and ends. An auto-switch rerun under another profile starts the same way. |

```
amplihack.recipe.usage_limit_wait end reason=reset reached
amplihack.recipe.usage_limit_wait resume attempt=2
```

Before the review of [#2992](https://github.com/rysweet/amplihack-rs/pull/2992)
the retry waited, in the supervisor, for the caller to close the pipe, and
wrote `waiting_for_prompt` while it did. A caller that never closed it held
the step for ever; the 2815 Gadugi scenario, which holds the pipe open for
the whole step, hung at its ceiling on that. The line is no longer written.

### A prompt that could not be read to its end

`failed reason=prompt_cut_short` is the one ending left that a step which has
met a limit can reach, and it is a prompt-loss failure, not a limit ending.
It is reached only when the prompt itself could not be read to its end:

- a read of the step's standard input failed part way through, or
- the copier panicked part way through it, which closes the attempt's pipe
  wherever the copier stopped.

Either way the attempt reaches the end of its input having been given only
part of its task, and it cannot tell that from a whole prompt. The supervisor
names the failure on standard error as it happens and refuses to report that
attempt's success as the step's, so such a step fails whether or not it meets
a limit. No retry can be given more than the attempt was, so no wait can earn
one. The limit is not why the step ends; it is only why a retry would have
been needed.

A step whose prompt was read whole never reaches this ending, however many
limits it meets: each one is waited out and every retry reads the same bytes.
`issue_2911_prompt_loss_ending` pins both sides, along with the step whose copy
cannot be set up, which is not run.

This is not a case a capacity wait could help. The step's copy of its
prompt was not lost; the prompt itself was, before it reached the copy, so a
retry after any wait would be given the same part of the task. The cases in
which a step used to end on a limit with no copy left (a copy that failed,
or one that could not be set up) are gone, as the table above shows.

When a limit also stands, the run learns of it as data, not from words. The
supervisor reports every limit an attempt meets to the recipe runner over
its recovery channel, as `Outcome::UsageLimit` for that step
([#2334](https://github.com/rysweet/amplihack-rs/pull/2334)), before
anything about a wait or this ending is decided. When the step fails, the
runner writes that into the run's result as `failure_cause`
(`{"class":"usage_limit","origin":{...}}`); `amplihack recipe run` classes
the run `usage_limit` from that field ("temporary and NOT a work failure")
and records the run as resumable. A relaunch resumes at the step, and the
runner, which holds the whole prompt, sends it again; if the limit still
stands, that step waits for capacity like any other. The agentic layer that
decides on a relaunch reads the classification as JSON.

Beside the event the step also writes a sentence for the person reading the
log: that the step failed because its prompt was lost, not because of its
usage limit, and that the limit is reported to the runner as the step's
recorded cause. The classification does not depend on it:
`limit_wait::tests_ending_evidence` classifies the run with and without the
sentence and gets `usage_limit` both times, and gets `indeterminate` from
the step's own lines when no cause was recorded. The sentence is followed by
one of three lines saying when the limit lifts, which the recorded cause
does not carry:

| What the step knows | What it writes | Reset hint |
|---------------------|----------------|------------|
| a reset still ahead of the clock | `amplihack: the limit resets <time>` | the time |
| a reset already behind it | `amplihack: the reset this limit stated, <time>, has already passed, so it does not say when to start this step again; a reading of the account does.` | none |
| no reset at all | `amplihack: no reset is known for this limit, so nothing here can say when it lifts; a reading of the account is what says so.` | none |

The first form is the one the runner's reset reader takes a time from, so the
verdict reads "Wait for the reset and resume" with the time in it. A past
reset is deliberately not written in that form, because a supervising loop
would schedule a resume for a moment already gone. This is pinned by
`limit_wait::tests_ending_evidence`.

A step whose prompt is cut short and that meets no limit writes no wait line.
It fails with `amplihack: this step's prompt was cut short on its way to the
attempt, so the attempt was not given the whole task and its result is not
this step's result`.

## Limit kinds

The supervisor reads the kind of limit from the harness's own words:

| Kind | Words that select it |
|------|----------------------|
| `session` | `5-hour limit`, `session limit`, and any limit wording it does not recognise |
| `weekly` | `weekly limit` |
| `spend` | the organisation spend-limit phrase |
| `credits` | `Your workspace is out of credits` (Codex) |

Unrecognised wording is read as `session`, the shortest window, so a malformed
or forged message holds a login out of rotation for at most five hours (see
[the limit record](#the-limit-record)).

## The limit record

Every limit the supervisor sees is written to the rotator's ledger, for the
login the attempt ran under, before the wait is planned. One function does
this for every kind, `note_limit_block` in
`crates/amplihack-cli/src/commands/profile/rotate/step_block.rs`. It returns
nothing: it reports a failure on standard error and in the rotate log, and no
caller branches on the outcome, so the record can never delay, refuse or end a
step.

The record is an entry in the ledger's `overflow_blocked` object, keyed by
login name:

```json
"overflow_blocked": {
  "work": { "at": 1792826100, "until": 1792843200, "reason": "spend_limit" }
}
```

| Field | Meaning |
|-------|---------|
| `at` | Epoch second the limit was met. |
| `until` | Epoch second the reset stated in the message falls on, moved onto the real clock, and held to the kind's window (rule 3 below). |
| `reason` | Always `spend_limit`, the one value every version of the rotator accepts. |

The record has this one shape for every kind of limit, because the rotator
refuses the whole ledger when an entry has an unexpected shape. The kind of
limit therefore does not travel in the ledger: it appears only in the ALERT
line below and on standard error. The ledger-validity rules in the
[rotate state directory](./profile-command.md#rotate-state-directory) are
unchanged, and a ledger written by this change loads in an older binary.

Rules the writer keeps:

1. The ledger must already exist. The writer never creates it, so a host with
   no rotator gains no file.
2. The stated span must satisfy the ledger's span rule (`until` after `at`,
   at most 62 days). A reset outside it is reported as
   `stated reset out of range` and writes nothing.
3. A `session` record ends at most five hours after `at`, and a `weekly`
   record at most seven days after it, whatever reset the message states. A
   genuine reset always lies inside its window, so a genuine message is
   recorded as stated. The step cannot tell the harness's own message from a
   model that printed one as the last line of its answer, and this rule
   limits how long a forged one can keep a login out of rotation. `spend` and
   `credits` records keep the stated reset, within rule 2.
4. It waits at most 10 seconds for the ledger's lock, for every kind of
   limit, then reports `rotator busy` and writes nothing. There is no retry.
5. An active block stands. It is not extended and raises no second alert.
6. At most 256 blocks are kept; expired ones are pruned first.
7. The write is atomic and the ledger keeps its owner-only permissions.

When the message states no reset, a `spend` limit is recorded until the
rotator's own spend reset (00:00 UTC on the 1st of the next month). A
`session`, `weekly` or `credits` limit with no stated reset writes no record,
because an invented end would keep a healthy login out of rotation. That case
is one line on standard error and in the rotate log with the fixed reason
`no stated reset`.

The rotate log line for a written record reads as follows; standard error
carries the same line without the leading time:

```
2026-11-21T18:05:00Z ALERT: login work hit its weekly limit; blocked until 2026-11-23T00:00:00Z
```

The words after `hit its` are `session limit`, `weekly limit`, `spend limit` or
`credit limit`. A spend limit keeps the 1.2.193 wording byte for byte.

## Which wait a limit starts

| Limit | Wait | Wakes on |
|-------|------|----------|
| Reset within five hours | reset wait | the reset, or a login with room |
| Reset more than five hours away | capacity wait | a login with room, or the failed login's own reset plus 120 s |
| Reset not readable | capacity wait | a login with room |
| Reset already behind the clock | capacity wait | a login with room |
| Spend limit stating no reset | login-change wait | the login in `~/.claude` changing |
| Credits (Codex) reaching the Claude supervisor | capacity wait | a login with room |

A capacity wait starts whether or not another login has room now. A login can
gain room during the wait, and the wait sees it. A login-change wait does not
need a running rotator: with none running it runs its length and retries. A
login-change wait is never ended by a capacity reading, because a reading
reports session and weekly windows and never organisation credit.

One reading in a supervised step's output is not in the table, because it is
not a limit: a bare `429` rate-limit envelope. It states no reset and says
nothing about an account's window, so there is nothing about it to wait out.
It is the only place where "could another login take over?" is still asked
before a wait begins: with such a login it becomes a capacity wait with
`reset=none`, and without one the step writes no wait line and hands the `429`
back to the recipe runner as the transient transport fault the classifier
calls it everywhere else.

No wait is planned with a deadline of now. A reset already behind the clock
becomes a capacity wait, so the retry does not meet the same limit in a loop.
The chain is paced by waiting, not by counting.

## The waiting_for_capacity line

When a wait begins with no known reset, the supervisor says so on standard
error and in the recipe log (`AMPLIHACK_RECIPE_LOG`) immediately before the
wait's `start` line:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_too_far harness=claude until=2026-11-21 22:40:00 +00:00
```

In the recipe log the line carries `account=<login>` in place of `harness=`:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=no_reset account=work until=2026-11-21 22:40:00 +00:00
```

| `reason=` | Why the wait has no reset to end on |
|-----------|-------------------------------------|
| `reset_too_far` | The stated reset is more than five hours away. |
| `no_reset` | The message states no readable reset. |
| `spend_limit` | A spend limit stating no reset. |
| `credits` | A credit-exhaustion message. |
| `passed_reset` | The stated reset is already behind the clock. |

`until` is the end of this one wait; the step then retries and, if it meets
the limit again, waits again. No text from the harness's output appears in the
line. It is built from static text, the closed reason set above, a
re-validated login name (`unknown` when the name is invalid) and a formatted
instant.

To see which runs are waiting for capacity, search the recipe log:

```sh
grep 'usage_limit_wait waiting_for_capacity' "$AMPLIHACK_RECIPE_LOG"
```

## What max_wait_secs means

`recipe.usage_limit_max_wait_secs` (`AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS`,
default `21600`) is the length of one capacity or login-change wait. It is not
a per-step budget, and a step's earlier waits do not reduce it. A reset wait
still ends at its reset, and a capacity wait still ends sooner on room or at
the failed login's own reset plus 120 s.

Because it is the length of one wait, and a step still limited when a wait
ends runs its agent again, it is also **how often a waiting step re-asks the
harness**, and each ask is a fresh agent launch. A wait is normally ended by
a signal (a login with room, a rotator cycle, the login in `~/.claude`
changing, a reset arriving); the length only decides how long the step waits
when no signal comes. At the default a step asks once every six hours. At 60
seconds it asks every minute, which is a poll of the harness and costs an
agent start each time.

The wait polls every 30 seconds, so any length below one poll interval would
give a wait that nothing could end. The length therefore has a floor of 60
seconds, and a configured `0` means 60 seconds. A host that set `0` to disable
waiting must set `recipe.usage_limit_wait` to `false` instead. See
[recipe.usage_limit_wait](./user-config.md#recipeusage_limit_wait).

Nothing refuses a short length. A step may need to re-ask quickly, if an
admin is raising a spend cap. But while the wait is on, a step that reads a
configured length below 600 seconds, the rotator's own cycle
(`AMPLIHACK_ROTATE_INTERVAL`) and so the soonest a host with no signal can
have learnt anything, says so on standard error once per step:

```
warning: recipe.usage_limit_max_wait_secs (AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS) is below 600 seconds, the rotator's own cycle. It is the length of one wait, and a step still limited when a wait ends runs its agent again, so this is how often such a step re-asks; the default of 21600 seconds asks once every six hours. Set recipe.usage_limit_wait = false (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting
```

A length below the 60-second floor says that instead, in the same line:

```
warning: recipe.usage_limit_max_wait_secs (AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS) is below 60 seconds, so each usage-limit wait lasts 60 seconds; 0 no longer turns the wait off. It is the length of one wait, and a step still limited when a wait ends runs its agent again, so this is how often such a step re-asks; the default of 21600 seconds asks once every six hours. Set recipe.usage_limit_wait = false (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting
```

## Codex credit exhaustion

`Your workspace is out of credits`, Codex's own words, is classed
`usage_limit`, not `indeterminate`. The recipe runner's failure class marker
therefore reads `"class":"usage_limit"`, with
`"signal":"your workspace is out of credits"`. `credit balance is too low`,
the Anthropic API message, still classes `environmental`.

Two readers match the phrase, by different rules:

| Reader | Rule |
|--------|------|
| The recipe runner's failure class | The phrase anywhere in a failed step's evidence, in any case, as for every other limit marker. A failed step whose output quotes the phrase inside prose is also classed `usage_limit`. |
| The usage-limit wait | A whole-line match: the phrase must be the whole line, or begin it and be followed by a delimiter, on a whole standard-error line or the last non-empty standard-output line. The phrase inside prose does not start a wait. |

Where such output reaches the Claude supervisor (a step that mixes
harnesses), it starts a capacity wait as in the table above. A Codex step
launched directly with `amplihack codex` is not supervised, so its credit
exhaustion is classed correctly but does not wait. That gap is tracked as a
follow-up issue.

## Removed endings

These paths no longer end a step on a limit. The wait and retry described
above replace them, and the closing-line reasons are no longer written.
`skipped reason=piped_prompt` is not written at all: each case it covered is
listed on its own row.

| Was | Now |
|-----|-----|
| `skipped reason=reset_too_far` when no other login has room now | capacity wait |
| `skipped reason=no_reset` | capacity wait |
| `skipped reason=spend_limit` (no stated reset) | login-change wait |
| `skipped reason=already_waited` | not written; the step waits again |
| `failed reason=already_waited` | not written |
| `failed reason=capacity_expired` | the step retries |
| `failed reason=spend_limit` after a login-change wait reaches its deadline | the step retries |
| the `max_wait_secs` per-step budget ending the chain | not a budget |
| `failed reason=step_timeout` and `bound=step_timeout` | not written; the step timeout does not cut a wait ([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)) |
| `skipped reason=piped_prompt` for an `--input-format` channel, a copy the disk refused, or a prompt still arriving | the copy is replayed, kept in memory, or fed to the retry as it arrives ([#3036](https://github.com/rysweet/amplihack-rs/issues/3036), review of [#2992](https://github.com/rysweet/amplihack-rs/pull/2992)) |
| `skipped reason=piped_prompt` for a copy that failed (a panic, a flush, a part on disk that could not be read back) | not possible: the copy is kept in memory, and a copier that panics is a prompt cut short |
| `skipped reason=piped_prompt` for a copy that could not be set up | the step is not run, so no limit is met |
| `skipped reason=piped_prompt` for a prompt cut short | `failed reason=prompt_cut_short`: the step still ends, as a lost prompt that fails whether or not it met a limit, not as a skipped wait |

An operator who greps for `reset_too_far`, `no_reset` or `spend_limit` still
finds them as the `reason=` of a `waiting_for_capacity` line.

## Limits of this behaviour

- A Codex step launched directly is not supervised and does not wait
  ([#2949](https://github.com/rysweet/amplihack-rs/issues/2949)).
- A forged `session` or `weekly` line can still keep a login out of rotation
  for up to five hours or seven days, and a forged `spend` line until its
  stated reset, within 62 days
  ([#3005](https://github.com/rysweet/amplihack-rs/issues/3005)).
- No line reports how long a step has waited in total
  ([#3003](https://github.com/rysweet/amplihack-rs/issues/3003)).
- A recipe step's own YAML `timeout:` is enforced by the runner and kills a
  step that is waiting, as it kills any step. None of the bundled recipes
  sets one on an agent step.
- A piped prompt is kept in memory for the life of the step, and the copy is
  not capped. A cap would be a budget that ends a step on a limit. An
  `--input-format` channel its caller keeps writing to grows the copy for as
  long as it writes.
- A step the supervisor cannot start runs once unsupervised and does not wait
  ([above](#a-step-that-is-not-supervised)).
- Output carrying both an authentication failure and the credits phrase classes
  `environmental` first, and does not wait. That fails closed.
- A pinned step with a spend limit that no login change lifts waits in cycles
  of one `max_wait_secs`, six hours by default, until the limit lifts or the
  run is stopped. It cannot be lifted by a capacity
  reading. The `waiting_for_capacity` line with `reason=spend_limit` is how an
  operator sees it, and `recipe.usage_limit_wait = false` is how a run opts out
  of waiting.
- The rotator's reading of the records (waking on a record, waking at the
  earliest `until`) is described with the rotator. This page covers only what
  the run writes.
