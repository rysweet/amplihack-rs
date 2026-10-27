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
- [Move from the tmux prototype](#move-from-the-tmux-prototype)
- [Where a login with no profile goes](#where-a-login-with-no-profile-goes)
- [Swap refused](#swap-refused)
- [Recovery](#recovery)
- [Live default and duplicate logins](#live-default-and-duplicate-logins)
- [Limits](#limits)

## Before you start

You need:

- Linux. On other systems see [Limits](#limits).
- At least two profiles, each logged in, created with
  `amplihack profile add <name>` and `amplihack profile login <name>`.
- A login in `~/.claude`, made with plain `claude` and `/login`.
- `~/.claude` and the profile root (`$AMPLIHACK_CLAUDE_PROFILE_ROOT`, by
  default `~/.claude-profiles`) on the same filesystem.

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
changes. The one-shot `rotate` still prints it on every run. The log
holds profile names, percentages and fixed text; it never holds account IDs,
e-mail addresses, tokens or Claude's output.

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
   `stayed` with no ALERT. After the first hour it reads the ledger
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
nowhere to move the login, so it stays put. The log is not the only place you
hear about it. When a cycle gets as far as deciding to swap, stay or raise
the ALERT, the rotator updates the shared capacity state from that cycle's
measurements, and the status line, Claude Code, `amplihack claude` and
`amplihack recipe run` show a warning such as:

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
`skip: state changed during measurement`, does not update the warning; the
next full cycle does.

The warning clears by itself when the first account resets. Where it
appears and how it is deduplicated is in
[When no account has capacity](claude-profiles.md#when-no-account-has-capacity);
the file and the exact rules are in the
[capacity notice reference](../reference/profile-command.md#capacity-notice).

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
- `ALERT: the account in ~/.claude is not the one the ledger names (live=work); no swap`.
  Someone ran `/login` in `~/.claude`. The login that was there has been
  replaced by Claude, so profile `work` has no login. Run
  `amplihack profile login work`, then delete
  `<root>/.auto-switch.d/rotate/ledger.json`. The next cycle works out the
  live profile again. If an old prototype ledger is still in
  `<root>/.rotator/`, it is used only if the profile it names has no login and
  a matching account; because `work` now has a login, a ledger naming `work`
  is ignored. If no profile matches the new account, the next swap adopts it
  as `default-original`.
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
the journal alone for the run with the right `HOME`.

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
`~/.claude-profiles/.auto-switch.d/rotate/ledger.json`; the next cycle works
out the live profile again from the accounts.

If you cannot tell which login belongs where, log each affected profile in
again with `amplihack profile login <name>`.

## Live default and duplicate logins

While a profile's login is in `~/.claude`, the profile's own directory has
no login. If that profile is also your default (`amplihack profile switch`),
`amplihack claude` runs against `~/.claude` instead of the empty directory
and prints:

```
note: profile 'team' is the rotator's live login, which is in ~/.claude; launching against ~/.claude.
```

The same happens for `--profile team` and for an inherited
`AMPLIHACK_CLAUDE_PROFILE=team`. Without this, Claude would ask you to log
in, and logging in there would put a second copy of an account into
`team`. To stop using a default at all, run
`amplihack profile switch --none`. See
[Launching the rotator's live profile](claude-profiles.md#launching-the-rotators-live-profile).

`amplihack profile login <name>` refuses a login that duplicates the account
in `~/.claude`, naming the holder as `~/.claude (rotator live profile 'team')`.
`profile usage` and `auto-switch status` warn about a profile that holds the
same account as `~/.claude`. The live profile itself is never flagged against
`~/.claude`. See [Duplicate logins](claude-profiles.md#duplicate-logins).

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
  refused.
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
- **Each cycle starts `claude` once per profile.** The probes run
  `claude -p /usage` in parallel, so a short `--interval` means many processes.
