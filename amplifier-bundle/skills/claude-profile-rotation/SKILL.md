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
- The user has an API-key (Claude Platform) profile, asks about per-token
  billing, or sees `api-key (billed per token)` or a launch refused with
  `subscription profile with no login of its own`.
- The user mentions rotating, swapping or balancing Claude logins, or the
  `claude-rotator` tmux session.
- A session stopped with "You've hit your ... limit" and did not resume, or
  the user asks about the tmux limit nudger.

## Rules

- Never open, read, print, copy, hash or log any `.credentials.json` or token.
  To inspect logins, use `ls -li` only. To see a profile's kind, use
  `amplihack profile kind list`, never `cat`. Never read
  `~/.config/anthropic`, `apiKeyHelper` or any API key value.
- Never copy a login file. Move it with `mv -n` if a person must move one.
- Never open `~/.config/anthropic/` or print any API key. Report only that a
  key is configured.
- Always run a dry run before the first real rotation, and after any threshold
  change.
- Do not stop the user's `claude-rotator` tmux session without asking them.
- **tmux:** run `kill-server` and `kill-session -a` only with `-S <private socket>` or `-L <name>`, for a server you started; inside a tmux pane `$TMUX` overrides `TMUX_TMPDIR`, so without them the command reaches the user's own server, and a PreToolUse hook denies it.

## Prerequisites

Check each one and tell the user what is missing:

1. Linux. On macOS logins are in the Keychain and cannot be rotated.
2. At least two profiles, each logged in:
   ```bash
   amplihack profile list
   amplihack profile usage
   ```
   Every profile to be used should show state `ok`. Log one in with
   `amplihack profile login <name>`. `profile usage` draws each profile's
   week and session as bars, marks the login in `~/.claude` with `(live)`,
   and sorts rows by the rotator's headroom (most room in the worse of
   session and week, then name), as its `order:` line says. Overflow and
   excluded profiles are sorted the same way, so the first row is not
   always the rotator's next pick.
   Reset times are local (`MM-DD HH:MM`, zone on the `times:` line); add
   `--utc` to compare them with the rotate log, which is in UTC. `--table`
   gives the older text table, and `--json` is for scripts. Bars are
   coloured only in a terminal; `NO_COLOR=1` turns colour off.
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
| `--target-week-max` | 95 | Never swap to a login whose weekly use is above this %. The test uses the figure as read, so a login at 95.0 is allowed and one at 95.4 is above 95 and is not. |
| `--margin` | 5 | While the live login still has room, a swap needs a login with this many more points of room (at least 1). |
| `--upgrade-margin` | 30 | Swap early when a profile has this many more points of headroom. |
| `--hold` | 3600 | Seconds after the last swap (a rotator swap or a `profile switch`) during which no early swap happens, and no swap onto a login at or above `--session-at` while the live login has room. |
| `--overflow-wait` | 1800 | Use an overflow profile only when the next plan reset is further away than this many seconds. |
| `--interval` | 600 | Seconds between checks (`start` and `watch` only). |

Flags go after `start`:

```bash
amplihack profile rotate start --session-at 75 --interval 300
```

To change settings of a running rotator, run `stop` then `start` again. To
keep a profile out of rotation: `amplihack profile auto-switch exclude <name>`.

## API-key profiles

An API-key profile runs Claude Code on the Claude Platform API key and is
billed per token. It is detected from key names and file presence only:

- its `.credentials.json` is `{}` or another object with no OAuth login; or
- it has no `.credentials.json` and names `ANTHROPIC_API_KEY`,
  `apiKeyHelper` or `primaryApiKey` in its settings.

A profile with no `.credentials.json` and no such setting is a subscription
profile with no login, not an API-key profile. The key itself is stored
outside the profile directory and shared by every profile in the home
directory.

- Check kinds with `amplihack profile kind list`; fix a wrong detection with
  `amplihack profile kind set <name> <subscription|api-key|auto>`.
