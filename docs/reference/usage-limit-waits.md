# Usage-limit waits and limit records

A supervised recipe agent step that meets a usage, spend or credit limit does
not end because of it. It records the limit for the login it ran under, waits,
and runs the step again. Nothing ends that chain: not the number of limits,
not the time spent waiting, and not the step timeout.

A wait ends on a limit event: its reset passing, a login with room, or the
login changing. It blocks on writes to the files that record those events and
reads them only after a write ([How a wait wakes](#how-a-wait-wakes)). A limit
that states a reset still ahead is waited for to that reset, however far away.
One timer is left, for limits that state no usable reset
([The timed parts that remain](#the-timed-parts-that-remain)).

This page owns the tables for limit kinds, the limit record, which wait a
limit starts and the `waiting_for_capacity` line. The log grammar is in the
[usage-limit wait reference](./profile-command.md#usage-limit-wait), and
task-oriented guidance is in the
[how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

## Contents

- [What ends a step and what does not](#what-ends-a-step-and-what-does-not)
    - [A step that is not supervised](#a-step-that-is-not-supervised)
    - [The step timeout does not cut a wait](#the-step-timeout-does-not-cut-a-wait)
    - [A prompt piped to the step](#a-prompt-piped-to-the-step)
    - [A prompt that could not be read to its end](#a-prompt-that-could-not-be-read-to-its-end)
- [Limit kinds](#limit-kinds)
- [The limit record](#the-limit-record)
- [Which wait a limit starts](#which-wait-a-limit-starts)
- [How a wait wakes](#how-a-wait-wakes)
- [The waiting_for_capacity line](#the-waiting_for_capacity-line)
- [The timed parts that remain](#the-timed-parts-that-remain)
- [What max_wait_secs means](#what-max_wait_secs-means)
- [Codex credit exhaustion](#codex-credit-exhaustion)
- [Removed endings](#removed-endings)
- [Limits of this behaviour](#limits-of-this-behaviour)

## What ends a step and what does not

A limit never ends a supervised step. These end a step that has met one:

| Ending | Reason |
|--------|--------|
| A stop signal | Returns at once, also during a wait. |
| The session ends | Nothing is left to continue. |
| `recipe.usage_limit_wait = false` | The operator turned the wait off, so the limit result is passed up at once (`skipped reason=disabled`). |
| `skipped reason=no_limit_message` | The output carried no message in a harness's limit shape, so it is not a limit. |
| `failed reason=prompt_cut_short` | The step's prompt came on a pipe and could not be read to its end. This is a lost prompt, not a limit ending ([below](#a-prompt-that-could-not-be-read-to-its-end)). |

There is no retry count and no budget. A step that meets a second, third or
tenth limit waits for each.

### A step that is not supervised

When the supervisor cannot start (Docker or auto mode, no profile store, no
private scratch directory, no stop-signal handler, or a first attempt that
never spawned), the step writes `skipped reason=not_supervised` and runs once
as a plain launch, which waits for nothing. The conditions are listed in the
[profile reference](./profile-command.md#stdin-handling).

### The step timeout does not cut a wait

`AMPLIHACK_STEP_TIMEOUT` (`--step-timeout`, `recipe.step_timeout`) does not end
a limit wait. `recipe-runner-rs` enforces only a step's own YAML `timeout:`,
so a wait that ended at the step timeout was the only code that turned the
variable into an ending, for steps waiting on a limit alone
([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)). It used to write
`failed reason=step_timeout`; it no longer does. A step's own YAML `timeout:`
still kills it, as it kills any step.

### A prompt piped to the step

A recipe agent step is given its prompt on a pipe. The supervisor keeps a copy
in memory as it arrives, so every retry reads the same bytes from the first
one. Nothing is written to disk.

| Case | What happens |
|------|--------------|
| stdin is an `--input-format` stream channel | It is copied and replayed like any piped prompt. Standard output of an attempt that is run again is discarded, so the caller sees only the attempt that finishes. |
| the copy cannot be set up (a process out of descriptors or threads) | The step is not run, so no limit is met. |
| the prompt is still arriving when the retry is due | The retry starts at once, fed the copy already held and then each byte as the caller writes it. Nothing waits on the caller's pipe. |

### A prompt that could not be read to its end

`failed reason=prompt_cut_short` is reached only when a read of the step's
standard input failed part way, or the copier panicked part way. The attempt
was given part of its task and cannot tell, so the step fails whether or not
it meets a limit, and no retry can be given more. A step whose prompt was read
whole never reaches this ending.

When a limit also stands, the run learns of it as data. The supervisor
reports every limit to the recipe runner as `Outcome::UsageLimit`
([#2334](https://github.com/rysweet/amplihack-rs/pull/2334)); the runner writes
it into the run's result as `failure_cause`
(`{"class":"usage_limit","origin":{...}}`), the run is classed `usage_limit`
and recorded as resumable, and the same command launched again resumes at the
step with its whole prompt. `issue_2911_prompt_loss_resumes` runs that path on
the pinned runner. No amplihack code relaunches a run by itself; whatever
started it does (auto-drive stops a drive as `USAGE_LIMIT`,
[auto-drive](./auto-drive-to-merge.md#usage-limits-are-not-stuck-or-hand-off)).
The resume starts at the failed step when the checkout ignores
`.claude/runtime/`, as the worktrees the bundled workflow uses do
([workflow runtime artifacts](./workflow-runtime-artifacts.md)); elsewhere it
starts at the first step that depends on the repository.

Beside the event the step writes one sentence for the person reading the log,
followed by when the limit lifts: `amplihack: the limit resets <time>` for a
reset still ahead, a line saying a stated reset has already passed, or
`amplihack: no reset is known for this limit.`

## Limit kinds

| Kind | Words that select it |
|------|----------------------|
| `session` | `5-hour limit`, `session limit`, and any limit wording it does not recognise |
| `weekly` | `weekly limit` |
| `spend` | the organisation spend-limit phrase |
| `credits` | `Your workspace is out of credits` (Codex) |

Unrecognised wording is read as `session`, the shortest window.

## The limit record

Every limit the supervisor sees is written to the rotator's ledger, for the
login the attempt ran under, before the wait is planned
(`note_limit_block` in `crates/amplihack-cli/src/commands/profile/rotate/step_block.rs`).
It returns nothing, so the record can never delay, refuse or end a step.

```json
"overflow_blocked": {
  "work": { "at": 1792826100, "until": 1792843200, "reason": "spend_limit" }
}
```

| Field | Meaning |
|-------|---------|
| `at` | Epoch second the limit was met. |
| `until` | The stated reset, on the real clock, held to the kind's window (rule 3). |
| `reason` | Always `spend_limit`, the one value every version of the rotator accepts. |

Rules the writer keeps:

1. The ledger must already exist; a host with no rotator gains no file.
2. The span must satisfy the ledger's rule (`until` after `at`, at most 62
   days), or nothing is written (`stated reset out of range`).
3. A `session` record ends at most five hours after `at` and a `weekly`
   record at most seven days after it. A genuine reset always lies inside its
   window. `spend` and `credits` keep the stated reset, within rule 2.
4. It waits at most 10 seconds for the ledger's lock, then reports
   `rotator busy` and writes nothing.
5. An active block is not extended and raises no second alert.
6. At most 256 blocks are kept; expired ones are pruned first.
7. The write is atomic and keeps the ledger's owner-only permissions.

A `spend` limit stating no reset is recorded until the rotator's own spend
reset (00:00 UTC on the 1st of the next month). A `session`, `weekly` or
`credits` limit stating no reset writes no record (`no stated reset`).

```
2026-11-21T18:05:00Z ALERT: login work hit its weekly limit; blocked until 2026-11-23T00:00:00Z
```

## Which wait a limit starts

| Limit | Wait | Ends on | Outer bound |
|-------|------|---------|-------------|
| Reset within five hours | reset wait | the reset plus 120 s, a login with room, or a login change | none |
| Reset more than five hours away | capacity wait | the reset plus 120 s, held to the kind's window, a login with room, or a login change | none |
| Reset not readable | capacity wait | a login with room, or a login change | `max_wait_secs` |
| Reset already behind the clock | capacity wait | a login with room, or a login change | `max_wait_secs` |
| Spend limit stating no reset | login-change wait | the login in `~/.claude` changing | `max_wait_secs` |
| Credits (Codex) reaching the Claude supervisor | capacity wait | a login with room, or a login change | `max_wait_secs` |

A stated reset is held to its kind's window, as the limit record holds it:
five hours for `session`, seven days for `weekly`, and the 62-day span for
`spend`. A weekly limit days away is therefore waited for once, to its reset,
and the agent runs again there. The step cannot tell the harness's message
from a model that printed one, and the hold bounds how long a forged message
can hold it.

A login-change wait is never ended by a capacity reading, because a reading
reports session and weekly windows and never organisation credit.

A bare `429` rate-limit envelope is not a limit. With another login that
could take over it becomes a capacity wait with `reset=none`; without one the
step writes no wait line and hands the `429` back to the recipe runner.

No wait is planned with a deadline of now. A reset already behind the clock
becomes a capacity wait, so the retry does not meet the same limit in a loop.

## How a wait wakes

The wait blocks in one call on everything it can end on, and reads its files
only after one of them is written:

| Source | What it records | How the wait learns of it |
|--------|-----------------|---------------------------|
| The rotate ledger and swap record (`<root>/.auto-switch.d/rotate/`) | a rotator cycle with room, the login it swapped in | a file event |
| The auto-switch readings and ledger (`<root>/.auto-switch.d/`) | another login with room | a file event |
| `.claude.json` and `.credentials.json` | the login in the step's directory | a file event |
| A stop signal (SIGTERM, SIGHUP, SIGINT) | | a socket the signal handlers write to |
| The exit of the process that started the step | | a pidfd on Linux, `EVFILT_PROC` on macOS |
| The reset, or the outer bound | | the call's timeout |

File events are inotify on Linux and kqueue on macOS. A directory that does
not exist yet is watched through the nearest one above it, so a rotator or
login that appears during the wait is seen. Writes that arrive within a
second of the last read are gathered into one read, because `$HOME/.claude.json`
is rewritten by every Claude session on the host.

The wait also wakes once a minute to write its `progress` line, and reads
nothing then. The line is how an operator sees the step is waiting rather
than hung, and it keeps a supervisor that ends a silent child
(`AMPLIHACK_IDLE_TIMEOUT_SECS`) from ending a step that waits for hours.

A `.claude.json` that is there but does not parse is a read made during
another process's write. It is no reading: it neither ends the wait nor
becomes its baseline, and the write that completes the file wakes the wait
again.

Where no file event can be had (another platform, or a host out of inotify
instances), the wait says so once and reads its files at each progress line
instead:

```
amplihack.recipe.usage_limit_wait no_file_events reason=watch_failed
```

## The waiting_for_capacity line

When a capacity or login-change wait begins, the supervisor says so on
standard error and in the recipe log (`AMPLIHACK_RECIPE_LOG`) immediately
before the wait's `start` line. In the recipe log `account=<login>` replaces
`harness=`:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=no_reset account=work until=2026-11-21 22:40:00 +00:00
```

| `reason=` | Why the wait is for capacity |
|-----------|------------------------------|
| `reset_too_far` | The stated reset is more than five hours away; the wait still ends at it. |
| `no_reset` | The message states no readable reset. |
| `spend_limit` | A spend limit stating no reset. |
| `credits` | A credit-exhaustion message. |
| `passed_reset` | The stated reset is already behind the clock. |

`until` is the end of this one wait. The line carries no text from the
harness's output.

```sh
grep 'usage_limit_wait waiting_for_capacity' "$AMPLIHACK_RECIPE_LOG"
```

## The timed parts that remain

| Part | What it is | What it does not do |
|------|------------|---------------------|
| The outer bound | A wait whose limit stated no reset still ahead (none stated, credits, a reset already passed, a bare 429, a spend limit stating none) and that no event ends runs to `max_wait_secs` (default six hours, floor 60 seconds), ends `capacity expired` or `max reached`, and the step runs once more. | It never ends a step, and it never applies to a wait whose limit stated a reset still ahead. |
| The progress line | The wait wakes once a minute to write `progress remaining=<n>s`. | It reads no file, checks no account and launches nothing. |

The outer bound is kept because such a limit records nothing about when it
lifts ([the limit record](#the-limit-record) writes no block for it), and on a
host with no rotator and no other login nothing but the bound would run the
step again without a person. Whether those waits should instead wait for a
person is left to the owner under
[#3098](https://github.com/rysweet/amplihack-rs/issues/3098).

## What max_wait_secs means

`recipe.usage_limit_max_wait_secs` (`AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS`,
default `21600`) is the [outer bound](#the-timed-parts-that-remain) of one wait
whose limit stated no reset still ahead. It is not a per-step budget, and it
does not shorten a wait for a stated reset.

A step still limited when such a wait reaches the bound runs its agent again,
so the value is how often such a step re-asks the harness. The floor is 60
seconds, so no value makes the bound a busy loop of agent launches, and a
configured `0` means 60 seconds; set
[recipe.usage_limit_wait](./user-config.md#recipeusage_limit_wait) to `false`
to turn waiting off. A configured length below 600 seconds, the rotator's own
cycle, is reported once per step:

```
warning: recipe.usage_limit_max_wait_secs (AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS) is below 600 seconds, the rotator's own cycle. It is the outer bound of a wait whose limit stated no reset still ahead, and a step still limited when such a wait ends runs its agent again, so this is how often such a step re-asks; the default of 21600 seconds asks once every six hours. A wait for a stated reset is not shortened by it. Set recipe.usage_limit_wait = false (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting
```

## Codex credit exhaustion

`Your workspace is out of credits` is classed `usage_limit`, with
`"signal":"your workspace is out of credits"`. `credit balance is too low`, the
Anthropic API message, still classes `environmental`.

| Reader | Rule |
|--------|------|
| The recipe runner's failure class | The phrase anywhere in a failed step's evidence, in any case. |
| The usage-limit wait | The phrase must open a whole standard-error line, or the last non-empty standard-output line, and be followed by a delimiter. Prose that mentions it does not start a wait. |

A Codex step launched directly with `amplihack codex` is not supervised, so
its credit exhaustion is classed but does not wait
([#2949](https://github.com/rysweet/amplihack-rs/issues/2949)).

## Removed endings

| Was | Now |
|-----|-----|
| `skipped reason=reset_too_far` | a capacity wait that ends at the reset |
| `skipped reason=no_reset` | capacity wait |
| `skipped reason=spend_limit` (no stated reset) | login-change wait |
| `skipped reason=already_waited`, `failed reason=already_waited` | not written; the step waits again |
| `failed reason=capacity_expired` | the step retries |
| `failed reason=spend_limit` after a login-change wait reaches its deadline | the step retries |
| the `max_wait_secs` per-step budget | the outer bound of one wait with no usable reset |
| `failed reason=step_timeout`, `bound=step_timeout` | not written ([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)) |
| `skipped reason=piped_prompt` | the copy is kept in memory and replayed; a step whose copy cannot be set up is not run; a prompt cut short is `failed reason=prompt_cut_short` ([#3036](https://github.com/rysweet/amplihack-rs/issues/3036)) |

`reset_too_far`, `no_reset` and `spend_limit` remain as the `reason=` of a
`waiting_for_capacity` line.

## Limits of this behaviour

- A Codex step launched directly is not supervised and does not wait
  ([#2949](https://github.com/rysweet/amplihack-rs/issues/2949)).
- A forged `session` or `weekly` line can keep a login out of rotation, and a
  step waiting, for up to five hours or seven days, and a forged `spend` line
  until its stated reset, within 62 days
  ([#3005](https://github.com/rysweet/amplihack-rs/issues/3005)).
- No line reports how long a step has waited in total
  ([#3003](https://github.com/rysweet/amplihack-rs/issues/3003)).
- A piped prompt is kept in memory for the life of the step and is not
  capped; a cap would be a budget that ends a step on a limit.
- A step with a spend limit that no login change lifts re-asks once per
  `max_wait_secs` until the limit lifts or the run is stopped.
- Where no file event can be had, the wait reads its files once a minute
  ([How a wait wakes](#how-a-wait-wakes)).
