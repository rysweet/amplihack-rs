# Usage-limit waits and limit records

A supervised recipe agent step that meets a usage, spend or credit limit does
not end because of it. It records the limit for the login it ran under, waits,
and runs the step again. Nothing ends that chain: not the number of limits,
not the time spent waiting, and not the step timeout.

A wait ends only on an event: its stated reset passing, a login with room,
the login changing, a stop signal, or the end of the step's session. No timer
ends a wait. It blocks on writes to the files that record those events and
reads them only after a write ([How a wait wakes](#how-a-wait-wakes)). A limit
that states a reset still ahead is waited for to that reset, however far away.
A limit that states no usable reset gives the wait no deadline: it waits for a
login with room or a login change, and says so
([A wait with no deadline](#a-wait-with-no-deadline)).

A wait is the last resort. Before any wait an auto-switch step moves to
another stored login under the auto-switch threshold, and when it does wait,
the wait also ends when the first of the other logins it could move to comes
back under that threshold ([Other logins](#other-logins)).

This page owns the tables for limit kinds, the limit record, which wait a
limit starts and the `waiting_for_capacity` line. The log grammar is in the
[usage-limit wait reference](./profile-command.md#usage-limit-wait), and
task-oriented guidance is in the
[how-to](../howto/claude-profiles.md#waiting-for-a-usage-limit-reset).

## Contents

- [What ends a step and what does not](#what-ends-a-step-and-what-does-not)
    - [Turning the wait off](#turning-the-wait-off)
    - [A step that is not supervised](#a-step-that-is-not-supervised)
    - [The step timeout does not cut a wait](#the-step-timeout-does-not-cut-a-wait)
    - [A prompt piped to the step](#a-prompt-piped-to-the-step)
    - [A prompt that could not be read to its end](#a-prompt-that-could-not-be-read-to-its-end)
- [Limit kinds](#limit-kinds)
- [The limit record](#the-limit-record)
- [Which wait a limit starts](#which-wait-a-limit-starts)
- [How a wait wakes](#how-a-wait-wakes)
    - [A wait with no file events](#a-wait-with-no-file-events)
- [The waiting_for_capacity line](#the-waiting_for_capacity-line)
- [A wait with no deadline](#a-wait-with-no-deadline)
    - [What ends a wait with no deadline](#what-ends-a-wait-with-no-deadline)
- [Other logins](#other-logins)
    - [Moving to another login before a wait](#moving-to-another-login-before-a-wait)
    - [The earliest-reset deadline](#the-earliest-reset-deadline)
    - [When the other logins cannot be read](#when-the-other-logins-cannot-be-read)
    - [The login a wait names](#the-login-a-wait-names)
    - [A reset that names no zone](#a-reset-that-names-no-zone)
- [The timed parts that remain](#the-timed-parts-that-remain)
- [A wait has no length setting](#a-wait-has-no-length-setting)
- [Codex credit exhaustion](#codex-credit-exhaustion)
- [Removed endings](#removed-endings)
- [Limits of this behaviour](#limits-of-this-behaviour)

## What ends a step and what does not

A limit never ends a supervised step. These end a step that has met one:

| Ending | Reason |
|--------|--------|
| A stop signal | Returns at once, also during a wait. |
| The session ends | Nothing is left to continue. |
| `recipe.usage_limit_wait = false` or `AMPLIHACK_USAGE_LIMIT_WAIT=0` | The operator turned the wait off, so the limit result is passed up at once (`skipped reason=disabled`). This is intended ([below](#turning-the-wait-off)). |
| `skipped reason=no_limit_message` | The output carried no message in a harness's limit shape, so it is not a limit. |
| `failed reason=prompt_cut_short` | The step's prompt came on a pipe and could not be read to its end. This is a lost prompt, not a limit ending ([below](#a-prompt-that-could-not-be-read-to-its-end)). |

There is no retry count and no budget. A step that meets a second, third or
tenth limit waits for each.

### Turning the wait off

`recipe.usage_limit_wait = false` is the one setting that lets a limit end a
step, and it does so on purpose: it is how a job with a timeout of its own, or
an operator who would rather relaunch by hand, asks for the limit result at
once. The run still learns of the limit as a limit. The recipe runner classes
the failure `usage_limit` and records the run as resumable, so the same
command launched again resumes at the step. Nothing in amplihack writes the
key or its variable; only an operator does.

`CI` does not turn the wait off. It used to, by default, and wrote
`skipped reason=ci`. A step under `CI=true` now waits like any other
([#2911](https://github.com/rysweet/amplihack-rs/issues/2911)), because `CI`
is exported by tools that only mean to suppress prompts. The delegation
layer in amplihack sets it for every delegated `claude -p`, so a recipe run
started from one ended at its first limit.

### A step that is not supervised

When the supervisor cannot start (Docker or auto mode, no profile store, no
private scratch directory, no stop-signal handler, or a first attempt that
never spawned), the step writes `skipped reason=not_supervised` and runs once
as a plain launch, which waits for nothing. The stop-signal handlers and the
socket they write to, which wakes a wait on a stop signal, are made together,
so a supervisor that cannot make the socket stands down in the same way
(`warning: the usage-limit wait cannot supervise this step (cannot catch stop
signals: ...); running it once`, with `auto-switch` in place of `the
usage-limit wait` when auto-switch is on). The conditions are listed in the
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

| Limit | Wait | Ends on | Deadline (`bound=`) |
|-------|------|---------|---------------------|
| Reset within five hours | reset wait | the reset plus 120 s, a login with room, or a login change | the reset plus 120 s (`reset+margin`) |
| Reset more than five hours away | capacity wait | the reset plus 120 s, held to the kind's window, a login with room, or a login change | the held reset plus 120 s (`reset+margin`) |
| Reset not readable | capacity wait | a login with room, or a login change | none (`none`) |
| Reset stated with no zone | capacity wait | a login with room, or a login change | none (`none`) |
| Reset already behind the clock | capacity wait | a login with room, or a login change | none (`none`) |
| A bare `429` that another login could take over | capacity wait | a login with room, or a login change | none (`none`) |
| Spend limit stating no reset | login-change wait | the login in `~/.claude` changing | none (`none`) |
| Credits (Codex) reaching the Claude supervisor | capacity wait | a login with room, or a login change | none (`none`) |

Every wait also ends on a stop signal or the end of the step's session, which
end the step rather than retry it.

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
becomes a capacity wait with no deadline, so the retry does not meet the same
limit in a loop.

## How a wait wakes

The wait blocks in one call on everything it can end on, and reads its files
only after one of them is written:

| Source | What it records | How the wait learns of it |
|--------|-----------------|---------------------------|
| The rotate ledger and swap record (`<root>/.auto-switch.d/rotate/`) | a rotator cycle with room, the login it swapped in | a file event |
| The auto-switch readings and ledger (`<root>/.auto-switch.d/`) | another login with room | a file event |
| `.claude.json` and `.credentials.json` | the login in the step's directory | a file event |
| A stop signal (SIGTERM, SIGHUP, SIGINT) | | the socket the step's signal handlers write to |
| The exit of the process that started the step | | a pidfd on Linux, `EVFILT_PROC` on macOS |
| The reset plus 120 s, for a wait that has a deadline | | the call's timeout |

File events are inotify on Linux and kqueue on macOS. A directory that does
not exist yet is watched through the nearest one above it, so a rotator or
login that appears during the wait is seen. Writes that arrive within a
second of the last read are gathered into one read, because `$HOME/.claude.json`
is rewritten by every Claude session on the host. That spacing starts only
from a write, and the wait keeps blocking on events while it lasts.

The stop-signal socket is made once for each supervised step, with the
stop-signal handlers, so every wait has one
([A step that is not supervised](#a-step-that-is-not-supervised)).

The wait also wakes once a minute to write its `progress` line. It reads no
file and launches nothing then. The line is how an operator sees the step is
waiting rather than hung, and it keeps a supervisor that ends a silent child
(`AMPLIHACK_IDLE_TIMEOUT_SECS`) from ending a step that waits for hours.

A `.claude.json` that is there but does not parse is a read made during
another process's write. It is no reading: it neither ends the wait nor
becomes its baseline, and the write that completes the file wakes the wait
again.

### A wait with no file events

Where no file event can be had, the wait writes one `no_file_events` line and
one sentence on standard error, once per wait:

```
amplihack.recipe.usage_limit_wait no_file_events reason=watch_failed
amplihack: this wait cannot watch its files, so it cannot see another login gain room or the login change; it ends at its reset, on a stop signal or when its session ends, and a wait whose limit stated no reset waits for one of the last two. See docs/reference/usage-limit-waits.md
```

| `reason=` | Cause |
|-----------|-------|
| `unsupported` | A platform other than Linux or macOS. |
| `watch_failed` | The watches could not be made, for example on a host out of inotify instances. |
| `wait_failed` | The event source failed part way through the wait. The wait reads its files once, for any write the failed source may have missed. |

After that the wait reads nothing on a timer. It cannot see a login gain room
or the login change, so it ends at its reset, on a stop signal, or when its
session ends. A wait whose limit stated no reset has no deadline, so only a
stop signal or the end of the session ends it.

## The waiting_for_capacity line

When a capacity or login-change wait begins, the supervisor says so on
standard error and in the recipe log (`AMPLIHACK_RECIPE_LOG`) immediately
before the wait's `start` line. In the recipe log `account=<login>` replaces
`harness=`:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_too_far account=work until=2026-11-23 00:02:00 +00:00
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=no_reset account=work until=none
```

| `reason=` | Why the wait is for capacity | `until=` |
|-----------|------------------------------|----------|
| `reset_too_far` | The stated reset is more than five hours away; the wait still ends at it. | the held reset plus 120 s |
| `no_reset` | The message states no readable reset, or a bare `429` that another login could take over. | `none` |
| `spend_limit` | A spend limit stating no reset. | `none` |
| `credits` | A credit-exhaustion message. | `none` |
| `passed_reset` | The stated reset is already behind the clock. | `none` |
| `reset_without_zone` | The stated reset is a clock time with no zone, which is not read ([A reset that names no zone](#a-reset-that-names-no-zone)). | `none` |

`until` is the wait's deadline, or `none` for a wait that has no deadline
([A wait with no deadline](#a-wait-with-no-deadline)). The line carries no
text from the harness's output.

```sh
grep 'usage_limit_wait waiting_for_capacity' "$AMPLIHACK_RECIPE_LOG"
```

## A wait with no deadline

A limit that states no usable reset gives its wait no deadline. That covers a
message stating no readable reset, Codex credit exhaustion, a stated reset
already behind the clock, a bare `429` that another login could take over,
and a spend limit stating no reset (the login-change wait). Nothing records
when such a limit lifts, so the wait waits for the events it can see: a login
with room, or a login change. It does not end on a timer.

This follows the rule in
[#2911](https://github.com/rysweet/amplihack-rs/issues/2911): "When nothing
can be known about when room returns, it still waits, and says plainly in its
log and its run status that it is waiting for capacity and why, so an
operator can see it; it does not end."

The wait's lines say so. `until=none` and `bound=none` mark a wait with no
deadline, and its progress line reads `remaining=none`:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=no_reset harness=claude until=none
amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=none until=none bound=none
amplihack.recipe.usage_limit_wait progress remaining=none
```

A passed reset keeps the stated time in `reset=` and reads
`reason=passed_reset`. A spend limit stating no reset starts a login-change
wait:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=spend_limit harness=claude until=none
amplihack.recipe.usage_limit_wait start reason=spend_limit wait=login_change harness=claude reset=none until=none bound=none
```

In the recipe log `account=<login>` replaces `harness=`. On standard error
only, between the `waiting_for_capacity` line and the `start` line, the step
also writes one sentence for the person reading it:

```
amplihack: nothing records when this limit lifts, so this step waits with no deadline until a login has room or the login changes. `amplihack profile switch` ends such a wait for a step on ~/.claude; a stop signal ends the step. See docs/reference/usage-limit-waits.md
```

### What ends a wait with no deadline

| Event | `end reason=` | Then |
|-------|---------------|------|
| The rotator records a cycle with room after the wait began (a step on `$HOME/.claude`) | `rotator capacity` | the step runs again on the live login |
| A reading shows another stored login with room (a step under an inherited profile with auto-switch on) | `login capacity` | the step runs again under that profile |
| The login in the step's directory changes | `login changed` | the step runs again on the login now live |
| A stop signal | `signal` | the step ends |
| The process that started the step exits | `session ended` | the step ends |

A login-change wait takes only the last three: no capacity reading ends it
([Login-change wait](./profile-command.md#login-change-wait)).

On a host with no rotator and no other stored login, nothing ends such a wait
by itself. A person who wants the step to run again makes an event:

- **`amplihack profile switch <profile>`**, for a step on `$HOME/.claude`.
  It moves the profile's login into `~/.claude` and writes a `switched`
  record to the rotate ledger. That record counts as room unless it says
  `live_room: false`, so it ends a capacity wait as `rotator capacity`; a
  login-change wait ends as `login changed`. See
  [Rotate switch](./profile-command.md#rotate-switch).
- **Logging the step's own config directory in to another account.** The
  login changes, and the wait ends as `login changed`. On a `$HOME/.claude`
  step with a rotator, that change is held to the rotator's room reading
  ([Waking only on room](./profile-command.md#waking-only-on-room)), so use
  `profile switch` there.
- **A stop signal** (Ctrl-C, `SIGTERM`, `SIGHUP`). It ends the step. The
  limit was already reported to the recipe runner for this step, and a run
  stopped by a signal stays resumable, so the same command launched again
  resumes at the step.

## Other logins

On 21 November 2026 a recipe step logged
`start reason=usage_limit account=unknown ... bound=reset+margin` and waited
nearly two hours for its own login's reset while other stored logins could
have run it ([#2967](https://github.com/rysweet/amplihack-rs/issues/2967)).
The rules below answer that. None adds a timer, a polling loop, a count or a
gate: each reads files the supervisor already reads, at moments it already
reads them.

### Moving to another login before a wait

Whenever an attempt of an auto-switch step meets a usage limit, the
supervisor first looks for another stored login by the ordinary
[candidate selection](./profile-command.md#candidate-selection): under the
auto-switch threshold (90% by default, `amplihack profile auto-switch
threshold <1-100>`), not excluded, not leased, not hosting another account,
within [organisation affinity](./profile-organisation-affinity.md), ready for
a launch, and not already tried by this step. When it finds one the step runs
again there at once, with no `start` or `end` line.

This is asked for every limit, whether or not the step has already waited.
Before #2967 a step that had waited once was never moved again, so a limit on
the retry its wait earned left it waiting while another login had room. A
move is not a wait and writes no mark. The moves end because each one adds a
login to the step's tried set, so a step makes at most one move per stored
login, and then waits.

The threshold is the one bound for "this login can run the step". The
`usable now` count of `amplihack profile usage` uses the rotator's own bound
(session below 95% by default), so it can count a login at 92% that a step
does not move to. Raise the auto-switch threshold to make the two agree.

A step stays where it is, and goes on to wait, when auto-switch is off, which
is the operator's standing instruction. A step on `~/.claude` does not move
itself: a running rotator swaps the login there, at once when a limit is
recorded ([#3012](https://github.com/rysweet/amplihack-rs/issues/3012)), and
with no rotator it waits
([#3122](https://github.com/rysweet/amplihack-rs/issues/3122)).

### The earliest-reset deadline

When no login can take the step, it waits, and the wait also ends at the
earliest moment another login it could move to comes back under the
threshold, when that is before the wait's own deadline. A login's window
resetting writes no file, so no file event would end the wait there; the
instant is known from that login's reading.

| Rule | |
|------|---|
| Which logins count | The eligible set above, without the logins this step has tried. A login with no reading, no reset ahead, or already under the threshold (the move would have taken it) contributes nothing. |
| A login's instant | The latest `resets_at` among its windows at or above the threshold: it is under the threshold only once all of them have reset. Held to the weekly window, as any reset read from a reading is. |
| When it applies | The instant is after now, and its deadline (the instant plus 120 s) comes before the wait's own, or the wait has none. A wait whose limit stated no usable reset therefore gains a deadline here. |
| What it changes | `until=` and `bound=earliest_reset+margin`. The kind of wait, its `wait=` token, what else may end it, and `reset=`, the reset the step's own limit stated, stay as they were. |
| What it never touches | A login-change wait: it waits for an organisation's credit, which no other login's window restores. |

```
amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude reset=2026-11-21 23:50:00 -08:00 until=2026-11-21 22:32:00 -08:00 bound=earliest_reset+margin
amplihack.recipe.usage_limit_wait end reason=reset reached
amplihack.recipe.usage_limit_wait resume attempt=2
amplihack: the earliest reset among this step's logins has arrived; rerunning this step under profile "lab" (auto-switch)
```

A wait that reaches such a deadline ends as `reset reached`, and the retry
runs on the login whose reset it was, with no fresh reading required,
because that reset is what the wait was for. That login must still pass the
checks that need no reading (stored, not excluded, not leased, holding its
own login), asked from the login the limited attempt ran on, as when the
login was chosen. If it does not pass, the retry runs on the step's own
profile and says why
([When the other logins cannot be read](#when-the-other-logins-cannot-be-read)).
The note above is printed only when the retry changes login. A retry back to
the step's inherited profile, for a step that was moved off it before it
waited, is such a change. The other logins are read again for each wait, so
no wait is timed by an instant an earlier one computed.

### When the other logins cannot be read

The other logins are read from the auto-switch ledger and usage readings,
under the auto-switch setting. When that read fails, the step goes on as it
would without them: the wait keeps its own deadline, or the retry runs on the
step's own profile. Nothing is retried and no gate is added. The step says why,
once on stderr in words, with the error, and once as a wait line that also
reaches the [recipe log](./profile-command.md#the-wait-in-the-recipe-log)
with `account=` added. A reader can then tell "the logins could not be
read" from "no other login had a reset ahead", which writes nothing. On
21 November 2026 the two looked the same.

| Cause | `reason=` |
|-------|-----------|
| Auto-switch was turned off while the step ran | `auto_switch_off` |
| The auto-switch ledger cannot be read | `ledger_unreadable` |
| The auto-switch usage readings cannot be read | `readings_unreadable` |
| The login whose reset ended the wait was excluded, leased, given another account, logged out or removed while the step waited (the retry only) | `login_not_eligible` |

Before a wait, the lines come before its `start`:

```
amplihack: this step's other logins were not read for its wait (reason=auto_switch_off): auto-switch was turned off while this step ran; no other login's reset can end this wait early (auto-switch)
amplihack.recipe.usage_limit_wait other_logins_unread reason=auto_switch_off
amplihack.recipe.usage_limit_wait start reason=usage_limit harness=claude reset=2026-11-21 23:50:00 -08:00 until=2026-11-21 23:52:00 -08:00 bound=reset+margin
```

After a wait that ended at another login's reset, they come after its
`resume`:

```
amplihack.recipe.usage_limit_wait end reason=reset reached
amplihack.recipe.usage_limit_wait resume attempt=2
amplihack: the wait ended at the reset of profile "lab", but this step reruns under its own profile "work" (reason=login_not_eligible): that profile was excluded, leased, given another account, logged out or removed while this step waited (auto-switch)
amplihack.recipe.usage_limit_wait retry_not_moved reason=login_not_eligible
```

For an unreadable file the stderr sentence carries the error after the
cause, as in `the auto-switch ledger cannot be read: auto-switch state is
inconsistent: .auto-switch.d/ledger.json: invalid JSON at line 1 column 1`.
The wait line carries the fixed `reason=` only.

A step that started with auto-switch off never had another login to
consider, and a step whose wait is off has no wait to bound, so neither
writes these lines. A ledger that is already unreadable when the step starts
stops the launch under its profile (`error: auto-switch state is
inconsistent`) before any limit is met, so `ledger_unreadable` comes only
from a ledger that became unreadable while the step ran.

### The login a wait names

Every wait line in the recipe log names the login the failed attempt ran on
in `account=`: the profile the supervisor launched it under, else the
inherited profile, else, for a `$HOME/.claude` step, the rotator's live
login. Before #2967 every step under a stored profile logged
`account=unknown`. `unknown` is now left for a host with no profile store, a
`CLAUDE_CONFIG_DIR` step, and a live login whose name cannot be read. The
name is a local profile name, never an account name or e-mail address.

### A reset that names no zone

Claude Code prints some limits with a bare clock time:

```
You've hit your individual spend limit · run /usage-credits to ask your admin for a higher limit · your 5-hour limit resets 11:50pm
```

No zone is guessed, so this reset is not read. The wait says so rather than
read as a limit that stated no reset:

```
amplihack.recipe.usage_limit_wait waiting_for_capacity reason=reset_without_zone account=work until=none
amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity account=work reset=none until=none bound=none
```

It is a capacity wait, even for a spend-limit line: the message names a
usage window that resets, so a login with room, a login change or another
login's reset ([The earliest-reset deadline](#the-earliest-reset-deadline))
ends it. Only a spend limit that names no reset at all takes the
login-change wait. A reset that names a zone the table does not know is a
stated zone, not a missing one, and is handled as before #2967.

## The timed parts that remain

| Part | What it is | What it does not do |
|------|------------|---------------------|
| The reset deadline | A wait whose limit stated a reset still ahead ends at that reset plus 120 s (`bound=reset+margin`), the reset held to its kind's window. The reset passing is a limit event: the failed login has recovered. | It never applies to a wait whose limit stated no usable reset. |
| The earliest reset of another login | A wait ends at the first reset among the other logins the step could move to, plus 120 s, when that comes first (`bound=earliest_reset+margin`). That login recovering is a limit event too, and the retry runs on it ([The earliest-reset deadline](#the-earliest-reset-deadline)). | It never applies to a login-change wait, and it never makes a deadline later. |
| The progress line | The wait wakes once a minute to write `progress remaining=<n>s`, or `progress remaining=none` for a wait with no deadline. | It reads no file, checks no account, launches nothing and ends nothing. |

Nothing else in a wait is timed. Where no file event can be had, the wait
reads nothing on a timer
([A wait with no file events](#a-wait-with-no-file-events)). The one-second
spacing between reads ([How a wait wakes](#how-a-wait-wakes)) starts only
from a write, so it reads nothing on its own.

## A wait has no length setting

No setting says how long a wait lasts. A wait for a stated reset ends at that
reset; a wait whose limit stated no usable reset ends when a login has room
or the login changes. A length setting would be a timer that runs the step's
agent again on a login that may still be spent, which
[#2911](https://github.com/rysweet/amplihack-rs/issues/2911) rules out: "No
retry counts and no wall-clock budget that ends a run; remove the paths above
rather than raising their numbers."

`recipe.usage_limit_max_wait_secs` has been removed from the config schema. A
config file that still sets it gets an `unknown key` warning when it loads,
and the rest of the file applies:

```
warning: /home/alice/.amplihack/config.toml: recipe.usage_limit_max_wait_secs: unknown key
```

`amplihack config set recipe.usage_limit_max_wait_secs ...` is refused as an
unknown key (exit code `2`). Delete the line from the file to clear the
warning.

`AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` is no longer read. While the wait is
on, a step that finds it exported with a value prints:

```
warning: AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS is no longer read. A usage-limit wait has no timed bound: one whose limit stated a reset ends at that reset, and one whose limit stated none ends when a login has room or the login changes (`amplihack profile switch` makes that change). Set recipe.usage_limit_wait = false (AMPLIHACK_USAGE_LIMIT_WAIT=0) to stop steps waiting
```

The one setting is
[recipe.usage_limit_wait](./user-config.md#recipeusage_limit_wait)
(`AMPLIHACK_USAGE_LIMIT_WAIT`). `false` or `0` turns the wait off, and the
limit is passed up at once as `skipped reason=disabled`
([Turning the wait off](#turning-the-wait-off)).

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
| `failed reason=capacity_expired`, `end reason=capacity expired` | not written; a capacity wait with no usable reset has no deadline ([A wait with no deadline](#a-wait-with-no-deadline)) |
| `failed reason=spend_limit`, `end reason=max reached` | not written; a login-change wait has no deadline |
| `bound=max_wait`, `recipe.usage_limit_max_wait_secs`, `AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS` | `bound=none` and `until=none`; the key is an unknown key and the variable is not read ([A wait has no length setting](#a-wait-has-no-length-setting)) |
| `no_file_events` followed by a read of the files at each progress line, or a sleep in one-second slices | the wait reads nothing on a timer ([A wait with no file events](#a-wait-with-no-file-events)) |
| `failed reason=step_timeout`, `bound=step_timeout` | not written ([#3037](https://github.com/rysweet/amplihack-rs/issues/3037)) |
| `skipped reason=ci`, the wait off by default under `CI` | not written; the step waits ([Turning the wait off](#turning-the-wait-off)) |
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
- A wait whose limit stated no usable reset, on a host with no rotator and no
  other stored login, waits until a person changes the login or stops the
  run. Nothing ends it on its own
  ([A wait with no deadline](#a-wait-with-no-deadline)).
- Where no file event can be had, the wait cannot see a login gain room or
  the login change. It ends at its reset, on a stop signal or when its
  session ends ([A wait with no file events](#a-wait-with-no-file-events)).
- The earliest-reset deadline reads stored readings, which have no age
  bound, so a wait can end at a reset an old reading named. The cost is one
  failed attempt, after which that login is in the step's tried set
  ([#3093](https://github.com/rysweet/amplihack-rs/issues/3093)).
- A move does not read session flags, so a resumed session can be moved onto
  a profile that lacks its transcript
  ([#3091](https://github.com/rysweet/amplihack-rs/issues/3091)).
- A reset that names no zone is not read; the wait names the cause
  (`reset_without_zone`) and is a capacity wait with no deadline of its own
  ([#3121](https://github.com/rysweet/amplihack-rs/issues/3121)).
- The recipe heartbeat of a waiting step still reads `working`; the wait's
  own `waiting_for_capacity`, `start` and `progress` lines say that it is
  waiting and until when
  ([#3120](https://github.com/rysweet/amplihack-rs/issues/3120)).
- A step on `~/.claude` with no rotator running waits for its own login's
  reset even when a stored profile has room
  ([#3122](https://github.com/rysweet/amplihack-rs/issues/3122)).
