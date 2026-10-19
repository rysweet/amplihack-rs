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
- The user mentions rotating, swapping or balancing Claude logins, or the
  `claude-rotator` tmux session.

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
