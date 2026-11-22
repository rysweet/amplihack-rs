# Usage-limit waits and limit records

A recipe agent step that meets a usage, spend or credit limit does not end
because of it. It records the limit for the login it ran under, waits, and
runs the step again. Two bounds that are not the limit can still end a step
that has met one - the step's own timeout, and a prompt no retry can be
given - and neither is a limit being refused; both are set out
[below](#what-ends-a-step-and-what-does-not), with the open question each
carries and what the step hands the run in its place. This page owns the
tables for limit kinds, the limit record, which wait a limit starts and the
`waiting_for_capacity` line. The
wait mechanics (reset time, wake conditions, the log grammar) are in the
[usage-limit wait reference](./profile-command.md#usage-limit-wait), which
links here rather than repeating these tables, and task-oriented guidance is in
the [how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

## Contents

- [What ends a step and what does not](#what-ends-a-step-and-what-does-not)
    - [Both endings resume, and say when](#both-endings-resume-and-say-when)
    - [The step timeout cuts a wait](#the-step-timeout-cuts-a-wait)
    - [A prompt that cannot be replayed](#a-prompt-that-cannot-be-replayed)
- [Limit kinds](#limit-kinds)
- [The limit record](#the-limit-record)
- [Which wait a limit starts](#which-wait-a-limit-starts)
- [The waiting_for_capacity line](#the-waiting_for_capacity-line)
- [What max_wait_secs means](#what-max_wait_secs-means)
- [Codex credit exhaustion](#codex-credit-exhaustion)
- [Removed endings](#removed-endings)
- [Limits of this behaviour](#limits-of-this-behaviour)

## What ends a step and what does not

A limit never ends a step by itself. These do, and the first and the last of
them can end a step that has met one, for reasons that are not the limit:

| Ending | Reason |
|--------|--------|
| The step's own timeout (`AMPLIHACK_STEP_TIMEOUT`) | A wait or a retry may not outlive its step, so a timeout cuts a wait short ([below](#the-step-timeout-cuts-a-wait)). |
| A stop signal | Returns promptly, also during a wait. |
| The session ends | Nothing is left to continue. |
| `skipped reason=no_limit_message` | The output carried no message in a harness's limit shape, so it is not a limit. |
| `skipped reason=piped_prompt` | The prompt came from a pipe and cannot be replayed, so a retry is impossible ([below](#a-prompt-that-cannot-be-replayed)). |

There is no retry count and no wall-clock budget *for limits*. A step that
meets a second, third or tenth limit waits for each, and nothing counts the
waits or totals their time. The step timeout is a wall-clock bound, but it
is the recipe runner's bound on the step as a whole and applies to every
step whether or not a limit is in play; what it costs a limit wait is the
[next section](#the-step-timeout-cuts-a-wait).

Without a step timeout, nothing bounds those waits. A step on a single login
with no rotator, whose harness keeps reporting a limit, waits for one
`max_wait_secs`, runs the agent again, and repeats until the limit clears.
Each retry runs the agent again, with its cost and its side effects.
Unattended runs should therefore set a step timeout, with `--step-timeout`,
[`recipe.step_timeout`](./user-config.md#recipestep_timeout) or
[`AMPLIHACK_STEP_TIMEOUT`](./environment-variables.md#amplihack_step_timeout).
The [waiting_for_capacity line](#the-waiting_for_capacity-line) at the start
of each wait is how an operator tells such a step from a hung one.

### Both endings resume, and say when

Neither ending is a limit being refused, and neither is work that failed.
Each writes its own sentence, and between that sentence and the event line
the step hands the recipe runner three things:

| What | How |
|------|-----|
| the failure class | the sentence contains the words *usage limit*, so the classifier answers `usage_limit`: "temporary and NOT a work failure" |
| when the limit lifts | a line of its own, `amplihack: the limit resets <time>`, in the one form the runner's reset reader takes a time from, so the verdict reads "Wait for the reset and resume" with the time in it |
| the attempt's own result | the exit code and output of the attempt that met the limit, unchanged |

The step gives one of three answers there, never a guess:

| What the step knows | What it writes | Reset hint |
|---------------------|----------------|------------|
| a reset still ahead of the clock | `amplihack: the limit resets <time>` | the time |
| a reset already behind it, because the wait ran to it or the message was stale | `amplihack: the reset this limit stated, <time>, has already passed, so it does not say when to start this step again; a reading of the account does.` | none |
| no reset at all: a capacity or login-change wait, a message that stated none, or an inherited login that never ran | `amplihack: no reset is known for this limit, so nothing here can say when it lifts; a reading of the account is what says so.` | none |

A past reset is deliberately not written where the reset reader would find
it. "Wait for the reset and resume", quoting a time that has been and gone,
would have a supervising loop schedule a resume for a moment already passed.
The step does not claim the limit has lifted either - it never re-ran - so
the sentence says only that the stated reset no longer answers the question.

The event lines alone (`skipped reason=piped_prompt`,
`end reason=step timeout`) carry no limit marker, so the sentence beside each
is what keeps the ending out of `indeterminate` - "nothing in the evidence
identifies the failure" - which a supervising loop reads as terminal. That is
pinned by `limit_wait::tests_ending_evidence`, which classifies each ending
exactly as the runner would, so a reword that dropped the words cannot pass
review silently.

### The step timeout cuts a wait

Taking that advice has a price, and it is the one thing on this page that
does end a step while a limit is still in force. A step timeout bounds
everything the step does, waiting included. A wait's deadline is pulled back
to the timeout less 60 seconds, the `start` line then reads
`bound=step_timeout`, and when that deadline arrives the step ends:

```
amplihack.recipe.usage_limit_wait end reason=step timeout
amplihack: this step's own timeout ($AMPLIHACK_STEP_TIMEOUT) ended a usage-limit wait before the limit lifted, so the step ends here with the limit's result. The timeout is the step's bound and not a budget for limits: raise it, or leave it unset, to let a limit wait run longer, or start this step again once the limit has lifted. See docs/reference/usage-limit-waits.md
amplihack: the limit resets 2026-10-18 08:30:00 -07:00
amplihack.recipe.usage_limit_wait failed reason=step_timeout: usage limit not lifted
```

The 60 seconds are not a margin of taste. The recipe runner kills the step
at its timeout, so a wait that ran past it would be killed mid-sleep, with
no line saying why and no attempt result passed up. Ending just before it is
what lets the step report at all. The bound is therefore the step's own,
applied by the layer that owns it, and not a budget this supervisor keeps
for limits: there is no limit counter, no cumulative wait total and no
per-step wait budget anywhere.

What follows is a choice, not a default. A step timeout caps how long a
limit can hold a step; leaving it unset lets a limit wait run as long as the
limit does. Which of the two an unattended run wants is
[#3037](https://github.com/rysweet/amplihack-rs/issues/3037); until that is
settled, a run that must not fail on a limit leaves the step timeout unset,
and a run that must not sit for hours sets one and accepts
`failed reason=step_timeout`.

Setting one does not mean losing the run. Raising the number is the
operator's remedy, and it is the weaker of the two: the step's own is the
reset line above, which is what lets the layer that still holds the work
start the step again at the time the limit clears rather than guessing a
larger timeout. A step timeout shorter than a limit wait therefore ends the
*step*, and ends it with everything the run needs to resume it.

### A prompt that cannot be replayed

`skipped reason=piped_prompt` is the other ending, and it is not a refused
wait: it is a step with no prompt for a retry to read, so there is no retry
for a wait to earn. Three shapes reach it, all narrow since the supervisor
began copying piped prompts aside:

| Shape | Why no copy exists |
|-------|--------------------|
| stdin is an `--input-format` stream channel | Its caller holds it open for the session, so it has no end to copy, and replaying a session's history to a retry would not be replaying a prompt. |
| the prompt came on a pipe and no copy could be made | The copy is what a later attempt would read. |
| a copy was made and was lost, or the prompt was still arriving when the wait ended | Asked at the moment a retry would be announced, so a prompt that arrives during a wait of hours is still replayed. |

A recipe agent step is given its prompt on a plain pipe, which is copied, so
none of these is the ordinary case. When one does happen the step says so in
words beside the event, naming which shape it is and that the step has to be
sent again:

```
amplihack: this step met a usage limit and cannot be run again: its prompt arrives on an --input-format stream channel, which its caller holds open for the session, so there is no end of it to copy and no prompt to replay. The limit is not lifted, and no wait can lift it for this step, because a retry would have no prompt to read; the attempt's own result is this step's result, and this step has to be sent again. See docs/reference/usage-limit-waits.md
amplihack: the limit resets 2026-10-18 08:30:00 -07:00
amplihack.recipe.usage_limit_wait skipped reason=piped_prompt
```

The step cannot send itself the prompt again. The caller that holds it can,
and the reset line is the one thing it still needs after being told to: a
caller that re-sends at once walks straight back into the same limit. The
attempt's own output is printed too, limit message and all, so the classifier
has the harness's words as well when they are still in the evidence tail.

Whether such a step should wait first anyway is
[#3036](https://github.com/rysweet/amplihack-rs/issues/3036). Waiting would
change nothing about this step - there is still no prompt for a retry to read
when the wait ends - so what that issue decides is whether the supervisor
should buffer a stream channel so that a retry becomes possible at all. Until
it does, this ending stands, and it is listed under
[limits of this behaviour](#limits-of-this-behaviour) rather than counted as
a terminal path removed.

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

These paths no longer end a step. The wait and retry described above replace
them, and the closing-line reasons are no longer written.

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

An operator who greps for `reset_too_far`, `no_reset` or `spend_limit` still
finds them as the `reason=` of a `waiting_for_capacity` line.

`failed reason=step_timeout` and `skipped reason=piped_prompt` are **not** in
that table. Both still end a step that met a limit, and neither is claimed as
removed. What changed for them is what the step says on the way out: which
bound ended it, that the bound is not the limit, and
[when the limit lifts](#both-endings-resume-and-say-when). The open questions
are [#3037](https://github.com/rysweet/amplihack-rs/issues/3037) and
[#3036](https://github.com/rysweet/amplihack-rs/issues/3036).

## Limits of this behaviour

- A Codex step launched directly is not supervised and does not wait
  ([#2949](https://github.com/rysweet/amplihack-rs/issues/2949)).
- A forged `session` or `weekly` line can still keep a login out of rotation
  for up to five hours or seven days, and a forged `spend` line until its
  stated reset, within 62 days
  ([#3005](https://github.com/rysweet/amplihack-rs/issues/3005)).
- No line reports how long a step has waited in total
  ([#3003](https://github.com/rysweet/amplihack-rs/issues/3003)).
- A step timeout cuts a limit wait and the step then fails, so a step timeout
  set shorter than a limit wait is a wall-clock bound that ends a step while
  the limit still stands. The run is handed the reset and may resume; the
  step does not
  ([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)).
- A step whose prompt cannot be replayed ends on a limit without waiting. The
  stream-channel shape cannot gain a copy however long it waits, so the
  question is whether to buffer such a channel at all
  ([#3036](https://github.com/rysweet/amplihack-rs/issues/3036)).
- Output carrying both an authentication failure and the credits phrase classes
  `environmental` first, and does not wait. That fails closed.
- A pinned step with a spend limit that no login change lifts waits in cycles
  of one `max_wait_secs`, six hours by default, until the limit lifts, the step
  timeout applies or the run is stopped. It cannot be lifted by a capacity
  reading. The `waiting_for_capacity` line with `reason=spend_limit` is how an
  operator sees it, and `recipe.usage_limit_wait = false` is how a run opts out
  of waiting.
- The rotator's reading of the records (waking on a record, waking at the
  earliest `until`) is described with the rotator. This page covers only what
  the run writes.
