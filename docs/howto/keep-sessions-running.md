---
type: how-to
---

# How to Keep a Session Running After Its Process Exits

A standing session is an interactive Claude Code session that should always be
running. If its process exits for any reason other than you ending it, for
example when a usage limit stopped it and the process went with it, amplihack
starts it again in the same pane with `claude --continue`, so the conversation
carries on.

## Contents

- [Mark a session as standing](#mark-a-session-as-standing)
- [What counts as you ending the session](#what-counts-as-you-ending-the-session)
- [What happens after a limit](#what-happens-after-a-limit)
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
| Claude Code exits non-zero, or fails to run | Not you | Relaunch |
| The wrapper receives `SIGHUP` or `SIGTERM` | The pane or host is going away | No relaunch |
| Claude Code never started | Nothing to continue | No relaunch, and a line says so |

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
  wait is bounded at seven days. If no login has room by then, the wrapper
  logs a refusal with `reason=gone` and does not relaunch.
- The session record changed during the wait, because the supervisor rebuilt
  the session in another window, or ended or stopped it: the wrapper logs one
  refusal with `reason=superseded` and does not relaunch. Two panes never run
  the same session.

During the wait, `SIGHUP`, `SIGTERM` and `SIGINT` have their default action,
so closing the pane or pressing Ctrl-C ends the waiting wrapper and nothing is
relaunched.

The room rule is the one the
[`limit-resume` hook](../reference/hook-specifications.md) uses. That hook
answers the usage-limit menu in a running session. The wrapper covers the case
where the process has gone.

## Read the relaunch line

Every relaunch is written to the pane and to the supervisor log, with a UTC
timestamp in the form `YYYY-MM-DDTHH:MM:SSZ`:

```text
2026-11-21T21:40:07Z standing session relaunch session=coordinator reason=exit-status-1 waited=room
```

It names the session, the exit as a status (`exit-status-1`) or a signal name,
and whether the wrapper waited for room (`waited=room`) or not
(`waited=none`). A refusal to relaunch is one line in the same form, with the
word `refused` in place of `relaunch` and one of these reasons:

| Reason | Meaning |
|--------|---------|
| `never-started` | Claude Code never started, so there is nothing to continue |
| `gone` | The seven-day wait for room ended with no login that has room |
| `superseded` | The session record changed while the wrapper waited: the supervisor rebuilt, ended or stopped the session |

```text
2026-11-21T21:40:07Z standing session refused session=coordinator reason=superseded
```

A session held for capacity writes one line in the same form with the word
`waiting`, so a quiet pane can be told apart from a stopped one:

```text
2026-11-21T21:40:07Z standing session waiting session=coordinator reason=no-room
```

It is written once per wait, after the watch on the swap record is armed, so
a record written from that moment on always ends the wait.

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

- There is no count-based limit on relaunches. The owner-ending rules, the
  refusal for a session that never started, and the wait for room still apply.
  A session that starts and then exits at once for a cause other than
  capacity, such as a broken configuration, is relaunched each time while a
  login has room, or while there is no swap record the wrapper can use (no
  rotator, or a record that is refused or cannot be parsed). The relaunch
  lines in the supervisor log show it. To stop the loop, close the pane or
  send `SIGTERM` to the wrapper, as listed under
  [What counts as you ending the session](#what-counts-as-you-ending-the-session),
  then fix the cause before you start the session again.
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
