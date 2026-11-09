---
title: How to Rotate Codex Accounts Automatically
description: Keep ~/.codex on the Codex account with the most room, and resume Codex sessions stopped by a usage limit.
doc_type: howto
review_schedule: as-needed
---

# How to Rotate Codex Accounts Automatically

The login rotator, `amplihack profile rotate`, also looks after Codex. On
each cycle it reads every Codex account live and points the default Codex
home, `~/.codex`, at the account with the most room. A Codex session that
stops on a usage limit can be resumed on another account, or after the limit
resets, with `amplihack profile codex-resume`.

Use it when you run plain `codex` (no `CODEX_HOME`) and have more than one
Codex account. Claude rotation is described in
[How to Rotate Claude Logins Automatically](claude-profile-rotation.md); the
same rotator does both.

The exact flags, line formats, refusal codes and exit statuses are in the
[Rotate Codex reference](../reference/profile-command.md#rotate-codex).

## Contents

- [Before you start](#before-you-start)
- [Make `~/.codex` a link](#make-codex-a-link)
- [Preview a Codex swap](#preview-a-codex-swap)
- [Run Codex rotation with the rotator](#run-codex-rotation-with-the-rotator)
- [Read the Codex lines in the log](#read-the-codex-lines-in-the-log)
- [When a swap is refused](#when-a-swap-is-refused)
- [Keep a job on one account](#keep-a-job-on-one-account)
- [Use another link or turn Codex rotation off](#use-another-link-or-turn-codex-rotation-off)
- [Resume a Codex job that hit its limit](#resume-a-codex-job-that-hit-its-limit)
- [Limits](#limits)

## Before you start

You need:

- Linux, to have `~/.codex` swapped. On macOS and other Unix systems the
  rotator still reads every account and writes the Codex lines, and
  `codex-resume` works, but every swap is refused with `proc-unreadable`
  because the rotator cannot see which `codex` processes are running. Codex
  rotation does not run on Windows.
- A `codex` that supports `codex app-server`, installed outside your account
  directories (see [Make `~/.codex` a link](#make-codex-a-link)).
- One directory per Codex account under `~/.codex-profiles` (or
  `CODEX_PROFILES_ROOT`), each signed in:

  ```bash
  mkdir -p -m 0700 ~/.codex-profiles/work
  CODEX_HOME=~/.codex-profiles/work codex login
  ```

Check that every account reads:

```bash
amplihack profile usage
```

The Codex block at the end lists each account as `signed in`, with its
windows. See [Codex accounts](../reference/profile-command.md#codex-accounts).

## Make `~/.codex` a link

The rotator swaps `~/.codex` only when it is a symlink to one of the account
directories. It never converts a real directory itself. Do it once, with no
Codex running:

```bash
mv ~/.codex ~/.codex-profiles/personal
ln -s ~/.codex-profiles/personal ~/.codex
ls -l ~/.codex
```

```
lrwxrwxrwx 1 ryan ryan 35 Nov  8 19:02 /home/ryan/.codex -> /home/ryan/.codex-profiles/personal
```

Then check where `codex` is installed:

```bash
readlink -f "$(command -v codex)"
```

If the path is inside `~/.codex-profiles/` or `~/.codex/`, a swap would also
change which `codex` program runs, so the rotator and `codex-resume` refuse
with `codex-bin-in-home`. Reinstall Codex outside the account directories, for
example with `npm install -g @openai/codex`, or move the installed package
out and put its new location on `PATH`.

The link's parent directory (your home) and `~/.codex-profiles` must be owned
by you and not group- or world-writable.

## Preview a Codex swap

```bash
amplihack profile rotate --dry-run
```

After the Claude lines, the dry run prints the Codex cycle line and what a
real cycle would do:

```
codex cycle live=personal personal:5h=91%@11-08T20:15Z,7d=70%@11-11T03:00Z,room=9 work:5h=10%@11-08T23:40Z,7d=34%@11-12T14:30Z,room=66 read=2026-11-08T19:09:01Z -> swap personal->work
dry-run would-swap personal->work
```

A dry run takes no lock and changes nothing.

## Run Codex rotation with the rotator

Codex rotation is on by default. Start the rotator as usual:

```bash
amplihack profile rotate start --interval 120 --session-at 70
```

Each cycle reads the Claude profiles, then every Codex account once, and
swaps `~/.codex` when another account has clearly more room. The rotator adds
no interval of its own for Codex: it runs once per cycle, straight after the
Claude part. Swaps share the Claude `--hold` and `--margin` settings, so
after a Codex swap the rotator does not swap again within `--hold` seconds
while the live account still has room.

To run one cycle by hand:

```bash
amplihack profile rotate
```

## Read the Codex lines in the log

```bash
grep ' codex \| dry-run ' ~/.amplihack/claude-profiles/.auto-switch.d/rotate/rotate.log | tail
```

```
2026-11-08T19:11:02Z codex cycle live=personal personal:5h=91%@11-08T20:15Z,7d=70%@11-11T03:00Z,room=9 work:5h=10%@11-08T23:40Z,7d=34%@11-12T14:30Z,room=66 unread=spare(not-signed-in) read=2026-11-08T19:11:01Z -> swap personal->work
2026-11-08T19:11:02Z codex swap personal->work link=/home/ryan/.codex reason=room
2026-11-08T19:13:03Z codex cycle live=work work:5h=11%@11-08T23:40Z,7d=34%@11-12T14:30Z,room=66 personal:5h=91%@11-08T20:15Z,7d=70%@11-11T03:00Z,room=9 unread=spare(not-signed-in) read=2026-11-08T19:13:02Z -> keep
```

Use the root `amplihack profile root` prints if yours is not the default.

| The line ends with | Meaning |
|--------------------|---------|
| `-> keep` | The live account is already the best. |
| `-> swap a->b` | The rotator chose to swap; the next line says whether it did (`codex swap`) or why not (`codex refuse`). |
| `-> none(no-room)` | No account has room. Nothing to swap to. |
| `-> none(no-live)` | `~/.codex` is not a link to an account; a `codex refuse` line follows. |
| `-> none(held)` | A Codex swap happened less than `--hold` seconds ago and the live account still has room. |
| `-> none(damped)` | The best account beats the live one by less than `--margin` points. |
| `-> none(recovering)` | An interrupted swap was finished or undone this cycle; a `codex recover` line follows. |

An account that could not be read is listed in `unread=` with the reason
(`not-signed-in`, `timeout`, or `error:<reason>` when the reader failed or
replied with something that is not JSON). It is never chosen.

`codex skip reason=no-accounts` means there is nothing under the profiles
root. `codex skip reason=no-codex-bin` means no `codex` was found on `PATH`.

## When a swap is refused

A refusal changes nothing and is tried again on the next cycle.

| Code | What to do |
|------|------------|
| `home-not-link` | `~/.codex` is a real directory, is missing, cannot be examined (`detail=unreadable`, usually a permission problem on its folder), or `~/.codex.amplihack-swap` is not a symlink. [Make `~/.codex` a link](#make-codex-a-link), or remove the stray `~/.codex.amplihack-swap` after checking what it is. |
| `link-outside-root` | Point the link at a direct child of the profiles root. With `detail=parent-not-private` or `detail=root-not-private`, make your home or the root yours and not group- or world-writable (`chmod go-w`). |
| `link-dangling` | The account the link points at is gone. Point the link at an existing account. |
| `journal-mismatch` | An interrupted swap does not match the link, or `codex-state.json` is damaged. Inspect `~/.amplihack/claude-profiles/.auto-switch.d/rotate/codex-state.json`, fix the link by hand, then remove the `journal` entry or the file; see [Clearing a Codex journal mismatch](../reference/profile-command.md#clearing-a-codex-journal-mismatch). |
| `default-home-in-use` | A `codex` running without `CODEX_HOME` uses `~/.codex`. The rotator tries again each cycle and swaps once it has exited. |
| `proc-unreadable` | The rotator cannot list processes: there is no `/proc` (macOS, other Unix) or `/proc` is mounted with `hidepid`. Swaps stay off on that host; readings and `codex-resume` still work. |
| `lock-busy` | Another rotator action held the lock. Nothing to do. |
| `codex-bin-in-home` | `codex` is under the profiles root or `~/.codex`. Install it outside them; see [Make `~/.codex` a link](#make-codex-a-link). |

Details of each check are in
[Codex refusals](../reference/profile-command.md#codex-refusals).

## Keep a job on one account

A job started with an explicit `CODEX_HOME` keeps its account. The rotator
only changes the `~/.codex` link, never an account directory, so this job
stays on `work` whatever the rotator does:

```bash
CODEX_HOME=~/.codex-profiles/work codex exec "Summarise the open issues in this repository"
```

The rotator also leaves such processes alone when it checks whether
`~/.codex` is in use.

## Use another link or turn Codex rotation off

To manage a different link, give an absolute path:

```bash
amplihack profile rotate stop
amplihack profile rotate start --codex-home-link /home/ryan/codex-default
```

To turn Codex rotation off and keep Claude rotation:

```bash
amplihack profile rotate stop
amplihack profile rotate start --codex off
```

The variables `AMPLIHACK_ROTATE_CODEX`, `AMPLIHACK_ROTATE_CODEX_FLOOR` and
`AMPLIHACK_ROTATE_CODEX_HOME_LINK` set the same things; see
[Codex rotate settings](../reference/profile-command.md#codex-rotate-settings).

## Resume a Codex job that hit its limit

Codex has no hook that runs when a session stops, so wrap the job. This
script runs a Codex task with JSON events and, when it fails, asks
`codex-resume` to continue it:

```bash
#!/usr/bin/env bash
# codex-job.sh: run a Codex task; resume it on another account if it hits its usage limit.
set -u
work=$(mktemp -d ~/.amplihack/work/codex-job.XXXXXX)
codex exec --json "$@" > "$work/events.jsonl"
status=$?
[ "$status" -eq 0 ] && exit 0

amplihack profile codex-resume --events "$work/events.jsonl" -- "Continue the task you were working on."
rc=$?
# 2: the events show no usage limit, so keep Codex's own status.
[ "$rc" -eq 2 ] && exit "$status"
exit "$rc"
```

```bash
./codex-job.sh "Fix the failing test in crates/amplihack-cli"
```

When the job stopped on its limit, `codex-resume`:

1. takes the session id from the events file and checks it shows a usage
   limit;
2. reads every account once and picks the one with the most room, leaving
   out the account the job used and any account at its limit;
3. links the session's rollout file into that account's `sessions`
   directory (a symlink, never a copy);
4. runs `codex resume <id> "Continue the task you were working on."` with
   `CODEX_HOME` set to that account.

It writes one line to stderr and the rotate log:

```
codex resume session=0199a213-81c0-7800-8aa1-bbab2a035a53 personal->work reason=room args=1
```

If no account has room, it sleeps once until the earliest reset that the
live reading reported (at most 7 days ahead), reads again and resumes on
whichever account has room then, which may be the original one
(`reason=after-reset`). It does not poll while it waits. If no reading gave
a reset time, it exits `4` at once with `reason=no-reset-known`. If there is
still no room after the reset, it also exits `4`:

```
codex resume none reason=no-room-after-reset session=0199a213-81c0-7800-8aa1-bbab2a035a53
```

A session is resumed at most 3 times in 24 hours; the fourth call exits `3`
with `reason=capped`. This is the same cap as Claude's limit-resume hook.

With a session id you already know, skip the events file:

```bash
amplihack profile codex-resume --session 0199a213-81c0-7800-8aa1-bbab2a035a53 --from-home ~/.codex-profiles/personal
```

`OPENAI_API_KEY` and `CODEX_API_KEY` are removed from the resumed session's
environment, so the chosen account is the one billed. The exit codes are in
[Codex resume exit status](../reference/profile-command.md#codex-resume-exit-status).

## Limits

- A `codex` that starts in the microseconds between the in-use check and the
  link swap is not covered.
- The in-use check reads `/proc`, so it works only on Linux. Where it cannot
  run (macOS, other Unix, or Linux with `hidepid`), every swap is refused
  with `proc-unreadable`. This fails closed on purpose.
- A resume on another account needs Codex to follow the symlinked rollout. If
  your Codex version does not, the resume fails in Codex, and only waiting
  for the same account's reset works.
- The usage-limit wording Codex prints may change between versions. The
  rotator also matches the `rate_limit_reached_type` field and the
  `usage_limit_reached` code.
- amplihack never opens `auth.json` and never moves or copies a login. The
  rotator opens no file in an account directory. `codex-resume` reads only
  the events file you name, and writes only one rollout symlink under the
  chosen account's `sessions/`, never over an existing entry; see
  [What the Codex rotation does not do](../reference/profile-command.md#what-the-codex-rotation-does-not-do).
  Codex itself may refresh an account's tokens while it reads it, as any
  `codex` run does.
