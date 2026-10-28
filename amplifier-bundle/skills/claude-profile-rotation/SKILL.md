---
name: claude-profile-rotation
description: Set up and run automatic rotation of the default Claude login in ~/.claude across several amplihack login profiles, so plain `claude` always uses the account with the most session and weekly capacity. Use when the user has more than one Claude account, hits usage limits, asks to rotate or swap Claude logins, or mentions the claude-rotator tmux session.
metadata:
  version: "1.0"
  author: amplihack
---

# Claude Profile Rotation

## Purpose

Drive `amplihack profile rotate`, which measures every Claude login profile
with `claude -p /usage` and moves the login with the most capacity into
`~/.claude` when the current account nears its limit. The outgoing login goes
back to the profile it came from. Logins are moved, never copied.

Full guide: `docs/howto/claude-profile-rotation.md`. Exact behaviour:
the "Rotate" section of `docs/reference/profile-command.md`.

## When I Activate

- The user has two or more Claude accounts and wants plain `claude` to use
  whichever has capacity.
- The user hits session or weekly usage limits and asks to switch accounts.
- The user has usage credits on an account and wants the rotator to use it
  when every plan login is exhausted, or mentions overflow or "running on
  credits".
- The user mentions rotating, swapping or balancing Claude logins, or the
  `claude-rotator` tmux session.
- A session stopped with "You've hit your ... limit" and did not resume, or
  the user asks about the tmux limit nudger.

## Rules

- Never open, read, print, copy, hash or log any `.credentials.json` or token.
  To inspect logins, use `ls -li` only.
- Never copy a login file. Move it with `mv -n` if a person must move one.
- Always run a dry run before the first real rotation, and after any threshold
  change.
- Do not stop the user's `claude-rotator` tmux session without asking them.

## Prerequisites

Check each one and tell the user what is missing:

1. Linux. On macOS logins are in the Keychain and cannot be rotated.
2. At least two profiles, each logged in:
   ```bash
   amplihack profile list
   amplihack profile usage
   ```
   Every profile to be used should show state `ok`. Log one in with
   `amplihack profile login <name>`.
3. A login in `~/.claude`, made with plain `claude` and `/login`.
4. `~/.claude` and the profile root on the same filesystem.

## Dry run first

```bash
amplihack profile rotate --dry-run
```

It prints a `cycle` line with every profile's session and weekly use, and a
`would swap`, `would stay`, `would alert` or `would adopt` line. It changes
nothing, and it does not finish an interrupted swap; a pending one shows as
`would recover an interrupted swap`. Explain the result to the user before
going further.

`would adopt ~/.claude as default-original` means the login in `~/.claude`
belongs to no profile; the first swap will keep it in a new profile called
`default-original`.

## Rotate once

```bash
amplihack profile rotate
```

## Run it in the background

Background rotation pauses while auto-switch is turned off (`auto-switch
off`); with no auto-switch setting at all it runs. Turning auto-switch on
also enables it for profiled sessions; tell the user.

```bash
amplihack profile auto-switch on
amplihack profile rotate start
amplihack profile rotate status
```

Stop it with:

```bash
amplihack profile rotate stop
```

If the external prototype is running, `start` fails with
`stop the external rotator (tmux session claude-rotator) first`. Ask the user
before running `tmux kill-session -t claude-rotator`. The built-in rotator
reads the prototype's ledger to learn the live profile, ignores it if the
profile it names has a login or a different account, and never writes to
`<root>/.rotator/`.

## Tuning

| Flag | Default | Meaning |
|------|---------|---------|
| `--session-at` | 85 | Swap when live session use reaches this %. |
| `--week-at` | 90 | Swap when live weekly use reaches this %. |
| `--margin` | 5 | A threshold swap needs this many more points of headroom. |
| `--upgrade-margin` | 30 | Swap early when a profile has this many more points of headroom. |
| `--hold` | 3600 | Seconds after a swap during which no early swap happens. |
| `--overflow-wait` | 1800 | Use an overflow profile only when the next plan reset is further away than this many seconds. |
| `--interval` | 600 | Seconds between checks (`start` and `watch` only). |

Flags go after `start`:

```bash
amplihack profile rotate start --session-at 75 --interval 300
```

To change settings of a running rotator, run `stop` then `start` again. To
keep a profile out of rotation: `amplihack profile auto-switch exclude <name>`.