- It is used only after the user marks it with
  `amplihack profile overflow add <name>`; the mark is consent to per-token
  billing. Do not mark it without asking. The rotator uses it last (tier 10):
  plan logins with capacity, then plan logins below 100% with their week at
  or below `--target-week-max` (`plan-room`), then
  credit overflow, then the API key, and only when every plan login was
  measured, none is below 100% on both windows, the earliest plan reset is further away than
  `--overflow-wait` and its login check passes. It moves `~/.claude` off it
  on the first cycle after a plan login has capacity or is a plan-room
  target, ignoring `--hold`. Auto-switch uses it only for a
  new nested launch or a usage-limit rerun; that session's next nested launch
  or rerun goes back to a plan profile.
- `rotate status` prints
  `~/.claude is on API key profile <name>: work is billed per token (since <ts>)`
  and `profile usage` prints
  `live: work is running on the API key (billed per token)` while work is on
  it. Tell the user when you see either line.
- `profile switch <name>` onto it pins rotation there and keeps billing until
  `amplihack profile switch --none`. Do not switch onto it without asking.
- An out-of-funds, revoked or rate-limited key blocks every API-key profile
  with one ALERT. After the user has funded or replaced the key, clear it
  with `amplihack profile overflow clear --platform`.
- A subscription profile with no OAuth login of its own is refused by `run`,
  `switch` and `--profile` launches with
  `error: profile <name> is a subscription profile with no login of its own; ...`,
  because it would fall back to the API key. Fix it with
  `amplihack profile login <name>`.

See `docs/reference/profile-command.md#api-key-profiles`.

## Reading the log

The log is `<root>/.auto-switch.d/rotate/rotate.log` (root defaults to
`~/.claude-profiles`):

```bash
grep -E ' (swap|ALERT:) ' ~/.claude-profiles/.auto-switch.d/rotate/rotate.log | tail
```

| ALERT | What to tell the user |
|-------|-----------------------|
| `no profile has capacity` | Every other plan login is known to be full or blocked, by a fresh reading or a last good one. Wait for a reset (the line gives the earliest block end as `blocked until <time>`) or add a profile. |
| `no capacity known` | The rotator could not show that no login has room. The line counts why, for example `(candidates: 6 unknown (HTTP 429), 3 failed)`: `unknown` logins were not read (rate-limited, transport, identity unavailable, timeout, still running, probe error), `failed` logins have an expired or refused token, and `above week ceiling` logins are over `--target-week-max`. The probes failed, not the accounts, when they are unknown: run `amplihack profile usage`. Never tell the user no account has capacity from this line. |
| `stay: live room <R>, ...` (not an ALERT) | The live login has room and no other login has `--margin` more. Nothing is wrong. `only a login in another organisation has room` means a login with room exists elsewhere, and the rotator moves to it once the live login has no room. `live room unknown` means the live probe failed: run `amplihack profile usage` for the live login. A `held after last swap` note means `--hold` is keeping a login at or above `--session-at` from taking over for now. |
| `reject: <name> <reason>` (not an ALERT) | A target's fresh reading showed no room, so the rotator tried the next one. |
| `no login in ~/.claude` | Log in with plain `claude` first. |
| `the account in ~/.claude is not the one the ledger names` | Someone ran `/login` in `~/.claude`. Log the named profile in again with `amplihack profile login <name>`, then delete `<root>/.auto-switch.d/rotate/ledger.json`. |
| `rotate recovery is inconsistent` | Follow manual recovery below. |
| `auto-switch recovery is inconsistent` | Run `amplihack profile auto-switch repair`. |

## Overflow onto usage credits

An account with usage credits turned on keeps working past its plan limit,
and the extra use is billed to the credits. Marked as an *overflow profile*,
the rotator moves work onto it only when no measured plan login, the live one
included, is below 100% on both windows, and the next plan reset is known
and more than `--overflow-wait` seconds away (default 1800). Until then the
plan-room swap (`reason=plan-room`) moves `~/.claude` to the plan-room target
with the most room it can use now. A target is a plan login below 100% on both
windows with its week at or below `--target-week-max` (95%), no recorded
block and a valid token; a login above the ceiling is never a target, however
low its session reads. Targets are ranked by session use below `--session-at`
first, then the most room, then the lowest weekly use, then name. While the
live login has room, a target needs `--margin` more room than it, and a
target at or above `--session-at` is also held for `--hold` seconds after the
rotator's last swap. When the live login has no room, any target qualifies.
With no target the rotator stays and writes the ALERT or a `stay:` line, and
credits stay unused while any plan login is below 100%. It moves work back in
the first cycle in which a plan login has capacity or is a plan-room
target. Before a swap, the target is confirmed by a fresh reading, and a
probe that was rate-limited (HTTP 429) is unknown, never exhausted. Auto-switch follows the
same rule through the same gate, whatever its own threshold: it uses an
overflow profile only when no plan profile is below 100% on both windows and
the next plan reset is beyond the wait. Earlier releases used credits once
every plan login reached `--session-at` or `--week-at`; tell users who
notice credits starting later that this is intended.
The overflow profile's own plan reading does not count: a credit account at
100% of its plan limits is still used, and only a spend block moves work
to the next overflow profile.

