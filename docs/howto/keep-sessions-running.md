---
type: how-to
---

# How to Keep a Session Running After Its Process Exits

A standing session is an interactive Claude Code session that should always be
running. If its process exits for any reason other than you ending it, for
example when a usage limit stopped it and the process went with it, amplihack
starts it again in the same pane with `claude --continue`, so the conversation
carries on.

Every exit is given a relaunch, so a session killed while it sat idle is
started again like any other. A session that cannot get anywhere is not
started again and again: a relaunch that was given on nothing but that rule,
and that gets nowhere either, is the end of the chain. The wrapper then says
why and hands the failure to the supervisor:
[what stops a session that keeps crashing](#what-stops-a-session-that-keeps-crashing).

## Contents

- [Mark a session as standing](#mark-a-session-as-standing)
- [What counts as you ending the session](#what-counts-as-you-ending-the-session)
- [What happens after a limit](#what-happens-after-a-limit)
- [What stops a session that keeps crashing](#what-stops-a-session-that-keeps-crashing)
- [Before a refusal: the rotator is asked](#before-a-refusal-the-rotator-is-asked)
- [Read the relaunch line](#read-the-relaunch-line)
- [What is carried into the relaunch](#what-is-carried-into-the-relaunch)
- [Limits of this feature](#limits-of-this-feature)
- [Related](#related)

## Mark a session as standing

Set the user key once for every supervised session:

```sh
amplihack config set agent.standing true
amplihack claude --profile work --supervise coordinator
```

Or mark one launch:

```sh
amplihack claude --profile work --supervise coordinator --standing
amplihack claude --profile work --supervise scratch --no-standing
```

The flag wins over the key. The key is
[`agent.standing`](../reference/user-config.md#agentstanding) and is read from
the user file only. `--standing` is not saved with the session: if the
supervisor later rebuilds the session in a new pane, the session is standing
only when the key says so.

## What counts as you ending the session

The wrapper that runs in the pane relaunches the session unless one of these
holds:

| Wrapper sees | Treated as | Action |
|--------------|------------|--------|
| Claude Code exits with status `0` | You ended it | No relaunch |
| The session's [ended marker](./supervise-agent-sessions.md#clean-exits-stay-ended) was written during this run, or the supervisor has already ended the session from it | You ended it | No relaunch |
| Claude Code exits non-zero, or fails to run | Not you | A relaunch is in question; see [what stops a session that keeps crashing](#what-stops-a-session-that-keeps-crashing) |
| The wrapper receives `SIGHUP` or `SIGTERM` | The pane or host is going away | No relaunch |

A marker older than the run is an earlier exit of the same conversation that
the supervisor has not removed yet, and does not stop the relaunch. A session
that was not marked standing is never relaunched by this feature.

## What happens after a limit

The wrapper reads the rotator's swap record at the moment of exit.

- A login has room: the session is relaunched at once.
- There is no rotator and no record, or the record is refused or cannot be
  parsed: treated the same way, and the session is relaunched at once.
- No login has room: the wrapper writes one waiting line and then waits for
  the swap record to change, relaunching when a login with room appears. The
  wait is bounded at seven days.
- The wait ends with no login that has room: the seven days pass, the
  rotator's directory is missing or not private to you, the watch on the
  swap record cannot be set up, or the watch is lost. The wrapper does not
  relaunch, and it hands the session to the supervisor in the same way as a
  [`no-progress` refusal](#what-stops-a-session-that-keeps-crashing): a
  `refused` line with `reason=gone` in the pane and the supervisor log, the
  cause on the session's record as an `ALERT`, and the record left `active`
  so the supervisor's recovery starts the session again. If that run also
  exits while no login has room, its wrapper waits again.
- The session record changed during the wait, because the supervisor rebuilt
  the session in another window, or ended or stopped it: the wrapper logs one
  refusal with `reason=superseded` and does not relaunch. Two panes never run
  the same session.

During the wait, `SIGHUP`, `SIGTERM` and `SIGINT` have their default action,
so closing the pane or pressing Ctrl-C ends the waiting wrapper and nothing is
relaunched. A `SIGHUP` or `SIGTERM` that arrives just before the wait blocks
is logged as `reason=gone` and, being a stop you chose, is not put on the
session's record.

```text
ALERT coordinator: not relaunched: it exited exit-status-1 while no login had room, and the watch on the rotator's swap record was lost, so the session was left for the supervisor
```

The part after `and` names why the wait ended:

| Text | Cause |
|------|-------|
| `the wait for one reached its limit of 7 days` | Seven days passed with no login that has room |
| `the rotator's directory is missing or not private to this user` | No event could ever arrive, so there was nothing to wait on |
| `the rotator's swap record could not be watched` | The watch could not be set up |
| `the watch on the rotator's swap record was lost` | The watch ended during the wait, for instance because the directory was moved |

The room rule is the one the
[`limit-resume` hook](../reference/hook-specifications.md) uses. That hook
answers the usage-limit menu in a running session. The wrapper covers the case
where the process has gone.

The rotator writes the swap record only when it swaps. At the moment of an
exit the record is therefore usually older than the run, and a record that
shows room says only that the login it swapped to had room then. It does not
show a limit the run has just met. The wrapper does not wait on that account,
so a session killed for any other reason is not held back. A run that goes
into a limit and exits is relaunched at once, and if that relaunch is the one
the [brake](#what-stops-a-session-that-keeps-crashing) would refuse, the wrapper
first asks the rotator whether the room was real; see
[before a refusal](#before-a-refusal-the-rotator-is-asked).

## What stops a session that keeps crashing

What the wrapper judges is the relaunch chain, not the run. There is no count
of attempts, no retry limit, no delay and no timer.

- **Every exit is given a relaunch.** A session that sat idle for hours and
  was then killed by the out-of-memory killer, by a host event or by a pane
  that went away has nothing to show for its run, and is started again
  anyway. "No work in this run" is not "this start is failing".
- **A relaunch given on nothing but that rule is a trial.** If the run it
  starts comes back the same way, with nothing to show and no room waited
  for, the chain is getting nowhere and the wrapper stops. One more start
  would repeat the one that just failed.

Two things count as something to show, and the wrapper reads both from the
exit itself:

| Changed since the launch that failed | Meaning |
|--------------------------------------|---------|
| The model wrote a reply to the conversation during the run | The run did something. There is more for `--continue` to carry on, so whatever fails next will be a new failure |
| The wrapper waited for room, and a login with room appeared, or the rotator swapped onto a login with room after the run began | The capacity that was spent has been replaced. This is the usage-limit case the feature exists for |

Either of those relaunches a session however often it exits, trial or not, so
a session that is working and a session that keeps hitting limits are never
stopped by this rule.

The trial is a count of one: a chain with nothing to show is given one
relaunch and no more. It does not add up across a session's life, because any
reply or any new room clears it, but it is a count all the same. Moving that
judgement to the supervisor agent is
[#3077](https://github.com/rysweet/amplihack-rs/issues/3077).

A reply is read from the transcript of the conversation the session's record
names, once, after the exit. It is a line of type `assistant` that names the
model that wrote it, is not marked as an API error, and is dated at or after
the moment just before the launch. A session that is working writes one every
turn. The reading is of that one conversation, so a second session running in
the same directory can neither pass a failing session off as working nor keep
its relaunch loop alive.

Claude Code does not document its transcript format. The fields above were
read from Claude Code 2.1.330. If a later version changes them so that no
reply is recognised, every run reads as idle: each standing session is
relaunched once after an exit and refused after the next one, with the
refusal in the pane, the supervisor log and the session's record. It fails
towards stopping, never towards a loop, but it stops sessions that were
working. [#3077](https://github.com/rysweet/amplihack-rs/issues/3077) proposes
that the supervisor agent make this judgement from the transcript's tail
instead.

### Why the size of the transcript is not used

Claude Code writes to the transcript when it does no work, so a start-up crash
loop measured by size would grow the transcript on every start and never be
braked. These are the bytes Claude Code 2.1.330 appended to an existing
conversation's transcript when started with `claude --continue`, recorded on
2026-11-22 in a private tmux server, with the transcript measured before and
after each run:

| Run | Bytes appended | Lines appended | A reply? |
|-----|----------------|----------------|----------|
| Reached the prompt, no turn, then SIGKILL to the session's process (twice) | 839 each time | `bridge-session`, `cost-state` | No |
| Started, no turn | 267 | `bridge-session` | No |
| One turn whose API request failed (`--model` set to a model that does not exist), then SIGKILL | 179,772 | the prompt as `user`, request records, and an `assistant` line with `"model":"<synthetic>"` and `"isApiErrorMessage":true` | No |
| One turn the model answered, then SIGKILL | 18,019 | the prompt, request records, and an `assistant` line naming the model | Yes |
| `--continue` that reported `No conversation found to continue` and exited 1 | 0 | none | No |

`claude --continue --fork-session` resumes under a new conversation id. Ended
before a turn, it wrote no file. After one turn, it wrote a new transcript
that held the earlier turns, each with the timestamp it was first written
with, followed by the new one. The wrapper follows the conversation the
session's record names, and a copied reply is dated before the launch, so a
resume that copies history is not counted as work; only the new turn is.

The lines of the start with no turn, the failed turn and the answered turn are
fixtures under `crates/amplihack-cli/src/commands/agent/testdata/issue_2912/`,
with ids, paths and accounts replaced and the request records left out. The
tests run the wrapper against a stand-in Claude Code that appends them, and
also one that copies the transcript onto itself as a resume under a new id
would. A loop that writes on every start is braked like one that writes
nothing.

A loop that the reply rule does not brake has a model reply in every run,
which needs a prompt sent and answered after each start. `--continue` sends
none, so such a loop is a session doing turns of work and is paced by them;
see [Limits of this feature](#limits-of-this-feature).

A record that names no conversation at all is a third case: the wrapper's
background look for the Claude process and its sessions file found none, or
another active session holds that conversation. There is then nothing of this
session's to read, which is neither evidence that the run worked nor
evidence that it was idle, so the chain rule decides on its own and the
refusal says that there was no conversation on record.

When the trial comes back with nothing either way, the process ran on a host
that could run it and got no reply from the model twice: a configuration it
cannot start under, a `--continue` that resolves to nothing, a crash during
start-up.
Starting it again would repeat it, so the wrapper stops and the failure goes
three places:

- one `refused` line with `reason=no-progress` in the pane and in the
  supervisor log;
- the reason on the session's own registry record, which
  `amplihack agent status` prints as an `ALERT` line;
- the record left `active`, so the
  [supervisor's own recovery](./supervise-agent-sessions.md#what-happens-on-a-terminal-loss)
  is the next thing to try. That recovery has the brake an in-pane relaunch
  must not have: it counts the restarts of the last hour and, at
  `max_restarts`, stops the session with an alert of its own. See
  [Crash loops](./supervise-agent-sessions.md#crash-loops).

A session is therefore never left silently looping, and never left silently
stopped either.

```text
2026-11-21T21:40:07Z standing session refused session=coordinator reason=no-progress
```

```text
ALERT coordinator: not relaunched: the relaunch of an exit that showed nothing itself exited exit-status-1 without a reply from the model in its conversation while a login had room, so another relaunch would repeat it
```

The alert says what was read of the conversation: `without a reply from the
model in its conversation`, or `with no conversation of its own on record`
when there was none to read. It ends with what the room the refusal rests on
was read from:

| Text | Meaning |
|------|---------|
| `while a login had room` | There is no swap record and no rotator to ask, so nothing held the session back for capacity |
| `while the rotator's first reading after the exit found a login with room` | The rotator measured after the exit and found room, so the exit was not a usage limit |
| `while the last swap record showed a login with room and no rotator reading after the exit could be had` | Only a swap record from before the exit showed room: no rotator runs, a login is pinned with `profile switch`, or the rotator stopped before it measured. A usage limit is not ruled out, so the supervisor should check the logins before it restarts the session |

## Before a refusal: the rotator is asked

A `no-progress` refusal rests on a login having had room. When the only
evidence of that is a swap record from before the exit, and a rotator is
running, the wrapper does not refuse yet. It writes one waiting line with
`reason=room-unconfirmed`, watches the rotator's directory, and acts on the
rotator's first cycle that measured after the exit:

| The rotator found | The wrapper |
|-------------------|-------------|
| A swap since the run began onto a login with room, or a `profile switch` after the exit | Relaunches with `waited=room`: the capacity has been replaced |
| The live login with room, and no swap | Refuses with `reason=no-progress`: the exit was not a usage limit |
| No capacity (its no-room alert) | Writes a waiting line with `reason=no-room`, waits until a swap onto a login with room or a cycle that finds room again, and then relaunches with `waited=room` |

So a trial relaunch that goes straight into a usage limit is waited for and
started again, not refused as a crash. The wait follows the rotator's own
cycle, so it normally lasts one rotator interval (`--interval`, 600 seconds by
default). It has no timer of its own and ends at once if the rotator exits.
A cycle whose reading of the live login was taken in the second of the exit or
before it is not used, because it may predate the exit; the next one is. A
reading the rotator took from a status line can be that old even in a later
cycle. The rotator reads the login directly once such a reading is older than
its passive limit (`AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS`, 300 seconds
by default) or one interval, so the wait is then a few cycles.

The wrapper does not wait, and the refusal is made at once on the swap record
alone, when no rotator is running, when a login is pinned with
`amplihack profile switch` (pinned cycles measure nothing), or when the
rotator's directory cannot be watched. If the rotator stops before it has
measured, the refusal is made then. The alert says so in each case, as in the
table above. A cycle already on disk that measured after the exit is still
acted on in these cases: room refuses on that reading, and a swap onto a login
with room relaunches. If the rotator found no capacity after the exit and is
not running, or stops while the wrapper waits for room, the wrapper refuses
with `reason=gone` and `the rotator stopped before one appeared` on the
record.

This also covers a session whose Claude Code never started: it wrote no
conversation, so there is nothing to continue and the same refusal applies
after the one relaunch its first exit is given. Which of the two it was is not
guessed from whether a process scan caught the process, so a session that died
in under a second is judged on what it did, like any other.

## Read the relaunch line

Every relaunch is written to the pane and to the supervisor log, with a UTC
timestamp in the form `YYYY-MM-DDTHH:MM:SSZ`:

```text
2026-11-21T21:40:07Z standing session relaunch session=coordinator reason=exit-status-1 waited=room
```

It names the session, the exit as a status (`exit-status-1`) or a signal name,
and whether the relaunch follows new room (`waited=room`) or not
(`waited=none`). New room is a login with room that the wrapper waited for,
or a swap the rotator made onto a login with room after the run began. A
refusal to relaunch is one line in the same form, with the
word `refused` in place of `relaunch` and one of these reasons:

| Reason | Meaning |
|--------|---------|
| `no-progress` | A relaunch that was given on nothing itself got no reply from the model while a login had room, so another relaunch would repeat it. The reason is also left on the session's record; see [what stops a session that keeps crashing](#what-stops-a-session-that-keeps-crashing) and [before a refusal](#before-a-refusal-the-rotator-is-asked) |
| `gone` | The wait for room ended with no login that has room: the seven days passed, the swap record could not be watched, or the rotator stopped after it found no capacity. The reason is also left on the session's record; see [what happens after a limit](#what-happens-after-a-limit). Also written, without the record, when the wrapper is signalled just before the wait |
| `superseded` | The session record changed while the wrapper waited: the supervisor rebuilt, ended or stopped the session |

```text
2026-11-21T21:40:07Z standing session refused session=coordinator reason=superseded
```

A session held for capacity writes one line in the same form with the word
`waiting`, so a quiet pane can be told apart from a stopped one:

```text
2026-11-21T21:40:07Z standing session waiting session=coordinator reason=no-room
```

A session held for the rotator's reading
[before a refusal](#before-a-refusal-the-rotator-is-asked) writes
`reason=room-unconfirmed` instead:

```text
2026-11-21T21:40:07Z standing session waiting session=coordinator reason=room-unconfirmed
```

Each is written once per wait, after the watch on the rotator's directory is
armed, so a record written from that moment on always ends the wait.

Control characters are stripped from the session name and from the reason. No
arguments, prompt text or environment appear in either line. The tests assert
the line exactly as shown here.

## What is carried into the relaunch

The relaunch rebuilds its Claude Code arguments from the session's registry
record, not from the original arguments: `--continue`, then model, effort,
extra directories, permission mode and whether permissions were skipped. The
conversation id and any first message are left out, because `--continue`
carries on the conversation.

Those arguments go through the same launch path as the first run, in the same
wrapper process, with the same working directory, `--profile` and
environment. So the relaunch has the privilege of the run it continues, no
more and no less: `amplihack claude` adds `--dangerously-skip-permissions` to
every launch for parity with the Python launcher, and it adds it to the
relaunch too, as it does when the supervisor resumes a session. The relaunch
never widens the extra directories.

## Limits of this feature

- There is no count-based limit on relaunches, so a session that is working
  is relaunched as often as it exits, however often that is. What stops a
  session that is not working is
  [the chain rule](#what-stops-a-session-that-keeps-crashing), not a number.
- A session that dies with nothing to show is started again once. If the run
  that relaunch starts does a turn of work, or has to wait for room, its own
  exit is given a relaunch in turn, so a host that kills a working session
  every few hours never runs out of relaunches. If that run dies with nothing
  to show either, the wrapper refuses and the restarts after that are the
  supervisor's, which counts them.
- One shape is still unbounded by the wrapper: a session that does a turn of
  work and then exits every time, or one that exits only when no login has
  room. Both are paced by something real, the work it is doing or the
  rotator's own swap record, and each relaunch is one line in the supervisor
  log. To stop one, close the pane or send `SIGTERM` to the wrapper, as listed
  under
  [What counts as you ending the session](#what-counts-as-you-ending-the-session),
  or run `amplihack config set agent.standing false` and end the session.
- Replies are read from the transcript the session's record names. A
  session whose conversation is never recorded, for instance because the
  configuration directory is not private, is never read as having done
  work, so the chain rule is all that is left: its first exit is relaunched
  and the next one is refused with `reason=no-progress`, with
  `with no conversation of its own on record` in the alert and the cause in
  the supervisor log next to it.
- `--continue` resumes the most recent conversation for the session's working
  directory in its configuration directory. If another session with the same
  working directory and configuration directory was more recently active, the
  relaunch can pick up that one.
- A Claude Code flag the registry does not record is not carried over, as with
  the supervisor's own resume.
- Claude Code exiting `0` for a reason other than you is not relaunched.

## Related

- [Bring Agent Sessions Back After a Terminal Loss](./supervise-agent-sessions.md)
- [`agent.standing`](../reference/user-config.md#agentstanding)
- [No resume cap in the limit-resume hook](../reference/profile-command.md#no-resume-cap)