## Reading the log

The log is `<root>/.auto-switch.d/rotate/rotate.log` (root defaults to
`~/.claude-profiles`):

```bash
grep -E ' (swap|ALERT:) ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log | tail
```

| ALERT | What to tell the user |
|-------|-----------------------|
| `no profile has capacity` | Every account is near its limit. Wait for a reset or add a profile. |
| `no login in ~/.claude` | Log in with plain `claude` first. |
| `the account in ~/.claude is not the one the ledger names` | Someone ran `/login` in `~/.claude`. Log the named profile in again with `amplihack profile login <name>`, then delete `<root>/.auto-switch.d/rotate/ledger.json`. |
| `rotate recovery is inconsistent` | Follow manual recovery below. |
| `auto-switch recovery is inconsistent` | Run `amplihack profile auto-switch repair`. |

## Overflow onto usage credits

An account with usage credits turned on keeps working past its plan limit,
and the extra use is billed to the credits. Marked as an *overflow profile*,
the rotator moves work onto it only when every plan login is out of capacity
and the next plan reset is known and more than `--overflow-wait` seconds away
(default 1800), and moves work back in the first cycle in which a plan login
has capacity. Auto-switch follows the same rule through the same gate: it
uses an overflow profile only when every plan profile is over the threshold
beyond the wait, and moves the session back once a plan profile has room.

An unknown plan reset (no reading reports one) does **not** move work onto
overflow unless the user opts in with `--overflow-unknown-reset 1`
(`AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET=1`); with it, an unknown reset
spends money at once. Overflow marks have a tier (`overflow add <name>
--tier N`, 1-9, default 1): lower tiers are tried first, for example usage
credits before an API key.

Before marking a profile, tell the user:

- **It costs real money**, and the rotator is not a spending cap. They must
  turn on usage credits and set an organization spend limit at
  https://claude.ai/settings/usage themselves; amplihack cannot change that
  setting.
- **Leave the default profile unset** (`amplihack profile switch --none`).
  A default wins over the rotator, so a default naming the overflow profile
  spends credits on every launch.
- **Upgrade every amplihack binary first** and restart the rotator. Older
  binaries treat the `overflow=` line in `<root>/.auto-switch` as corrupt and
  reject the new ledger fields.

```bash
amplihack profile overflow add ryan-mistt-org
amplihack profile overflow list
amplihack profile rotate stop
amplihack profile rotate start --overflow-wait 1800
amplihack profile rotate status
```

The mark is the `overflow=` line in `<root>/.auto-switch`, not a
`config.toml` key: the rotator and auto-switch already re-read that file on
every cycle, and one location avoids two sources of truth. `add` and
`remove` wait up to 10 seconds for a running cycle; if they fail with
`.auto-switch is being changed by another process ...; retry`, run them
again. `overflow list` prints `missing` after a marked name with no profile
directory.

`--overflow-wait` takes `0` to `604800` seconds; `0` uses overflow as soon as
plan capacity is gone and a plan reset is known. A plan login loses capacity when either its session
or its week use reaches the threshold.

`rotate status` shows `overflow:` and `on overflow:` lines. `on overflow: yes
(<name>)` needs the live login to be marked and the last cycle to have
recorded `overflow=in` or `on`; `no (<name> is overflow; plan capacity
remains)` means it was put there while a plan login had room. A `default:`
line appears when the default pointer names an overflow profile. `profile
usage` marks the profile `overflow` and ends with `running on overflow:
<name>` only when its login is in `~/.claude` and its probe shows the session
or weekly limit reached. Log lines: `reason=overflow-in`,
`reason=overflow-out`, `overflow: on|staying on|off ...`.

An excluded profile is never used, even when marked. If the user moves an
overflow login into `~/.claude` by hand while a plan login has capacity, the
next cycle moves it out; to stay on it, remove the mark or stop the rotator.

While on credits the rotator reports capacity, so the limit-resume hook
resumes stopped sessions and they spend credits.

| ALERT | What to tell the user |
|-------|-----------------------|
| `overflow profile <name> hit its spend limit; blocked until <time>` | The probe read `You've hit your org's monthly spend limit` or `... has reached its monthly spend limit`: the credits are used up or turned off. The profile is not used until 00:00 UTC on the 1st of next month. After raising the limit or adding credits, run `amplihack profile overflow clear <name>`. If it says `rotator busy, retry`, run it again. |

