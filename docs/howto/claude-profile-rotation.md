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
- [Change the thresholds](#change-the-thresholds)
- [When a swap happens](#when-a-swap-happens)
- [Which profiles can take over](#which-profiles-can-take-over)
- [Read the log](#read-the-log)
- [Move from the tmux prototype](#move-from-the-tmux-prototype)
- [Where a login with no profile goes](#where-a-login-with-no-profile-goes)
- [Swap refused](#swap-refused)
- [Recovery](#recovery)
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

A profile in any state other than `ok`, `session_exhausted` or
`weekly_exhausted` is never chosen.

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
rotate: running
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

The background process keeps running after you log out. It does not start
again after a reboot; run `amplihack profile rotate start` again, or add it to
your login scripts.

To run the loop in the foreground instead, for example under your own
supervisor, use `amplihack profile rotate watch`. It takes the same flags as
`start` and stops on Ctrl-C.

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
   the weekly use is at or above `--week-at`, or the live account could not be
   measured, it swaps when the candidate has at least `--margin` more headroom.
   A live account that could not be measured counts as headroom 0.
2. **ALERT.** If a threshold swap is needed but no candidate has that margin,
   it writes one `ALERT` line and leaves `~/.claude` alone.
3. **Upgrade.** If no threshold is reached, it swaps when the candidate has at
   least `--upgrade-margin` more headroom, unless the last swap was less than
   `--hold` seconds ago.

The hold stops flapping. Just after a swap, the account that was swapped out
often has its session window reset and looks fresh; without the hold the next
cycle would swap straight back. Threshold swaps are never held back.

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
```

To see only swaps and alerts:

```bash
grep -E ' (swap|ALERT:) ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log
```

The same ALERT is written once. It is written again only after something
changes: a swap succeeds, a cycle needs no swap, or the reason changes. The log
holds profile names, percentages and fixed text; it never holds account IDs,
e-mail addresses, tokens or Claude's output.

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
error: rotate refused the swap: profile "work" already has a login
hint: nothing was moved; see "Swap refused" in docs/howto/claude-profile-rotation.md
```

| Reason | What to do |
|--------|------------|
| `profile "<name>" already has a login` | The profile the live login should return to has a `.credentials.json` of its own. Decide which login to keep; log that profile in again with `amplihack profile login <name>` if needed. |
| `~/.claude and the profile root are on different filesystems` | Move the profile root onto the same filesystem as your home directory. Logins are only ever renamed, never copied. |
| `the login in "<path>" cannot be moved` | It is a symlink, has more than one link, or is not owned by you. Replace it with a plain file by logging in again. |
| `"<path>" is not owned by you or is writable by others` | Fix the ownership or mode (`chmod go-w`). |
| `atomic exchange is not supported here` | The system or filesystem cannot swap two files in one step. Only Linux can. Nothing was moved. |

Two situations write an ALERT instead of refusing:

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
refresh locks it took, and only those.

When it cannot tell which way is safe, it marks the journal `inconsistent`,
writes an ALERT, and stops swapping until you resolve it:

```
error: rotate state is inconsistent: a login is not where the journal says
hint: see the Recovery section of docs/howto/claude-profile-rotation.md
```

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

If you cannot tell which login belongs where, log each affected profile in
again with `amplihack profile login <name>`.

## Limits

- **Linux only in practice.** On macOS Claude keeps logins in the Keychain, so
  there is no `.credentials.json` to move and the rotator writes the
  no-login ALERT. Hosts without `/proc` cannot tell which profiles have running
  sessions, so every profile counts as in use and none is a candidate;
  `status` reports `unverifiable`. On Windows every `rotate` command is
  refused.
- **Token refresh race.** A `claude` process that refreshes its token during
  the two renames of a swap can write the old account's token into
  `~/.claude`. The rotator holds Claude's refresh lock while it swaps and keeps
  the window to two system calls, but cannot close it. If a session suddenly
  needs `/login` after a swap, log the affected profile in again. A refresh
  after the exchange is harmless to recovery, which never expects a
  particular file in `~/.claude`.
- **Stale refresh lock.** If the rotator is killed in the instant between
  making Claude's refresh lock and recording it, the lock stays behind and
  `claude` may wait on it. With no `claude` and no rotator running, remove it
  with `rmdir ~/.claude/.oauth_refresh.lock`.
- **Pid reuse.** Before `stop` signals the process it checks the process's
  owner and command line. On Linux the signal goes through a process handle,
  so it cannot reach a new process with the same pid. Elsewhere a short window
  remains between the check and the signal.
- **Sessions keep their login until they reread it.** A running `claude` may
  keep using the outgoing account until it next reads its credentials.
- **Each cycle starts `claude` once per profile.** The probes run
  `claude -p /usage` in parallel, so a short `--interval` means many processes.
