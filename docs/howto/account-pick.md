---
title: How to Spread Launches Across Accounts
description: Let amplihack choose the Claude or Codex account with the most headroom for each launch, pin one when needed, or turn picking off.
doc_type: howto
review_schedule: as-needed
---

# How to Spread Launches Across Accounts

When you have several Codex accounts or Claude login profiles, amplihack
chooses one for each launch: the account with the most live headroom after
subtracting the expected use of jobs already running on it. Launches started
together spread across accounts instead of all choosing the same one. The
exact rules, output formats and files are in the
[`account pick` reference](../reference/account-pick.md).

## Contents

- [Before you start](#before-you-start)
- [See which account would be picked](#see-which-account-would-be-picked)
- [Launch with a picked account](#launch-with-a-picked-account)
- [Use a specific account](#use-a-specific-account)
- [Pick for Codex only](#pick-for-codex-only)
- [Turn picking off](#turn-picking-off)
- [Use the pick in a script](#use-the-pick-in-a-script)
- [Tune how far launches spread](#tune-how-far-launches-spread)
- [When a launch prints a pick warning](#when-a-launch-prints-a-pick-warning)

## Before you start

- **Codex:** each account is a signed-in directory under
  `~/.codex-profiles` (or `CODEX_PROFILES_ROOT`). `~/.codex` is included when
  it is signed in. Check them with `amplihack profile usage`.
- **Claude:** each account is a
  [login profile](claude-profiles.md#create-and-log-in). The profile whose
  login is live in `~/.claude` is not a candidate.
- Counting running jobs needs Linux. On other platforms only the
  [leases](../reference/account-pick.md#leases) of recent picks spread
  launches.

## See which account would be picked

```console
$ amplihack account pick --cli codex --list
  80.0  /home/ryan/.codex-profiles/b  (used 20%)
  35.0  /home/ryan/.codex-profiles/a  (used 50%, 1 job)
  -1.0  /home/ryan/.codex-profiles/c  (not signed in)
```

The first column is the headroom left after running jobs and recent picks.
`--list` reserves nothing, so it does not affect the next launch. Use
`--cli claude` for Claude login profiles.

## Launch with a picked account

Launch as usual, with no account set:

```bash
amplihack codex -- exec "fix the failing test"
amplihack claude
```

`amplihack codex` runs Codex with `CODEX_HOME` set to the picked account.
`amplihack claude` runs as the picked login profile, exactly as if you had
passed `--profile <name>`. Recipe agent steps launch through the same
commands, so each step is picked in the same way, and a step started inside a
picked launch keeps its parent's account.

## Use a specific account

Any explicit choice wins and is passed through unchanged:

```bash
CODEX_HOME=~/.codex-profiles/a amplihack codex -- exec "summarise the diff"
amplihack claude --profile work
amplihack profile switch work        # default for every launch until --none
CLAUDE_CONFIG_DIR=~/.claude amplihack claude
```

The full order is in [Pins](../reference/account-pick.md#pins).

## Pick for Codex only

A picked Claude profile runs with its own `CLAUDE_CONFIG_DIR`, so the
[login rotator](claude-profile-rotation.md) and the usage-limit resume do not
manage that session. If you rely on the rotator for Claude work, keep Claude
launches on `~/.claude` and pick only for Codex:

```bash
export AMPLIHACK_ACCOUNT_PICK=codex
```

## Turn picking off

```bash
export AMPLIHACK_ACCOUNT_PICK=off
```

Launches then behave as they did before the picker: Codex uses its default
home and Claude uses `~/.claude` unless a profile is set. The
`amplihack account pick` command itself still works.

## Use the pick in a script

```bash
home=$(amplihack account pick --cli codex) || exit 1
CODEX_HOME=$home codex exec "run the benchmarks"
```

The command exits 1 when no account is usable. For structured output:

```bash
amplihack account pick --cli codex --json | jq -r '.account, .effective_headroom'
```

`codex-pick` takes the same options and prints the same line:

```bash
CODEX_HOME=$(codex-pick --floor 1000) codex exec "run the benchmarks"
```

A standalone pick reserves its account for two minutes
(`AMPLIHACK_ACCOUNT_LEASE_TTL_SECS`), so the launch it feeds is briefly
counted twice. See
[The codex-pick wrapper](../reference/account-pick.md#the-codex-pick-wrapper).

## Tune how far launches spread

Each running job or recent pick lowers an account's score by
`AMPLIHACK_ACCOUNT_JOB_COST_PCT` points (default 15). Raise it to spread
launches more evenly; lower it to favour the fullest account more strongly.

```bash
export AMPLIHACK_ACCOUNT_JOB_COST_PCT=25
```

With three accounts at 80%, 50% and 20% headroom and the default cost, ten
launches started at once go 6, 3 and 1. See the
[worked example](../reference/account-pick.md#worked-example).

## When a launch prints a pick warning

```
warning: account pick failed: no usable codex account (none has headroom after running jobs and leases); using default home
```

The launch still runs, on the default home. Check the accounts with
`amplihack account pick --cli codex --list`. If every pick waits without
returning, a process holding the lease lock has hung; see
[Limits](../reference/account-pick.md#limits).