A plan login *has capacity* when its probe gave `ok` and its session and
week use are below `--session-at` and `--week-at`; `--margin` does not
apply. The rotator never moves onto credits while one has capacity.

To turn it off: `amplihack profile overflow remove <name>`. Removing the
mark, or `rotate stop`, does not stop spending while that login is in
`~/.claude` and no plan login has the margin: the login stays there. To stop
spending at once, tell the user to turn off usage credits or lower the spend
limit at https://claude.ai/settings/usage, then remove the mark. If sessions
on the overflow profile fail with the spend-limit message but `overflow list`
shows no block: after `remove`, the rotator swaps it out only after three
failed probes in a row; after `rotate stop`, nothing moves it.

## Resuming sessions stopped by a limit

`amplihack install` registers the `amplihack-hooks limit-resume` hook on
Claude Code's `StopFailure` event (matcher `rate_limit`, `asyncRewake: true`).
When a turn ends on a usage limit and auto-switch is on, it waits up to 7
days for a rotator cycle that started measuring after the limit and
ended `swapped` or `stayed` with no ALERT, then exits `2` with a continue
message, which wakes the session. That wake, and Claude Code running
`StopFailure` instead of `Stop` on a limit, were verified live on Claude Code
2.1.306 and 2.1.307; if a session does not wake, read `resume.log` first.
The tmux limit nudger (`limit-nudger.sh`) is no longer needed once this hook
is installed.

Tell the user it needs:

- Claude Code 2.1.306 or later, and `amplihack install` run again after
  upgrading amplihack or Claude Code (install leaves the hook out for an
  older `claude`);
- auto-switch on and `amplihack profile rotate start` running.

Check it is registered:

```bash
jq '.hooks.StopFailure' ~/.claude/settings.json
```

Read its decisions (no message text or tokens are logged):

```bash
tail ~/.claude-profiles/.auto-switch.d/resume/resume.log
```

| Outcome | What to tell the user |
|---------|-----------------------|
| `resumed` | The session was told to continue. |
| `no-capacity-timeout` | No login had capacity within 7 days. |
| `rotator-down` | The rotator was not running (or its ledger was unreadable) when the limit hit; start it. |
| `session-gone` | The session exited while the hook waited; nothing was done. |
| `moved-on` | The user sent a prompt in that session during the wait (for example `continue`), so the hook did not wake it. |
| `disabled` | Auto-switch was turned off while the hook was waiting. |
| `duplicate` | That limit event (same session, text and live login) was already resumed, or another hook woke the session first. |
| `cap` | The session resumed 3 times in 24 hours; it will not resume again until older resumes age out. |
| `stop-hook-active` | Claude Code reported a stop hook already running; nothing was done. |
| `guard-failed` | The resume state under `<root>/.auto-switch.d/resume/` could not be used safely. Check its owner and mode (`0700`). |

It never resumes when every login is at capacity, at most once per limit
event and once per wait, and at most 3 times per session per day. Each
`resume.log` line also has `cycle=`, the rotator's last cycle outcome.

The tmux limit nudger (`~/.claude-profiles/.backups/limit-nudger.sh`) is no
longer needed once the hook is registered. Ask the user before stopping it.
Whether the wake gets past Claude Code's `What do you want to do?` limit menu
has not been tested; if a session stays on the menu after `resumed`, the user
presses Esc and types `continue`.

## Manual recovery

Only when the rotator reports `inconsistent`:

1. `amplihack profile rotate stop`
2. Read `<root>/.auto-switch.d/rotate/journal.json`. It holds profile names,
   a phase and inode numbers, no secrets.
3. List the logins by inode, without reading them:
   ```bash
   ls -li ~/.claude/.credentials.json ~/.claude-profiles/*/.credentials.json
   ```
4. Help the user move each login to its place with `mv -n`, so each inode is
   in exactly one place. The login in `~/.claude` may have a new inode if
   Claude refreshed it; that is expected.
5. If `.oauth_refresh.lock` exists in `~/.claude` or the `to` profile and its
   inode (`ls -di`) matches the journal's `home_lock_ino` or `to_lock_ino`,
   remove it with `rmdir`.
6. Delete `journal.json`, then run `amplihack profile rotate --dry-run`.

If it is unclear which login is which, have the user log the affected
profiles in again with `amplihack profile login <name>`.