An unknown plan reset (no reading reports one) does **not** move work onto
overflow unless the user opts in with `--overflow-unknown-reset 1`
(`AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET=1`); with it, an unknown reset
spends money at once. Overflow marks have a tier (`overflow add <name>
--tier N`, 1-9, default 1): lower tiers are tried first. A marked API-key
profile is always tier 10 and `--tier` is refused for it (see below). Every
move onto a credit or API-key profile first logs
`per-token <name>: no plan login below 100%; <gate>; marked overflow tier <N> via profile overflow add; <check>`.

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
usage` marks the profile `overflow` and prints `running on overflow:
<name>` after its rows only when its login is in `~/.claude` and its probe shows the session
or weekly limit reached. Log lines: `reason=overflow-in`,
`reason=overflow-out`, `overflow: on|staying on|off ...`.

An excluded profile is never used, even when marked. If the user moves an
overflow login into `~/.claude` by hand while a plan login has capacity, the
next cycle moves it out; to stay on it, remove the mark or stop the rotator.

Recipe steps waiting on a usage limit end their wait on a login change or a
rotator cycle only when the ledger's last cycle has capacity and says the live
login has room (`last_cycle.live_room: true`). A swap onto a login at 98% of
its session wakes no step, and neither does `rotate stop`, which records no
cycle. A `profile switch` still wakes a step: it records `switched` with no
`live_room`, which counts as room unless the record says `false`. The waits and
hosts the rule does not apply to are listed under "What is not gated" in
`docs/reference/profile-command.md`. After upgrading, restart the rotator
(`rotate stop`, then `rotate start`): an older rotator never writes the field,
and until it is restarted the waiting steps sleep to their reset, or to a
`profile switch`.

A step that wakes and meets a limit on its first attempt waits again, and the
recipe log says so once (`reentered met=usage_limit` or `met=rate_limit`). When
the limit message states a reset the clock has already passed, which a retry at
08:32 sees in "resets 8:30am", the second wait is a capacity wait: it ends when
a login with room is live, not at the reset, and its `start` line shows
`wait=capacity` with the past time in `reset=`. It does not depend on a rotator
or on a second login: with neither, nothing ends it early, so it runs to the
step's remaining `max_wait_secs` budget (default 21600 seconds, shared by all
its waits) and the step then retries once. A limit on that retry is refused
because the budget is spent, so the budget ends the chain and no retry count
applies. If `AMPLIHACK_STEP_TIMEOUT` is reached first, the wait ends with
`step_timeout` and the step does not retry. A spend limit is never waited out
this way, even when its message states a session reset. A host with the rotator
running and a second stored login ends the wait sooner, because a login with
room ends it. If steps are sitting out the whole budget, start the rotator and
check `amplihack profile list` for a second login before raising the budget. See
"A reset that has already passed" in `docs/reference/profile-command.md`.

While on credits the rotator reports capacity, so recipe steps in the
usage-limit wait rerun and spend credits, provided that cycle also records room
for the credit login by the same rule. The limit-resume hook does not resume
interactive sessions onto a login whose plan windows read 100%.

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

## API key as the last resort

An API-key profile runs on the Claude Platform API key that Claude Code
stores outside the profile directory, **billed per token**. Its kind is
detected: its `.credentials.json` holds `{}` or no `claudeAiOauth` key. It
is used only once marked with `amplihack profile overflow add <name>`;
`overflow list` shows `<name>  api-key tier  not marked (not used)` until
then. Hosts that relied on the old implicit tier must run that command;
nothing is marked automatically. An unmarked API-key profile left live in
`~/.claude` by an older release shows as `platform_not_marked` in
`rotate status`; the rotator moves off it once a plan login has capacity or
is a plan-room target and never moves back. Check with
`amplihack profile kind list`; fix a wrong guess with
`amplihack profile kind set <name> api-key|subscription|auto` (writes the
`kind=` line in `<root>/.auto-switch`; upgrade every binary and restart the
rotator first, because older binaries read that line as corrupt).

Order: plan logins with capacity, then plan logins past a threshold but
below 100% with their week at or below `--target-week-max` (`reason=plan-room`), then
marked credit overflow tiers 1-9, then
the marked API-key tier (10). The rotator moves onto the API key only under
the same `--overflow-wait` gate, only when no credit profile can be used and
no plan login is below 100% on both windows, and only after a login check
finds a non-empty `primaryApiKey` (`~/.claude.json`), `env.ANTHROPIC_API_KEY`
or `apiKeyHelper` (`~/.claude/settings.json`), where plain `claude` reads after
the move. The swap moves only `.credentials.json`, so a key in the profile's
own files does not pass, and nor does a `.credentials.json` alone. The check never runs the helper or makes a request, so
it confirms a key is configured, not that it works. A failed check moves
nothing, leaves `~/.claude` unchanged and writes
`key_check_refused <name>: <reason>` and, once,
`ALERT: API key profile <name> failed its login check (<reason>); not moving onto it`;
`--dry-run` prints `would_refuse <name>: <reason>`.
While on it, every cycle, with no hold: plan capacity -> move to plan; else
a plan login below 100% -> move to it; else a usable credit profile -> move
to credits; else stay. Auto-switch follows the
same rule. All API-key profiles share one key, so only the first by name is
used. They are never probed with `/usage`.

What to tell the user:

- `rotate status` shows `~/.claude is on API key profile <name>: work is
  billed per token (since <ts>)` while it is live; `profile usage` shows
  `api-key (billed per token)` on its row and `live: work is running on the API key
  (billed per token)`.
- Log lines: `per-token <name>: ...; marked overflow tier 10 via profile
  overflow add; login check passed (<detail>)`, then the swap line, then
  `moved ~/.claude onto API key profile <name> (billed per token)`; and
  `moved ~/.claude off API key profile <name> to <target>`.
- `profile run`, `launch --profile` and `profile switch` refuse a
  subscription profile with no login of its own, because it would fall back
  to the API key. Fix: `amplihack profile login <name>`.

| ALERT | What to tell the user |
|-------|-----------------------|
| `the API key is blocked (<reason>) until <time>; API-key profiles are not used` | A supervised agent step failed with a key error: `key_exhausted` (out of funds), `key_revoked` (invalid key) or `key_rate_limited`. The first two last until cleared (stored as 62 days). After fixing the key: `amplihack profile overflow clear --platform`. |
| `swap refused: <name> has no oauth login and is not an API-key profile` | The profile's login file has no OAuth login. Log it in again, or `kind set <name> api-key` if it is meant to bill per token. |
| `API key profile <name> failed its login check (<reason>); not moving onto it` | No key is where plain `claude` will read it after the move. Report it to the user. On a host that bills only plan logins, unmark the profile with `amplihack profile overflow remove <name>`. Otherwise the user can put the key in `primaryApiKey` (`~/.claude.json`) or `env.ANTHROPIC_API_KEY` or `apiKeyHelper` (`~/.claude/settings.json`), but it then applies to plan logins too; never ask for the key value itself. |
| `more than one API-key profile could be live in ~/.claude; no swap` | `~/.claude` holds a login without OAuth and the ledger does not say which API-key profile it belongs to. They all share one key, so the user needs only one: suggest `amplihack profile remove <extra> --yes` for the others, after asking. |

To stop using it: `amplihack profile auto-switch exclude <names, including
the API-key profile>`; to stop billing at once, disable the key in the
Claude Console. Full guide: "Use an API key as the last resort" in
`docs/howto/claude-profile-rotation.md`.

## Resuming sessions stopped by a limit

`amplihack install` registers the `amplihack-hooks limit-resume` hook on
Claude Code's `StopFailure` event (matcher `rate_limit`, `asyncRewake: true`).
When a turn ends on a usage limit and auto-switch is on, it waits up to 7
days for the rotator to swap `~/.claude` to a login with room. It does not
poll: it watches `<root>/.auto-switch.d/rotate/swap.json`, which the rotator
writes only after a successful swap. A wake requires a
[qualifying swap record](https://rysweet.github.io/amplihack-rs/howto/claude-profile-rotation/#the-swap-record)
with a reading below 100% on both windows. Stays, ALERTs and `profile switch`
leave the hook waiting. The recipe-step wait uses `last_cycle.live_room`.

For a session in tmux, the hook answers the usage-limit menu itself. It runs
in the session's own process tree, so once the rotator reports a login with
room it answers its own session's pane (`$TMUX_PANE`) and no other. It walks
up its own ancestry to the nearest process whose standard input is a
terminal, and uses the pane only if tmux reports that terminal as the pane's
`#{pane_tty}`, that process holds the terminal's foreground process group,
and Claude Code is reading it. It then reads that pane once, with `tmux
capture-pane -p`, and types nothing unless the visible screen holds one of
six fixed phrases: `what do you want to do?`, `usage limit`, `continuing
automatically`, `session limit`, `5-hour limit` or `weekly limit`. Only
then does it send Esc, Ctrl+U, `continue` and Enter as four separate `tmux
send-keys` runs. Esc is the key that dismisses the usage-limit menu and the
timed wait. If the swap carries an organisation-affinity warning, the hook
includes it in the continuation text. `resumed-keys` records key delivery;
confirm continuation in the session. The hook's own run against a live
usage limit is not confirmed; what is tested, against a stand-in `tmux`, is
that a session with room, a proved pane and a recognised screen receives
one `capture-pane` and exactly these four inputs for that pane and no
other, and that a session with no room, no proved pane or an unrecognised
screen receives none.

