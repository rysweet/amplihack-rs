# How to Bring Agent Sessions Back After a Terminal Loss

Long-lived Claude Code sessions, such as a coordinator and its area
controllers, usually run inside one tmux server. When that server ends, every
session in it ends too. `amplihack agent` keeps a registry of these sessions,
and a supervisor service checks them every minute. When a registered
session's `claude` process and its tmux session are both gone, the supervisor
creates the tmux session again and resumes the session from its own saved
conversation, in the same directory and with the same profile, model, effort,
directories and permissions. The resumed session is told when and why it was
resumed. Each recovery is one line in a log and in `amplihack agent status`.

The exact flags, file formats, messages and exit codes are in the
[`amplihack agent` reference](../reference/agent-command.md).

## Contents

- [Before you start](#before-you-start)
- [Run the supervisor service](#run-the-supervisor-service)
- [Register sessions](#register-sessions)
- [Register the coordinator and its areas](#register-the-coordinator-and-its-areas)
- [Check what is registered](#check-what-is-registered)
- [What happens on a terminal loss](#what-happens-on-a-terminal-loss)
- [Clean exits stay ended](#clean-exits-stay-ended)
- [Crash loops](#crash-loops)
- [Run one pass by hand](#run-one-pass-by-hand)
- [Change the timing](#change-the-timing)
- [Stop supervising a session](#stop-supervising-a-session)
- [Unattended resumes and other users](#unattended-resumes-and-other-users)
- [Limits](#limits)

## Before you start

You need:

- Linux. The supervisor reads `/proc` to find running `claude` processes. On
  other systems `amplihack agent supervise` exits without starting anything.
- tmux, git and Claude Code on the `PATH` that the service uses.
- Each long-lived session in **its own tmux session**. The supervisor
  recreates one tmux session per agent, and leaves an agent alone while its
  tmux session exists. Two active agents cannot share a tmux session.
- systemd user services, to run the supervisor as a service. Without them,
  run `amplihack agent supervise` in a terminal of its own, outside the tmux
  server it supervises.
- `~/.amplihack`, `~/.claude` and every profile directory owned by you and
  not writable by other users. Group write is accepted when the group is your
  own private group, which is the usual layout with a umask of `002`.
- amplihack's `SessionEnd` hook, installed by `amplihack install` or by the
  [amplihack Claude Code plugin](install-claude-code-plugin.md), so that a
  session you register while it runs is not resumed after you leave it with
  `/exit`. See [Clean exits stay ended](#clean-exits-stay-ended).

Do not run any `amplihack agent` command, or `amplihack claude --supervise`,
under `sudo`. They refuse files that the user they run as does not own.

## Run the supervisor service

amplihack does not install the service. Write the unit yourself.

1. Find the programs the supervisor runs:

   ```bash
   command -v amplihack tmux git claude
   ```

   ```text
   /home/alex/.local/bin/amplihack
   /usr/bin/tmux
   /usr/bin/git
   /home/alex/.local/bin/claude
   ```

2. Create `~/.config/systemd/user/amplihack-agent-supervisor.service`:

   ```ini
   [Unit]
   Description=amplihack agent supervisor

   [Service]
   ExecStart=%h/.local/bin/amplihack agent supervise
   Environment=PATH=%h/.local/bin:/usr/local/bin:/usr/bin:/bin
   Restart=always
   RestartSec=10
   KillMode=process

   [Install]
   WantedBy=default.target
   ```

   Change `ExecStart` if `amplihack` is installed somewhere else, and make
   `PATH` list every directory from step 1. systemd replaces `%h` with your
   home directory. Do not add credentials, sandboxing settings such as
   `PrivateTmp=` or `ProtectHome=`, or resource limits such as `MemoryMax=`:
   every agent the supervisor resumes inherits them. The
   [reference](../reference/agent-command.md#service-unit) gives the reasons.

3. Load and start it:

   ```bash
   systemctl --user daemon-reload
   systemctl --user enable --now amplihack-agent-supervisor.service
   ```

4. Let your user services run without a login session, so the supervisor
   starts at boot and keeps running after you log out:

   ```bash
   loginctl enable-linger "$USER"
   ```

   Without lingering, systemd starts the supervisor when you log in, and at
   logout it stops the supervisor and every agent the supervisor resumed.

`KillMode=process` keeps resumed agents alive when the service stops or
restarts. The tmux servers that the supervisor starts belong to the service's
control group, and systemd's default would end them with it. Recent systemd
versions log a warning about `KillMode=process` when they load the unit; the
warning is expected.

The login rotator has the same need to outlive a tmux server. Issue #1759
covers running it as its own user service, `amplihack-rotator.service`, and an
installer shared by both units belongs with that work. The two units are
independent: either runs without the other.

## Register sessions

There are three ways to put a session in the registry. Each records the
session under a NAME of your choice: letters, digits, `.`, `_` and `-`,
starting with a letter or digit, at most 64 characters.

### Start a session under supervision

Create a tmux session for the agent, then start Claude Code in it with
`--supervise NAME`:

```bash
tmux new-session -s coordinator -n main -c ~/src/mistt
```

In the new tmux session:

```bash
amplihack claude --profile work --supervise coordinator -- --model opus --effort high
```

Before Claude Code starts, `--supervise` records:

- the tmux socket, session and window;
- the current directory, which is the directory Claude Code finds the
  conversation by;
- the Claude Code options after `--`: `--model`, `--effort`, each
  `--add-dir`, and `--permission-mode` or `--dangerously-skip-permissions`;
- `--profile`, `--subprocess-safe` and `--no-reflection`;
- the [model and effort variables](../reference/agent-command.md#recorded-environment)
  set in your environment, such as `ANTHROPIC_MODEL`.

Then Claude Code starts as `amplihack claude` always starts it. The
conversation id is recorded as soon as Claude Code writes its session file,
usually within seconds. With `--resume ID` after `--`, it is recorded at
once.

`--supervise` needs Linux and tmux. It refuses `-p` and `--print`, and it
cannot be combined with `--auto`, `--append`, `--docker` or `--checkout-repo`.

Every session the supervisor resumes runs with `--supervise`, so after its
first recovery an agent's clean exits are always recorded.

### Register a session that is already running

`amplihack agent register NAME` reads a running `claude` process and records
it. From inside the session, use Claude Code's `!` prefix:

```text
! amplihack agent register coordinator
```

```text
registered coordinator: session coordinator, window main, conversation 0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c, directory /home/alex/src/mistt
```

From any other shell, name the tmux pane the session runs in:

```bash
amplihack agent register area-search --pane area-search:main
```

`--pane` asks the tmux server of the shell you run it in, or tmux's default
server outside tmux. For a pane on another server, add
`--tmux-socket` with that server's socket.

or the `claude` process id, if you know it:

```bash
amplihack agent register area-search --pid 41237
```

`register` reads the conversation id and directory from the process's session
file, the Claude Code options from its command line, `--profile`,
`--subprocess-safe` and `--no-reflection` from the `amplihack claude` that
started it, and the tmux names from tmux. A flag you give replaces the value
it found. A `warning:` line on stderr means a value was found but not
recorded; read it before you rely on the registration.

### Register a session that is already gone

With no running process, give the tmux names, the directory and the
conversation id yourself, with the options the session used:

```bash
amplihack agent register area-reports \
  --tmux-socket /tmp/tmux-1000/default --session area-reports --window main \
  --dir /home/alex/src/reports \
  --conversation 6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36 \
  --profile work --model opus --effort high
```

- **Conversation id.** Each `<id>.jsonl` in the project's transcript
  directory is one conversation. The directory is under `~/.claude/projects/`,
  or under the profile's directory for a session that used `--profile`, and
  is named after the launch directory with `/` and `.` replaced by `-`. The
  most recently changed transcript is usually the one you want:

  ```bash
  ls -t ~/.claude/projects/-home-alex-src-reports/ | head -3
  ```

- **Socket.** `--tmux-socket` is the socket of the tmux server the session ran
  on. Inside any tmux session on that server,
  `tmux display-message -p '#{socket_path}'` prints it. Without
  `--tmux-socket`, the supervisor uses tmux's default server for its own
  environment.
- **Environment.** Only the variables you give with `--env KEY=VALUE` are
  recorded.

You can run this from inside another Claude session, such as the
coordinator's. `register` uses the session it runs in only when that
session's conversation is the one `--conversation` names. Here it is not, so
`register` records no process and takes nothing from the coordinator: the
area agent's entry holds only the values you gave.

The supervisor's next pass finds the agent lost and resumes it.

## Register the coordinator and its areas

A typical layout has a coordinator, a writer, a custodian and several area
controllers, each in its own tmux session named after the agent, with one
window called `main`. Register the coordinator from inside its session:

```text
! amplihack agent register coordinator
```

Then register the others from any shell, including the coordinator's:

```bash
for a in writer custodian area-billing area-docs area-ingest area-reports area-search; do
  amplihack agent register "$a" --pane "$a:main"
done
```

Each conversation and each tmux session belongs to at most one active agent.
`register` refuses a second registration with
`this conversation is already used by registered agent OTHER` or
`tmux session S is already used by registered agent OTHER`.

Registering a NAME again replaces its record and clears its restart history
and any alert. Do this after you move a session to another tmux session or
directory. Start new sessions with
[`--supervise`](#start-a-session-under-supervision) instead.

## Check what is registered

```bash
amplihack agent status
```

```text
NAME          STATE   CONVERSATION                          DIRECTORY               LAST RECOVERY
area-billing  active  5c2f8e1a-7d3b-4a9e-b6c4-1e8f2a7d3c95  /home/alex/src/billing  -
area-docs     active  2b7e4f1c-8d3a-4c6e-9f2b-5a1d7e3c8b64  /home/alex/src/docs     -
area-ingest   active  9a4e7c21-5b3d-4e8f-a1c6-3f2d8b7e6a90  /home/alex/src/ingest   -
area-reports  active  6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36  /home/alex/src/reports  -
area-search   active  e7b1d4c9-2a6f-4e3b-8d5c-9f1a7e2b6d48  /home/alex/src/search   -
coordinator   active  0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c  /home/alex/src/mistt    -
custodian     active  3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57  /home/alex/src/mistt    -
writer        active  b4e9a2d7-6c1f-4d8b-a3e5-7f2c9d6b1a84  /home/alex/src/mistt    -
```

`status` reads the registry only; it checks no processes and no tmux
sessions. A conversation shown as `-` has not been found yet. For scripts,
`amplihack agent status --json` prints the same records with the tmux names,
the restart count for the last hour, the alert and the end time. The fields
are in the [reference](../reference/agent-command.md#status).

## What happens on a terminal loss

Say the tmux server ends at 23:27:09. At its next pass, 31 seconds later, the
supervisor finds that the `claude` processes and the tmux sessions of all
eight agents are gone.

### What is resumed

The supervisor resumes an agent when all of these hold:

- the agent is `active`, not `ended` or `stopped`;
- no `claude` process of yours is running its conversation, under
  `~/.claude` or under any profile directory;
- its tmux session does not exist;
- it has a conversation id, its launch directory exists, and the
  conversation's transcript is in the config directory the resume uses (the
  recorded profile's directory, or `~/.claude`);
- it has been resumed fewer than 3 times in the last hour.

For each one, it creates the tmux session and window again in the recorded
directory, starting the tmux server if none is running. In the window it runs
`amplihack claude --supervise NAME` with the recorded profile and flags,
`--resume` with the conversation id, and the recorded Claude Code options. The
resumed session's first message is:

> amplihack agent supervisor: the terminal session for this agent was lost at
> 2026-10-29T23:27:40Z. This conversation was resumed at 2026-10-29T23:27:41Z.
> Before continuing, check what your interrupted step had already done (git
> status, open pull requests, running processes) and do not repeat finished
> steps.

The first time is when the supervisor first found the agent gone. That is
usually within one interval of the loss. A loss that happens while a pass is
still resuming later batches is found by the next pass, after that pass ends
and the interval has passed.

### Order and batches

Agents with uncommitted changes to tracked files in their launch directory go
first, most changed lines first; the rest follow by name. The supervisor
resumes 6 agents, waits 60 seconds, resumes the next 6, and so on, so that a
mass restart does not use up a login's capacity at once. Before each resume it
checks the agent again, in case you resumed the session yourself during the
wait.

Here `area-billing` and `coordinator` had uncommitted work, and `custodian`
and `writer` were in the second batch:

```text
2026-10-29T23:27:41Z agent=area-billing conversation=5c2f8e1a-7d3b-4a9e-b6c4-1e8f2a7d3c95 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:41Z result=resumed
2026-10-29T23:27:41Z agent=coordinator conversation=0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:41Z result=resumed
2026-10-29T23:27:42Z agent=area-docs conversation=2b7e4f1c-8d3a-4c6e-9f2b-5a1d7e3c8b64 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:42Z result=resumed
2026-10-29T23:27:42Z agent=area-ingest conversation=9a4e7c21-5b3d-4e8f-a1c6-3f2d8b7e6a90 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:42Z result=resumed
2026-10-29T23:27:43Z agent=area-reports conversation=6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:43Z result=resumed
2026-10-29T23:27:43Z agent=area-search conversation=e7b1d4c9-2a6f-4e3b-8d5c-9f1a7e2b6d48 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:27:43Z result=resumed
2026-10-29T23:28:43Z agent=custodian conversation=3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:28:43Z result=resumed
2026-10-29T23:28:44Z agent=writer conversation=b4e9a2d7-6c1f-4d8b-a3e5-7f2c9d6b1a84 lost=2026-10-29T23:27:40Z resumed=2026-10-29T23:28:44Z result=resumed
```

Attach to a resumed session as usual:

```bash
tmux attach -t coordinator
```

### What is left alone

The supervisor never starts a conversation that is already running, and never
creates a tmux session that already exists. A healthy agent, with its
conversation running and its tmux session present, writes nothing. Otherwise
the pass writes one of these lines:

| Result | Meaning | What to do |
|--------|---------|------------|
| `skipped: already running` | The conversation is running in a `claude` process outside the agent's tmux session. | Nothing, if you resumed it elsewhere on purpose. Register the agent again for its new tmux session. |
| `skipped: tmux session exists` | The tmux session is there, but no `claude` process runs the conversation. Claude Code may have exited and left a shell in the window. | Restart the session in that window, or close the tmux session and the next pass resumes it. |
| `skipped: already running (liveness unknown: REASON)` | The supervisor could not tell whether the conversation is running, so it assumes it is. | Fix REASON, for example a Claude config directory that other users can write to. |
| `skipped: tmux session exists (tmux state unknown: REASON)` | tmux did not answer clearly, so the supervisor assumes the session exists. | Fix REASON. `socket directory is not private` is explained in [Unattended resumes and other users](#unattended-resumes-and-other-users). |
| `failed: no conversation id` | The agent has no conversation id to resume. | Register it again with `--conversation`. |
| `failed: launch directory missing` | The recorded directory no longer exists. | Restore the directory, or register the agent again with `--dir`. |
| `failed: no transcript` | The conversation's transcript is not in the config directory the resume would use. | Check the recorded profile, or register the agent again with the right `--profile`. |
| `failed: tmux: MESSAGE` | tmux refused to create the session. | Read MESSAGE. The attempt does not count towards the restart limit. |

Each of these lines is written once. The same result for the same agent and
conversation is not written again at every pass; a resume is always written.

### Read the log

Every line goes to `~/.amplihack/agents/supervisor.log` and to the
supervisor's output, which the journal keeps:

```bash
journalctl --user -u amplihack-agent-supervisor.service -f
```

`amplihack agent status` shows each agent's latest line in its
`LAST RECOVERY` column:

```text
NAME          STATE   CONVERSATION                          DIRECTORY               LAST RECOVERY
area-billing  active  5c2f8e1a-7d3b-4a9e-b6c4-1e8f2a7d3c95  /home/alex/src/billing  2026-10-29T23:27:41Z resumed
area-docs     active  2b7e4f1c-8d3a-4c6e-9f2b-5a1d7e3c8b64  /home/alex/src/docs     2026-10-29T23:27:42Z resumed
area-ingest   active  9a4e7c21-5b3d-4e8f-a1c6-3f2d8b7e6a90  /home/alex/src/ingest   2026-10-29T23:27:42Z resumed
area-reports  active  6f3b9d2e-4a7c-4e1b-8c5d-2e9a7f1b4d36  /home/alex/src/reports  2026-10-29T23:27:43Z resumed
area-search   active  e7b1d4c9-2a6f-4e3b-8d5c-9f1a7e2b6d48  /home/alex/src/search   2026-10-29T23:27:43Z resumed
coordinator   active  0f8e5c1a-3b2d-4c6e-9a7f-1d2e3f4a5b6c  /home/alex/src/mistt    2026-10-29T23:27:41Z resumed
custodian     active  3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57  /home/alex/src/mistt    2026-10-29T23:28:43Z resumed
writer        active  b4e9a2d7-6c1f-4d8b-a3e5-7f2c9d6b1a84  /home/alex/src/mistt    2026-10-29T23:28:44Z resumed
```

## Clean exits stay ended

An agent whose session finishes normally is marked `ended` and is not
resumed. How the supervisor learns of the exit depends on how the session
was started.

- **Sessions started with `--supervise`,** including every session the
  supervisor resumed. When Claude Code exits normally, for example after
  `/exit`, the agent is marked `ended` by `wrapper`. A session that exits with
  an error status, or ends because its terminal or tmux server was lost
  (SIGHUP) or it was told to stop (SIGTERM), stays `active` and is resumed.
- **Sessions registered with `amplihack agent register`.** Here amplihack's
  `SessionEnd` hook reports the exit. When you leave the session from its
  prompt, for example with `/exit`, or log out with `/logout`, the hook writes
  a marker file under `~/.amplihack/agents/ended/`. At its next pass the
  supervisor marks the agent `ended` by `session-end` and deletes the marker.
  `/clear` and `/resume` keep the session running with another conversation,
  and write no marker; the supervisor follows the new conversation id. A
  session lost with its terminal or tmux server writes no marker either, and
  stays `active`. The [reference](../reference/agent-command.md#writing-a-marker)
  lists every reason Claude Code sends.

The hook comes with `amplihack install` and with the amplihack Claude Code
plugin, and it must be installed in the config directory the session uses
(`~/.claude`, or the profile's directory). Without it, an `/exit` in a
registered session is not recorded. The agent then stays `active`, and the
next time its tmux session is lost the supervisor resumes it.

Ending an agent writes no line in the log. `amplihack agent status` shows an
ended agent's state with the time and the source:

```text
NAME          STATE                                 CONVERSATION                          DIRECTORY               LAST RECOVERY
...
writer        ended 2026-10-30T08:02:15Z (wrapper)  b4e9a2d7-6c1f-4d8b-a3e5-7f2c9d6b1a84  /home/alex/src/mistt    2026-10-29T23:28:44Z resumed
```

To supervise an ended agent again, start it with
`amplihack claude --supervise NAME`, or register it again.

## Crash loops

Every resume counts towards a limit of 3 per agent in any hour. When an agent
is lost again after 3 resumes in the last hour, the supervisor does not
resume it. It marks the agent `stopped` and writes an alert:

```text
2026-10-30T00:41:40Z agent=custodian conversation=3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57 lost=2026-10-30T00:41:40Z resumed=- result=stopped: restart limit
2026-10-30T00:41:40Z ALERT: agent custodian stopped: restart limit (3 restarts in the last hour)
```

`amplihack agent status` shows the agent as `stopped`, with the alert below
the table:

```text
NAME          STATE    CONVERSATION                          DIRECTORY               LAST RECOVERY
...
custodian     stopped  3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57  /home/alex/src/mistt    2026-10-30T00:41:40Z stopped: restart limit
...
ALERT custodian: stopped: restart limit (3 restarts in the last hour)
```

`status --json` gives the same text in the agent's `alert` field. A stopped
agent stays stopped until you act.

Find out why the session fails before you re-arm it. A session that exits
with an error closes its tmux window, so run it yourself in a window that
stays open. First read the recorded options:

```bash
jq '.agents.custodian | {tmux_session, tmux_window, launch_dir, conversation_id,
  model, effort, add_dirs, permission_mode, skip_permissions, amplihack_flags}' \
  ~/.amplihack/agents/registry.json
```

```json
{
  "tmux_session": "custodian",
  "tmux_window": "main",
  "launch_dir": "/home/alex/src/mistt",
  "conversation_id": "3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57",
  "model": "opus",
  "effort": "high",
  "add_dirs": [],
  "permission_mode": null,
  "skip_permissions": true,
  "amplihack_flags": {
    "subprocess_safe": false,
    "no_reflection": false,
    "profile": "work"
  }
}
```

Then open the agent's tmux session with a shell, and start the session there
with the same options:

```bash
tmux new-session -s custodian -n main -c /home/alex/src/mistt
```

```bash
amplihack claude --profile work --supervise custodian -- \
  --resume 3d8a6f2b-1e5c-4b7a-9d2e-6c4f8a1b3e57 --model opus --effort high
```

Claude Code's error stays on screen. Once the session starts cleanly, the
agent is `active` again and its alert is cleared. Its restart history is
kept, so if it is lost again within the hour the supervisor stops it again
instead of resuming it.

To clear the restart history as well, register the agent again with
`amplihack agent register`. Registering replaces the whole record, so give
every option, as in
[Register a session that is already gone](#register-a-session-that-is-already-gone).

## Run one pass by hand

Only one supervisor runs at a time. To run a single pass yourself, stop the
service, run the pass, and start the service again:

```bash
systemctl --user stop amplihack-agent-supervisor.service
amplihack agent supervise --once
systemctl --user start amplihack-agent-supervisor.service
```

Stopping the service does not stop the agents it resumed. With the service
running, `--once` prints
`another amplihack agent supervisor is running (pid N)` and exits 1.

## Change the timing

| Flag | Default | What it controls |
|------|---------|------------------|
| `--interval SECS` | 60 | Time from the end of one pass to the start of the next |
| `--batch N` | 6 | Agents resumed per batch |
| `--gap SECS` | 60 | Wait between batches |
| `--max-restarts N` | 3 | Resumes allowed per agent in any hour before it is stopped |

To change them, override `ExecStart` in a drop-in:

```bash
systemctl --user edit amplihack-agent-supervisor.service
```

```ini
[Service]
ExecStart=
ExecStart=%h/.local/bin/amplihack agent supervise --batch 3 --gap 120
```

`systemctl --user edit` reloads systemd when you save. Then restart the
service:

```bash
systemctl --user restart amplihack-agent-supervisor.service
```

## Stop supervising a session

```bash
amplihack agent unregister writer
```

```text
unregistered writer
```

This removes the record only. The session, its tmux session and its
processes are left alone. Earlier lines in the log are kept.

## Unattended resumes and other users

- **The registry is a list of sessions that start on their own.** The
  supervisor resumes every active agent with no one watching, and
  `amplihack claude` starts Claude Code with `--dangerously-skip-permissions`,
  so tool calls run without permission prompts. With lingering on, this
  happens after a reboot before anyone logs in. Register only sessions you
  would leave running that way, and review `amplihack agent status` from time
  to time: any program running as your user can add an agent.
- **Credentials are not recorded.** Only eleven variables that choose models,
  effort and output limits, such as `ANTHROPIC_MODEL` and
  `CLAUDE_CODE_EFFORT_LEVEL`, are ever recorded.
  API keys, tokens and `ANTHROPIC_BASE_URL` are not, and `--env` refuses
  them. A value typed after `--env` is already in your shell history and
  visible to other users in the process list, even when `register` refuses
  it. Do not put credentials in the unit either. Claude Code keeps its login
  in `~/.claude` and needs none.
- **The journal is readable by some other users.** Log lines name agents,
  conversation ids, directories and results. On most distributions members of
  the `adm` or `systemd-journal` group can read your user journal.
- **Private directories.** The supervisor resumes nothing while it cannot
  trust what it reads:
  - When `~/.claude`, the profile root, a profile directory or a `sessions/`
    directory below them is not owned by you, or other users can write to
    it, every lost agent is skipped with
    `skipped: already running (liveness unknown: Claude config directory is not private)`.
    `amplihack agent register` names the directory in its error; remove the
    write bits with `chmod go-w`.
  - Before it talks to a tmux socket, the supervisor checks the socket's
    directory, for example `/tmp/tmux-1000`. If it is not yours, or other
    users can open it, the agent is skipped with
    `skipped: tmux session exists (tmux state unknown: socket directory is not private)`.
    After a reboot clears `/tmp`, another user can create that directory
    before the supervisor does; until it is removed, which needs root because
    the directory is not yours, recovery stays withheld. Your own tmux refuses
    the same directory.

## Limits

- **Linux only.** On other systems `register` needs every field as a flag,
  and neither `amplihack agent supervise` nor `amplihack claude --supervise`
  starts anything.
- **One tmux session per agent.** The supervisor finds an agent's tmux
  session by its exact name. Do not rename the tmux session or window of a
  registered agent; register it again after you do. Session names cannot
  contain `:`, `.` or `#`, and window names cannot contain `#`.
- **A conversation change just before an exit.** For a session registered
  with `amplihack agent register`, the supervisor follows a new conversation
  id after `/clear` or `/resume` at its next pass. If you run `/clear` and
  then `/exit` before that pass, the exit names a conversation the registry
  does not know, and a later tmux loss resumes the old conversation. Sessions
  started with `--supervise` are not affected.
- **Transcript directory names.** Claude Code names a project's transcript
  directory after the launch directory. The supervisor checks the two names
  Claude Code is known to use. If Claude Code uses another, for example for a
  very long path, the result is `failed: no transcript` although the
  transcript exists.
- **No log rotation.** `~/.amplihack/agents/supervisor.log` grows by one line
  per recovery attempt. Truncate it while the service is stopped if it gets
  large.

## Related

- [`amplihack agent` reference](../reference/agent-command.md) - Every flag,
  the registry fields, the result texts, the status formats and the unit
- [Rotate Claude Logins](claude-profile-rotation.md) - Keep `~/.claude` on
  the account with the most capacity while many sessions resume
- [Manage Claude Login Profiles](claude-profiles.md) - The profiles that
  `--profile` refers to
- [Install amplihack as a Claude Code plugin](install-claude-code-plugin.md) -
  The plugin that carries the `SessionEnd` hook
