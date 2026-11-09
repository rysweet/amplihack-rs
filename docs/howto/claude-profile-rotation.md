---
title: How to Rotate Claude Logins Automatically
description: Rotate and switch Claude logins with credential-bound ownership checks.
doc_type: howto
review_schedule: as-needed
---

# How to Rotate Claude Logins Automatically

`amplihack profile rotate` keeps the default Claude login in `~/.claude` on
the account with the most capacity left. It measures every
[login profile](claude-profiles.md) with
[`profile usage`](claude-profiles.md#check-usage-across-profiles), and when the
account in `~/.claude` is close to its session or weekly limit it moves a
better profile's login into `~/.claude` and puts the outgoing login back in the
profile it came from.

Use it when you run plain `claude` (no `CLAUDE_CONFIG_DIR`) and have several
profiles. [Auto-switch](claude-profiles.md#switch-accounts-automatically) is
the separate feature that swaps logins inside sessions started with
`amplihack claude --profile`. Both use the same on/off toggle and the same
state lock, so they never move logins at the same time.

Rotation and manual switching identify each login from trusted evidence
bound to that login. Running sessions can rewrite `~/.claude.json`; its
`oauthAccount` is display state and cannot establish ownership. A stale
session account does not block an otherwise authorized swap or trigger
recovery, even after the recovery grace period ends. See
[Rotation login identity](../reference/profile-command.md#rotation-login-identity).

The exact flags, files, exit codes and log lines are in the
[`profile rotate` reference](../reference/profile-command.md#rotate).

## Contents

- [Before you start](#before-you-start)
- [Preview a rotation](#preview-a-rotation)
- [Rotate once](#rotate-once)
- [Switch to a login by hand](#switch-to-a-login-by-hand)
- [Run rotation in the background](#run-rotation-in-the-background)
- [Start the rotator automatically](#start-the-rotator-automatically)
- [Run the rotator under a service manager](#run-the-rotator-under-a-service-manager)
- [See who turned rotation off](#see-who-turned-rotation-off)
- [Which root the rotator uses](#which-root-the-rotator-uses)
- [Change the thresholds](#change-the-thresholds)
- [When a swap happens](#when-a-swap-happens)
- [When the live login cannot be measured](#when-the-live-login-cannot-be-measured)
- [When the host is heavily loaded](#when-the-host-is-heavily-loaded)
- [Which profiles can take over](#which-profiles-can-take-over)
- [Read the log](#read-the-log)
- [Resume sessions stopped by a usage limit](#resume-sessions-stopped-by-a-usage-limit)
- [See how much capacity is left](#see-how-much-capacity-is-left)
- [When no account has capacity](#when-no-account-has-capacity)
- [Use usage credits when every plan login is exhausted](#use-usage-credits-when-every-plan-login-is-exhausted)
- [Move from the tmux prototype](#move-from-the-tmux-prototype)
- [Where a login with no profile goes](#where-a-login-with-no-profile-goes)
- [Swap refused](#swap-refused)
- [Recovery](#recovery)
- [Use an API key as the last resort](#use-an-api-key-as-the-last-resort)
- [Live default and duplicate logins](#live-default-and-duplicate-logins)
- [Limits](#limits)

## Before you start

You need:

- Linux. On other systems see [Limits](#limits).
- At least two profiles, each logged in, created with
  `amplihack profile add <name>` and `amplihack profile login <name>`.
- A login in `~/.claude`, made with plain `claude` and `/login`.
- `~/.claude` and the profile root (`$AMPLIHACK_CLAUDE_PROFILE_ROOT`, then
  the root saved by `amplihack profile root`, by default
  `~/.amplihack/claude-profiles`) on the same filesystem. The examples on this
  page use a root saved as `~/.claude-profiles`.

Check that every profile reports usage:

```bash
amplihack profile usage
```

```
PROFILE      WEEK             W-RESET      SESSION           S-RESET     STATE
spare        █░░░░░░░░░  12%  10-23 09:00  ░░░░░░░░░░   0%   -           ok
personal     ███░░░░░░░  35%  10-22 02:00  ██░░░░░░░░  20%   10-18 15:00 ok
work (live)  ████░░░░░░  41%  10-21 18:00  ████████░░  88%   10-18 13:00 ok
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)
```

The rows are sorted by the rotator's headroom, most room first, and
`(live)` marks the login in `~/.claude`. Overflow and excluded profiles are
sorted the same way, so the first row is not always the rotator's next
pick. Only a profile in state `ok` can be chosen; an exhausted profile, or
one whose probe failed, never is. A probe that failed or timed out means
the account's usage is unknown, not that it is used up: such a profile is not treated as
exhausted and is probed again on the next cycle.

## Preview a rotation

```bash
amplihack profile rotate --dry-run
```

```
cycle live=work work:s=88%,w=41%,age=2s personal:s=20%,w=35%,age=2s spare:s=0%,w=12%,age=2s
would swap work->spare live s=88% w=41% age=2s target s=0% w=12% age=2s reason=threshold
```

A dry run probes every profile and prints what it would do. It takes no lock,
writes no rotate state, moves nothing and logs nothing. It never refreshes a
token or starts `claude`, so an expired login shows as unmeasured with its
cause, and the target can differ from a real run's. It also does not finish an
interrupted swap: if one is pending it prints
`would recover an interrupted swap (phase <phase>)` and leaves it for the next
real run. Run a dry run first, and again whenever you change a threshold.

## Rotate once

```bash
amplihack profile rotate
```

```
2026-10-18T12:40:05Z cycle live=work work:s=88%,w=41%,age=2s personal:s=20%,w=35%,age=2s spare:s=0%,w=12%,age=2s
2026-10-18T12:40:05Z swap work->spare live s=88% w=41% age=2s target s=0% w=12% age=2s reason=threshold
```

The same lines go to the log. Afterwards `spare`'s login is in `~/.claude` and
`work`'s login is back in `~/.claude-profiles/work/`:

```bash
amplihack profile usage
```

```
PROFILE       WEEK             W-RESET      SESSION           S-RESET     STATE
spare (live)  █░░░░░░░░░  12%  10-23 09:00  ░░░░░░░░░░   0%   -           ok
personal      ███░░░░░░░  35%  10-22 02:00  ██░░░░░░░░  20%   10-18 15:00 ok
work          ████░░░░░░  41%  10-21 18:00  ████████░░  88%   10-18 13:00 ok
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)
```

A running `claude` picks up the new login when it next reads its credentials;
start new sessions to be sure.

The one-shot command runs whether the auto-switch toggle is on or off.

## Switch to a login by hand

To put a particular profile's login in `~/.claude` now, and keep it there:

```bash
amplihack profile switch spare
```

```
Live login moved: work -> spare
Default for new launches: spare
Rotation held on spare until `amplihack profile switch --none`.
```

`switch` makes the same swap the rotator makes, under the same lock, and
writes the same ledger entry, so `profile usage` and `rotate status` show
`spare` as live afterwards. The move works whether or not a rotator is
running (waking stopped sessions does not; see below), and it does not
measure usage, so it starts no `claude` process and works with no `claude` on
`PATH`. It does make the bounded identity reads needed to authorize the
swap. It moves the login even when `spare` has no capacity
left: you asked for `spare`, so the usage checks that stop a rotate swap do
not apply.

It also does three more things:

- **Sets the default.** `amplihack claude` with no `--profile` resolves to
  `spare`, which is the
  [live profile](#live-default-and-duplicate-logins), so it runs against
  `~/.claude`.
- **Holds rotation.** While the hold is in force, no rotate cycle swaps away
  from `spare`, even when it reaches its session or weekly limit. A
  background rotator logs one line per cycle and measures nothing:

  ```
  2026-10-18T13:10:05Z skip: held by switch to spare; run `amplihack profile switch --none` to release
  ```

  `amplihack profile rotate` prints the same line and exits 0, and
  `rotate status` shows `hold: spare`.
- **Wakes sessions stopped by a usage limit.** `switch` records the swap in
  the rotator's ledger as a cycle with capacity, measured at the moment of
  the switch. The
  [limit-resume hook](#resume-sessions-stopped-by-a-usage-limit) resumes
  sessions that stopped on a limit before the switch on its next ledger
  read (every 15 seconds in the first hour of a session's wait, every 5
  minutes after), as it does after a rotator swap. The held cycles that
  follow keep that record, so the hold does not undo it.

  The hook needs a running rotator. With none running, it exits
  `rotator-down` when the limit hits and never reads the ledger, so a later
  `switch` wakes nothing; resume those sessions by hand. To have switches
  wake them, keep a rotator running with
  [`amplihack profile rotate start`](#run-rotation-in-the-background)
  before the limit hits.

If no profile owned the login in `~/.claude`, `switch` first keeps it as
`default-original` (or the next free name), as the rotator does, and the
first line reads `Live login moved: default-original -> spare`.

If `spare` is already the live login, nothing moves; the first line is
`spare is already the live login; nothing moved.` and the default and the
hold are still set. Running `switch` with another name swaps again and moves
the hold to that profile.

### Hand control back to the rotator

```bash
amplihack profile switch --none
```

```
Default profile cleared.
Rotation released; the rotator picks the best login on its next cycle.
```

This moves no login. `spare` stays in `~/.claude` until a cycle decides to
swap, by the usual [rules](#when-a-swap-happens). Run
`amplihack profile rotate` to have that cycle now.

### What the hold changes

- **A limit hit after the switch waits.** A session that stops on a limit
  on the login you switched to is not resumed by that switch, because the
  switch is older than the limit, and the held cycles measure nothing. It is
  resumed by your next `switch`, by a `switch --none` followed by a rotator
  cycle that finds capacity, or when its limit resets. To resume it yourself,
  type `continue` in the session.
- **The hold beats every rule that picks a target.** The cycle checks the
  hold before it measures or chooses a profile. The hold is its own file, so
  `.default` can stay unset for features that ask for that; `switch --none`
  clears both.
- **The hold beats usage credits too.** A `switch` onto an
  [overflow profile](#use-usage-credits-when-every-plan-login-is-exhausted)
  keeps you on credits, spending, even while a plan login has capacity. A
  `switch` onto a plan login never moves onto credits, even when every plan
  login runs out. Run `switch --none` to let the rotator decide again. See
  [Pin and overflow](../reference/profile-command.md#pin-and-overflow).
- **An older rotator does not see it.** If the running rotator was started
  from another amplihack binary, `switch` still moves the login and exits 0,
  but prints a warning:

  ```
  amplihack: the Claude login rotator (pid 48211) runs another amplihack binary and ignores the switch hold; restart it: amplihack profile rotate stop && amplihack profile rotate start
  ```

  Run those two commands, or that rotator may swap away on its next cycle.

### When switch refuses

`switch` checks everything before it moves anything, and it refuses to move
the login of every profile the rotator would not swap to: the reasons in
[Rotate candidates](../reference/profile-command.md#rotate-candidates). Not
moving the login does not stop the profile from becoming the default for
new launches, which `switch` has always set. For a refusal about the live
login or the profile itself (every row below except the first two),
`switch` still writes `.default`, then exits with status 1:

```
Default for new launches: spare
Live login not moved; rotation not held.
error: swap refused: profile spare is not a rotate candidate (session running): a running process has CLAUDE_CONFIG_DIR set to it; end that session first
hint: nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md
```

It writes no hold, no ALERT and no swap. A hold from an earlier `switch`
stays where it was, and the second line names it:
``Live login not moved; rotation still held on work until `amplihack profile switch --none`.``
Plain `claude` keeps using the login in `~/.claude`; amplihack launches
without `--profile` use `spare` in its own directory.

A busy lock or an inconsistent recovery writes nothing at all, `.default`
included: they say nothing about the profile, so try again once they are
resolved.

Before those checks it runs [recovery](#recovery), as every rotate cycle
does. If an earlier swap was interrupted, recovery finishes or rolls it back
and logs a `recovery: ...` line, and that stays done even when `switch`
then refuses. Without a pending recovery, a refusal changes nothing but
`.default`.

| Message | What to do |
|---------|------------|
| `state lock busy` | A rotate cycle or an auto-switch swap is running. Try again in a moment. |
| `rotate recovery is inconsistent: ...`, `auto-switch recovery is inconsistent` | See [Recovery](#recovery). |
| `no login in ~/.claude; no swap` | Log in with plain `claude` first. `switch` never fills an empty `~/.claude`. |
| `the account in ~/.claude is not the one the ledger names (live=work); no swap` | Someone ran `/login` in `~/.claude`; follow the steps in [Swap refused](#swap-refused). |
| ``profile spare has no stored login; run `amplihack profile login spare` first`` | The profile has no `.credentials.json` of its own. Log it in, then switch. Not checked when `spare` is already live. |
| `profile spare is not a rotate candidate (session running): ...` | A running session uses `spare` (`CLAUDE_CONFIG_DIR` set to its directory). Moving its login would log that session out. End it, then switch; `spare` is already the default for new launches. If the message says running sessions cannot be seen, `/proc` cannot be scanned here (every system but Linux, including macOS), so no login can be moved; the rotator cannot swap there either. `switch` there only sets the default for new launches and exits 1. |
| `profile spare is not a rotate candidate (leased): ...`, `(hosting)`, `(parked)` | Auto-switch has lent `spare`'s login to another session, or `spare`'s directory holds another account's login. Wait for that session to end; auto-switch then returns the logins. |
| `profile spare is not a rotate candidate (excluded): ...` | `spare` is in the auto-switch exclude list, which keeps it out of rotation; a switch is a rotation done by hand. Set the list without it with `amplihack profile auto-switch exclude`, then switch. |
| `profile spare is not a rotate candidate (no account): ...` | The login has no usable trusted account evidence. Resolve the identity-read failure; editing `oauthAccount` cannot authorize the switch. |
| `profile spare is not a rotate candidate (directory refused): ...` | The `spare` directory is a symlink, not yours, or open to group or other users. Run `chmod 700` on it, or recreate it if it is not yours. While it is refused, the rotator does not move onto overflow profiles; see [When a plan login cannot be measured](#when-a-plan-login-cannot-be-measured). |
| `profile spare is not a rotate candidate (no login): ...` | `spare`'s `.credentials.json` is a symlink, not yours, readable by others or hard-linked. See [Swap refused](#swap-refused). |
| Any reason in the [Swap refused](#swap-refused) table | As described there. |

If the swap ran but the hold or the default could not be written, `switch`
says so and exits with status 1:

```
error: the live login moved to spare, but writing the default pointer failed: Permission denied (os error 13); run `amplihack profile switch spare` again
```

The login did move. Fix the cause and run the same `switch` again: it finds
`spare` already live and writes what was missing.

## Run rotation in the background

Background rotation runs unless auto-switch has been turned off. To be sure
it runs, turn it on:

```bash
amplihack profile auto-switch on
amplihack profile rotate start
```

```
rotate: started (pid 48213), checking every 600s
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
root: /home/you/.claude-profiles
```

Turning the toggle on also enables auto-switch for profiled sessions; see
[Switch accounts automatically](claude-profiles.md#switch-accounts-automatically).
If auto-switch has been turned off (`auto-switch off`, or
`AMPLIHACK_CLAUDE_AUTO_SWITCH=0` in the environment `start` ran in), `start`
prints a warning and starts anyway; the background process then writes
`idle: auto-switch is off` and does nothing until it is turned on. With no
auto-switch setting at all, rotation runs. It re-reads the toggle on every cycle, so
`auto-switch off` and `auto-switch on` take effect without a restart.

Check on it:

```bash
amplihack profile rotate status
```

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
capacity: headroom 2.07 login-weeks of 4 (52%), usable 2 of 4 logins, runway ~9 h (as of 2026-10-18T12:50:05Z, 3m ago, rotate cycle: probed profiles only)
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

The `capacity:` line is read from the usage history the rotator keeps; see
[See how much capacity is left](#see-how-much-capacity-is-left).

Stop it:

```bash
amplihack profile rotate stop
```

```
rotate: stopped (pid 48213)
```

`stop` signals the process only after checking it is this user's
`amplihack profile rotate watch`. A cycle that is moving a login finishes
before the process exits. A cycle that is still measuring is abandoned: the
probes are stopped and nothing is moved. If the pid file names a process that has gone, or
some other program, `status` reports `stale`, and `stop` removes the pid file
without signalling anything.

The background process keeps running after you log out. It does not survive
a reboot, but with auto-switch on the next `amplihack claude`,
`amplihack launch` or `amplihack profile run` starts it again; see
[Start the rotator automatically](#start-the-rotator-automatically).

To run the loop in the foreground instead, for example under your own
supervisor, use `amplihack profile rotate watch`. It takes the same flags as
`start` and stops on Ctrl-C.

## Start the rotator automatically

When auto-switch is on, every amplihack launch of Claude checks whether the
rotator is running and starts it if it is not. You no longer need to run
`rotate start` after a reboot.

```bash
amplihack profile auto-switch on
amplihack claude
```

```
amplihack: started Claude login rotator (pid 48213); 'amplihack profile rotate autostart off' to disable
```

That line goes to stderr, and only from the launch that started the rotator.
A launch that finds it already running prints nothing. Claude then starts as
usual; the check never waits, never changes the launch's exit status and
never writes to stdout.

### What starts it

These commands check, before they start Claude:

- `amplihack claude`
- `amplihack launch`
- `amplihack profile run <name> -- ...`

The Claude Code `SessionStart` hook does not start the rotator, and neither
does any other `profile` command.

A launch with `--profile <name>` also starts it. The rotator always manages
`~/.claude`, whichever profile the launch itself uses.

### When it does not start

A launch starts the rotator only when all of these hold:

- Auto-switch is **on**: you have run `amplihack profile auto-switch on`, or
  `AMPLIHACK_CLAUDE_AUTO_SWITCH=1` is set. With no auto-switch setting at all,
  nothing starts automatically, even though a rotator you start by hand still
  rotates.
- Autostart is on (it is by default; see
  [Turn autostart off](#turn-autostart-off)).
- The profile root exists, is not a symlink, is owned by you and is not
  writable by other users.
- You are not running as root.
- The launch is not under Docker: no `--docker`, and Docker not turned on
  through the environment. The environment turns it on only when
  `AMPLIHACK_USE_DOCKER` has a true value, the launch is not already inside a
  container, and Docker is running; `AMPLIHACK_USE_DOCKER=0` does not stop the
  rotator starting. `--auto` launches do start it.
- amplihack can tell whether a rotator is already running, which needs
  `/proc`. Without it nothing starts.
- No rotator is running for that root.
- `rotate start` would not refuse: the
  [tmux prototype](#move-from-the-tmux-prototype) is not holding its lock.
- The launch is not a step of a recipe run: `AMPLIHACK_RECIPE_RUN_ID` is
  unset or empty. `amplihack recipe run` sets it for every step.
- No service manager owns rotation: there is no
  [owner marker](#run-the-rotator-under-a-service-manager).

When auto-switch is off, a launch does nothing extra: it creates no file,
takes no lock and prints nothing.

### Many launches at once

Recipe runs and `multitask` start many Claude sessions in parallel. Launches
inside a recipe run never start the rotator: they see
`AMPLIHACK_RECIPE_RUN_ID` and stop before reading anything. Start the rotator
before the run, or let the `amplihack claude` you start the run from start it.

Other launches check as usual, and only one rotator ever runs: a launch that
finds the rotator running stops there, and otherwise it tries the state lock
without waiting. If another launch holds
the lock, it gives up silently. The launch that gets the lock checks again
before it starts the rotator and writes the pid file while it still holds the
lock.

### What the started rotator uses

An automatically started rotator is the same background process `rotate
start` runs, with the same pid file, log and
[root record](#which-root-the-rotator-uses). The differences:

- It uses the profile root the launch resolved.
- It keeps the `AMPLIHACK_ROTATE_*` [thresholds](#change-the-thresholds) and
  the `AMPLIHACK_PROFILE_USAGE_*` [probe settings](#when-the-host-is-heavily-loaded)
  set in the environment of the launch that started it, and that launch's
  `amplihack` binary. To change them, run `rotate stop`, then
  `rotate start` with the settings you want.
- It does not run [recovery](#recovery) before it starts; its first cycle
  does that under the lock.

If a `AMPLIHACK_ROTATE_*` value in the launch's environment is invalid, the
rotator is not started. The launch carries on, and the reason is written to
`<root>/.auto-switch.d/rotate/rotate.err`, as is every other failure to
start. Each start adds a line naming the pid and binary:

```
2026-10-20T08:02:11Z autostart: started pid 48213 exe /home/you/.cargo/bin/amplihack
```

A rotator started from a development build (`target/debug/amplihack`) keeps
running that binary after you remove the worktree it came from. Check the
`exe` in `rotate.err` and restart it from your installed binary if needed.

### Turn autostart off

```bash
amplihack profile rotate autostart off
```

```
autostart: off (setting)
```

This stops launches from starting the rotator. It does not stop one that is
already running. To stop rotation for good, turn autostart off first, then
stop the rotator, so that no launch in between starts it again:

```bash
amplihack profile rotate autostart off
amplihack profile rotate stop
```

Turn it back on with `amplihack profile rotate autostart on`. With no
argument, the command prints the current state:

```bash
amplihack profile rotate autostart
```

```
autostart: on (default)
```

To turn it off for one shell or one CI job, which overrides the setting:

```bash
export AMPLIHACK_ROTATE_AUTOSTART=0     # or off
export AMPLIHACK_ROTATE_AUTOSTART=1     # or on
```

`rotate status` and `auto-switch status` both show the state:

```
rotate: running
autostart: on (default)
```

```
switch: off (file)
autostart: on (default) — inactive (auto-switch off)
```

The source in brackets is `default` (no setting), `setting` (from
`rotate autostart on|off`), `env` (from `AMPLIHACK_ROTATE_AUTOSTART`) or
`corrupt` (the setting file cannot be trusted; autostart counts as off until
you run `rotate autostart on` or `off` again).

## Run the rotator under a service manager

To have a service manager such as systemd keep the rotator running, run
`rotate watch` in the foreground with `--supervised`. In a systemd user unit:

```ini
ExecStart=/home/you/.cargo/bin/amplihack profile rotate watch --supervised=amplihack-rotator.service --interval 120 --session-at 70
```

Stop any rotator that is already running before you enable the unit. A
supervised `watch` refuses to run beside another rotator, exits with status 1
and writes no owner marker, so the unit would restart and fail until that
rotator stops:

```bash
amplihack profile rotate stop
systemctl --user enable --now amplihack-rotator.service
```

If `systemctl --user status amplihack-rotator.service` shows the unit
failing because a rotator is running, a launch started one between the two
commands. Run `amplihack profile rotate stop` again; the unit's next restart
then succeeds.

A supervised `watch` writes an owner marker,
`<root>/.auto-switch.d/rotate/owner`, naming the unit and its pid. While the
marker exists, launches never start a rotator of their own, even when
autostart is on, so the unit's rotator is the only one. The marker stops
autostart only: `amplihack profile rotate start` does not read it, so do not
run `rotate start` while the unit is enabled. A `watch` that systemd runs
as a service, where `INVOCATION_ID` is set and the last component of its
`/proc/self/cgroup` path is a `.service` unit other than `user@<uid>.service`,
records itself even without the flag; the flag lets you name the unit.
Without a name, the unit is read from `/proc/self/cgroup`, and failing that
`amplihack-rotator.service` is used.

`rotate status` shows the owner straight after the autostart line (excerpt):

```
rotate: running
autostart: on (default) (deferring to amplihack-rotator.service)
owner: amplihack-rotator.service pid 48213 (running); autostart defers to it
pid: 48213
```

Show the owner on its own with:

```bash
amplihack profile rotate owner
```

```
owner: amplihack-rotator.service pid 48213 (running); autostart defers to it
```

### When the owner stops

The marker stays when the unit stops, so that launches do not start a rotator
while the service manager is restarting it. When the unit restarts, the new
`watch` writes its own pid. Until then, each launch prints one line to stderr
and still starts Claude:

```
amplihack: rotation is owned by amplihack-rotator.service, which is not running; run 'amplihack profile rotate owner clear' to let launches start the rotator
```

With autostart off, the line ends after `which is not running`, since
clearing the marker would not start a rotator. `status` shows:

```
owner: amplihack-rotator.service pid 48213 (not running); autostart defers to it; run 'amplihack profile rotate owner clear' to release
```

If the marker is damaged, or is not a private file owned by you, launches
still defer, and the line says the marker is unreadable. `status` then shows:

```
autostart: on (default) (deferring to an unreadable owner marker)
owner: unreadable; autostart defers to it; run 'amplihack profile rotate owner clear' to release
```

If the profile root itself is not private, launches never start a rotator,
and `status` does not read the marker:

```
autostart: on (default) (skipped: root is not private)
owner: not read (root is not private)
```

### Hand rotation back to launches

When you retire the unit, stop and disable it, then clear the owner:

```bash
systemctl --user disable --now amplihack-rotator.service
amplihack profile rotate owner clear
```

```
owner: cleared (was amplihack-rotator.service)
```

The next launch starts the rotator as described in
[Start the rotator automatically](#start-the-rotator-automatically).
`owner clear` is the only command that removes the marker: `rotate stop`,
`rotate start` and a `watch` without `--supervised` leave it alone. A `watch`
without `--supervised` that finds a marker prints a note to stderr and runs
anyway.

### A watch that claims ownership by mistake

systemd sets `INVOCATION_ID` for every process in a unit, including a shell
inside one. A shell started from a tmux or terminal server that the user
manager runs inherits it too, but such a shell runs in a `.scope`, so a
`rotate watch` started from it does not claim ownership. A `rotate watch` you
start by hand from a shell inside a `.service` unit does record itself as
the owner. Stop it, then run `amplihack profile rotate owner clear`.
Rotators started by `rotate start` or by a launch never claim ownership,
because `INVOCATION_ID` is removed from their environment.

An `amplihack` binary older than this feature ignores the marker. Upgrade
every binary that launches Claude against the root.

## See who turned rotation off

Each change to the autostart setting or to the auto-switch toggle adds a
`change:` line to the rotate log, naming the process that made it, its
parent's command line and, for a recipe step or a systemd unit, the run or
invocation id:

```bash
grep ' change: ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

```
2026-11-01T09:14:02Z change: auto-switch on -> off by pid=51877 ppid=51702 parent="bash /home/you/bin/pause-rotation.sh" recipe_run=3f9c2a1e-6b7d-4c1e-9a52-0d8e7b6f4a21
2026-11-01T09:20:45Z change: rotate-autostart on -> off by pid=52011 ppid=4120 parent="-bash"
```

Only real changes are logged: running `off` when the setting is already off
adds nothing. Threshold and exclusion changes, and the
`AMPLIHACK_ROTATE_AUTOSTART` and `AMPLIHACK_CLAUDE_AUTO_SWITCH` variables,
are not logged. Secrets in the parent command line are replaced by `***`
before the line is written: `NAME=value` pairs, the values of flags such as
`--api-key`, `--token` or `--pass`, headers such as `Authorization: ...`,
`curl -u user:password`, `mysql -p<password>`, passwords in URLs and URL
parameters such as `access_token=`. Like the rest of the log,
the file is private to you (`0600`).

While auto-switch is off, the rotator idles. If the live login is exhausted
at the same time, it writes an ALERT on every cycle, and `rotate status`
shows it as an `alert:` line:

```
2026-11-01T10:10:05Z ALERT: auto-switch is off while the live login is exhausted (spare: 100% used, threshold 90%)
```

```
alert: auto-switch is off while the live login is exhausted (spare: 100% used, threshold 90%)
```

The percentage is rounded down. A login stopped by a usage limit shows
`(spare: usage limit reached, resets 2026-11-01T14:00:00Z)` instead, with
the reset time in UTC. If a ledger or `usage.json` cannot be read, the
check cannot run: the log shows `skip: alert check failed: <error>` and
`status` shows `alert: unavailable (<error>)`.

The ALERT changes nothing by itself. Turn auto-switch back on with
`amplihack profile auto-switch on`, or move the login by hand with
[`profile switch`](#switch-to-a-login-by-hand). The `change:` lines show what
turned it off.

## Which root the rotator uses

The rotator keeps the profile root it was started with. `start` passes that
root to the background process in `AMPLIHACK_CLAUDE_PROFILE_ROOT`, so the
process does not depend on the shell that started it. Closing that shell,
unsetting the variable or changing the saved root with `amplihack profile
root` does not move a running rotator.

`start` also records the root in `~/.amplihack/rotate-root`. When `status` or
`stop` runs in a shell whose root now resolves elsewhere, for example a cron
job with no saved root, they find the running rotator through that record and
say so on the first line:

```bash
env -i HOME="$HOME" PATH="$PATH" amplihack profile rotate status
```

```
root: /home/you/.claude-profiles (pinned by running rotator)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

To move the rotator to a new root, stop it and start it again from a shell
that resolves the new root:

```bash
amplihack profile rotate stop
amplihack profile root /home/you/new-profiles
amplihack profile rotate start
```

Only one rotator is tracked: the record names the most recent `start`. See
[Rotate root record](../reference/profile-command.md#rotate-root-record) for
the exact rules.

## Change the thresholds

| Flag | Variable | Default | Meaning |
|------|----------|---------|---------|
| `--session-at N` | `AMPLIHACK_ROTATE_SESSION_AT` | `85` | Swap when the live session use reaches N%. A target must be below N%. |
| `--week-at N` | `AMPLIHACK_ROTATE_WEEK_AT` | `90` | Swap when the live weekly use reaches N%. A target must be below N%. |
| `--margin N` | `AMPLIHACK_ROTATE_MARGIN` | `5` | When no target is below both thresholds, a target needs at least N more points of weekly headroom than the live login. |
| `--upgrade-margin N` | `AMPLIHACK_ROTATE_UPGRADE_MARGIN` | `30` | Swap without reaching a threshold when a target has at least N more points of headroom. |
| `--hold SECS` | `AMPLIHACK_ROTATE_HOLD` | `3600` | No upgrade swap for this long after any swap. `0` turns the hold off. |
| `--overflow-wait SECS` | `AMPLIHACK_ROTATE_OVERFLOW_WAIT` | `1800` | Use an [overflow profile](#use-usage-credits-when-every-plan-login-is-exhausted) only when the next plan reset is further away than this. |
| `--interval SECS` | `AMPLIHACK_ROTATE_INTERVAL` | `600` | Time between cycles, for `start` and `watch` only. A reading older than `min(300, SECS)` seconds is not used. |

A flag beats the variable, and the variable beats the default. For example, to
rotate earlier and check every five minutes:

```bash
amplihack profile rotate start --session-at 75 --interval 300
```

Flags for `start` and `watch` go after the subcommand.
`amplihack profile rotate --session-at 75 start` is a usage error.

`start` fixes the settings when it starts. Changing a variable later has no
effect on a running background process; run `stop` and then `start`.

## When a swap happens

The rotator decides only on fresh readings: a reading taken no longer ago
than `--interval` seconds, and never more than 300. An older reading is
shown in the log with its age and `(not used)`, and a failed probe is shown
as failed, never as its last figures. See
[Read the log](#read-the-log).

A login can take over when its fresh reading is below `--session-at` on
its session and below `--week-at` on its week. Such logins are ranked by
weekly use first, lowest first, then by session use, then by name. Weekly
use comes first because a weekly limit lasts for days.

On each cycle:

1. **Threshold swap.** If the live session use is at or above
   `--session-at`, or the weekly use is at or above `--week-at`, the
   rotator swaps to the first login that can take over. No margin is
   needed, so the move happens before the live login runs out. A live
   account that could not be measured is swapped only as described in
   [When the live login cannot be measured](#when-the-live-login-cannot-be-measured).
2. **Margin swap.** If no login can take over, it swaps to the first
   login below 100% on both windows whose weekly headroom (`100 - week %`)
   is at least `--margin` points more than the live login's.
3. **Stay.** Otherwise it writes `stay: no login with room`, listing every
   login's reading, and one `ALERT` line, and leaves `~/.claude` alone.
4. **Upgrade.** If no threshold is reached, it swaps to the first login,
   in rank order, that can take over and has at least `--upgrade-margin`
   more headroom (`100 - max(session %, week %)`), unless the last swap
   was less than `--hold` seconds ago.

For example, with the defaults, live `nnorth` at 98% of its week, and
logins at 97%, 11% and 52% weekly, all at 10% session use, the swap goes to
the login at 11%:

```
2026-11-05T14:02:12Z swap nnorth->eelsworth live s=10% w=98% age=13s target s=10% w=11% age=13s reason=threshold
```

The login at 97% is never chosen: it is at `--week-at`. If every other
login were at 97% or more, none would gain 5 points of weekly headroom over
`nnorth`, and the rotator would stay.

The plan-room step and auto-switch use the same rule, so they choose the
same login. The rules in full are in
[Eligible logins and ranking](../reference/profile-command.md#rotate-eligible-logins-and-ranking).

The hold stops flapping. Just after a swap, the account that was swapped out
often has its session window reset and looks fresh; without the hold the next
cycle would swap straight back. Threshold swaps on a measured reading are
never held back.

## When the live login cannot be measured

If the probe of the live login fails, the rotator cannot tell whether the
account is near its limit. A failure that follows whichever login is live
would otherwise make it swap on every cycle. So a failed live probe leads to
a swap only when:

- it is the **third failure in a row**, and
- at least `--hold` seconds have passed since the last swap.

Until then the rotator stays and logs why. In this example the new live
login fails too. The count starts again after the swap, and the third failure
comes inside the hold:

```
2026-10-20T09:00:05Z cycle live=work work:failed personal:s=20%,w=35%,age=2s spare:s=0%,w=12%,age=2s
2026-10-20T09:00:05Z reading: work probe failed: timeout
2026-10-20T09:00:05Z stay: live probe failed (1/3)
2026-10-20T09:10:05Z stay: live probe failed (2/3)
2026-10-20T09:20:05Z swap work->spare live probe failed: timeout target s=0% w=12% age=2s reason=failed-probe
2026-10-20T09:30:05Z stay: live probe failed (1/3)
2026-10-20T09:40:05Z stay: live probe failed (2/3)
2026-10-20T09:50:05Z stay: live probe failed; held after last swap (1800s left)
```

The count is kept by the running `watch` process. It goes back to zero when
the live login is measured, after any swap, and when the rotator restarts.
The one-shot `amplihack profile rotate` counts every failure as the first, so
it never swaps because of a failed probe alone. Even with `--hold 0`, the
rotator swaps for this reason at most once every three cycles.

A failed probe of a *candidate* only stops that candidate being chosen, as
before. When every account is at or above a threshold, the rotator writes
one `ALERT` and stays, as before.

A probe that timed out or could not run shows as `<name>:failed` in the
cycle line, never as a percentage or as the figures it last read, followed
by `reading: <name> probe failed: <reason>`, and writes
`warning: rotate: probe of <name> timed out` (or `failed: <reason>`) to
`rotate.err`. Its usage is unknown: the rotator does not choose it, does not
count or record it as exhausted, and probes it again on the next cycle.
While a plan profile's usage is unknown this way, the rotator does not move
onto
[usage credits](#use-usage-credits-when-every-plan-login-is-exhausted); see
[When a plan login cannot be measured](#when-a-plan-login-cannot-be-measured).
A login that reached a limit shows the figures its probe read, with the
window that ran out never below `100%`, so `work:s=100%,w=62%` is a login at
its session limit with 38% of its week left. A login at its weekly limit
shows `w=100%` and the session it read, for example `s=30%,w=100%`, and
`w=?` means no weekly figure was read. Either way the login has headroom
0 and is not chosen until the window resets. See
[Exhausted figures](../reference/profile-command.md#rotate-exhausted-figures).

A login whose read had not finished when the cycle stopped waiting shows as
`<name>:running`, followed by `reading: <name> probe running since <n>s (not used)`.
Its usage is unknown for that cycle, as for `failed`; see
[Readings that take longer than a cycle](#readings-that-take-longer-than-a-cycle).

To see why a probe fails, run `amplihack profile usage --raw`. If the state
is `timeout`, see
[When the host is heavily loaded](#when-the-host-is-heavily-loaded).

## When the host is heavily loaded

The rotator measures each login as
[`profile usage`](claude-profiles.md#check-usage-across-profiles) does:

1. it uses a reading the status line stored in the last 300 seconds, if
   there is one;
2. otherwise it asks Anthropic's OAuth usage endpoint directly, with the
   login stored in the profile, which takes well under a second and starts
   no `claude` process;
3. only if that fails does it start `claude`, first with a `get_usage`
   request and then with `claude -p /usage`.

The last step is the slow one: `/usage` reads every transcript the profile
can see, which can take a minute or more of CPU on a busy host. A profile
marked as overflow is always probed with `/usage` alone, because only
`/usage` shows a used-up spend limit. So each login's `claude` probe gets
120 seconds, and only a few run at once: between 2 and 4, fewer as the load
average rises towards the number of CPUs. Direct reads have their own
bound, 8 by default. The live login counts as one of each. The formula is
in the [reference](../reference/profile-command.md#probe-parallelism).

If probes still time out, give them longer, run fewer at once, or both. The
rotator reads these variables from the environment it was started in, so
set them in the shell that starts it, then restart it:

```bash
export AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS=300
export AMPLIHACK_PROFILE_USAGE_PARALLEL=1
amplihack profile rotate stop
amplihack profile rotate start --interval 120 --session-at 70
```

A rotator that a launch [started automatically](#start-the-rotator-automatically)
has that launch's values, so restart it the same way. Changing a variable
in another shell does nothing to a running rotator.

| Variable | Values | Default |
|----------|--------|---------|
| `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS` | `1` to `600` seconds | `120` |
| `AMPLIHACK_PROFILE_USAGE_PARALLEL` | `1` to `16` | derived from CPUs and load |
| `AMPLIHACK_PROFILE_USAGE_DIRECT_PARALLEL` | `1` to `16` | `8` |
| `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS` | `0` to `3600` seconds; `0` turns passive readings off | `300` |
| `AMPLIHACK_PROFILE_USAGE_PROBE` | `direct`, `auto` or `text` | unset: passive, direct, then `claude` |
| `AMPLIHACK_PROFILE_USAGE_API_BASE` | an `https://` URL, or `http://` on a loopback address | `https://api.anthropic.com` |
| `AMPLIHACK_PROFILE_USAGE_TOKEN_URL` | an `https://` URL, or `http://` on a loopback address | `https://platform.claude.com/v1/oauth/token` |

On an idle host with many profiles that need `claude`, a higher
`AMPLIHACK_PROFILE_USAGE_PARALLEL` makes each cycle shorter.
`AMPLIHACK_PROFILE_USAGE_PROBE=direct` starts no `claude` to measure a
login: a login whose direct read fails reads as failed for that cycle, and
overflow logins always do, with `direct read: overflow needs /usage`, so the
rotator never moves onto one. The one `claude` it still starts is the
spend-limit check of a held cycle whose pinned live login is an overflow
profile; see [Pin and overflow](../reference/profile-command.md#pin-and-overflow). `AMPLIHACK_PROFILE_USAGE_PROBE=auto` goes back to the behaviour before
direct reads (the `get_usage` request, then `/usage`), and `text` uses
`/usage` alone; use them only if the direct read gives wrong figures (see
[Compare the two probe methods](claude-profiles.md#compare-the-two-probe-methods)),
because every probe then starts `claude`. An empty value is treated as
unset. An invalid value is reported once per cycle in `rotate.err` beside
the log, and the default is used instead.

When a direct read fails and the login is measured with `claude` instead,
the rotator writes nothing about it to its log or `rotate.err`; the cycle
line shows the figures as usual. To see why a login fell back, run
`amplihack profile usage --raw <name>`.

The direct read may refresh a candidate's expired token, under Claude's own
refresh lock, and write the new token back to that profile's
`.credentials.json`. It never refreshes a login lent to or from another
directory; `claude` does that itself. The login in `~/.claude` is refreshed
once per cycle only when its token has already expired, so that its
[identity can be read](../reference/profile-command.md#an-expired-live-login).
The details are in the
[reference](../reference/profile-command.md#token-refresh).

### Readings that take longer than a cycle

A cycle does not wait for every reading. It waits until 2 seconds before
`--interval` is up, counted from the start of the cycle, and at least
1 second. A login whose reading has not finished by then shows as
`<name>:running` in the cycle line and counts as unmeasured for that cycle.
Its reading keeps running, and the next cycle shows the result, with its
age counted from when the reading started, but does not decide on it:
only readings taken in the cycle that uses them are fresh. No second
reading of that login starts while one is outstanding, and the cycle after
that reads it as usual. With `--interval 120`:

```
2026-11-01T10:00:00Z cycle live=work work:s=41%,w=30%,age=2s spare:running third:s=5%,w=12%,age=2s
2026-11-01T10:00:00Z reading: spare probe running since 118s (not used)
2026-11-01T10:02:01Z cycle live=work work:s=43%,w=30%,age=2s spare:stale third:s=5%,w=12%,age=2s
2026-11-01T10:02:01Z reading: spare s=0% w=8% stale age=239s (not used)
```

A reading still outstanding after twice the probe deadline plus 5 seconds
(245 seconds by default) is given up, and the next cycle starts a new one.
When the rotator stops, it waits a little over the deadline for any token
refresh in progress, so that it does not leave Claude's refresh lock
behind.

Cycles do not overlap: the next one starts `--interval` seconds after the
previous one ends.

Each cycle stores the readings it took, and later cycles use a stored
reading while it is younger than `AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS`
(300 seconds by default). With `--interval 120`, a login is therefore asked
afresh only about every third cycle. A stored reading can rank a login, but
the rotator does not swap to it on that reading alone: a target read more
than 300 seconds ago, earlier than the live login, or not since the rotator
left it full, is read again first (the way `AMPLIHACK_PROFILE_USAGE_PROBE`
says, so never directly under `auto` or `text`), and a login it left full is not
chosen again until a later reading shows room. The `reread:` and `damp:` log
lines show this; see
[Rotate reading freshness and swap-back damping](../reference/profile-command.md#rotate-reading-freshness-and-swap-back-damping).
For the rotator to react sooner, lower the age in the shell that starts it:

```bash
export AMPLIHACK_PROFILE_USAGE_PASSIVE_MAX_AGE_SECS=100
amplihack profile rotate stop
amplihack profile rotate start --interval 120 --session-at 70
```

A value just under `--interval` makes every cycle ask each login again, at
the cost of one direct read per login per cycle; `0` does the same and also
ignores the status line's own readings.

## Which profiles can take over

A profile is skipped as a target when:

- it is the live profile;
- it has no `.credentials.json` of its own that can be moved (a regular file
  owned by you with one link);
- it is in the auto-switch exclusion list
  (`amplihack profile auto-switch exclude <name>`);
- a Claude session is running with that profile's directory as its
  `CLAUDE_CONFIG_DIR`, because moving its login would break that session;
- auto-switch has its login on loan to, or parked away from, another
  session.

To keep a profile out of rotation for good, exclude it:

```bash
amplihack profile auto-switch exclude review
```

A profile marked as overflow is taken over only by the rules in
[Use usage credits when every plan login is exhausted](#use-usage-credits-when-every-plan-login-is-exhausted),
never as an ordinary target.

## Read the log

The log is `<root>/.auto-switch.d/rotate/rotate.log`. Each line is a UTC time
and a message. When it passes 1 MiB it is renamed to `rotate.log.1`, replacing
the previous one.

```
2026-10-18T13:00:05Z cycle live=spare spare:s=4%,w=13%,age=2s personal:s=20%,w=35%,age=2s work:s=88%,w=41%,age=2s
2026-10-18T17:10:05Z swap spare->personal live s=86% w=30% age=2s target s=0% w=36% age=2s reason=threshold
2026-10-18T19:40:05Z ALERT: no profile has capacity live=personal s=97% w=52%
2026-10-18T19:50:05Z skip: state lock busy
2026-10-20T09:00:05Z stay: live probe failed (1/3)
```

A login at its session limit keeps its weekly figure in the log. Here
`work` used its five-hour window with 62% of its week used, and the rotator
moved to `personal`:

```
2026-11-01T14:40:05Z cycle live=work work:s=100%,w=62%,age=2s personal:s=20%,w=35%,age=2s
2026-11-01T14:40:05Z swap work->personal live s=100% w=62% age=2s target s=20% w=35% age=2s reason=threshold
```

To follow one login's weekly use across cycles, including cycles where it
was exhausted:

```bash
grep -oE ' work:s=[0-9]+%,w=[0-9]+%' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

To see only swaps and alerts:

```bash
grep -E ' (swap|ALERT:) ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

`change:` lines record who changed the autostart setting or the auto-switch
toggle; see [See who turned rotation off](#see-who-turned-rotation-off).

The same no-capacity ALERT is written to the log once. It is written again
only after something changes: a swap succeeds, a cycle needs no swap, or the
reason changes. The one-shot `rotate` still prints it on every run. The
ALERTs for a [manual `/login` in `~/.claude`](#after-a-manual-login-in-claude),
their `fix` lines, the `has no login anywhere` line and the
[exhausted-while-off](#see-who-turned-rotation-off) ALERT are written on
every cycle until the problem is resolved. Swap refusals are limited to
once an hour; see [Repeated alerts](#repeated-alerts). The log
holds profile names, percentages, ages, fixed text and, in `recovered:` and
`fix` lines, paths under your home directory and the profile root.
`change:` lines also hold a redacted parent command line and run ids. Swap
refusals and `repair:` lines hold the e-mail address each source reports,
so that you can see which source disagreed. Earlier releases never wrote
an address to this log, so check the log before you share it. The log
never holds account IDs, organization IDs, tokens or Claude's output.

### Reading ages in the log

Every reading carries its age in seconds. A reading older than
`--interval` (and never more than 300 seconds) is *stale*: the rotator
shows it but does not decide on it. A failed probe shows its reason, never
the figures it read before.

| In the `cycle` line | Followed by | Meaning |
|---------------------|-------------|---------|
| `work:s=41%,w=97%,age=35s` | nothing | A fresh reading, used for the decision. |
| `work:stale` | `reading: work s=41% w=97% stale age=7320s (not used)` | Too old to use. |
| `work:failed` | `reading: work probe failed: http 429` | The probe failed; there are no figures. |
| `work:running` | `reading: work probe running since 130s (not used)` | A read from an earlier cycle has not finished. |

When the live login's own reading is stale, the cycle makes no swap on it
and writes `stay: live reading stale age=<n>s (not used)`. The next cycle
reads it again as usual.

To see every reading that was not used:

```bash
grep -E ' reading: | stay: live reading stale' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

The formats and the age limit are in
[Reading display](../reference/profile-command.md#reading-display).

### Repeated alerts

A swap refusal, a `repair:` line and a `stay: no login with room` line are
written at most once an hour for the same cause and profile. The rotator
counts the repeats it does not write, and the next line written gives the
count:

```
2026-11-07T09:14:05Z ALERT: swap refused (account-mismatch, profile nnorth): ...
2026-11-07T10:14:05Z ALERT: swap refused (account-mismatch, profile nnorth): ... (repeated 29 times since 2026-11-07T09:14:05Z)
2026-11-07T11:14:05Z ALERT: swap refused (account-mismatch, profile nnorth): ... (repeated 29 times since 2026-11-07T10:14:05Z)
```

The count and the time in the suffix cover the period since the line was
last written.

The rotator still tries the swap on every cycle. Once the cause is gone,
the swap goes ahead in that cycle, and the next occurrence of the same
cause is written straight away. `amplihack profile rotate status` and the
ledger's `alert` field show the current state on every cycle, whatever the
log shows. See
[Rotate alert limit](../reference/profile-command.md#rotate-alert-limit).

### Hand-change notes and alerts

These lines come from the check for a
[manual `/login` in `~/.claude`](#after-a-manual-login-in-claude). The
rotator reads the OAuth profile endpoint with the login in `~/.claude` and
with each profile's own login, and compares their account identities.
Session JSON and usage reset times cannot establish ownership. Hand-change
recovery does not run for 10 minutes after a swap (`SWAP_GRACE`, 600
seconds); swap authorization still checks trusted identity during that time.

To see only these lines:

```bash
grep -E ' (note: |recovered: |ALERT: (the account in ~/.claude |~/.claude holds the same )|fix [0-9]+: )' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

| Line | What it means | What to do |
|------|---------------|------------|
| `note: ~/.claude holds the same account as profile <M> (ledger names <L>); nothing moved, the next cycle reads again` | First of two readings. The cycle does not swap. | Nothing. The next cycle reads again. |
| `note: cannot tell whose login is in ~/.claude (live=<L>): <why>; nothing moved` | A failed read, more than one match, or an account that is not the recorded one. Required uncertain evidence blocks login movement and ledger relabeling. | Nothing, unless it persists; then see the `<why>`. |
| `recovered: ~/.claude and <M> held the same account; kept ~/.claude, moved <M>'s copy to <path>; recorded <M> as live` | Two readings matched and `<L>` keeps its own login, so `<M>`'s copy was moved to a backup. | Nothing. |
| `ALERT: ~/.claude holds the same account as profile <M> on two readings (ledger names <L>), but <L> has no login of its own, so recording <M> would leave <L> with no login anywhere; nothing moved, no swap` | The only-login rule refused. Both logins stay. | Follow `fix 1` or `fix 2`. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<L>); profile <M> holds the same account and has its own login; recovery refused: <reason>; no swap` | Two readings matched, but a check before the move failed. | Follow its `fix 1` line. |
| `ALERT: the account in ~/.claude is not the one the ledger names (live=<L>); no swap` | A suspicion is open, so the cycle does not swap. The identity refusal remains visible on each affected cycle. | Nothing; it follows a note or ALERT above. |

The `<why>` of a cannot-tell note is one of:

| `<why>` | Usual cause |
|---------|-------------|
| `~/.claude identity read failed: <cause>` | The lookup failed, and `<cause>` says why: for example `http 429` (wait), `http 401` (log in again), `token expired` (the refresh failed; see its `refresh:` line) or `directory not private` (see the [directory rule](../reference/profile-command.md#login-directory-rule)). All causes are in [Identity read causes](../reference/profile-command.md#identity-read-causes). |
| `profile <P1> not read (<cause>), <P2> not read (<cause>)` | Those profiles' lookups failed, each for the cause shown. |
| `it holds the same account as more than one profile (<M1>, <M2>)` | Those profiles hold the same account; keep it in one of them. |
| `it holds the same account as profile <M>, which is not the account recorded for <M>; run amplihack profile verify` | `<M>`'s folder holds another profile's login. See [Check which account each folder holds](#check-which-account-each-folder-holds). |
| `it holds an account other than the one recorded for <L>; run amplihack profile verify` | `~/.claude` holds an account no profile's own login holds, and not `<L>`'s. Usually a `/login` to a new account. See [Check which account each folder holds](#check-which-account-each-folder-holds). |
| `the reads ran out of time` | The reads took more than 30 seconds in all. |
| `the last swap time is later than now` | The clock went back. Recovery stays off until 10 minutes after the last swap time. |

A note is written once and not again until its text changes, so a lasting
cause writes one note, not one per cycle. When uncertainty blocks a
required ownership check, its identity-refusal ALERT still repeats on
every affected cycle. ALERT and `fix` lines are written on every cycle
while they hold. None of these lines holds a token, an
account ID or an e-mail address. The full rules are in
[Rotate hand-change recovery](../reference/profile-command.md#rotate-hand-change-recovery).

## Resume sessions stopped by a usage limit

A swap only helps a session that sends another request. A session that has
already stopped with `You've hit your session limit` sits idle until someone
types into it. amplihack's `limit-resume` hook restarts it: when a turn ends
on a usage limit, the hook waits for the rotator to finish a cycle that leaves
a login with capacity in `~/.claude`, then tells the session to continue.

You need:

- Claude Code 2.1.306 or later (`claude --version`). For an older `claude`,
  `amplihack install` leaves the hook out and says so: Claude Code before
  2.1.78 does not know `StopFailure` and ignores a settings file that names
  it. If an earlier amplihack already registered it, the next `amplihack`
  launch of Claude Code removes it, and an automatic re-stage after an
  upgrade warns and names `amplihack install`. Launches of copilot or codex
  do not check the `claude` version and leave the file as it is.
- `amplihack install` run with this version of amplihack, which registers the
  hook in `~/.claude/settings.json`.
- Auto-switch on and the rotator running:
  ```bash
  amplihack profile auto-switch on
  amplihack profile rotate start
  ```

Check that the hook is registered:

```bash
jq '.hooks.StopFailure' ~/.claude/settings.json
```

```json
[
  {
    "matcher": "rate_limit",
    "hooks": [
      {
        "type": "command",
        "command": "/home/you/.local/bin/amplihack-hooks limit-resume",
        "timeout": 608400,
        "asyncRewake": true
      }
    ]
  }
]
```

### What happens on a limit

1. The turn ends with a usage-limit error, and Claude Code runs the hook in
   the background.
2. If no rotator is running, the hook exits at once (`rotator-down`).
   Otherwise it reads the rotator's ledger every 15 seconds. It waits for a
   rotator cycle whose measurement, or the reading behind it, was taken after
   the limit hit and that ended `swapped` or `stayed` with no ALERT, or for a
   [`profile switch`](#switch-to-a-login-by-hand) made after the limit hit. After the first hour it reads the ledger
   every 5 minutes instead. It stops waiting if you turn auto-switch off
   (`disabled`), if you send a prompt in that session during the wait, for
   example `continue` (`moved-on`), or if the session exits
   (`session-gone`). Background task reports and `/usage` are not prompts
   and do not stop it.
3. When it sees one, the hook exits `2`, and the session gets this message
   and starts a new turn:
   ```
   amplihack: the login rotator reports a login with capacity. Continue the task you were working on; if the usage limit is still in place, stop and wait.
   ```
4. If no such cycle comes within 7 days, the longest any usage limit lasts,
   the hook exits quietly and the session stays stopped. So a session that
   stops while every login is spent resumes when the first login resets,
   even a weekly limit days away.

Steps 1 and 3 depend on Claude Code running `StopFailure` instead of `Stop`
for a limit stop and starting a turn when an `asyncRewake` hook exits `2`.
Verified on Claude Code 2.1.307: a weekly-limit stop runs `StopFailure` with
`error: "rate_limit"` and the limit text in `last_assistant_message`, and an
`asyncRewake` exit `2` closes the limit menu and starts a new turn.

The hook trusts the rotator's verdict. It never applies the thresholds itself
and never reads a login.

The rotator records a stay as `stayed` only when it read the live login in
that cycle and the reading was below 100% on both the session and the week
window. A stay after a failed, carried or unknown live probe, for example
the line `stay: live probe failed (1/3)`, is recorded as `stayed-unread`,
and the hook keeps waiting. When every other login has passed a threshold
but the live login still reads below 100%, the rotator writes the
`ALERT: no profile has capacity` line once and records `stayed`, so the
session resumes on the login it already has. When the live login is at 100%,
at a spend limit, or was not read in this cycle (a failed, carried or
unknown probe), the ALERT blocks and the hook waits. A stay on an overflow
profile that was read in that cycle runs on credits, so it records `stayed`
whatever its plan figures. The full rule is in
[Stays and capacity](../reference/profile-command.md#rotate-stays-and-capacity).

### See what it did

Each decision is one line in `<root>/.auto-switch.d/resume/resume.log`:

```bash
tail ~/.claude-profiles/.auto-switch.d/resume/resume.log
```

```
2026-10-21T14:12:46Z resume session=3f9a1c2b7e04 event=7d41e0a9c2b1 outcome=resumed live=spare cycle=swapped
2026-10-21T16:40:02Z resume session=3f9a1c2b7e04 event=a0c55e1f9d38 outcome=no-capacity-timeout live=work cycle=alerted
```

`no-capacity-timeout` usually means every login was at capacity, so the
rotator wrote `ALERT: no profile has capacity`. `rotator-down` means the
rotator was not running when the limit hit; start it with
`amplihack profile rotate start`.
`moved-on` means you sent a prompt in that session during the wait. The other outcomes are listed in the
[resume log reference](../reference/profile-command.md#resume-log).

### Why it cannot loop

- **One resume per limit event.** A second hook run for the same session, the
  same error text and the same live login logs `duplicate`. A limit after the
  rotator swapped to another login is a new event.
- **One wake per wait.** When two hooks wait for the same session, only the
  first to see capacity wakes it; the other logs `duplicate`.
- **A fresh cycle every time.** Each resume needs a rotator cycle recorded
  after the limit hit, so one session resumes at most once per `--interval`.
- **No resume without capacity.** When every login is at its limit the
  rotator records an ALERT, and the hook does nothing. A stay whose live
  probe failed is not capacity either.
- **At most 3 resumes per session in 24 hours.** After that it logs `cap`.

If anything it needs cannot be read or written safely, it does not resume.

### Limits during a wait, after a resume, and in Claude Code's timed wait

The hook acts only when Claude Code runs it, on a `StopFailure` event with
`error: "rate_limit"`. Every run it gets writes one line to `resume.log`,
including `duplicate`. A limit stop with no line in `resume.log` is one for
which Claude Code did not run the hook.

- **A limit while a hook for the same session is waiting.** Claude Code
  starts a second hook. Its event differs in the error text or the live
  login, so it waits as well. The first to see capacity wakes the session;
  the other logs `duplicate`.
- **A limit just after a resume.** The same text on the same live login is
  the same event: the hook logs `duplicate` and does not wake the session
  again. A limit after the rotator swapped to another login is a new event,
  and the hook waits for a fresh cycle.
- **A stop on one login, then a swap to another.** Cycles on the first login
  that are not capacity (`alerted`, `stayed-unread`) do not wake the
  session. The swap to the second login does, once, and the log line reads
  `live=<second login> cycle=swapped`.

Some Claude Code versions show their own timed wait at a limit:
`Usage limit reached · Continuing automatically at <time>`. It has not been
established whether Claude Code runs `StopFailure` when it enters that wait,
or whether the hook's wake ends it. On 3 November 2026 sessions in that wait
were not woken after a swap to a fresh login
([#1907](https://github.com/rysweet/amplihack-rs/issues/1907)). The hook
sends no keystrokes and never chooses a menu option, so it cannot leave that
wait by itself. To continue such a session before the time shown:

1. Press Esc.
2. Check that the input box is empty. Text drawn dimmed is Claude Code's
   predicted next prompt, not yours; clear it with Ctrl+U.
3. Type `continue` and press Enter.

If Esc does not end the wait, the session continues by itself at the time
shown. Establishing what the timed wait accepts, and resuming from it, is
tracked in [#1932](https://github.com/rysweet/amplihack-rs/issues/1932).

### Stop using the tmux nudger

The hook replaces the tmux stopgap that watched panes and typed "please
continue" into them (`~/.claude-profiles/.backups/limit-nudger.sh`). Once the
hook is registered, stop the nudger so a session is not told to continue
twice:

```bash
pkill -f limit-nudger.sh
```

Newer Claude Code versions show a `What do you want to do?` menu at a limit.
Whether the hook's wake gets past that menu has not been tested yet. If a
session stays on the menu after `resume.log` shows `resumed`, press Esc and
type `continue` yourself, or keep the nudger for that case. If you pick the
menu's own "Wait here, then continue automatically" option, both it and this
hook may continue the session: at most one extra short turn, bounded by the
per-session cap.

### Turn it off

`amplihack profile auto-switch off` turns the hook off together with
background rotation and auto-switch. With auto-switch off the hook exits at
once, prints nothing, and reads only the saved root and the toggle. To keep rotation but not
the resume, delete the `StopFailure` entry from `~/.claude/settings.json`.
`amplihack install` adds it again.

## See how much capacity is left

`amplihack profile usage` ends with a summary across every login. After
the table, it prints:

```
weekly headroom      2.07 login-weeks of 4 (52%)
usable now           2 of 4 logins (session < 95% and weekly < 100%)
next session reset   2026-10-30T18:00:00Z  (night, team: +2 usable)
runway               ~9 h at the last 6 h rate (0.230 login-weeks/h)
resets in next 24 h  2026-10-31T02:00:00Z night +0.85; 2026-10-31T09:00:00Z work +0.30
unknown              2 profiles (no valid reading; excluded from all figures)
```

A login-week is one login's weekly allowance, so `2.07 login-weeks of 4`
means a little over two logins' worth of weekly capacity is left across the
four logins that could be read. Logins that could not be read are counted
as unknown and left out of the figures; they are never counted as empty or
full. The rows above the summary show which they are.

The runway needs about six hours of history. Each rotator cycle and each
`profile usage` run without profile names adds a line to
`<root>/.auto-switch.d/usage-history.jsonl`. Until there is enough it reads
`unknown`. The rate is per hour and uses between four and a half and six
hours of history. It is each login's net rise in weekly use, weighted by
its [capacity weight](../reference/profile-command.md#capacity-weights).
A dip of 10 points or less, such as a stale status-line figure, adds
nothing. It starts from a `profile usage` line, or from a rotator line
from a cycle that skipped none of the logins read now. A
rotator cycle does not probe logins that are running a session, so while
some are busy, run `amplihack profile usage` now and then (every few hours)
to give the runway a starting point.

`amplihack profile rotate status` prints the same figures in one
`capacity:` line from the rotator's last cycle, without probing anything.
That cycle may have measured only some logins, so its `of N` can be smaller
than in `profile usage`; the logins it skipped as busy, and any whose
login has no usable trusted identity, are counted in its `unknown`.
When that cycle left logins out, the line does not use it for the runway:
it shows the runway of the newest line that read every login, with that
line's time (`runway ~14 h from the <time> entry (this cycle left out 2
logins)`), or `runway unknown (this cycle left out 2 logins)` when there is
none in the last six hours.

If your logins are on different plans, give the larger ones more weight in
`<root>/.auto-switch.d/capacity-weights.json`, for example
`{ "team": 2.0 }`, and the summary adds a weighted figure. To count a login
as usable only below a lower session percentage, set
`AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT=80` or pass
`--usable-session-pct 80`.

For scripts, `amplihack profile usage --summary-json` prints the rows and the
summary as one JSON object. The details are in
[Capacity summary](../reference/profile-command.md#capacity-summary). To
clear the history, delete `usage-history.jsonl` and `usage-history.lock`.

## When no account has capacity

When the `ALERT: no profile has capacity` line is written, the rotator has
nowhere to move the login, so it stays put. Just before it, a
`stay: no login with room` line lists every login's reading with its age,
so you can see whether the others are near their limits, stale or failed:

```
2026-11-06T22:49:05Z stay: no login with room; live nnorth s=17% w=98% age=4s; ffarmer s=10% w=97% age=4s; oole s=41% w=99% age=4s; eelsworth probe failed: timeout
```

That line is written at most once an hour while the situation lasts; see
[Repeated alerts](#repeated-alerts). With an
[overflow profile](#use-usage-credits-when-every-plan-login-is-exhausted)
marked, it moves there instead when the next plan reset is far enough away.

If the live login still reads below 100% on both windows, the ALERT is
advisory: the ledger keeps it in `advisory_alert`, `alert` stays `null`,
and the cycle records `stayed`, so sessions stopped by a limit resume on
that login. Once the live login reaches 100% or a spend limit, or its probe
fails, is carried or is unknown in that cycle, the ALERT moves to `alert`
and the cycle records `alerted`. See
[Advisory alert](../reference/profile-command.md#rotate-advisory-alert).

When some candidates could not be measured, the ALERT ends with a count,
for example:

```
2026-10-29T20:36:58Z ALERT: no profile has capacity live=work s=100% w=100% (candidates: 2 failed, 0 exhausted)
```

A candidate whose probe failed is never chosen, but it does not stop a swap
to another candidate that measured with enough headroom. If every candidate
is counted as failed, the probes are at fault rather than the accounts: run
`amplihack profile usage` to see each profile's state.

The log is not the only place you hear about it. When a cycle gets as far
as deciding to swap, stay or raise the ALERT, the rotator updates the shared
capacity state from that cycle's measurements, and the status line, Claude
Code, `amplihack claude` and `amplihack recipe run` show a warning such as:

```
⚠ all Claude accounts ≥90% · next reset Oct 22 2am
```

The capacity state uses the auto-switch threshold and exclusion list, not
`--session-at` and `--week-at`, so it agrees with what the status line and
`profile usage` record. The rotator's `ALERT` can therefore appear at
different percentages from the warning. A profile whose probe failed counts
as having capacity, so it does not cause a warning.

The warning needs auto-switch to be on. The rotator rotates with no
auto-switch setting at all, but then it writes only the `ALERT` to its log
and no warning appears. To get the warning as well, run:

```bash
amplihack profile auto-switch on
```

A cycle that stops early, for example on `skip: state lock busy` or
`skip: state changed during measurement (live)`, does not update the
warning; the next full cycle does. The text in brackets names what changed;
see [Rotate revalidation](../reference/profile-command.md#rotate-revalidation).

The warning clears by itself when the first account resets. Where it
appears and how it is deduplicated is in
[When no account has capacity](claude-profiles.md#when-no-account-has-capacity);
the file and the exact rules are in the
[capacity notice reference](../reference/profile-command.md#capacity-notice).

## Use usage credits when every plan login is exhausted

If one of your accounts has usage credits turned on, the rotator can move
work onto it when every plan login is at 100% of its session or week and
the next reset is a long way off, and move work back as soon as a plan login has capacity
or room again. That account is an *overflow profile*. Every other profile is a
*plan profile*.

**Overflow costs real money.** Work on an overflow profile past its plan
limit is billed to the account's usage credits. The rotator keeps that time
short, but it is not a spending cap. The organization spend limit at
<https://claude.ai/settings/usage> is the only hard limit, so set one before
you start.

### Turn on usage credits

Usage credits are an account setting, and amplihack cannot change it. Sign
in to claude.ai as the account, open <https://claude.ai/settings/usage>,
turn on usage credits and set a spend limit. Without this, the account stops
at its plan limit like any other, and the rotator gets nothing from it.

### Mark the profile

The account needs a logged-in profile, as for rotation in general. Mark it:

```bash
amplihack profile overflow add ryan-mistt-org
```

```
overflow: added ryan-mistt-org
note: ryan-mistt-org is used only when no measured plan login is below 100% on both windows and the earliest plan reset is more than --overflow-wait away; work on it is billed to that account.
note: set a spend limit for the organization at https://claude.ai/settings/usage; the rotator is not a spending cap.
```

The mark is stored as `overflow=ryan-mistt-org` in `<root>/.auto-switch`,
next to the auto-switch exclude list, and not in `config.toml`. The rotator
and auto-switch already read that file on every cycle and every check, so a
running rotator picks the mark up without a restart, and auto-switch skips
the profile from its next check. With several overflow profiles, the rotator
tries them in the order you added them.

`add` and `remove` wait for a running rotator cycle to finish before they
write. If one holds the file for more than 10 seconds, the command changes
nothing and fails with:

```
error: .auto-switch is being changed by another process (the rotator or auto-switch); retry
```

Run it again. A `remove` that printed `overflow: removed <name>` is never
undone by a concurrent write.

Check the marks:

```bash
amplihack profile overflow list
```

```
ryan-mistt-org
```

A marked name with no profile directory under the root is listed with
`missing` after it, so a typo or a deleted profile shows up here.

Leave the default profile unset. A launch that resolves to a profile
through the default pointer uses that profile directly, whatever the rotator
does. If the default is the overflow profile, every such launch spends
credits; if it is a plan profile, those launches never follow the rotator.
`add` warns when the default names the profile. Clear it with:

```bash
amplihack profile switch --none
```

An excluded profile is never used, even when it is marked as overflow.
`add` warns when the profile is in the exclude list; remove it from the list
with `amplihack profile auto-switch exclude <the other names>`.

### Choose how long to wait for a reset

The rotator moves onto credits only when the earliest plan reset is more
than `--overflow-wait` seconds away. The default is 1800 (30 minutes). To
wait up to an hour before spending credits:

```bash
amplihack profile rotate stop
amplihack profile rotate start --overflow-wait 3600
```

`AMPLIHACK_ROTATE_OVERFLOW_WAIT` sets the same value. Accepted values are
whole seconds from `0` to `604800` (7 days); anything else is a usage error
that names the flag or the variable. `0` means credits are used as soon as
no measured plan login is below 100% on both windows. The live login still has to
reach `--session-at` or `--week-at` first.

Auto-switch uses the running rotator's wait: `rotate watch` records its
overflow settings when it starts, and every Claude session reads them from
there, so there is nothing to keep in step. Check with
`amplihack profile auto-switch status`, whose `overflow gate:` line shows
the values and says `from the rotator`. Only while no rotator runs does
auto-switch read `AMPLIHACK_ROTATE_OVERFLOW_WAIT` from its own environment.
See [Auto-switch and overflow profiles](../reference/profile-command.md#auto-switch-and-overflow-profiles).

### When no plan reset time is known

A plan profile that reads exhausted without a reset time, or whose probe
read `not_logged_in`, `unparsed` or `spend_limit`, has no known reset.
Unknown resets are left out: one known reset decides the wait. (A plan
profile whose probe timed out, or whose `claude` could not be started or
read, stops the move onto credits for that cycle altogether; see
[When a plan login cannot be measured](#when-a-plan-login-cannot-be-measured).)
When **none** is known, the rotator does not spend money by default: it
writes the no-capacity ALERT and stays on the plan login. To let an unknown
reset move work onto credits at once, opt in:

```bash
amplihack profile rotate stop
amplihack profile rotate start --overflow-unknown-reset 1
```

`AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET=1` in the environment of
`rotate start` does the same. Auto-switch follows the running rotator's
choice either way. The `overflow: on` log line then reads `next plan reset
unknown`.

### When a plan login cannot be measured

If a plan profile's probe times out, its `claude` cannot be started or
read, or its directory is refused just before the spawn, the rotator cannot
tell whether that login has capacity, so it does not move onto credits in
that cycle, however far away the known resets are. It writes the no-capacity
ALERT (or makes the free swap onto a plan login at a threshold) as it would
with no overflow profile, and probes again on the next cycle:

```
2026-10-24T09:00:05Z cycle live=work work:s=97%,w=91%,age=2s personal:failed ryan-mistt-org:s=100%,w=100%,age=2s
2026-10-24T09:00:05Z ALERT: no profile has capacity live=work s=97% w=91% (candidates: 1 failed, 1 exhausted)
```

A plan probe that timed out or could not run never moves work onto
credits. If the probes keep timing out, see
[When the host is heavily loaded](#when-the-host-is-heavily-loaded).

A plan profile whose directory fails the ownership and mode check (it is a
symlink, is not yours, or is open to group or other users) is skipped
before any probe:

```
2026-10-24T09:00:05Z skip: personal (directory refused)
```

Its login cannot be checked, so it might have capacity. The rotator never
chooses it, and does not move onto credits on any cycle until the directory
is fixed. `amplihack profile usage personal` shows `error` with `profile
directory refused`. Run `chmod 700` on the directory (or recreate it if it
is not yours) to bring the profile back into rotation and release the hold.
A directory that changes between that check and the probe reads `error` in
the cycle line instead, and holds credits back in the same way. An excluded
profile is skipped as `excluded` whatever its directory, and an overflow
profile with a refused directory is skipped; neither holds anything back.

A profile that is not logged in, whose `/usage` output cannot be read
(`unparsed`) or that is at its spend limit does not block credits: the
login reported that state, and it would otherwise keep the rotator off
credits for good. Such a profile is never chosen; fix it or remove it.

The rule stops only a move from a plan login onto credits. If the overflow
profile you are on hits its spend limit, the rotator still moves to the next
overflow profile, and it moves back to a plan login as soon as one reads
capacity.

Auto-switch does not apply this rule. It decides from the readings it has
recorded, and a probe that timed out or could not run is never recorded.

### Rank overflow profiles

Each overflow mark has a tier from 1 to 9 (default 1). Lower tiers are tried
first, so a cheaper kind of overflow comes before a dearer one:

```bash
amplihack profile overflow add ryan-mistt-org          # tier 1
amplihack profile overflow add team-credits --tier 2   # tried after tier 1
amplihack profile overflow list
```

### What the rotator does

On each cycle:

1. While the live login is a plan profile, the rotator decides as usual.
   When it would write `ALERT: no profile has capacity`, it first tries the
   [plan-room swap](../reference/profile-command.md#rotate-plan-room-swap):
   a move to the plan login below 100% on both windows with the most room.
   Only when no measured plan login, the live one included, has room does it look
   at overflow: if the earliest plan reset is more than `--overflow-wait`
   away, no plan profile's probe timed out or could not start in this cycle,
   and an overflow profile can be used, it swaps to the first one by
   tier with `reason=overflow-in`. A failed live probe never leads to
   credits.
2. While the live login is an overflow profile, it checks every plan
   profile. As soon as one has capacity, or failing that has room, it swaps
   back with `reason=overflow-out`, without waiting for `--hold` or needing
   `--margin` or `--upgrade-margin`. Otherwise it stays.

A plan profile *has capacity* when its probe succeeded and its session and
week use are both below `--session-at` and `--week-at`. It *has room* when
its probe succeeded and both are below 100%. Credits are billed, so the
rotator never moves onto them while a measured plan profile has room, and it
moves back as soon as one has. It does not bounce: from the plan login it
moves to, it returns to credits only once no plan login has room again.

Not every plan login is measured. One with a session on it, or leased,
hosting, parked or with no account, is skipped; one whose probe failed gave
no reading. Either may have room. Credits do not wait for a skipped login or
one whose probe read a state such as `not_logged_in`, and the `per-token` line
says how many there were (`2 not measured`). Credits do wait for a probe that
timed out or could not run, and for a plan profile skipped as
`directory refused`: that login might have capacity. The API key waits for
all of these: see [The order the rotator follows](#the-order-the-rotator-follows).

Earlier releases moved onto credits as soon as every plan login had reached
a threshold. They now wait until no plan login is below 100%; until then
the rotator spreads work over the plan logins with room.

An overflow profile's own session and weekly limits show as exhausted in
`profile usage`. That does not stop it being used, by the rotator or by
auto-switch; those are the plan limits the credits cover. Only a
[spend block](../reference/profile-command.md#overflow-spend-limit-block)
moves work from one overflow profile to the next.

The log shows each move. Auto-switch writes its own moves onto a per-token
profile to the same log, as a `per-token` line that ends `auto-switch from
<profile>`:

```
2026-10-24T09:00:05Z cycle live=work work:s=100%,w=91%,age=2s personal:s=100%,w=100%,age=2s ryan-mistt-org:s=100%,w=100%,age=2s
2026-10-24T09:00:05Z per-token ryan-mistt-org: no measured plan login below 100%, 0 not measured; earliest reset 2026-10-24T14:00:00Z beyond overflow_wait; marked overflow tier 1; probe ok, no spend limit
2026-10-24T09:00:05Z swap work->ryan-mistt-org live s=100% w=91% age=2s target s=100% w=100% age=2s reason=overflow-in
2026-10-24T09:00:05Z overflow: on ryan-mistt-org; no measured plan login below 100%, next plan reset 2026-10-24T14:00:00Z
2026-10-24T14:10:05Z swap ryan-mistt-org->personal live s=100% w=100% age=2s target s=0% w=62% age=2s reason=overflow-out
2026-10-24T14:10:05Z overflow: off ryan-mistt-org; back on plan profile personal
```

To see only overflow moves:

```bash
grep -E 'reason=overflow|^\S+ per-token |overflow: (on|off|staying)|spend limit|key_check_refused' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

### See whether you are running on credits

```bash
amplihack profile rotate status
```

```
root: /home/you/.claude-profiles (source: config)
rotate: running
autostart: on (default)
owner: none
pid: 48213
live: ryan-mistt-org
last cycle: 2026-10-24T09:10:05Z stayed overflow=on
overflow: ryan-mistt-org
on overflow: yes (ryan-mistt-org)
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

`on overflow: yes` needs two things: the login in `~/.claude` is an overflow
profile, and the rotator's last cycle recorded `overflow=in` or
`overflow=on`, meaning it found no plan capacity. If you put the overflow
login in `~/.claude` yourself while a plan login still has capacity, the line
reads `on overflow: no (ryan-mistt-org is overflow; plan capacity remains)`
until the next cycle moves it out. After `amplihack profile switch
ryan-mistt-org`, the line reads `on overflow: yes (ryan-mistt-org, held by
switch)`: the hold keeps you on credits until `amplihack profile switch
--none`. `status` runs no probe.

When the default pointer names an overflow profile, `status` adds a warning
line:

```
default: ryan-mistt-org (overflow; launches use it directly)
```

`profile usage` marks the profile `overflow` and, when its login is in
`~/.claude` and its own probe in that run shows the session or all-models
weekly limit reached, ends with `running on overflow: ryan-mistt-org`. A live overflow
login that still has plan capacity is not on credits and gets no footer. While the
rotator is on credits it reports capacity, so the no-capacity warning clears
and the [limit-resume hook](#resume-sessions-stopped-by-a-usage-limit)
resumes sessions stopped by a limit. Those sessions then run on credits.

### When the credits run out

Claude reports `You've hit your org's monthly spend limit …` (older
versions say `Your organization has reached its monthly spend limit`) when
the credits are used up or turned off. The rotator's usage probe recognises
both and runs against the live overflow profile on every cycle, so it sees
the message within one cycle. When the probe reads this for an overflow
profile, it blocks that profile until 00:00 UTC on the 1st of the
next month, writes one ALERT, and moves to the next overflow profile, a plan
profile, or nowhere:

```
2026-10-24T11:20:05Z ALERT: overflow profile ryan-mistt-org hit its spend limit; blocked until 2026-11-01T00:00:00Z
```

It does not retry the profile on every cycle. After raising the spend limit
or adding credits, lift the block:

```bash
amplihack profile overflow clear ryan-mistt-org
```

```
overflow: cleared the spend block on ryan-mistt-org
```

`clear` waits up to 10 seconds for a running cycle and otherwise fails with
`rotator busy, retry`; run it again.

The block depends on the spend-limit message appearing in the output of
`claude -p /usage`. An overflow profile needs its usage credits turned on
(see [Turn on usage credits](#turn-on-usage-credits)), and a profile marked
as overflow is always probed with `/usage`, so the rotator can see that
message. With
credits off there is no spend limit to reach: the account stops at its plan
limit like a plan login. If sessions on the overflow profile fail with the
spend-limit message but no block appears in `overflow list`, the rotator
cannot tell, and you have two ways out:

- `amplihack profile overflow remove <name>` makes it a plan profile. A
  spend-limit reading then counts as a failed probe, so the rotator swaps it
  out only after three failed cycles in a row (30 minutes at the default
  interval), and only to a plan profile.
- `amplihack profile rotate stop` stops all swapping and leaves that login
  in `~/.claude` until you start the rotator again.

### Turn overflow off

**Removing the mark does not stop spending.** If the overflow profile's
login is in `~/.claude` and no plan profile has the margin, the rotator
writes `ALERT: no profile has capacity` and leaves the login there, and
Claude keeps billing work on it to the credits. Stopping the rotator leaves
the login where it is too.

If `rotate status` shows `on overflow: yes`, stop the spending first: turn off
usage credits or lower the spend limit at
<https://claude.ai/settings/usage>. Claude then refuses work on that login,
and while the mark is still there the rotator blocks the profile and moves
away from it, as in [When the credits run out](#when-the-credits-run-out).
Ending the Claude sessions on that login also stops spending until the next
launch.

Then remove the mark:

```bash
amplihack profile overflow remove ryan-mistt-org
```

```
overflow: removed ryan-mistt-org
```

The next cycle treats the profile as a plan profile again. If its login is
in `~/.claude` at that moment, it is swapped out by the usual rules: because
its plan limits are exhausted, that is a threshold swap whenever another
profile has the margin.

If you move an overflow profile's login into `~/.claude` yourself while a
plan profile has capacity, the rotator moves it back out on the next cycle.
To stay on it, remove the mark or stop the rotator.

### Upgrading and rolling back

Older `amplihack` binaries do not know the new settings:

- they count a `.auto-switch` that has an `overflow=` line as corrupt, so
  auto-switch is off for them and the exclude list is ignored;
- they reject a rotate ledger that has overflow fields.

Upgrade every `amplihack` binary that uses the root, including the one the
rotator and the hooks run, and then restart the rotator:

```bash
amplihack profile rotate stop
amplihack profile rotate start
```

To go back to an older binary:

1. Note every marked profile and every block. `overflow list` shows blocks
   only for marked profiles, so run it before removing anything:
   ```bash
   amplihack profile overflow list
   ```
2. Make sure the login in `~/.claude` is not on credits. If `rotate status`
   shows `on overflow: yes`, follow
   [Turn overflow off](#turn-overflow-off) first.
3. Remove each mark, then lift each block. `clear` works on a name that is
   no longer marked:
   ```bash
   amplihack profile overflow remove ryan-mistt-org
   amplihack profile overflow clear ryan-mistt-org
   ```
4. Stop the rotator, then run one cycle by hand. With no marks left, the
   cycle records `last_cycle` without the `overflow` field:
   ```bash
   amplihack profile rotate stop
   amplihack profile rotate
   ```
5. Check that nothing is left. `rotate status` must show a `last cycle:`
   line with no `overflow=` suffix and `overflow: -`, and the ledger must
   have no overflow keys:
   ```bash
   amplihack profile rotate status
   grep -cE '"overflow(_blocked)?"' ~/.claude-profiles/.auto-switch.d/rotate/ledger.json
   ```
   The `grep` prints `0`. If the `last cycle:` line still ends in
   `overflow=`, the cycle was skipped (for example `skip: state lock busy`);
   run `amplihack profile rotate` again.

Then install the older binary and start the rotator.

The exact rules are in the
[overflow reference](../reference/profile-command.md#rotate-overflow).

## Move from the tmux prototype

The rotator refuses to run while the external prototype holds
`<root>/.rotator/lock`:

```
error: the external login rotator holds <root>/.rotator/lock
hint: stop the external rotator (tmux session claude-rotator) first
```

To move over:

1. Stop the prototype: `tmux kill-session -t claude-rotator`.
2. Preview: `amplihack profile rotate --dry-run`. It runs even while the
   prototype is running.
3. Start: `amplihack profile rotate start`.

On its first run the rotator learns which profile is live from the
prototype's `<root>/.rotator/ledger.json`, but only if the profile it names
has no login of its own and, where that profile records an account, the
account matches the one in `~/.claude.json`. Otherwise it writes
`skip: prototype ledger ignored (<reason>)` and finds the live profile by
account, or adopts the login as described below. It only reads that file.
After its first swap it keeps its own ledger, and never creates, writes or
deletes anything under `<root>/.rotator/`. `profile usage` reads both ledgers
and believes whichever was updated more recently.

## Where a login with no profile goes

When trusted evidence establishes that no profile owns the login in
`~/.claude` and no ledger assigns it, the first swap creates
`default-original` (or the next free numbered name) and returns the
outgoing login there. Its display account and any ownership record come
from the trusted outgoing identity. A dry run prints
`would adopt ~/.claude as default-original`.

Adoption requires a complete ownership check. An unreadable login or
ownership record cannot prove that the login is unclaimed. If the check
is incomplete, the command alerts and leaves logins and ledger identity
unchanged. Session JSON cannot authorize adoption or let an empty profile
claim the live login. See [Rotate live profile](../reference/profile-command.md#rotate-live-profile).

## Swap refused

The rotator checks everything before it moves anything. If a check fails it
moves nothing and exits with status 1:

```
error: swap refused: profile work already has a login
hint: nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md
```

In the log, a refusal names its kind, what each source said and the fix:

```
ALERT: swap refused (has-login, profile work): profile work already has a login. Evidence: credentials/verified=work@example.com, ledger=work@example.com, record=work@example.com, .claude.json=work@example.com. Fix: decide which login work keeps, then run: amplihack profile login work
```

The evidence values are the accounts reported by the login in `~/.claude`
(checked with the server), the ledger, the profile's record and the
`oauthAccount` block of `~/.claude.json`. Only the first three decide;
`.claude.json` is shown so you can see whether it disagrees. A value shows
`none` when the source has none and `unreadable` when it could not be
read. Every kind and its fix are listed in
[Rotate refusal alerts](../reference/profile-command.md#rotate-refusal-alerts).

The rotator has no separate retry. Each cycle tries again, and the first
cycle after the cause is gone makes the swap. The same refusal is logged at
most once an hour, with a count; see [Repeated alerts](#repeated-alerts).

| Reason | What to do |
|--------|------------|
| ``profile <name> has no stored login; run `amplihack profile login <name>` first`` | `profile switch <name>` found no login in that profile to move into `~/.claude`. Log it in with `amplihack profile login <name>`, then switch. |
| `profile <name> already has a login` | The profile the live login should return to has a `.credentials.json` of its own. Decide which login to keep; log that profile in again with `amplihack profile login <name>` if needed. |
| `~/.claude and the profile root are on different devices` | Move the profile root onto the same filesystem as your home directory. Logins are only ever renamed, never copied. |
| `a login file is not a private regular file with one link` | The login in `~/.claude` or in the target profile is a symlink, has more than one link, is not owned by you, or can be read by other users. Run `chmod 600` on it, or replace it with a plain file by logging in again. |
| `HOME is not a directory owned by you`, `~/.claude is not a private directory owned by you` | Fix the ownership or mode (`chmod go-w`). |
| `profile root is owned by another user`, `profile root is writable by other users` | Fix the ownership or mode of the profile root (`chmod 700`). |
| `atomic exchange is not supported here` | The system or filesystem cannot swap two files in one step. Only Linux can. Nothing was moved. |
| `Claude holds its refresh lock; the swap waits for the next cycle` | A `claude` session is refreshing its token in `~/.claude` or the target profile. The rotator never breaks that lock; it tries again on the next cycle. If it repeats with no `claude` running, see "Stale refresh lock" under Limits. |
| `~/.claude.json is not a JSON object` | Kind `home-unreadable`. The file cannot be parsed, so the rotator neither repairs it nor records the new account in it, and refuses before anything moves. The file holds other Claude settings, so it is never rewritten blind. Repair the file so it holds a JSON object. |
| `the login in ~/.claude is not the account recorded for profile <name>` | The live login would go back to `<name>`, but it is not the account recorded for `<name>`. Saving it there would leave `<name>` holding another account's login. See [The live login is not the profile's account](#the-live-login-is-not-the-profiles-account). |
| `the recorded account could not be written to the .claude.json of profile <name>: <reason>` | The login in `~/.claude` is the account recorded for `<name>`, but `<root>/<name>/.claude.json` names another account and could not be updated. Moving the login there would leave the file naming the wrong account. Repair the file so it holds a JSON object, or check its ownership and mode; the next cycle writes it. |

Three situations write an ALERT instead of refusing:

- `ALERT: no login in ~/.claude; no swap`. Log in with plain `claude` first.
  The rotator never fills an empty `~/.claude`.
- `ALERT: the account in ~/.claude is not the one the ledger names (live=work); no swap`.
  The login in `~/.claude` holds the same account as another profile's login. A manual
  `/login` in `~/.claude` replaces whatever login was there; the profile
  that login belonged to is left with no login. The rotator reads again on
  the next cycle, and moves a duplicate copy aside only when no profile is
  left with no login. Otherwise it writes an ALERT with numbered `fix`
  lines, repeated on every cycle until it is resolved. See
  [After a manual /login in ~/.claude](#after-a-manual-login-in-claude).
- `ALERT: auto-switch recovery is inconsistent; no swap`. Run
  `amplihack profile auto-switch repair`.

### The live login is not the profile's account

Both automatic rotation and `profile switch` check the outgoing login,
incoming login, expected ownership records and duplicate claims before
moving a login. They use credential-bound account evidence, including the
organization, and recheck the evidence under the swap locks. A login that
changes during a read must be read again successfully; a normal token
refresh is accepted only after fresh evidence confirms the same account.

If a required identity read fails, times out, lacks the fields needed for
the ownership check, or conflicts with a record, the command reports an
identity refusal and moves nothing. It also keeps the existing ledger identity. An unreadable
duplicate candidate is uncertainty, not proof that no duplicate exists.
Identity refusals remain visible on every affected attempt or cycle.

For example, if `work`'s login is live while a running session writes
`personal` into `~/.claude.json`, a switch to `spare` still succeeds when
the trusted evidence and records authorize it. `spare` becomes live,
`work`'s login returns to `work`, and the ledger identifies `spare`'s
trusted account. Neither login is moved to rejection storage. A later
session rewrite to `work` does not change that result, including after
10 minutes have passed.

amplihack records the account each profile is meant to hold when you run
`amplihack profile login <name>` and the login is accepted. A swap that
adopts a login into `default-original` records that profile too, and a
swap away from a profile with no record records it when the ledger shows the
outgoing login came from that profile and no other profile holds or has
recorded the same account. The record is
`<root>/.auto-switch.d/expected/<name>.json` and holds the account and its
organization. The same user in another organization counts as another
account, so a `/login` to your personal organization is refused for a
profile recorded with your team organization.

Before a swap moves the login in `~/.claude` back to the live profile, the
rotator compares the account the server reports for it with that profile's
record. If they differ, the swap is refused and nothing moves:

```
2026-11-01T09:40:05Z ALERT: swap refused (account-mismatch, profile work): the login in ~/.claude is not the account recorded for profile work. Evidence: credentials/verified=personal@example.com, ledger=work@example.com, record=work@example.com, .claude.json=personal@example.com. Fix: run plain claude, /login to the account recorded for work, then wait for the next cycle
```

Here `credentials/verified` differs from the record, so the login in
`~/.claude` is not `work`'s. The ALERT is written once an hour while the
conflict lasts, with a count of the cycles not logged. It names the
accounts by e-mail address, never by ID or token.
`amplihack profile switch` reports the same refusal on every attempt and
exits 1.

When only `.claude.json` disagrees, and the server, the ledger and the
record name the same account, there is no refusal. The cycle rewrites the
`oauthAccount` block from the verified account, writes one line, and goes
on with any swap that is due:

```
2026-11-06T20:32:54Z repair: home account metadata named ffarmer@example.com, verified identity is nnorth@example.com; rewritten
```

A running Claude Code session can write its old block back; the next cycle
repairs it again, and the line is logged once an hour. See
[Home account repair](../reference/profile-command.md#rotate-home-account-repair).

If the block cannot be written, the cycle writes nothing and logs a
different line, also once an hour:

| Line | What it means | What to do |
|------|---------------|------------|
| `repair refused: ~/.claude.json was not rewritten: <error>` | The evidence agreed, but the write was refused or failed; `<error>` says why. Nothing was written. | Make `~/.claude.json` a regular file you own in a home directory you can write to. The next cycle tries again. |

This usually follows a `/login` in `~/.claude` to the wrong account, or a
browser that signed Claude in to whichever account it already had. To fix
it, put the right login back in `~/.claude`:

1. Sign out of claude.ai in the browser, or open a private window.
2. Run plain `claude`, then `/login`, and sign in to the account `work` is
   meant for.
3. Run `amplihack profile rotate` once, or wait for the next cycle. The
   ledger still names `work`, and the new login is the account recorded for
   `work`, so `work` stays live and nothing is logged about it. When a
   swap is due, it now goes ahead instead of being refused, because the
   login in `~/.claude` is the account recorded for `work`. Before
   moving the login into `work`, the swap writes its account into
   `<root>/work/.claude.json`, which still named the account before the
   `/login`, so the next swap back into `work` puts the right account in
   `~/.claude.json`. While the login is still wrong, the one-shot command
   prints the refusal and exits 1. A preview does not reserve the evidence
   or authorize a later swap: the actual attempt validates it again.

`amplihack profile login work` does not help here: `work` is the live
profile, so `login` refuses it before Claude starts.

If `work` is meant to hold a different account, reconcile the ownership
record with that intended account before retrying. Use the supported
[profile login](../reference/profile-command.md#login-check) flow when the
profile is no longer live. Removing a record alone cannot prove ownership
or authorize the next swap.

A missing record may be adopted or backfilled only when trusted evidence
establishes ownership and no other profile claims the account. A record
that exists but cannot be read or trusted blocks the affected operation;
it is not treated as a missing record. Resolve the record's permissions or
contents before retrying. Do not delete a record merely to bypass a conflict.

Supported legacy email-keyed records continue to match under the existing
[record compatibility rules](../reference/profile-command.md#recorded-account).
A UUID is not newly required for every legacy account. A UUID-keyed record
still needs evidence sufficient to match that UUID and organization;
a session's email-only `oauthAccount` neither satisfies nor invalidates it.

## Recovery

A swap is two renames and an update to `~/.claude.json`, recorded in a
journal before the first rename. If the rotator is killed part-way, the next
`rotate`, `start` or cycle attempts recovery before doing anything else.
The recorded inode placement determines whether to finish or undo the
swap; the last recorded step is only a hint. Before further login movement
or ledger settlement, recovery reconstructs fresh trusted login identity
and applicable ownership-record evidence. It only renames logins; it never
deletes or overwrites one.

If required trusted evidence is unavailable, insufficient, changed or
conflicting, recovery retains the unresolved journal, leaves login
placement and ledger identity unchanged, and reports the identity refusal
on each affected attempt. A retry requires fresh authorization; session
JSON cannot supply it.

Recovery safely releases only the refresh locks whose inodes match those
it recorded, including when identity is uncertain or it marks a valid
journal `inconsistent`. A lock with another inode, or one acquired but
never recorded, is left alone. A run with a different `HOME` (under `sudo`
or `cron`) leaves the journal and locks untouched for the correct `HOME`;
an invalid journal cannot authorize cleanup either. See the
[recovery reference](../reference/profile-command.md#rotate-recovery).
After recovery completes, the cycle checks for a
[manual `/login` in `~/.claude`](#after-a-manual-login-in-claude) and
recovers from it where that is safe.

When login placement is inconsistent or a directory fails its safety
check, recovery marks the journal `inconsistent`, writes an ALERT, and
stops swapping until you resolve it:

```
error: rotate state is inconsistent: the login files are not where the journal expects
hint: see the Recovery section of docs/howto/claude-profile-rotation.md
```

Later runs report `an earlier swap stopped half way` in place of the first
reason.

To resolve it by hand, without opening any credentials file:

1. Stop the background process: `amplihack profile rotate stop`.
2. Read the journal, which holds only profile names, phases and inode numbers:
   ```bash
   cat ~/.claude-profiles/.auto-switch.d/rotate/journal.json
   ```
3. List the logins with their inode numbers:
   ```bash
   ls -li ~/.claude/.credentials.json ~/.claude-profiles/*/.credentials.json
   ```
4. Each inode should appear exactly once. Using the journal's `from` and `to`
   profiles and inodes, move each login to where it belongs with `mv -n`.
   Never copy one. `live_ino` is the login that was in `~/.claude` and belongs
   in `from`; `target_ino` belongs in `~/.claude` if the swap went ahead, or
   in `to` if it did not. Claude may have refreshed the login in `~/.claude`,
   so the file there can have an inode that is in neither field.
5. Remove refresh locks the rotator left behind. For each of
   `~/.claude/.oauth_refresh.lock` and `~/.claude-profiles/<to>/.oauth_refresh.lock`,
   compare its inode with the journal's `home_lock_ino` or `to_lock_ino`:
   ```bash
   ls -di ~/.claude/.oauth_refresh.lock
   ```
   If they match, remove it with `rmdir`. If the journal has no inode for a
   lock that exists, and no `claude` is running, it was left by the rotator
   and can also be removed.
6. Delete `journal.json`, then run `amplihack profile rotate --dry-run` and
   check that it reports no pending journal.
7. Start the background process again: `amplihack profile rotate start`.

A `ledger.json` that cannot be read gives the same error. Delete
`<root>/.auto-switch.d/rotate/ledger.json` (`<root>` is the `root:` line of
`amplihack profile rotate status`); the next cycle works out the live profile
again from the accounts.

### After a manual /login in ~/.claude

A manual `/login` in `~/.claude` replaces whatever login was there; the
profile that login belonged to is left with no login. The rotate ledger still
names that profile as live. The rotator finds out whose login `~/.claude`
now holds by asking Anthropic's OAuth profile endpoint, the one Claude
Code's `/status` uses, with the login in `~/.claude` and with each profile's
own login. The server answers with the account each token belongs to, and
the rotator compares those accounts, and the account recorded for the live
profile. It never decides from `~/.claude.json`, which running sessions
rewrite, from `claude auth status`, which reads that file, or from usage
reset times, which accounts created together share.

It does this only when the last swap is at least 10 minutes old
(`SWAP_GRACE`, 600 seconds), and it acts only on two matching readings taken
on separate cycles. The tokens are used in memory only, in the request
header; they are never printed, logged or stored.

The examples below use the root `/home/you/.claude-profiles`, a ledger
naming `ryan-ryansweet`, and a `/login` in `~/.claude` as the account of
profile `ryan-mistt-org`, which also has its own login.

#### Right after a swap

For 10 minutes after a swap the rotator skips hand-change recovery,
whatever `~/.claude.json` says. Normal swap authorization still requires
trusted identity evidence. In
[#1886](https://github.com/rysweet/amplihack-rs/issues/1886) the rotator
swapped from `rssourcelabs` to `hharkness`, and two minutes later a running
session still had `rssourcelabs` in `~/.claude.json`. An earlier release took
that as a `/login` and moved `rssourcelabs`'s only login aside. Now the next
cycles simply carry on:

```
2026-11-03T04:50:10Z swap rssourcelabs->hharkness live s=91% w=40% age=2s target s=3% w=22% age=2s reason=threshold
2026-11-03T04:52:10Z cycle live=hharkness hharkness:s=4%,w=22%,age=2s rssourcelabs:s=91%,w=40%,age=2s
```

After 10 minutes, the server reports `hharkness`'s account for the login in
`~/.claude`, which is the account recorded for `hharkness`, so nothing
happens then either. On 3 November the same pattern hit `nnorth` and
`mmorrison`, whose usage windows reset at the same times; because identity
is now the server's account, a swap between those two goes ahead with no
note and no ALERT.

#### One profile answers the same

The first cycle that finds `~/.claude` answering as `ryan-mistt-org` writes a
note and stops without swapping:

```
2026-10-27T03:17:05Z note: ~/.claude holds the same account as profile ryan-mistt-org (ledger names ryan-ryansweet); nothing moved, the next cycle reads again
2026-10-27T03:17:05Z ALERT: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); no swap
```

On a later cycle, if the reading is the same, the rotator would keep the copy
in `~/.claude` and move `ryan-mistt-org`'s copy to a backup. Here it does
not: `ryan-ryansweet`'s only login was the one the `/login` replaced, and
recording `ryan-mistt-org` as live would leave `ryan-ryansweet` with no login
anywhere. The rotator refuses, leaves both logins where they are, and says
what to do:

```
2026-10-27T03:19:05Z ALERT: ~/.claude holds the same account as profile ryan-mistt-org on two readings (ledger names ryan-ryansweet), but ryan-ryansweet has no login of its own, so recording ryan-mistt-org would leave ryan-ryansweet with no login anywhere; nothing moved, no swap
2026-10-27T03:19:05Z fix 1: if a /login in ~/.claude replaced ryan-ryansweet's login: close every claude session that uses ~/.claude, then run: mkdir -m 700 /home/you/.claude-profiles/.rejected-logins/manual-20261027T031905Z && mv -n /home/you/.claude-profiles/ryan-mistt-org/.credentials.json /home/you/.claude-profiles/.rejected-logins/manual-20261027T031905Z/ && rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json; then run: amplihack profile login ryan-ryansweet
2026-10-27T03:19:05Z fix 2: if ryan-ryansweet and ryan-mistt-org are profiles of one account, run amplihack profile verify to see the account each folder holds, then remove one of the two profiles
```

The server reported one account for both logins, so this is not a
coincidence of reset times. If you ran `/login` in `~/.claude`, follow
`fix 1`. If you did not, the two profiles hold one account: run
`amplihack profile verify` and remove one of them.

`amplihack profile login ryan-ryansweet` can run straight after `rm`; you do
not need to run a rotate cycle first. With no ledger, it treats `ryan-ryansweet` as
the live profile only if `~/.claude.json` still names `ryan-ryansweet`'s
account. If it is refused with `profile 'ryan-ryansweet' is the rotator's
live profile`, a session rewrote that file before you closed it. Run plain
`claude` without `/login` and quit it, so that it writes the account it
holds; remove the ledger again if a cycle has rewritten it; then run
`amplihack profile login ryan-ryansweet` again.

The rotator moves a copy itself only when `ryan-ryansweet` still has its own
login in its folder, so that no profile is left with none. Then the line is:

```
2026-10-27T03:19:05Z recovered: ~/.claude and ryan-mistt-org held the same account; kept ~/.claude, moved ryan-mistt-org's copy to /home/you/.claude-profiles/.rejected-logins/rotate-ryan-mistt-org-20261027T031905Z/.credentials.json; recorded ryan-mistt-org as live
```

The backup directory is `0700`. If that name is taken, `-2`, `-3` and so on
are added. Backups are never deleted. The profile's `.claude.json` is not
changed.

Before that move, the rotator checks that:

- no `claude` session is running on `ryan-mistt-org`, and the rotator can
  see running sessions at all;
- Claude is not refreshing its token in `~/.claude` or in `ryan-mistt-org`;
- auto-switch has not lent `ryan-mistt-org`'s login to a session, and its
  directory does not host another account's login;
- the profile's login is a private regular file with one link;
- `~/.claude`, the profile and the profile root are on the same filesystem,
  and all of them are private and owned by you;
- no swap journal is pending or inconsistent;
- just before the rename, both logins still read the same and neither file
  has been replaced.

When a check fails, nothing moves and the cycle writes an ALERT and one
`fix` line:

```
2026-10-27T03:19:05Z ALERT: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); profile ryan-mistt-org holds the same account and has its own login; recovery refused: a claude session is running on ryan-mistt-org; no swap
2026-10-27T03:19:05Z fix 1: close the claude sessions that use ryan-mistt-org; the next cycle retries
```

Refusals, with the fix line written for each (`<dir>` is
`/home/you/.claude-profiles/.rejected-logins/manual-20261027T031905Z`):

| `<reason>` | Fix line |
|------------|----------|
| `an earlier swap stopped half way` | `see the Recovery section of docs/howto/claude-profile-rotation.md` |
| `HOME is not a directory owned by you` | `make /home/you owned by you and run: chmod go-w /home/you` |
| `~/.claude is not a private directory owned by you` | `make /home/you/.claude owned by you and run: chmod 700 /home/you/.claude` |
| `profile root is owned by another user`, `profile root is writable by other users` | `make /home/you/.claude-profiles owned by you and run: chmod 700 /home/you/.claude-profiles` |
| `a claude session is running on ryan-mistt-org` | `close the claude sessions that use ryan-mistt-org; the next cycle retries` |
| `cannot see running sessions` | `close the claude sessions that use ryan-mistt-org, then run: mkdir -m 700 <dir> && mv -n /home/you/.claude-profiles/ryan-mistt-org/.credentials.json <dir>/` |
| `Claude holds its refresh lock` | `wait; the next cycle retries` |
| `ryan-mistt-org's login is lent out or it hosts another login` | `wait for the auto-switch session to end; the next cycle retries` |
| `a login file is not a private regular file with one link`, `~/.claude and the profile root are on different devices`, `cannot create the backup directory: <error>`, `backup directory is not private: <path>`, `no free backup name` | `move ryan-mistt-org's copy by hand: mkdir -m 700 <dir> && mv -n /home/you/.claude-profiles/ryan-mistt-org/.credentials.json <dir>/` |
| `~/.claude changed during recovery` | `none needed; the next cycle retries` |

#### No profile answers the same

A complete trusted scan that finds no duplicate does not by itself prove
which profile owns the live login. The rotator retains the ledger's live
profile and applies the expected-record and ownership checks before any
swap. A credential-bound conflict alerts and moves nothing, even when
`~/.claude.json` still names the expected account.

If the live profile has no ownership record, only the existing adoption
or backfill rules can establish ownership, using trusted login evidence.
Session JSON cannot supply the missing proof. A cycle can also seed the
live profile's record, and when it cannot it names the command to run; see
[Record a profile that shows `no record`](#record-a-profile-that-shows-no-record)
and [Recorded account](../reference/profile-command.md#recorded-account).

A legacy ledger containing only the user UUID cannot prove which
organization owns an unrecorded outgoing login. The existing ledger now
retains sanitized full identity after an authorized swap or confirmed
credential recovery; ordinary observations do not upgrade legacy keys.
An interrupted swap retains both authorized identities in its journal,
so recovery cannot substitute a different unrecorded incoming account or
lose outgoing ownership after the ledger has advanced. Missing legacy
evidence refuses conservatively. New journals also retain preflight lock
ownership before final requests; recovery of `checking` moves no login.

#### When the rotator cannot tell

An unavailable identity endpoint, expired token, incomplete response,
conflicting record, changed login, unreadable duplicate candidate or
exhausted identity-read budget leaves the required evidence uncertain.
The affected operation writes an actionable ALERT on each attempt and
moves no login or ledger identity. The ALERT names the
[cause](../reference/profile-command.md#identity-read-causes):

```
2026-11-05T09:27:02Z ALERT: trusted login identity unavailable: http 429
```

Follow the reported cause: wait for
network or refresh failures to clear, and resolve genuine ownership
conflicts before retrying. Editing session JSON does not resolve them.

An expired login in `~/.claude` is refreshed before any identity read, even
when the ledger is unreadable or the lease view bars the live profile. The
refresh proves nothing about the account: only the fresh reading after it can
let a login move. With an unreadable ledger the cycle then stops, with no move
and no record; see
[Unreadable-ledger stop](../reference/profile-command.md#unreadable-ledger-stop).

An otherwise eligible stored OAuth login still receives the usual usage
read, which can refresh expired credentials. A reading started without
trusted identity cannot select that login. A later cycle must validate
the refreshed credentials and take a new usage reading before a swap.

Recovery still requires two consistent readings on separate cycles.
An uncertain intervening reading cannot count as confirmation: a later
match starts the confirmation sequence again. The 600-second grace and
the protection against moving a profile's only login remain in force.

#### Check which account each folder holds

`amplihack profile verify` asks the OAuth profile endpoint which account the
login in `~/.claude` and in each profile folder belongs to, and compares the
answer with the account recorded for that profile:

```bash
amplihack profile verify
```

```
~/.claude (live ffarmer) -> ffarmer@example.com: ok
ffarmer -> no login
mmorrison -> no login
nnorth -> mmorrison@example.com: MISMATCH, recorded for mmorrison
```

Here `nnorth`'s folder holds `mmorrison`'s login, as the 3 November misfire
left it. To correct a folder, run `amplihack profile login <name>` with the
browser signed out of claude.ai or in a private window. Do not use the `--fix`
flag until [#1960](https://github.com/rysweet/amplihack-rs/issues/1960) is
closed. Run `amplihack profile verify` again afterwards; it exits 0 once every
folder holds its recorded account. The full rules are in
[profile verify](../reference/profile-command.md#profile-verify).

#### Record a profile that shows `no record`

Plain `verify` shows `no record` for a profile the rotator has no recorded
account for. The rotator cannot swap away from a live profile until it has
ownership evidence, and on a ledger written before login identities were kept
it logs:

```
note: no record for live profile ryan-mistt-personal: the ledger has no login identity; nothing moved; run: amplihack profile seed-record ryan-mistt-personal
```

Do this once, in this order:

1. Install the fixed build and restart the rotator. An older rotator running
   beside a new record has not been tested.
2. Read the dry run. It must name the live login and report no identity alert:

   ```bash
   amplihack profile rotate --dry-run
   ```

   If it reports `token expired`, wait for the rotator's next cycle, which
   refreshes the live login, and run the dry run again. If it reports `http 429`,
   wait a few minutes and run it again. Other causes are in
   [Identity read causes](../reference/profile-command.md#identity-read-causes).

3. Preview the record. This writes nothing:

   ```bash
   amplihack profile seed-record ryan-mistt-personal
   ```

   ```
   ryan-mistt-personal -> you@example.com, organization 00000000-0000-0000-0000-000000000000: no record; nothing written; run with --write to record it
   ```

4. Check that this is the right account and organization. Then write it:

   ```bash
   amplihack profile seed-record ryan-mistt-personal --write
   ```

If step 3 refuses with
`the ledger's older key is an e-mail, which cannot prove ownership`, this login
cannot be seeded while the ledger names it. Stop the rotator, run
`rm <root>/.auto-switch.d/rotate/ledger.json` and start it again. With no ledger,
the next swap keeps this login under a new `default-original` profile, as
[Rotate live profile](../reference/profile-command.md#rotate-live-profile)
describes. Then run `amplihack profile login <name>` to give `<name>` a login of
its own.

The command moves no login and never overwrites a usable record. Two profiles
that share an e-mail, such as `ryan-mistt-personal` and `ryan-mistt-org`, are
told apart by account uuid and organization. If they cannot be, the command
refuses, and a rotate cycle's alert names both.

Step 4 matters most for a profile that isn't live: its folder name is the only
link to its login, so a misfiled login could be recorded as correct. To remove a
wrong record, run `rm <root>/.auto-switch.d/expected/<name>.json`. The full rules
are in
[profile seed-record](../reference/profile-command.md#profile-seed-record).

#### Stuck `~/.claude.json`

A missing or stale `oauthAccount` cannot establish ownership or trigger
recovery. The swap constructs display account details from trusted
identity, preserving unrelated JSON fields and the existing file mode, and
every cycle rewrites a missing or stale block from the verified identity
when the ledger and the record agree with it, logging a `repair:` line.
If the display file cannot be safely read or updated, such as when it is
not a JSON object or its permissions are unsafe, the swap can still
refuse on that file-safety check before moving a login.

Repair the reported file or permission problem and retry. Do not run
`/login` or move a credential solely to repair a display account. See
[Stuck `~/.claude.json`](../reference/profile-command.md#stuck-claudejson).


Some recovery diagnostics retain their earlier display-file wording. These
messages describe file safety or older retained guidance; they never make
session account values proof of login ownership. A stale or missing
`oauthAccount` alone does not trigger recovery. Resolve the actual reported
file problem and trusted-identity refusal before following recovery guidance.
The retained message grammar is:

```text
ALERT: the account in ~/.claude.json cannot be read (live=<L>); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace <L>'s login
ALERT: ~/.claude.json has had no account that can be read for 5 minutes or more (live=<L>): <problem>; no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace <L>'s login
fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if <home>/.claude.json then holds an account, the next cycle goes on
fix 2: or give <L> its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e <root>/<L>/.credentials.json && mv -n <home>/.claude/.credentials.json <root>/<L>/ && rm <ledger>; then run claude with no CLAUDE_CONFIG_DIR and /login
```

Older guidance can include `it has no oauthAccount block`. That display
condition cannot establish which account the moving credential belongs to.

#### See the current state

`amplihack profile rotate status` shows the current note or ALERT lines:

```
live: ryan-ryansweet
last cycle: 2026-10-27T03:17:05Z alerted
note: ~/.claude holds the same account as profile ryan-mistt-org (ledger names ryan-ryansweet); nothing moved, the next cycle reads again
```

A profile that an earlier release recorded as having lost its login is
still reported on every cycle, and in `status` as a `warning:` line, until
it is logged in again:

```
2026-10-27T03:17:05Z ALERT: profile ryan-ryansweet has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login ryan-ryansweet
```

The rotator no longer adds profiles to that list, because it refuses any
move that would leave a profile with no login.

While a suspicion like this is open, `amplihack profile switch` refuses. If
the rotator is stopped, run one cycle to settle it:

```bash
amplihack profile rotate
```

To see what the next cycle would do, without moving anything:

```bash
amplihack profile rotate --dry-run
```

It makes the same reads and prints `would recover:`, `would note:` or
`would alert:` with the `fix` lines. A dry run never records a suspicion, so
it reports `would recover:` only when a real cycle has already recorded one
for the same profile.

```
would note: ~/.claude holds the same account as profile ryan-mistt-org (ledger names ryan-ryansweet); nothing moved, the next cycle reads again
```

### Go back to an older amplihack

An amplihack older than
[#1659](https://github.com/rysweet/amplihack-rs/issues/1659) cannot read the
`last_cycle.measured_at` field this version writes to `ledger.json`. Its
rotator does not rewrite a ledger it cannot read, so it logs a failed cycle
every interval and never swaps again, and the limit-resume hook waits its
full 7 days for a cycle that does not come. Before you run the older binary,
or launch anything with it while [autostart](#start-the-rotator-automatically)
is on:

1. Stop the rotator: `amplihack profile rotate stop`.
2. Delete `<root>/.auto-switch.d/rotate/ledger.json`, where `<root>` is the
   path on the `root:` line of `amplihack profile rotate status`. With no
   root set, it is `~/.amplihack/claude-profiles`, so the file is
   `~/.amplihack/claude-profiles/.auto-switch.d/rotate/ledger.json`.
3. Start the older rotator: `amplihack profile rotate start`.

The ledger holds only bookkeeping, and the next cycle rebuilds it. Deleting
it also clears the swap hold, so the first cycle may make an upgrade swap
sooner than it otherwise would. The same steps recover an older rotator that
is already running and logging failures.

If you cannot tell which login belongs where, log each affected profile in
again with `amplihack profile login <name>`.

## Use an API key as the last resort

An API-key profile runs Claude Code on a Claude Platform API key and is
billed per token. The rotator uses it only after you mark it, only when no
plan login is below 100% on both windows and every
[credit overflow profile](../reference/profile-command.md#rotate-overflow)
is blocked or cannot be used, and leaves it as soon as it can. The exact
rules are in the [API-key profiles reference](../reference/profile-command.md#api-key-profiles).

### Set up the profile

Create the profile and log it in to the Claude Platform once:

```bash
amplihack profile add api-key
amplihack profile login api-key   # choose the Claude Platform (API key) option
```

Claude stores the platform credential outside the profile directory, in the
real home directory, and leaves the profile's `.credentials.json` as `{}`.
Check that the profile is detected as an API-key profile:

```
$ amplihack profile kind list
api-key         api-key       (detected: login file has no oauth login)
ryan-mistt      subscription  (detected: oauth login)
ryan-mistt-org  subscription  (detected: oauth login)
ryan-ryansweet  subscription  (detected: oauth login)
```

A profile with no `.credentials.json` at all shows
`subscription  (detected: no login)` and is refused at launch; it is not
treated as an API key. There is one platform credential per home
directory. A second API-key profile uses the same account and the same
block and is never chosen, so one is enough.

If detection is wrong, set the kind by hand. `auto` returns to detection:

```bash
amplihack profile kind set api-key api-key
amplihack profile kind set api-key auto
```

### Mark the profile

The rotator never uses an API-key profile you have not marked. Mark it to
allow per-token billing:

```bash
amplihack profile overflow add api-key
```

`overflow list` shows `api-key  api-key tier  marked` once it is marked and
`api-key  api-key tier  not marked (not used)` before. An API-key profile is
always tier 10, after every credit tier, so `--tier` is refused for it with
`the API-key tier is fixed at 10`. A kind override or the profile's name does
not mark it. If the profile already has a mark from when it was a credit
account, `kind set <name> api-key` warns you: that mark now allows per-token
billing on the key. Run `overflow remove <name>` if you did not mean that.

Hosts that relied on an earlier release, which used every API-key profile
without a mark, must run this command once; nothing is marked for you.

The rotator checks for a key before every move onto the profile, in the
files plain `claude` reads after the move: a non-empty `primaryApiKey` in
`~/.claude.json`, or `ANTHROPIC_API_KEY` under `env` or `apiKeyHelper` in
`~/.claude/settings.json`. The move carries only `.credentials.json`, so a
key in the profile's own files does not pass, and neither does a
`.credentials.json` of `{}`. The check reads those files only: it does not
run the helper or make a request, so it confirms that a key is configured,
not that it works.

A key in those files is used ahead of a plan login while that login is
live: every session on the host bills per token, marked or not, and the
rotator cannot read the live login's usage. `overflow add` warns about it and
the rotator logs `ALERT: home_key`. Leave those files without a key on a host
that bills only plan logins: the rotator then refuses every move onto the
API-key profile. See
[API-key login check](../reference/profile-command.md#api-key-login-check).

### The order the rotator follows

1. Plan (subscription) logins with capacity.
2. Plan logins past a threshold but below 100% on both windows
   (`reason=plan-room`): those with session use below `--session-at` first,
   then the most weekly room, then the most session room.
3. Only when no measured plan login is below 100% on both windows: marked
   credit overflow profiles, tiers 1 to 9.
4. The marked API-key profile, tier 10, only with every plan login measured.

The rotator moves onto the API key only when it is marked, every plan login
was measured and is at 100% of its session or week, no credit overflow profile can be used,
the earliest plan reset is further away than `--overflow-wait` and the login
check passes. If the reset is closer, it waits. A failed live probe never
moves work onto the API key, and when every probe fails no reset is known,
so nothing moves unless you set `--overflow-unknown-reset 1`. The log names
the reason before the swap:

```
per-token api-key: no measured plan login below 100%, 0 not measured; earliest reset 2026-11-01T02:14:00Z beyond overflow_wait; marked overflow tier 10; login check passed (primaryApiKey set)
swap iivanovich->api-key live s=100% w=61% age=2s target s=0% w=0% age=2s reason=overflow-in
moved ~/.claude onto API key profile api-key (billed per token)
```

A plan login that was skipped as busy or failed its probe may have room, so
it keeps the key out, and the rotator writes one ALERT naming it:

```
ALERT: API key not used live=iivanovich: no measured plan login has room, but these were not measured and may have: ccapstone
```

A login in the `amplihack profile auto-switch exclude` list does not count,
so excluding a login that is never available lets the key be used. If `overflow=` marks a profile that was
detected as API-key (no `kind set`, for example a credit account whose login
file lost its OAuth login), the first cycle logs one `warning:` line saying
so, with the command that removes the mark.

If the login check fails, nothing moves, `~/.claude` is unchanged and the
log reads:

```
key_check_refused api-key: no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting
ALERT: API key profile api-key failed its login check (no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting); not moving onto it
```

The ALERT is written once, not on every cycle. `--dry-run` prints
`would_refuse api-key: <reason>` in its place.

The full rules are in
[Rotate plan-room swap](../reference/profile-command.md#rotate-plan-room-swap)
and [API-key rotation](../reference/profile-command.md#api-key-rotation).

### Moving back off the API key

While the API key is live, every cycle checks again and ignores `--hold`.
As soon as a plan login has capacity, the rotator moves `~/.claude` to it.
Failing that, it moves to the plan login below 100% on both windows with the
most room. Failing both, if a credit overflow profile can be used, it moves
to credits, which cost less than tokens. All three use
`reason=overflow-out` and log:

```
moved ~/.claude off API key profile api-key to ryan-mistt
```

It never moves from a plan login to credits, or to the API key, while a
plan login with a usage reading has room below 100%. A plan login with no
reading may have room, so it keeps auto-switch off the API key too, and
auto-switch writes its own form of the ALERT:

```
ALERT: API key not used session=work: no measured plan login has room, but these were not measured and may have: ccapstone; auto-switch from work
```

`session=` names the account of the session that ran out, which is not the
login in `~/.claude`; only the rotator's lines use `live=`. The line goes to
stderr on every exhausted launch and rerun, and to the rotate log once for
each set of unmeasured logins. It is written to the log again when that set
changes, or after every plan login has been measured.

As for the rotator, a profile in the `exclude` list does not count, and
neither does one with no login file or no OAuth login: nothing can measure
it. The rotator's live login counts, though its login file is in
`~/.claude`. Auto-switch cannot measure that login itself: the status line
records only a session's own profile, and `profile usage` probes each
profile's own directory. It uses the rotator's measurement instead: every
cycle, the rotator records in its ledger the logins it measured with no room,
and while that record holds (past the next cycle, at most an hour after the
measurement, and no later than the first reset the rotator read for one of
those logins) those logins no longer keep auto-switch off the key. That includes a login the rotator has
just moved off. When no rotator is running, or the rotator found room on the
live login, the live login keeps the key out until a reading is recorded for
it. To let auto-switch use the key in that state, add the live login to the
`exclude` list.

### See when work is on the API key

```
$ amplihack profile rotate status
...
~/.claude is on API key profile api-key: work is billed per token (since 2026-10-28T14:02:11Z)
...
$ amplihack profile usage api-key
PROFILE         WEEK             W-RESET      SESSION           S-RESET     STATE
api-key (live)  api-key (billed per token)                                  api_key
live: work is running on the API key (billed per token)
times: Europe/London
order: rotator pick (most room in the worse of session and week, then name)
```

`profile usage` runs no `/usage` probe for an API-key profile; there are no
plan limits to show.

### Switch to the API key by hand

`amplihack profile switch api-key` moves the API-key login into `~/.claude`
and pins rotation to it. A pinned API key keeps billing per token even after
plan capacity returns, until you release it:

```bash
amplihack profile switch --none
```

### When the key runs out or is revoked

If an agent step on the API key fails because the credit balance is too
low, the key is invalid or revoked, or the key is rate-limited, every
API-key profile is blocked and one ALERT is logged:

```
ALERT: the API key is blocked (key_exhausted) until 2026-12-29T10:14:03Z; API-key profiles are not used
```

A rate-limit block (`key_rate_limited`) ends at the time the error gave, or
after an hour. An out-of-funds (`key_exhausted`) or revoked (`key_revoked`)
block lasts until you clear it; it is stored as 62 days, after which the key
is tried once more. Fund or replace the key at the Claude Platform console,
then:

```
$ amplihack profile overflow clear --platform
overflow: cleared the API-key block (key_exhausted)
```

### Launches that would bill the API key by mistake

A subscription profile with no OAuth login of its own has nothing in its
directory for Claude to use, so Claude would fall back to the API key.
`profile run`, `launch --profile`, `claude --profile` and `profile switch`
refuse such a profile and exit 1:

```
error: profile old-team is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login old-team', or run 'amplihack profile kind set old-team api-key' if per-token billing is intended.
hint: amplihack profile login <name>, or: amplihack profile kind set <name> api-key
```

A profile whose login is live in `~/.claude` is not refused: the launch runs
against `~/.claude`, which holds the login. The rotator likewise refuses any
swap that would leave `~/.claude` with no OAuth login unless the incoming
profile is an API-key profile, and logs
`ALERT: swap refused (api-key, profile <name>): <name> has no oauth login and is not an API-key profile. Evidence: ... Fix: ...`.

## Live default and duplicate logins

While a profile's login is in `~/.claude`, the profile's own directory has
no login. `amplihack profile switch <name>` always leaves it this way: it
moves `<name>`'s login into `~/.claude` and makes `<name>` the default (see
[Switch to a login by hand](#switch-to-a-login-by-hand)). When the default,
or any profile a launch resolves to, is the live profile,
`amplihack claude` runs against `~/.claude` instead of the empty directory
and prints:

```
note: profile 'team' is the rotator's live login, which is in ~/.claude; launching against ~/.claude.
```

The same happens for `--profile team` and for an inherited
`AMPLIHACK_CLAUDE_PROFILE=team`. Without this, Claude would ask you to log
in, and logging in there would put a second copy of an account into
`team`. To stop using a default and let the rotator choose again, run
`amplihack profile switch --none`. See
[Launching the rotator's live profile](claude-profiles.md#launching-the-rotators-live-profile).

`amplihack profile login <name>` refuses a login that duplicates the account
in `~/.claude`, naming the holder as `~/.claude (rotator live profile 'team')`.
`profile usage` and `auto-switch status` warn about a profile that holds the
same account as `~/.claude`. The live profile itself is never flagged against
`~/.claude`. See [Duplicate logins](claude-profiles.md#duplicate-logins).

After a [manual `/login` in `~/.claude`](#after-a-manual-login-in-claude),
the ledger can name a profile whose login is no longer in `~/.claude`.
`profile usage` does not try to tell whose login is there: it marks the
profile the ledger names, as always, and its row shows the numbers of the
login that is actually in `~/.claude`. The duplicate-login warning for
`'~/.claude'` and another profile is printed while both are the same
account. Only the rotator decides whose login `~/.claude` holds, from what
the credential returns; see
[Hand-change notes and alerts](#hand-change-notes-and-alerts).

`amplihack profile login`, the live-profile launch and the duplicate-login
warnings do not change in this state: they still go by the ledger.

Accounts are compared by user and organization: one user in two
organizations is two accounts, and both can be rotation candidates. The
full rule is in
[Which logins count as the same account](claude-profiles.md#which-logins-count-as-the-same-account).

## Limits

- **Linux only in practice.** On macOS Claude keeps logins in the Keychain, so
  there is no `.credentials.json` to move and the rotator writes the
  no-login ALERT; on any system other than Linux a swap is refused with
  `atomic exchange is not supported here`. Hosts without `/proc` cannot tell which profiles have running
  sessions, so every profile counts as in use and none is a candidate;
  `status` reports `unverifiable`. On Windows every `rotate` command is
  refused. On all of these, `profile switch <name>` still makes `<name>` the
  default for new launches, prints `Live login not moved; ...`, holds
  nothing and exits 1.
- **Token refresh race.** A `claude` process that refreshes its token during
  the two renames of a swap can write the old account's token into
  `~/.claude`. The rotator swaps only after taking Claude's refresh lock in
  `~/.claude` and in the target profile; if either is already held, it skips
  the swap until the next cycle. This stops a refresh that Claude starts
  during the swap. A `claude` that refreshes without taking the lock can
  still race the swap, which takes two system calls. If a session suddenly
  needs `/login` after a swap, log the affected profile in again. A refresh
  after the exchange is harmless to recovery, which never expects a
  particular file in `~/.claude`.
- **Stale refresh lock.** If the rotator is killed in the instant between
  making Claude's refresh lock and recording it, or between removing the
  record and removing the lock, the lock stays behind and
  `claude` may wait on it. With no `claude` and no rotator running, remove it
  with `rmdir ~/.claude/.oauth_refresh.lock`.
- **Pid reuse.** Before `stop` signals the process it checks the process's
  owner and command line. On Linux the signal goes through a process handle,
  so it cannot reach a new process with the same pid. Elsewhere a short window
  remains between the check and the signal.
- **Sessions keep their login until they reread it.** A running `claude` may
  keep using the outgoing account until it next reads its credentials.
- **Resume needs the rotator and a fresh cycle.** Without a running rotator,
  or when every login is at capacity, a session stopped by a limit is never
  resumed. The resume waits for at most 7 days; a shorter `--interval`
  resumes sooner. If you send a prompt in the session during the wait, the
  hook stops without waking it (`moved-on`). A rotator started after the
  limit does not help that session: its hook already ended as
  `rotator-down`. A session is treated as exited when the hook's parent
  process changes; if Claude Code runs the hook under a shell that outlives
  it, the hook waits out the 7 days.
- **The ledger records the user, not the organization.** The rotate ledger
  is unchanged and names the account in `~/.claude` by user (or e-mail).
  If someone runs `/login` in `~/.claude` and signs in to the same user in
  another organization, the rotator does not see a hand change. Run
  `amplihack profile usage` to check: the live row then reports the other
  organization's usage, and if another profile already holds that
  organization, both commands warn about a duplicate login.
- **Overflow depends on what `/usage` shows.** The rotator assumes that an
  account on credits still answers `claude -p /usage` after its plan limit,
  and that a used-up spend limit shows there as `spend_limit`. A profile
  marked as overflow is always probed with `/usage`, so it always pays the
  transcript scan. An overflow profile whose probe fails is never used. A spend block
  always lasts until the 1st of the next month (UTC), whatever reset Claude
  states; lift it sooner with `amplihack profile overflow clear <name>`.
- **The warning can say exhausted while on credits.** The status line and
  `profile usage` do not know about credits and may record the no-capacity
  warning between cycles; the rotator's next cycle clears it.
- **A manual `/login` in `~/.claude` loses a login.** A manual `/login` in
  `~/.claude` replaces whatever login was there; the profile that login
  belonged to is left with no login. The rotator recovers the rest by itself
  only when exactly one profile has the new account; it never restores the
  lost login and never changes what `/login` does. Log that profile in again
  with `amplihack profile login <name>`. Backups under
  `<root>/.rejected-logins/rotate-…` are never pruned; delete them yourself
  once you no longer need them.
  A manual `/login` onto an overflow profile's account is recovered the same
  way, and the same cycle then moves off it whenever a plan login has
  capacity, so a `/login` by hand never leaves work running on credits.
- **A rotator started before an upgrade ignores `switch`.** The hold from
  `profile switch` is a file the rotator reads on each cycle; a
  `rotate watch` started by an older `amplihack` does not know about it and
  may swap away. `switch` prints a warning when it finds one. Restart the
  rotator after upgrading: `amplihack profile rotate stop`, then
  `amplihack profile rotate start`.
- **No early resume during a hold.** The limit-resume hook does not resume
  sessions while a `switch` hold is in force, and the usage-limit wait reruns
  a step only when its limit resets; see
  [What the hold changes](#what-the-hold-changes).
- **Each cycle starts `claude` once or twice per profile.** Each probe
  starts `claude` for the `get_usage` request, and again for `/usage` when
  that request falls back, a few profiles at a time. A short `--interval`
  means many processes.
- **`get_usage` is experimental in Claude Code.** A Claude Code update can
  change or remove it. amplihack then falls back to `/usage` on every probe,
  which is slower but still correct. When Claude's own usage lookup fails,
  it can answer `get_usage` with figures it stored earlier, without marking
  them as old; compare with `AMPLIHACK_PROFILE_USAGE_PROBE=text` as in
  [Compare the two probe methods](claude-profiles.md#compare-the-two-probe-methods).
