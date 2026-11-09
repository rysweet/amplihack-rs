---
title: "Reference: amplihack account pick"
description: Command contract, ranking rule, job counting, leases and launch integration of the account picker.
doc_type: reference
review_schedule: as-needed
---

# Reference: `amplihack account pick`

`amplihack account pick` chooses the Claude or Codex account with the most
live headroom left once the expected use of the jobs already running on each
account is subtracted. `amplihack codex`, `amplihack claude` and recipe agent
steps call the same picker when no account is pinned. For the common tasks,
see [How to spread launches across accounts](../howto/account-pick.md).

## Contents

- [Synopsis](#synopsis)
- [Output](#output)
- [Exit codes and messages](#exit-codes-and-messages)
- [Candidates](#candidates)
- [Live headroom](#live-headroom)
- [Ranking](#ranking)
- [Running jobs](#running-jobs)
- [Leases](#leases)
- [Launch integration](#launch-integration)
- [Pins](#pins)
- [The off switch and the rotator](#the-off-switch-and-the-rotator)
- [The codex-pick wrapper](#the-codex-pick-wrapper)
- [Environment variables](#environment-variables)
- [Security notes](#security-notes)
- [Limits](#limits)

## Synopsis

```
amplihack account pick --cli <claude|codex> [--json | --list] [--floor N]
```

| Option | Meaning |
|--------|---------|
| `--cli claude\|codex` | Required. Which kind of account to pick. |
| `--json` | Print one JSON object instead of the home path. Cannot be combined with `--list`. |
| `--list` | Print every candidate, ranked, one per line. Writes no lease. Cannot be combined with `--json`. |
| `--floor N` | Codex only. Credit balance above which an account whose windows are full still counts as usable (headroom 0.5). A finite number from `0` to `1000000000`; default `3000`. With `--cli claude` it is ignored and a warning is printed on stderr. |

A plain pick and `--json` write a [lease](#leases) for the chosen account.
`--list` writes none. The standalone command ignores
[`AMPLIHACK_ACCOUNT_PICK`](#the-off-switch-and-the-rotator); the switch applies
only to launches.

## Output

### Plain

One line containing the absolute home directory of the chosen account:

```console
$ amplihack account pick --cli codex
/home/ryan/.codex-profiles/b
```

For Codex this is a `CODEX_HOME` value. For Claude it is the login profile's
configuration directory, exactly the `CLAUDE_CONFIG_DIR` value that
`amplihack claude --profile <name>` sets for that profile. The two must be
equal, or the picker would not count that launch as a running job and its
lease would last until the TTL.

### `--json`

One object on one line. The key set is fixed; every key is always present.
The example below is wrapped over several lines for reading; the real output
has no line breaks.

```json
{"cli":"codex","account":"b","home":"/home/ryan/.codex-profiles/b",
 "live_headroom":80.0,"running_jobs":0,"active_leases":0,
 "effective_headroom":80.0,"job_cost_pct":15.0,"read_at":1794233458,
 "error":null,
 "candidates":[
  {"account":"b","home":"/home/ryan/.codex-profiles/b","live_headroom":80.0,
   "running_jobs":0,"active_leases":0,"effective_headroom":80.0,
   "usable":true,"note":"used 20%"},
  {"account":"a","home":"/home/ryan/.codex-profiles/a","live_headroom":50.0,
   "running_jobs":1,"active_leases":0,"effective_headroom":35.0,
   "usable":true,"note":"used 50%"},
  {"account":"c","home":"/home/ryan/.codex-profiles/c","live_headroom":null,
   "running_jobs":0,"active_leases":0,"effective_headroom":null,
   "usable":false,"note":"not signed in"}]}
```

| Key | Type | Meaning |
|-----|------|---------|
| `cli` | string | `claude` or `codex`. |
| `account` | string or null | Codex: the label `amplihack profile usage` shows (the profile directory name, or `~/.codex`). Claude: the login profile name. Null when nothing was chosen. |
| `home` | string or null | Absolute, canonical home of the chosen account. |
| `live_headroom` | number or null | [Live headroom](#live-headroom) of the chosen account. |
| `running_jobs` | integer or null | [Running jobs](#running-jobs) counted on it. |
| `active_leases` | integer or null | Unexpired [leases](#leases) on it, not counting the one this pick wrote. |
| `effective_headroom` | number or null | `live_headroom - (running_jobs + active_leases) * job_cost_pct`. |
| `job_cost_pct` | number | The cost per job in use. |
| `read_at` | integer | Unix seconds at which the live readings were taken. Each account is read separately; this is the earliest of those times. |
| `error` | string or null | Null on success. On failure, the same reason the plain form prints after `amplihack account pick: `. |
| `candidates` | array | Every candidate, ranked as in [Ranking](#ranking). Unusable candidates come last, in path order. |

Each candidate has `account`, `home`, `live_headroom`, `running_jobs`,
`active_leases`, `effective_headroom`, `usable` (boolean) and `note`.
`live_headroom` and `effective_headroom` are null when the account gave no
reading. On failure the top-level account fields are null, `error` is set
and the exit status is 1. When no candidate is usable, `candidates` still
lists every candidate. When the lease directory could not be used,
`candidates` is empty.

### `--list`

One line per candidate, best first, in the format `codex-pick --list` used:

```console
$ amplihack account pick --cli codex --list
  80.0  /home/ryan/.codex-profiles/b  (used 20%)
  35.0  /home/ryan/.codex-profiles/a  (used 50%, 1 job)
  -1.0  /home/ryan/.codex-profiles/c  (not signed in)
```

The number is the effective headroom with one decimal place, right-aligned to
six characters. An unusable candidate shows `-1.0`. `--list` exits 0 even when
no candidate is usable.

### Notes

A note holds derived values only: the used percentage, the credit balance,
`1 job` or `N jobs`, `N leases`, or one of these fixed reasons:
`not signed in`, `timed out`, `error: <kind>`, `unsafe path`. Control
characters are removed and a note is at most 200 characters. Raw app-server
or probe output never appears.

## Exit codes and messages

| Status | When | stderr |
|--------|------|--------|
| `0` | An account was chosen, or `--list` ran. | Warnings only. |
| `1` | No candidate is usable, or the lease directory could not be used. | `amplihack account pick: no usable <cli> account (<reason>)`, or `amplihack account pick: <operation> <path>: <error>` |
| `2` | Usage error: missing `--cli`, `--json` with `--list`, or an invalid `--floor`. | The argument parser's message. |

`<reason>` is one of `no accounts found`, `none signed in or readable`, or
`none has headroom after running jobs and leases`.

When the command runs as the [`codex-pick` wrapper](#the-codex-pick-wrapper),
the no-account line keeps the original wording:

```
codex-pick: no signed-in profile has headroom above the credit floor
```

## Candidates

### Codex

The subdirectories of `CODEX_PROFILES_ROOT` (default `~/.codex-profiles`),
plus `~/.codex` when it is signed in. This is the list
[`amplihack profile usage`](profile-command.md#codex-accounts) reads.

### Claude

The login profiles of Claude kind in the
[profile root](profile-command.md#profile-root), except the profile whose
login is currently live in `~/.claude`. `~/.claude` itself is never a
candidate; it stays the fallback when the picker chooses nothing.

### Unsafe homes

A home whose path contains a byte below `0x20` is marked unusable with the
note `unsafe path` and is never printed raw.

## Live headroom

Every pick reads every candidate live, in parallel, before it takes the lock.
Readings are not cached between picks.

| CLI | Live headroom | Unusable when |
|-----|---------------|---------------|
| Codex | 100 minus the highest used percentage across the account's windows. When no window has room, `0.5` if the account has credits with a balance above `--floor`, otherwise unusable. This is the rule of [`profile usage`](profile-command.md#codex-accounts). | Not signed in, timed out, read error, or no room and no credits above the floor. |
| Claude | 100 minus the higher of the five-hour and seven-day used percentages, from the same read [`amplihack profile usage`](profile-command.md#probe-method) uses. | The read failed, the profile is not signed in, or it is not [usable now](profile-command.md#usable-now). |

Each Codex read is bounded by `AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS` and each
Claude read by `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS`. The Claude read follows
`AMPLIHACK_PROFILE_USAGE_PROBE`.

## Ranking

```
effective = live - (running_jobs + active_leases) * job_cost_pct
```

A candidate qualifies when it is usable live and its effective headroom is
above 0. Among qualifying candidates the pick is, in order:

1. the highest effective headroom;
2. then the highest live headroom;
3. then the fewest running jobs plus leases;
4. then the greatest home path, compared as bytes.

The rule is deterministic, so the same readings, jobs and leases always give
the same choice.

### Worked example

Three Codex accounts, A at 80, B at 50 and C at 20, no running jobs, cost 15,
and ten picks started at once. Each pick sees the leases of the picks before
it:

| Pick | A | B | C | Chosen |
|------|---|---|---|--------|
| 1 | 80 | 50 | 20 | A |
| 2 | 65 | 50 | 20 | A |
| 3 | 50 | 50 | 20 | A (tie on effective; A has more live headroom) |
| 4 | 35 | 50 | 20 | B |
| 5 | 35 | 35 | 20 | A |
| 6 | 20 | 35 | 20 | B |
| 7 | 20 | 20 | 20 | A |
| 8 | 5 | 20 | 20 | B |
| 9 | 5 | 5 | 20 | C |
| 10 | 5 | 5 | 5 | A |

A gets 6 picks, B 3 and C 1. The split depends only on the number of earlier
leases, not on the order in which the ten processes reach the lock.

## Running jobs

The picker counts **jobs, not processes**. On Linux it walks `/proc` and, for
each process owned by the same real user, reads the exec-time environment in
`/proc/<pid>/environ`. A process matches an account when the key below is set
to that account's home, compared after both paths are canonicalised (so a
symlinked home matches its target).

| CLI | Key |
|-----|-----|
| Codex | `CODEX_HOME` |
| Claude | `CLAUDE_CONFIG_DIR` |

A matching process counts as a job only when its parent does not match the
same home, so Codex or Claude and all their children count once. The picker
skips its own process, zombies, other users' processes, unreadable entries,
and values that are empty, relative or longer than `PATH_MAX`.

A job started without the key set (for example plain `codex`, which uses
`~/.codex` by default) is not counted. On platforms other than Linux the count
is always 0.

## Leases

A lease records a pick that has not yet become a running job. Without leases,
ten launches started in the same second would all read the same figures and
choose the same account.

### Files

```
$AMPLIHACK_ACCOUNT_LEASE_DIR/          default ~/.amplihack/state/account-leases
  codex/
    .lock
    48211-1794233458123456789-0.json
  claude/
    .lock
```

A lease file is named `<pid-or-0>-<nanoseconds>-<sequence>.json` and contains:

```json
{"version":1,"cli":"codex","home":"/home/ryan/.codex-profiles/b","pid":48211,"created_at":1794233458}
```

`pid` is null for a standalone pick. Files are written to a temporary name
with mode `0600`, synced and renamed into place, so a reader never sees a
partial lease.

### The lock

Each pick holds an exclusive `flock` on `<cli>/.lock` while it counts jobs,
reads and sweeps leases, ranks, and writes its own lease. The lock is taken
after the live reads, so no child process or network call runs while it is
held. A pick waits for the lock with no deadline and no retry.

### Whose pid is recorded

| Caller | `pid` | How the lease ends |
|--------|-------|--------------------|
| `amplihack codex` or `amplihack claude` | The `amplihack` process, which waits for its child | Deleted when the child exits, or by the sweep below. |
| `amplihack account pick` or `codex-pick` | null | Expires by TTL only. |

### Sweep

While holding the lock, each pick deletes a lease when any of these holds:

1. its pid has exited;
2. a counted job on the same home has the lease's pid as itself or one of its
   ancestors, so the launch the lease reserved is now counted as a job;
3. it is at least `AMPLIHACK_ACCOUNT_LEASE_TTL_SECS` old (default `120`).
   This applies to every lease, with or without a pid.

A pid of `0` or above `2147483647` is malformed. A pid the picker may not
signal (`EPERM`) counts as alive. A file with an unknown `version` or
unparseable content is left in place, not counted, and named once in a
warning. Only names matching `<digits>-<digits>-<digits>.json` are read.

A lease's `home` is only compared with candidate homes. It can lower a
candidate's score but is never used as the chosen home.

### Directory checks

The picker creates missing directories with mode `0700`. An existing lease
directory, `<cli>` directory or `.lock` must not be a symlink, must be owned
by the effective user, and must not be group- or world-writable. Otherwise the
pick fails with exit 1 and a launch falls back to its default home. The
picker never changes permissions or ownership.

## Launch integration

| Launch | Picks when | Effect of a pick |
|--------|------------|------------------|
| `amplihack codex` | No Codex [pin](#pins), not Docker, not `--auto`, switch is `on` or `codex` | The child gets `CODEX_HOME` and `AMPLIHACK_CODEX_HOME` set to the picked home. Bootstrap writes its Codex settings to that home's `config.toml`. |
| `amplihack claude`, `amplihack launch` | No Claude [pin](#pins), not Docker, not `--auto`, switch is `on` | The picked profile name is resolved exactly as `--profile <name>` would be, with the same ownership and kind checks, and pinned for nested launches. |
| Recipe agent steps | Each step runs `amplihack codex` or `amplihack claude`, so the rows above apply. A step inside a launch that already has a home inherits it and does not pick again. | |

The launcher keeps its lease until the child exits, then deletes it.

### Failure never blocks a launch

When a launch's pick fails for any reason (no usable account, unreadable
lease directory, failed reads), it prints

```
warning: account pick failed: <reason>; using default home
```

on stderr and continues as it would without the picker: Codex with its
default home, Claude with no profile and `~/.claude`.

### Docker and `--auto`

The picker is skipped. Profile resolution already refuses or skips login
profiles in these modes.

### A picked Codex home and amplihack's hooks

When a launch picks a Codex home, it writes `approval_policy` to that home's
`config.toml`, as bootstrap does for `~/.codex`, and touches nothing else
there. This happens in non-interactive launches too, because a picked home
was not set up for this launch by the user. A pinned home is left as it is. Hooks and plugins are whatever that home was set
up with; install them into each Codex profile you want picked.

## Pins

A pin bypasses the picker. The value passes through unchanged and no lease is
written.

| CLI | Pins, in order |
|-----|----------------|
| Codex | `CODEX_HOME`, then `AMPLIHACK_CODEX_HOME` (must be an absolute, non-empty path) |
| Claude | `--profile`, `AMPLIHACK_CLAUDE_PROFILE`, the default profile pointer (`amplihack profile switch <name>`), `CLAUDE_CONFIG_DIR`, `AMPLIHACK_CLAUDE_CONFIG_DIR` |

`amplihack codex` with only `AMPLIHACK_CODEX_HOME` set restores `CODEX_HOME`
from it in the child. A Claude launch with only `AMPLIHACK_CLAUDE_CONFIG_DIR`
set restores `CLAUDE_CONFIG_DIR` from it in the child.

### Pins across recipe steps

The recipe runner passes only `AMPLIHACK_*` variables to steps, so a raw
`CODEX_HOME` or `CLAUDE_CONFIG_DIR` would otherwise be lost and a nested step
would pick a different account. To keep the parent's account:

- `amplihack codex` sets `AMPLIHACK_CODEX_HOME` on its child alongside
  `CODEX_HOME`, whether the home was pinned or picked;
- `amplihack recipe run` sets `AMPLIHACK_CODEX_HOME` from `CODEX_HOME`, and
  `AMPLIHACK_CLAUDE_CONFIG_DIR` from `CLAUDE_CONFIG_DIR`, when the alias is
  not already set. The variables are set on the runner's process only;
  amplihack's own environment is not changed;
- a picked Claude profile is pinned for descendants as any selected profile
  is; see [Nested sessions keep the parent's profile](../howto/claude-profiles.md#nested-sessions-keep-the-parents-profile).

## The off switch and the rotator

`AMPLIHACK_ACCOUNT_PICK` controls launches only.

| Value | Codex launches | Claude launches |
|-------|----------------|-----------------|
| `on` (default) | Pick | Pick |
| `codex` | Pick | No pick: behaviour before the picker existed |
| `off` | No pick | No pick |

The value is matched without regard to case; empty counts as unset.

A picked Claude profile runs with `CLAUDE_CONFIG_DIR` set to that profile.
Such a session is not on `~/.claude`, so the
[login rotator](../howto/claude-profile-rotation.md) and the usage-limit
resume do not manage it. Hosts that rely on the rotator and launch Claude work
without `CLAUDE_CONFIG_DIR` should set `AMPLIHACK_ACCOUNT_PICK=codex`.

With the default `on`, every unpinned Claude launch reads each candidate
profile's usage before it starts, which runs `claude` once per candidate.
Anything that counts `claude` runs, such as a test fixture, sees those
reads; the usage-limit wait tests set `AMPLIHACK_ACCOUNT_PICK=off` for this
reason.

## The codex-pick wrapper

`scripts/codex-pick` is a POSIX `sh` script that runs

```sh
exec amplihack account pick --cli codex "$@"
```

with `AMPLIHACK_ACCOUNT_PICK_CALLER=codex-pick`, so `--floor` and `--list`
behave as before and the no-account message keeps its wording. It differs
from the earlier Python `codex-pick` in three ways:

- headroom comes from a live read, not the newest session rollout file;
- `--list` shows effective headroom, after running jobs and leases;
- a plain pick writes a lease with `pid: null`, which counts against the
  account for `AMPLIHACK_ACCOUNT_LEASE_TTL_SECS`.

Because a standalone lease has no pid, a launch started with
`CODEX_HOME=$(codex-pick)` is counted twice (lease and job) until the lease
expires.

The wrapper is transitional and stays until the factory calls
`amplihack account pick` directly. It does not replace a `codex-pick` already
installed in `~/.local/bin`; copy or link it there yourself.

## Environment variables

| Name | Default | Meaning |
|------|---------|---------|
| `AMPLIHACK_ACCOUNT_PICK` | `on` | `on`, `codex` or `off`; see [The off switch](#the-off-switch-and-the-rotator). |
| `AMPLIHACK_ACCOUNT_JOB_COST_PCT` | `15` | Headroom points subtracted per running job or lease. Finite, above 0, at most 100. |
| `AMPLIHACK_ACCOUNT_LEASE_TTL_SECS` | `120` | Age at which a lease expires, `1` to `86400`. |
| `AMPLIHACK_ACCOUNT_LEASE_DIR` | `~/.amplihack/state/account-leases` | Lease and lock directory. Must be an absolute path. |
| `AMPLIHACK_CODEX_HOME` | unset | Codex home carried to nested launches. Set by amplihack; see [Pins](#pins). |
| `AMPLIHACK_CLAUDE_CONFIG_DIR` | unset | Raw Claude configuration directory carried to nested launches. Set by `amplihack recipe run`. |
| `AMPLIHACK_ACCOUNT_PICK_CALLER` | unset | Set by `scripts/codex-pick`. Do not set by hand. |

An invalid value falls back to its default and prints one warning on stderr
with the value quoted. The picker also reads `CODEX_PROFILES_ROOT`,
`CODEX_BIN`, `AMPLIHACK_CODEX_USAGE_TIMEOUT_SECS` and the
`AMPLIHACK_PROFILE_USAGE_*` variables listed in the
[environment variable table](amplihack-env-vars.md).

## Security notes

- The picker's own code never opens, copies or logs `auth.json`,
  `.credentials.json` or keyring entries. Codex readings come from the Codex
  app server. For Claude, the candidate list reuses the profile store's
  existing checks, which `profile rotate` also uses: whether each profile has
  a login file, and which kind of login it holds (issue #1700). Claude
  readings come from the existing usage read.
- `/proc/<pid>/environ` is read only for processes owned by the same real
  user, with a size bound and without following symlinks. Only the value of
  `CODEX_HOME` or `CLAUDE_CONFIG_DIR` is kept; the rest is discarded at once
  and never printed.
- Leases and notes hold home paths, pids, timestamps, derived numbers and
  fixed reason words only.
- The chosen home reaches the child through the child's environment only; the
  `amplihack` process's own environment is not changed.

## Limits

- The lock wait has no deadline. A process of the same user that hangs while
  holding `.lock` blocks later picks of that CLI. Kill the holder, or set
  `AMPLIHACK_ACCOUNT_PICK=off` to launch without picking.
- A Claude pick reads every candidate profile before launch, which can take up
  to `AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS`. `AMPLIHACK_ACCOUNT_PICK=codex`
  avoids it.
- Job counting needs Linux `/proc`. Elsewhere only leases spread launches.

## Related

- [How to spread launches across accounts](../howto/account-pick.md)
- [`amplihack profile` reference](profile-command.md), including
  [Codex accounts](profile-command.md#codex-accounts)
- [How to rotate Claude logins automatically](../howto/claude-profile-rotation.md)
- [Environment variable table](amplihack-env-vars.md)