The proof shows that the keys go to the session's own terminal; the screen
check shows that the terminal is on a prompt they answer. Neither is a
parser: the screen check is a substring match, so a screen that merely
mentions a usage limit still gets the keys. A session that had already
recovered can receive one extra `continue` turn, and Ctrl+U clears any
draft in its input box.

When the pane cannot be proved, the screen cannot be recognised, or the
session is not in tmux, the hook sends nothing to any terminal and exits
`2` with a continue message. The outcome says why: `resumed` with
`detail=no-tmux-pane` (the session is not in tmux, so there is no pane to
check and nothing new is done), `pane-unverified` (a pane was named but not
proved to be the session's terminal; `detail=` names the check),
`pane-not-reading` (it is the terminal, but Claude Code is not reading it)
or `screen-unrecognised` (the pane is the terminal, but its screen matched
no phrase, `detail=screen-unknown`, or could not be read,
`detail=screen-unreadable`). The wake cleared the menu in a Claude Code
2.1.307 check, but standing sessions have stayed on the usage-limit menu
after a wake and a swap, so these four outcomes can leave a session
stopped.

For a session left stopped, and when the hook logs `resume-keys-failed`,
direct the owner to the
[manual recovery steps](https://rysweet.github.io/amplihack-rs/howto/claude-profile-rotation/#continue-a-session-yourself).
The hook types only into its own session's pane; recovery input for any
other session, including `tmux send-keys` to its pane, is entered by that
session's owner.

The keys are the ones a person used by hand in the sessions reported in
[#2085](https://github.com/rysweet/amplihack-rs/issues/2085) and
[#2695](https://github.com/rysweet/amplihack-rs/issues/2695), not the result
of the hook's own run on a live usage limit.

It never presses a digit or types `/usage-credits` or `/rate-limit-options`.
If a session does not resume, read `resume.log` first. The tmux limit
nudger (`limit-nudger.sh`) is no longer needed once this hook is installed.

Tell the user it needs:

- Claude Code 2.1.306 or later, and `amplihack install` run again after
  upgrading amplihack or Claude Code (install leaves the hook out for an
  older `claude`);
- auto-switch on and a rotator from this amplihack version running (a
  rotator started before the upgrade writes no swap record: restart it);
- Linux (elsewhere the hook ends as `watch-lost`);
- tmux, for a session that may be held on the usage-limit menu: the
  session must run in a pane, so that `$TMUX` and `$TMUX_PANE` are set.
  Without tmux the hook can only wake the session, which has not released
  the menu in the sessions reported in #2695.

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
| `resumed-keys` | tmux accepted all four inputs to the session's own pane. Check for a reply in the original conversation. |
| `resumed` | Logged with `detail=no-tmux-pane`. The session had no tmux pane to check, so the hook exited `2` with a continue message. A session held on the menu stays there. |
| `pane-unverified` | A pane was named but not proved to be the terminal of the hook's own Claude Code (`detail=` is `bad-pane-id`, `no-tty-ancestor`, `walk-limit`, `unreadable-proc`, `tty-mismatch` or `not-foreground`; `tty-mismatch` also covers `tmux` failing or printing nothing). Nothing was typed and the hook exited `2`. If the session is still on the menu, its owner should follow [Continue a session yourself](https://rysweet.github.io/amplihack-rs/howto/claude-profile-rotation/#continue-a-session-yourself). |
| `pane-not-reading` | The pane is the session's terminal, but Claude Code is not the process reading it. Nothing was typed and the hook exited `2`. Same follow-up as `pane-unverified`. |
| `screen-unrecognised` | The pane is the session's terminal, but what it was showing is not a screen the keys answer (`detail=screen-unknown`) or could not be read (`detail=screen-unreadable`). Nothing was typed and the hook exited `2`, naming the refusal in the wake message. Same follow-up as `pane-unverified`. |
| `resume-keys-failed` | A key failed or the session exited part way (`keys_sent=` says how many went). If the session is still open, its owner should follow [Continue a session yourself](https://rysweet.github.io/amplihack-rs/howto/claude-profile-rotation/#continue-a-session-yourself). |
| `swap-no-room` / `swap-stale` | A swap record was at 100% or swapped before the stop; the hook kept waiting. |
| `watch-lost` | The hook could not watch for swaps (not Linux, old kernel, unsafe or deleted directory; see `detail=`). It never resumes. |
| `no-capacity-timeout` | No swap to a login with room within 7 days. |
| `rotator-down` | The rotator was not running (or its ledger was unreadable) when the limit hit; start it. |
| `session-gone` | The session exited while the hook waited; nothing was done. |
| `moved-on` | The user sent a prompt in that session during the wait (for example `continue`), so the hook did not wake it. |
| `disabled` | Auto-switch was off when a swap would have resumed the session. |
| `duplicate` | A wake attempt was already recorded for that limit event (same session, text and login), or another hook recorded an attempt for the session during the wait. |
| `cap` | The session reached its limit of 3 recorded wake attempts in 24 hours. Another attempt can be made once an older one ages out. |
| `stop-hook-active` | Claude Code reported a stop hook already running; nothing was done. |
| `guard-failed` | The resume state under `<root>/.auto-switch.d/resume/` could not be used safely. Check its owner and mode (`0700`). |

The hook requires a qualifying swap record and records a wake attempt before
delivering it, so a partial send counts toward the limit. It records at most
one attempt per limit event, one per wait and 3 per session in 24 hours. Each
`resume.log` line also has `cycle=`, the rotator's last cycle outcome, and,
when they apply, `swap_seq=`, `swap_live=`, `pane=` (a hash, not the pane
id), `keys_sent=` and `detail=`.

Claude Code's `autoContinueAtUsageLimit` setting controls whether it starts
a wait that can continue the conversation when the limit resets. The
documented default is `true`. Managed settings take precedence over
settings supplied when launching Claude Code, which take precedence over
user settings. When none of these sets the key, a project or local setting
for this key disables the automatic wait. See
[Check automatic continuation at reset](https://rysweet.github.io/amplihack-rs/howto/claude-profile-rotation/#check-automatic-continuation-at-reset).

Inspection of the Claude Code 2.1.325 binary suggested that billing type
and a server-side flag affect whether it starts a reset wait. The
inspection also suggested that a reset continuation could request another
turn after a hook wake. These are inferences from the binary and remain
unverified in live sessions. See the
[usage-limit investigation](https://github.com/rysweet/amplihack-rs/issues/2695#issuecomment-6887891038).

The tmux limit nudger (`~/.claude-profiles/.backups/limit-nudger.sh`) is no
longer needed once the hook is registered. Ask the user before stopping it,
and stop it by pid, not by pattern.

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
