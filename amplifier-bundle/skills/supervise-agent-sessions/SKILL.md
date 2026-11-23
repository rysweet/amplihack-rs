---
name: supervise-agent-sessions
description: Read the alerts that amplihack's supervised agent sessions leave on their records and decide what each one needs. Use on every supervisor pass over a host that runs `amplihack agent` sessions, when `amplihack agent status` shows an ALERT, when a standing session keeps exiting and being relaunched in its pane, when a session was left `not relaunched`, or when the supervisor service has stopped an agent at its restart limit. Judgement from evidence by an agent, not a script.
metadata:
  version: "1.0"
  author: amplihack
---

# Supervise Agent Sessions

## Purpose

amplihack keeps registered sessions running without judging them. The pane of
a standing session relaunches every exit its owner did not cause, with no
count and no timer, and `amplihack agent supervise` resumes a session whose
pane and process are both gone. Neither decides whether a session that keeps
exiting is a crash loop, and neither tells anyone that a session has been left
stopped. Each leaves its evidence on the session's record instead.

This skill is the step that reads that record and decides. Run it on every
supervisor pass.

Guides: `docs/howto/keep-sessions-running.md` for standing sessions and the
record of their relaunches, and `docs/howto/supervise-agent-sessions.md` for
the supervisor service, its log and its restart limit.

## When I Activate

- A supervisor pass over a host that runs `amplihack agent` sessions.
- `amplihack agent status` prints an `ALERT` line, or an agent's `alert` field
  in `amplihack agent status --json` is set.
- A pane shows `standing session relaunch` or `standing session refused`
  lines.
- The owner asks whether a session is stuck, looping or stopped.

## Rules

- Decide from evidence: the record, the supervisor log, the pane and the tail
  of the transcript. Never stop a session on the number of relaunches alone.
- Do not write a script, a counter or a timer that stops sessions for you. A
  count that stopped a session is the fault this design removed (issue
  #2912).
- Stop a session only as set out in [Stop a crash loop](#stop-a-crash-loop).
- Put every decision in the pass's report with the evidence it rests on.

## Each pass

1. Read every agent's state and alert:

   ```bash
   amplihack agent status --json
   ```

   Each agent has `state`, `alert`, `last_recovery`, `restarts_last_hour`,
   `conversation_id`, `tmux_session` and `tmux_window`. An agent with no
   `alert` needs nothing from this skill.

2. Match the start of each alert to a row, and follow the row:

   | Alert begins with | What it records | Next |
   |-------------------|-----------------|------|
   | `relaunched in its pane at` | The pane relaunched the session after an exit its owner did not cause. The record holds the latest relaunch; the supervisor log holds every one | [Judge the relaunches](#judge-the-relaunches) |
   | `not relaunched: the launch failed without an exit status` | `launch-failed`: the pane could not start Claude Code at all, so it stopped and left the session `active` | [Bring back a session left stopped](#bring-back-a-session-left-stopped) |
   | `not relaunched: it exited` | `gone`: no login had room when the session exited, and the wait for one ended without it | [Bring back a session left stopped](#bring-back-a-session-left-stopped) |
   | `stopped: restart limit` | The supervisor service resumed the session 3 times within an hour and has stopped resuming it | Find the cause as in "Crash loops" in `docs/howto/supervise-agent-sessions.md`, then re-arm it |

3. Report, for each agent with an alert, its name, the alert, what you read,
   the decision and the action taken.

## Judge the relaunches

A relaunch record ends in one of two ways:

- `the model replied during the run`: the session did a turn of work before
  it exited. Leave it running.
- `<n> relaunches since <time> without a reply from the model in its
  conversation`: the relaunches since the model last replied, this one
  included, and when the first of them was made.

Read the session's lines in the supervisor log, and its pane:

```bash
grep " session=NAME " ~/.amplihack/agents/supervisor.log | tail -n 30
tmux capture-pane -p -t "TMUX_SESSION:TMUX_WINDOW" -S -80
```

Then decide:

| What the evidence shows | Likely cause | Decision |
|-------------------------|--------------|----------|
| Many relaunches within minutes, `waited=none` on each line, no reply | A start-up crash loop | [Stop it](#stop-a-crash-loop), find the cause, and fix it or hand it to the owner |
| A few relaunches hours apart, `SIGKILL` or `exit-status-137`, no reply | The host kills the session while it is idle, for example the out-of-memory killer | Leave it running, and report the host's cause |
| `waited=room` on the relaunch lines | Usage limits | Leave it running; check the logins with the `claude-profile-rotation` skill |
| `the model replied during the run` | A working session that exits after its work | Leave it running; read the pane for the exit's cause if it repeats |

When the evidence does not settle it, leave the session running and compare
the record at the next pass. A count that has climbed steeply since then, with
no reply, is a crash loop.

The reply is read from the transcript format of one Claude Code version. If
the pane or the transcript's tail shows the session working while the record
says it has no reply, trust the pane and the transcript; see "How a reply is
recognised" in `docs/howto/keep-sessions-running.md`.

## Stop a crash loop

Unregistering removes the record, so first copy its recorded options into
the report, with the `jq` command under "Crash loops" in
`docs/howto/supervise-agent-sessions.md`. Then:

```bash
amplihack agent unregister NAME
```

The pane refuses its next relaunch with `reason=superseded`, and the
supervisor service no longer resumes the session. Name the cause from the pane
in the report. Once it is fixed, start the session again in its pane with
`amplihack claude --supervise NAME` and the recorded options.

## Bring back a session left stopped

A `not relaunched` alert means the pane stopped without relaunching and left
the session `active`. The supervisor service resumes it at its next pass once
the pane has closed and no Claude Code runs its conversation. A wrapper that
starts clears the alert, so an alert still there after a pass means that
nothing has started the session since. Check, in this order:

1. The service is running:
   `systemctl --user is-active amplihack-agent-supervisor.service`. If it is
   not, start it, or run one pass by hand as in "Run one pass by hand" in
   `docs/howto/supervise-agent-sessions.md`.
2. `last_recovery.result` is `skipped: tmux session exists`. The pane was kept
   open, usually by a shell, so the service will not resume into it. Start the
   session again in that window, or close the tmux session so that the next
   pass resumes it.
3. For `launch-failed`, read the error in the pane first. A resume before the
   cause is fixed fails in the same way, and counts towards the service's
   restart limit of 3 in an hour.
4. For `gone`, check the logins with `amplihack profile usage` and the
   `claude-profile-rotation` skill. A session resumed while no login has room
   waits for one again.

If none of these brings the session back, hand it to the owner with the alert,
the log lines and the pane.
