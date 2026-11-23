---
type: how-to
---

# How to Keep a Session Running After Its Process Exits

A standing session is an interactive Claude Code session that should always be
running. If its process exits for any reason other than you ending it, for
example when a usage limit stopped it and the process went with it, amplihack
starts it again in the same pane with `claude --continue`, so the conversation
carries on.

Every such exit is relaunched, however often it happens. The wrapper keeps no
count that can stop a session and has no brake. It writes each relaunch to the
pane, to the supervisor log and to the session's own record, with the number of
relaunches since the model last replied. Whether a session that keeps exiting
is a crash loop, and whether to stop it, is for you or the supervisor agent to
judge from that evidence:
[when a session keeps exiting](#when-a-session-keeps-exiting). The agent step
that reads the record and decides is the bundled `supervise-agent-sessions`
skill: [who reads the record](#who-reads-the-record).

## Contents

- [Mark a session as standing](#mark-a-session-as-standing)
- [What counts as you ending the session](#what-counts-as-you-ending-the-session)
- [What happens after a limit](#what-happens-after-a-limit)
- [When a session keeps exiting](#when-a-session-keeps-exiting)
  - [Who reads the record](#who-reads-the-record)
- [Stop a session that keeps exiting](#stop-a-session-that-keeps-exiting)
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
| Claude Code exits non-zero, or is killed by a signal | Not you | Relaunched, once a login has room |
| The launch fails with an error that is not an exit status of Claude Code, for example because `claude` cannot be found | No process exited | No relaunch; the session is left for the supervisor with `reason=launch-failed` |
| The wrapper receives `SIGHUP` or `SIGTERM` | The pane or host is going away | No relaunch |

A marker older than the run is an earlier exit of the same conversation that
the supervisor has not removed yet, and does not stop the relaunch. A session
that was not marked standing is never relaunched by this feature.

A launch that fails without an exit status started no process, so there is no
exit to answer, and a relaunch would fail the same way within moments with no
Claude Code start to pace it. The error is printed in the pane, a `refused`
line with `reason=launch-failed` goes to the pane and the supervisor log, and
the session's record gets an `ALERT` and stays `active`, so the
[supervisor's own recovery](./supervise-agent-sessions.md#what-happens-on-a-terminal-loss)
is the next thing to try. Who acts on it is set out under
[who reads the record](#who-reads-the-record):

```text
ALERT coordinator: not relaunched: the launch failed without an exit status from Claude Code, so no process exited to be relaunched; the error is in the pane, and the session was left for the supervisor
```

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
  relaunch. It writes a `refused` line with `reason=gone` in the pane and the
  supervisor log, puts the cause on the session's record as an `ALERT`, and
  leaves the record `active`, so the supervisor's recovery starts the session
  again; see [who reads the record](#who-reads-the-record). If that run also
  exits while no login has room, its wrapper waits again.
- The session record changed during the wait, because the supervisor rebuilt
  the session in another window, or ended, stopped or unregistered it: the
  wrapper logs one refusal with `reason=superseded` and does not relaunch. Two
  panes never run the same session.

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
show a limit the run has just met. The wrapper relaunches on it all the same,
so a session killed for any other reason is not held back. `claude --continue`
sends no prompt, so a session relaunched into a limit waits at its prompt
rather than meeting the limit again.

## When a session keeps exiting

The wrapper does not decide whether a session that keeps exiting should be
stopped. It has no count of attempts, no retry limit, no delay and no timer.
Every process exit you did not cause is relaunched once a login has room.
An earlier version of this feature stopped a chain after one relaunch with
nothing to show; that was a retry count of one, the same kind of stop that
held a supervisor session for fourteen hours.

Instead, each relaunch is recorded where the supervisor agent reads it:

- a `relaunch` line in the pane and in the supervisor log, one per relaunch
  and with its time, so the log shows how often the session exits;
- the record of the relaunch on the session's registry entry, which
  `amplihack agent status` prints as an `ALERT` line and `status --json` gives
  in the agent's `alert` field.

```text
ALERT coordinator: relaunched in its pane at 2026-11-21T21:40:07Z after it exited exit-status-1; 212 relaunches since 2026-11-21T21:31:55Z without a reply from the model in its conversation
```

| Part | Meaning |
|------|---------|
| `relaunched in its pane at <time>` | When the relaunch was made |
| `after it exited <reason>` | The exit, as `exit-status-<n>` or a signal name such as `SIGKILL` |
| `and a login with room was waited for` | Present when no login had room at the exit and the wrapper waited for one |
| `the model replied during the run` | In place of the count: the run before this relaunch did a turn of work |
| `<n> relaunches since <time> without a reply from the model in its conversation` | The relaunches since the model last replied, this one included, and when the first of them was made |

Each relaunch replaces the record of the one before; the supervisor log keeps
every line. Nothing in the wrapper reads the count. A reply ends the chain, and
the next relaunch without one starts a new chain at 1.

### Who reads the record

Two readers act on what a standing session leaves behind. Neither is the
wrapper.

| Record | Reader | What the reader does |
|--------|--------|----------------------|
| A `not relaunched: …` alert, from `launch-failed` or `gone`, on a session left `active` | `amplihack agent supervise`, the supervisor service | Resumes the session at its next pass once the pane has closed and no Claude Code runs its conversation, within its limit of 3 resumes in an hour |
| The same alert, still there after a pass | The `supervise-agent-sessions` skill | Finds why nothing started the session: the service is not running, the pane was kept open by a shell (`skipped: tmux session exists`), or the cause of the failed launch is still there. It starts the session again or hands it to the owner |
| A `relaunched in its pane at …` alert and the `relaunch` lines in the log | The `supervise-agent-sessions` skill | Judges from the record, the log, the pane and the transcript whether the session is a crash loop, and [stops it](#stop-a-session-that-keeps-exiting) if it is |

The skill is bundled with amplihack under
`amplifier-bundle/skills/supervise-agent-sessions/` and is installed with the
other bundled skills. A supervisor agent runs it on every pass. It is an
agent's judgement, so it holds no count and no timer either.

Tests tie the readers to what the wrapper writes. In
`crates/amplihack-cli/tests/issue_2912_standing_relaunch.rs`, every alert a
run of the wrapper leaves on the record is checked against the skill's table
of alert forms, and a session left `active` by `launch-failed` or `gone` is
resumed by one pass of the supervisor service.
`crates/amplihack-cli/tests/issue_2912_supervisor_skill.rs` checks the skill
itself and that these guides name it.

When no supervisor agent runs the skill, nothing reads the record of the
relaunches. A crash loop then runs until a person reads
`amplihack agent status`, and a session left stopped in a pane that a shell
keeps open stays stopped. On a host with standing sessions, run the skill on
every supervisor pass, or read the status yourself.

### Read the evidence

The record is evidence, not a verdict. Read it together with the supervisor
log, the pane and the transcript's tail:

| What the evidence shows | Likely cause |
|-------------------------|--------------|
| Many relaunches within minutes, `waited=none` on each line, no reply | A start-up crash loop: a configuration the session cannot start under, a `--continue` that finds no conversation to continue, or a crash during start-up |
| A few relaunches hours apart, `SIGKILL` or `exit-status-137`, no reply | A host that kills the session while it sits idle, for example the out-of-memory killer. Starting it again is what the session needs |
| `waited=room` on the relaunch lines | Usage limits. Each relaunch followed a login with room |
| `the model replied during the run` | The session is doing work and exits after it |

A start-up crash loop needs its cause fixed; relaunching will not fix it. A
session the host keeps killing needs to keep running, which is the case this
feature was built for. The two can look alike in a count alone, which is why
the decision is left to a reader that can look at the pane, the log and the
transcript.

### How a reply is recognised

A reply is read from the transcript of the conversation the session's record
names, once, after the exit. It is a line of type `assistant` that names the
model that wrote it, is not marked as an API error, and is dated at or after
the moment just before the launch. A session that is working writes one every
turn. The reading is of that one conversation, so a second session running in
the same directory cannot pass a failing session off as working.

When the record names no conversation, there is nothing to read, and the
relaunch is recorded as without a reply. `amplihack agent status` then shows
`-` in the session's `CONVERSATION` column. This happens when the wrapper's
background look for the Claude process and its sessions file found none, or
when another active session holds that conversation.

Claude Code does not document its transcript format. The fields above were
read from Claude Code 2.1.330. If a later version changes them so that no reply
is recognised, every relaunch is recorded as without a reply, and a working
session reads like a crash loop. Nothing is stopped on that account, because
the wrapper relaunches either way, but read the transcript's tail before you
stop a session on the strength of this record.

### Why the size of the transcript is not used

Claude Code writes to the transcript when it does no work, so a start-up crash
loop measured by size would look like work on every start. These are the bytes
Claude Code 2.1.330 appended to an existing conversation's transcript when
started with `claude --continue`, recorded on 2026-11-22 in a private tmux
server, with the transcript measured before and after each run:

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
resume that copies history is not counted as a reply; only the new turn is.

The lines of the start with no turn, the failed turn and the answered turn are
fixtures under `crates/amplihack-cli/src/commands/agent/testdata/issue_2912/`,
with ids, paths and accounts replaced and the request records left out. The
tests run the wrapper against a stand-in Claude Code that appends them, and
also one that copies the transcript onto itself as a resume under a new id
would. A loop that writes on every start is recorded as without a reply, like
one that writes nothing.

## Stop a session that keeps exiting

When you or the supervisor agent decide that a session is a crash loop, stop
it in one of these ways:

- Remove its record with `amplihack agent unregister NAME`. The pane refuses
  its next relaunch with `reason=superseded`, so the session stops when its
  current run exits, and the supervisor no longer recovers it. Once the cause
  is fixed, start it again with `amplihack claude --supervise NAME` in its
  pane.
- Close the pane, or send `SIGTERM` to the wrapper. The wrapper and its Claude
  Code end and nothing is relaunched. The record stays `active`, so
  `amplihack agent supervise`, if it runs, may start the session again on a
  later pass. Unregister it as well if it must stay stopped.
- Run `amplihack config set agent.standing false`. This applies to wrappers
  started after the change, including the supervisor's next recovery; a
  wrapper that is already running keeps the value it read at its start.

## Read the relaunch line

Every relaunch is written to the pane and to the supervisor log, with a UTC
timestamp in the form `YYYY-MM-DDTHH:MM:SSZ`:

```text
2026-11-21T21:40:07Z standing session relaunch session=coordinator reason=exit-status-1 waited=room
```

It names the session, the exit as a status (`exit-status-1`) or a signal name,
and whether the relaunch followed a wait for a login with room (`waited=room`)
or not (`waited=none`). A refusal to relaunch is one line in the same form,
with the word `refused` in place of `relaunch` and one of these reasons:

| Reason | Meaning |
|--------|---------|
| `launch-failed` | The launch returned an error that is not an exit status of Claude Code, so no process exited. The error is in the pane and the reason is on the session's record; see [what counts as you ending the session](#what-counts-as-you-ending-the-session) |
| `gone` | The wait for room ended with no login that has room: the seven days passed, or the swap record could not be watched. The reason is also left on the session's record; see [what happens after a limit](#what-happens-after-a-limit). Also written, without the record, when the wrapper is signalled just before the wait |
| `superseded` | The session record changed: the supervisor rebuilt, ended, stopped or unregistered the session |

```text
2026-11-21T21:40:07Z standing session refused session=coordinator reason=superseded
```

A session held for capacity writes one line in the same form with the word
`waiting`, so a quiet pane can be told apart from a stopped one:

```text
2026-11-21T21:40:07Z standing session waiting session=coordinator reason=no-room
```

It is written once per wait, after the watch on the swap record is armed, so a
record written from that moment on always ends the wait.

Control characters are stripped from the session name and from the reason. No
arguments, prompt text or environment appear in any of these lines, or in the
record of a relaunch. The tests assert the lines exactly as shown here.

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

- There is no limit on relaunches. A session that cannot start is relaunched
  as fast as Claude Code can start and exit, with one line in the supervisor
  log and one registry write for each relaunch, until you or the supervisor
  agent [stop it](#stop-a-session-that-keeps-exiting). Only a launch that
  fails before any process exits is not relaunched, because nothing would pace
  that loop. The loop is noticed only by a reader of the record: the
  `supervise-agent-sessions` skill on a supervisor pass, or you; see
  [who reads the record](#who-reads-the-record).
- The count of relaunches without a reply rests on the transcript format of
  Claude Code 2.1.330. A change to that format makes a working session read as
  one without replies; see [how a reply is recognised](#how-a-reply-is-recognised).
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
