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

The exact flags, files, exit codes and log lines are in the
[`profile rotate` reference](../reference/profile-command.md#rotate).

## Contents

- [Before you start](#before-you-start)
- [Preview a rotation](#preview-a-rotation)
- [Rotate once](#rotate-once)
- [Switch to a login by hand](#switch-to-a-login-by-hand)
- [Run rotation in the background](#run-rotation-in-the-background)
- [Start the rotator automatically](#start-the-rotator-automatically)
- [Which root the rotator uses](#which-root-the-rotator-uses)
- [Change the thresholds](#change-the-thresholds)
- [When a swap happens](#when-a-swap-happens)
- [When the live login cannot be measured](#when-the-live-login-cannot-be-measured)
- [Which profiles can take over](#which-profiles-can-take-over)
- [Read the log](#read-the-log)
- [Resume sessions stopped by a usage limit](#resume-sessions-stopped-by-a-usage-limit)
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
PROFILE   MARKS              STATE  SESSION  SESSION RESETS                     WEEK  WEEK RESETS
spare                        ok     0%       -                                  12%   Oct 23, 9am (Europe/London)
personal                     ok     20%      Oct 18, 3pm (Europe/London)        35%   Oct 22, 2am (Europe/London)
work      live in ~/.claude  ok     88%      Oct 18, 1pm (Europe/London)        41%   Oct 21, 6pm (Europe/London)
```

Only a profile in state `ok` can be chosen; an exhausted profile, or one whose
probe failed, never is.

## Preview a rotation

```bash
amplihack profile rotate --dry-run
```

```
cycle live=work work:s=88%,w=41% personal:s=20%,w=35% spare:s=0%,w=12%
would swap work->spare live s=88% w=41% target s=0% w=12% reason=threshold
```

A dry run probes every profile and prints what it would do. It takes no lock,
writes nothing, moves nothing and logs nothing. It also does not finish an
interrupted swap: if one is pending it prints
`would recover an interrupted swap (phase <phase>)` and leaves it for the next
real run. Run a dry run first, and again whenever you change a threshold.

## Rotate once

```bash
amplihack profile rotate
```

```
2026-10-18T12:40:05Z cycle live=work work:s=88%,w=41% personal:s=20%,w=35% spare:s=0%,w=12%
2026-10-18T12:40:05Z swap work->spare live s=88% w=41% target s=0% w=12% reason=threshold
```

The same lines go to the log. Afterwards `spare`'s login is in `~/.claude` and
`work`'s login is back in `~/.claude-profiles/work/`:

```bash
amplihack profile usage
```

```
PROFILE   MARKS              STATE  SESSION  SESSION RESETS                     WEEK  WEEK RESETS
spare     live in ~/.claude  ok     0%       -                                  12%   Oct 23, 9am (Europe/London)
personal                     ok     20%      Oct 18, 3pm (Europe/London)        35%   Oct 22, 2am (Europe/London)
work                         ok     88%      Oct 18, 1pm (Europe/London)        41%   Oct 21, 6pm (Europe/London)
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
running (waking stopped sessions does not; see below), and it measures
nothing, so it starts no `claude` process and works with no `claude` on
`PATH`. It moves the login even when `spare` has no capacity
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
| `profile spare is not a rotate candidate (no account): ...` | `spare/.claude.json` has no `oauthAccount`, so `~/.claude.json` could not be updated. Run `amplihack profile login spare`. |
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
pid: 48213
live: spare
last cycle: 2026-10-18T12:50:05Z stayed
log: /home/you/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

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

When auto-switch is off, a launch does nothing extra: it creates no file,
takes no lock and prints nothing.

### Many launches at once

Recipe runs and `multitask` start many Claude sessions in parallel, and
nested launches inside a recipe run check like any other launch. Only one
rotator ever runs: a launch that finds the rotator running stops there, and
otherwise it tries the state lock without waiting. If another launch holds
the lock, it gives up silently. The launch that gets the lock checks again
before it starts the rotator and writes the pid file while it still holds the
lock.

### What the started rotator uses

An automatically started rotator is the same background process `rotate
start` runs, with the same pid file, log and
[root record](#which-root-the-rotator-uses). The differences:

- It uses the profile root the launch resolved.
- It keeps the `AMPLIHACK_ROTATE_*` [thresholds](#change-the-thresholds) set
  in the environment of the launch that started it, and that launch's
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
| `--session-at N` | `AMPLIHACK_ROTATE_SESSION_AT` | `85` | Swap when the live session use reaches N%. |
| `--week-at N` | `AMPLIHACK_ROTATE_WEEK_AT` | `90` | Swap when the live weekly use reaches N%. |
| `--margin N` | `AMPLIHACK_ROTATE_MARGIN` | `5` | A threshold swap needs a target with at least N more points of headroom. |
| `--upgrade-margin N` | `AMPLIHACK_ROTATE_UPGRADE_MARGIN` | `30` | Swap without reaching a threshold when a target has at least N more points of headroom. |
| `--hold SECS` | `AMPLIHACK_ROTATE_HOLD` | `3600` | No upgrade swap for this long after any swap. `0` turns the hold off. |
| `--overflow-wait SECS` | `AMPLIHACK_ROTATE_OVERFLOW_WAIT` | `1800` | Use an [overflow profile](#use-usage-credits-when-every-plan-login-is-exhausted) only when the next plan reset is further away than this. |
| `--interval SECS` | `AMPLIHACK_ROTATE_INTERVAL` | `600` | Time between cycles, for `start` and `watch` only. |

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

Each profile's *headroom* is `100 - max(session %, week %)`. A profile with no
session window yet counts as 0% session use. A profile that is
`session_exhausted` or `weekly_exhausted` has headroom 0.

On each cycle the rotator compares the live account with the candidate that
has the most headroom (ties go to the name that sorts first):

1. **Threshold swap.** If the live session use is at or above `--session-at`,
   or the weekly use is at or above `--week-at`, it swaps when the candidate
   has at least `--margin` more headroom. A live account that could not be
   measured counts as headroom 0, but is swapped only as described in
   [When the live login cannot be measured](#when-the-live-login-cannot-be-measured).
2. **ALERT.** If a threshold swap is needed but no candidate has that margin,
   it writes one `ALERT` line and leaves `~/.claude` alone.
3. **Upgrade.** If no threshold is reached, it swaps when the candidate has at
   least `--upgrade-margin` more headroom, unless the last swap was less than
   `--hold` seconds ago.

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
2026-10-20T09:00:05Z cycle live=work work:failed personal:s=20%,w=35% spare:s=0%,w=12%
2026-10-20T09:00:05Z stay: live probe failed (1/3)
2026-10-20T09:10:05Z stay: live probe failed (2/3)
2026-10-20T09:20:05Z swap work->spare live failed target s=0% w=12% reason=failed-probe
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

To see why a probe fails, run `amplihack profile usage --raw`.

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
2026-10-18T13:00:05Z cycle live=spare spare:s=4%,w=13% personal:s=20%,w=35% work:s=88%,w=41%
2026-10-18T17:10:05Z swap spare->personal live s=86% w=30% target s=0% w=36% reason=threshold
2026-10-18T19:40:05Z ALERT: no profile has capacity live=personal s=97% w=52%
2026-10-18T19:50:05Z skip: state lock busy
2026-10-20T09:00:05Z stay: live probe failed (1/3)
```

To see only swaps and alerts:

```bash
grep -E ' (swap|ALERT:) ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

The same ALERT is written to the log once. It is written again only after
something changes: a swap succeeds, a cycle needs no swap, or the reason
changes. The one-shot `rotate` still prints it on every run. The ALERT for a
[manual `/login` in `~/.claude`](#after-a-manual-login-in-claude), its `fix`
lines and the `has no login anywhere` line are the exception: they are
written on every cycle until the problem is resolved. The log
holds profile names, percentages, fixed text and, in `recovered:` and `fix`
lines, paths under your home directory and the profile root; it never holds
account IDs, e-mail addresses, tokens or Claude's output.

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
   cycle that started measuring after the limit hit and ended `swapped` or
   `stayed` with no ALERT, or for a
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
  rotator records an ALERT, and the hook does nothing.
- **At most 3 resumes per session in 24 hours.** After that it logs `cap`.

If anything it needs cannot be read or written safely, it does not resume.

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

## When no account has capacity

When the `ALERT: no profile has capacity` line is written, the rotator has
nowhere to move the login, so it stays put. With an
[overflow profile](#use-usage-credits-when-every-plan-login-is-exhausted)
marked, it moves there instead when the next plan reset is far enough away.

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

A plan profile whose probe failed, or that reads exhausted without a reset
time, has no known reset. Unknown resets are left out: one known reset
decides the wait. When **none** is known, the rotator does not spend money
by default: it writes the no-capacity ALERT and stays on the plan login. To
let an unknown reset move work onto credits at once, opt in:

```bash
amplihack profile rotate stop
amplihack profile rotate start --overflow-unknown-reset 1
```

`AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET=1` in the environment of
`rotate start` does the same. Auto-switch follows the running rotator's
choice either way. The `overflow: on` log line then reads `next plan reset
unknown`.

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
   away and an overflow profile can be used, it swaps to the first one by
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
no reading. Either may have room. Credits do not wait for these logins, and
the `per-token` line says how many there were (`2 not measured`). The API
key does wait: see [The order the rotator follows](#the-order-the-rotator-follows).

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
2026-10-24T09:00:05Z cycle live=work work:s=100%,w=91% personal:s=100%,w=100% ryan-mistt-org:s=100%,w=100%
2026-10-24T09:00:05Z per-token ryan-mistt-org: no measured plan login below 100%, 0 not measured; earliest reset 2026-10-24T14:00:00Z beyond overflow_wait; marked overflow tier 1; probe ok, no spend limit
2026-10-24T09:00:05Z swap work->ryan-mistt-org live s=100% w=91% target s=100% w=100% reason=overflow-in
2026-10-24T09:00:05Z overflow: on ryan-mistt-org; no measured plan login below 100%, next plan reset 2026-10-24T14:00:00Z
2026-10-24T14:10:05Z swap ryan-mistt-org->personal live s=100% w=100% target s=0% w=62% reason=overflow-out
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
`claude -p /usage`. If sessions on the overflow profile fail with that
message but no block appears in `overflow list`, the rotator cannot tell, and
you have two ways out:

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

If the login in `~/.claude` belongs to no profile and there is no ledger, the
first swap keeps it by creating a profile for it called `default-original`
(or `default-original-2`, and so on, if that name is taken) and moving the
outgoing login there. Only the `oauthAccount` block from `~/.claude.json` is
copied into the new profile's `.claude.json`. A dry run prints
`would adopt ~/.claude as default-original`.

If the rotator is stopped after creating `default-original` but before
copying the account into it, the profile is left empty and nothing has moved.
The next swap does not recognise it and adopts the login again as
`default-original-2`. Remove the empty one:

```bash
amplihack profile remove default-original --yes
```

## Swap refused

The rotator checks everything before it moves anything. If a check fails it
moves nothing and exits with status 1:

```
error: swap refused: profile work already has a login
hint: nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md
```

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
| `~/.claude.json is not a JSON object` | The swap could not record the new account in `~/.claude.json`, so it refuses before anything moves. Repair the file so it holds a JSON object. |

Three situations write an ALERT instead of refusing:

- `ALERT: no login in ~/.claude; no swap`. Log in with plain `claude` first.
  The rotator never fills an empty `~/.claude`.
- `ALERT: the account in ~/.claude is not the one the ledger names (live=work); <what it found>; no swap`.
  Someone ran `/login` in `~/.claude`. A manual `/login` in `~/.claude`
  replaces whatever login was there; the profile that login belonged to is
  left with no login. When the new account belongs to exactly one profile,
  the rotator recovers by itself and this ALERT is not written. Otherwise
  the ALERT is followed by numbered `fix` lines with the exact commands, and
  it is repeated on every cycle until it is resolved. See
  [After a manual /login in ~/.claude](#after-a-manual-login-in-claude).
- `ALERT: auto-switch recovery is inconsistent; no swap`. Run
  `amplihack profile auto-switch repair`.

## Recovery

A swap is two renames and an update to `~/.claude.json`, recorded in a
journal before the first rename. If the rotator is killed part-way, the next
`rotate`, `start` or cycle finishes the swap or undoes it before doing
anything else. It decides by where the outgoing and incoming logins actually
are, found by inode number, not by the step the journal last recorded. It
only renames logins; it never deletes or overwrites one. It also removes the
refresh locks it took, and only those, even when it then marks the journal
`inconsistent`. A run with a different `HOME` (under `sudo` or `cron`) leaves
the journal alone for the run with the right `HOME`. After that it checks for
a [manual `/login` in `~/.claude`](#after-a-manual-login-in-claude) and
recovers from it where that is safe.

When it cannot tell which way is safe, it marks the journal `inconsistent`,
writes an ALERT, and stops swapping until you resolve it:

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
names that profile as live, so the account in `~/.claude.json` no longer
matches it. On every `rotate`, `start` and background cycle the rotator then
asks which profiles hold the account that is now in `~/.claude`, comparing
user and organization, and acts on the answer before it measures anything.

The examples below use the case from
[#1693](https://github.com/rysweet/amplihack-rs/issues/1693): the ledger names
`ryan-ryansweet`, someone ran `/login` in `~/.claude` as the account of
profile `ryan-mistt-org`, and the root is `/home/you/.claude-profiles`.

#### One profile has the account and no login of its own

The login in `~/.claude` is that profile's, and nothing needs to move. The
rotator records `ryan-mistt-org` as live and says what was lost:

```
2026-10-27T03:17:05Z recovered: ~/.claude holds ryan-mistt-org's login (ledger named ryan-ryansweet); recorded ryan-mistt-org as live
2026-10-27T03:17:05Z ALERT: profile ryan-ryansweet has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login ryan-ryansweet
```

#### One profile has the account and its own login too

Now the same account is logged in twice: in `~/.claude` and in
`ryan-mistt-org`. The rotator keeps the copy in `~/.claude` and moves the
profile's copy to a backup. It renames the file; it never copies, opens or
deletes a login:

```
2026-10-27T03:17:05Z recovered: ~/.claude and ryan-mistt-org held the same account; kept ~/.claude, moved ryan-mistt-org's copy to /home/you/.claude-profiles/.rejected-logins/rotate-ryan-mistt-org-20261027T031705Z/.credentials.json; recorded ryan-mistt-org as live
2026-10-27T03:17:05Z ALERT: profile ryan-ryansweet has no login anywhere: a manual /login in ~/.claude replaced it. Run: amplihack profile login ryan-ryansweet
```

The backup directory is `0700`. If that name is taken, `-2`, `-3` and so on
are added. Backups are never deleted. The profile's `.claude.json` is not
changed.

The move happens only when every check passes:

- no `claude` session is running on `ryan-mistt-org`, and the rotator can
  see running sessions at all;
- Claude is not refreshing its token in `~/.claude` or in `ryan-mistt-org`;
- auto-switch has not lent `ryan-mistt-org`'s login to a session, and its
  directory does not host another account's login;
- the profile's login is a private regular file with one link;
- `~/.claude`, the profile and the profile root are on the same filesystem,
  and all of them are private and owned by you;
- no swap journal is pending or inconsistent;
- `~/.claude` still holds the same account just before the move.

If one fails, nothing moves, and you get a refusal with its fix line (see
below). Most refusals clear by themselves, and the next cycle tries again.
The checks on `~/.claude`, the profile root and the swap journal also apply
when the profile has no login of its own; a failure there is refused the
same way, with the ALERT saying `profile ryan-mistt-org holds the same
account; recovery refused: <reason>`.

If the rotator is killed part-way, the next cycle finishes the job: an empty
backup directory is harmless, and a moved login with no ledger update is
handled as the first case.

#### The same cycle carries on

After recovery the cycle measures and decides as usual. `ryan-ryansweet` has
no login, so it is skipped. On 27 Oct `ryan-mistt-org` was weekly-exhausted
and `rssourcelabs` had capacity, so the same cycle swaps:

```
2026-10-27T03:17:05Z skip: ryan-ryansweet (no login)
2026-10-27T03:17:35Z cycle live=ryan-mistt-org ryan-mistt-org:s=100%,w=100% rssourcelabs:s=12%,w=40%
2026-10-27T03:17:35Z swap ryan-mistt-org->rssourcelabs live s=100% w=100% target s=12% w=40% reason=threshold
```

With no candidate that has capacity, it writes the usual
`ALERT: no profile has capacity` instead.

The `has no login anywhere` line is written on every cycle until
`ryan-ryansweet` has a login again or the profile is removed:

```bash
amplihack profile login ryan-ryansweet
```

If the profile the ledger names is the one that matches (the ledger's
account was out of date), the same two rules apply and there is no
`has no login anywhere` line.

#### When the rotator cannot tell

In every other case nothing moves. The ALERT says what it found, and the
numbered `fix` lines after it are the commands to run, with the real
profile names and paths. Paths are shell-quoted where needed. The lines are
written on every cycle until the account in `~/.claude` matches the ledger
again, and `amplihack profile rotate status` shows them too. The
`fix 1: amplihack profile login <name>` line appears only when the profile
the ledger names has no login.

| What it found | ALERT ends with | Fix lines after `amplihack profile login ryan-ryansweet` |
|---------------|-----------------|------------------------------------------------------------|
| Several profiles have the account | `it matches more than one profile (mistt-spare, ryan-mistt-org); no swap` | `keep this account in one of mistt-spare, ryan-mistt-org: log the others in to another account or remove them; the next cycle then recovers` |
| No profile has the account | `no profile has that account; no swap` | Two lines; see the example below. |
| A profile's account cannot be read | `the account of profile old-work cannot be read, so the match is uncertain; no swap` | `amplihack profile login old-work; the next cycle then recovers` |
| `~/.claude.json` lost its account between two reads | `the account in ~/.claude.json cannot be read; no swap` | `none needed; the next cycle reads ~/.claude.json again` |
| `~/.claude` has no login | `~/.claude has no login; no swap` | `run claude with no CLAUDE_CONFIG_DIR and /login` |
| One profile has the account, and a check failed | `profile ryan-mistt-org holds the same account and has its own login; recovery refused: <reason>; no swap` (without `and has its own login` when it has none) | One line for that reason; see the refusal table below. |

When `~/.claude.json` has no account the rotator can read at all, it is not
treated as a hand change. Claude rewrites that file often, and one bad read
says nothing about whose login is in `~/.claude`. The cycle moves nothing,
does not swap, and writes one line:

```
ALERT: the account in ~/.claude.json cannot be read (live=ryan-ryansweet); no swap this cycle, the next cycle reads it again; do not /login in ~/.claude, it would replace ryan-ryansweet's login
```

Do not run `/login` in `~/.claude` because of this line. If the ledger is
right, the login there is `ryan-ryansweet`'s, and a `/login` would leave that
profile with no login anywhere. The next cycle reads the file again.

If the file still has no readable account 5 minutes later, the line changes:
it says what is wrong with the file and gives two fixes, both of which keep
`ryan-ryansweet`'s login. `rotate status` shows them too:

```
ALERT: ~/.claude.json has had no account that can be read for 5 minutes or more (live=ryan-ryansweet): it has no oauthAccount block; no swap until it is fixed; do not /login in ~/.claude before the login there is set aside, it would replace ryan-ryansweet's login
fix 1: run claude with no CLAUDE_CONFIG_DIR and without /login, then quit it; if /home/you/.claude.json then holds an account, the next cycle goes on
fix 2: or give ryan-ryansweet its login back and start over: close every claude session that uses ~/.claude, then run: test ! -e /home/you/.claude-profiles/ryan-ryansweet/.credentials.json && mv -n /home/you/.claude/.credentials.json /home/you/.claude-profiles/ryan-ryansweet/ && rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json; then run claude with no CLAUDE_CONFIG_DIR and /login
```

Try `fix 1` first. `fix 2` puts the login back in the profile it belongs to
before you log in to `~/.claude` again. The other things the file can be:
missing, not valid JSON, not a JSON object, or an `oauthAccount` with no
usable account id or e-mail. The reference lists each wording under
[Stuck `~/.claude.json`](../reference/profile-command.md#stuck-claudejson).

For example, with no profile holding the account:

```
2026-10-27T03:17:05Z ALERT: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); no profile has that account; no swap
2026-10-27T03:17:05Z fix 1: amplihack profile login ryan-ryansweet
2026-10-27T03:17:05Z fix 2: rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json (the next swap keeps the login in ~/.claude as profile default-original)
2026-10-27T03:17:05Z fix 3: or, to set that login aside instead: close every claude session that uses ~/.claude, then run: mkdir -m 700 /home/you/.claude-profiles/.rejected-logins/manual-20261027T031705Z && mv -n /home/you/.claude/.credentials.json /home/you/.claude-profiles/.rejected-logins/manual-20261027T031705Z/ && rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json; then run claude with no CLAUDE_CONFIG_DIR and /login
```

Without the final `/login`, `~/.claude` stays empty and every cycle only
writes `ALERT: no login in ~/.claude; no swap`.

Refusals, with the fix line written for each (`<dir>` is
`/home/you/.claude-profiles/.rejected-logins/manual-20261027T031705Z`):

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

The first four can appear whether or not the profile has its own login; the
rest only when it does.

`amplihack profile rotate status` shows the current lines:

```
live: ryan-ryansweet
last cycle: 2026-10-27T03:17:05Z alerted
alert: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); no profile has that account; no swap
alert: fix 1: amplihack profile login ryan-ryansweet
alert: fix 2: rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json (the next swap keeps the login in ~/.claude as profile default-original)
alert: fix 3: or, to set that login aside instead: close every claude session that uses ~/.claude, then run: mkdir -m 700 /home/you/.claude-profiles/.rejected-logins/manual-20261027T031705Z && mv -n /home/you/.claude/.credentials.json /home/you/.claude-profiles/.rejected-logins/manual-20261027T031705Z/ && rm /home/you/.claude-profiles/.auto-switch.d/rotate/ledger.json; then run claude with no CLAUDE_CONFIG_DIR and /login
```

After a recovery the `alert:` lines are gone, and `status` shows
`warning: profile ryan-ryansweet has no login anywhere: …` until that
profile is logged in again.

To see what the next cycle would do, without moving anything:

```bash
amplihack profile rotate --dry-run
```

It prints the recovery it would make, or the ALERT and its `fix` lines, and
stops there without measuring:

```
would alert: the account in ~/.claude is not the one the ledger names (live=ryan-ryansweet); no profile has that account; no swap
fix 1: amplihack profile login ryan-ryansweet
```

The `fix 2` and `fix 3` lines that follow are the same as in the log example above.

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
swap iivanovich->api-key live s=100% w=61% target s=0% w=0% reason=overflow-in
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
profile's own directory. It uses the rotator's measurement instead: once the
rotator's capacity file lists the live login at or over the threshold, that
login no longer keeps auto-switch off the key. Until then, which is the case
when no rotator is running or the rotator found room on it, the live login
keeps the key out until the rotator moves `~/.claude` to another login. To
let auto-switch use the key in that state, add the live login to the
`exclude` list.

### See when work is on the API key

```
$ amplihack profile rotate status
...
~/.claude is on API key profile api-key: work is billed per token (since 2026-10-28T14:02:11Z)
...
$ amplihack profile usage api-key
PROFILE  MARKS              STATE    SESSION                     SESSION RESETS  WEEK  WEEK RESETS
api-key  live in ~/.claude  api_key  api-key (billed per token)  -               -     -
live: work is running on the API key (billed per token)
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
`ALERT: swap refused: <name> has no oauth login and is not an API-key profile`.

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
and until the rotator has recovered, the ledger names a profile whose
account is no longer in `~/.claude`. In that state `profile usage` still
reports the account that is actually there:

- If exactly one profile has that account, its row is marked
  `live in ~/.claude` and its numbers are read from `~/.claude`, even when
  the profile also has its own login. The duplicate-login warning for
  `'~/.claude'` and that profile is still printed, because both copies exist
  until recovery moves one aside. The warnings still go by the ledger, so
  only the profile the ledger names (`ryan-ryansweet`) is treated as the
  live login, and `profile login` still names the holder
  `~/.claude (rotator live profile 'ryan-ryansweet')`.
- If no profile has it, and no profile names were given, an extra row named
  `~/.claude` shows its numbers. In `--json` its `profile`, `account` and
  `live_in` are all `"~/.claude"`. That row is never recorded as an
  auto-switch reading.
- Otherwise no row is marked, as before.

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
  and that a used-up spend limit shows there as `spend_limit`. An overflow
  profile whose probe fails is never used. A spend block always lasts until
  the 1st of the next month (UTC), whatever reset Claude states; lift it
  sooner with `amplihack profile overflow clear <name>`.
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
- **Each cycle starts `claude` once per profile.** The probes run
  `claude -p /usage` in parallel, so a short `--interval` means many processes.
