---
title: Diagnose a Vanished Observed Task
type: howto
updated: 2026-11-16
related: issue #2716, issue #2637, issue #1267
---

# Diagnose a vanished observed task

Use this guide when a Linux recipe run prints

```
amplihack: warning: observed task 481392 was gone at PTRACE_CONT
```

or when an older install ends a run with

```
failed to wait for recipe-runner-rs: required process observation failed: No such process (os error 3)
```

The first line is a warning about a traced process that exited while the run
observer was holding it. It is normal in a tree of `claude`, `node`, `cargo`
and shell processes, and the run carries on. The second line is the same event
on an install from before issue #2716, where it ended the run instead.

For the full behaviour, see
[Vanished observed tasks in the Linux run observer](../reference/run-isolation-vanished-observed-tasks.md).

## Contents

- [Confirm the run was not harmed](#confirm-the-run-was-not-harmed)
- [Count the tasks that were lost](#count-the-tasks-that-were-lost)
- [Decide whether it needs attention](#decide-whether-it-needs-attention)
- [Recognise the failure from an older install](#recognise-the-failure-from-an-older-install)
- [What not to do](#what-not-to-do)

## Confirm the run was not harmed

The run's status comes from the runner's own exit, never from the warning.
Check the published record of the run:

```sh
RUN_DIR=~/.amplihack/work/runs/$RUN_ID   # AMPLIHACK_RUN_DIR, as the run printed it
jq -r '.outcome' "$RUN_DIR/run.json"
# success
jq -r '.coverage' "$RUN_DIR/git-recording.json"
# established
```

`established` says the observer traced the tree and its evidence is complete,
warning or not. `unavailable` means something else happened — the observer
never attached on this host. That is a separate case, covered by
[The Linux observer could not attach](../reference/recipe-runner-logging.md#the-linux-observer-could-not-attach).

The Git commands the run issued are where they always are:

```sh
wc -l "$RUN_DIR/git-commands.jsonl"
```

## Count the tasks that were lost

A run that kept its evidence has the count in its journal receipt. Publication
removes the trace folder on a run that succeeded, so this works on a run that
failed for some other reason, or on retained evidence:

```sh
jq -r '.vanished' "$RUN_DIR/git-trace2/observer-$RUN_ID.receipt"
# 2
```

The receipt's other fields are unchanged, so the same file still tells you the
coverage, the scope and the backend:

```sh
jq -r '{coverage, scope, backend, vanished}' "$RUN_DIR/git-trace2/observer-$RUN_ID.receipt"
```

At most 16 warnings are printed per run. The count in the receipt stays exact
beyond that, so use the count rather than counting lines in a log.

The warnings go to the run's stderr, so where they are kept depends on how the
run was started. A detached run has its own log, whose path `recipe run
--detach` prints — `<session>.log` in `~/.amplihack/recipe-logs` by default
(see [Detached runs](../reference/recipe-command.md#detached-runs)):

```sh
grep -c 'was gone at' ~/.amplihack/recipe-logs/"$SESSION".log   # at most 16, not the real total
```

A foreground run has no log of its own, so counting lines there means counting
them in whatever you redirected stderr to, or in your CI job's output. Either
way the number is capped at 16; `.vanished` in the receipt is not.

## Decide whether it needs attention

| What you see | What it means | What to do |
| --- | --- | --- |
| A few warnings, `outcome: success`, `coverage: established` | Ordinary process churn in the traced tree. | Nothing. |
| Warnings on nearly every run of one recipe | That recipe's steps create and destroy many short-lived processes. | Nothing, unless you are reducing noise in logs. |
| A run that stops making progress and prints no further steps | The observer is waiting for a task. Issue #2716 rules out timers, so there is no automatic recovery. | Capture the run's stderr and the process tree, then stop the run with `amplihack recipe stop <run id>` — the supported path, which signals only the pids in that run's own record. If the run has no record to stop by, stop the process id you started and recorded, and nothing else. Then file an issue referencing #2716. |
| `required process observation failed` with an error other than `No such process` | A different ptrace error, which still ends the run by design. | Report the message as it stands; the error number is the useful part. |
| `coverage: unavailable` | The observer never attached on this host. | See [The Linux observer could not attach](../reference/recipe-runner-logging.md#the-linux-observer-could-not-attach). |

## Recognise the failure from an older install

Before issue #2716, a lost task ended the run:

```
failed to wait for recipe-runner-rs: required process observation failed: No such process (os error 3)
```

Often with a second line, because the journal was never finished:

```
Git recording also failed: required observer did not finalize; partial evidence retained: No such file or directory (os error 2)
```

Those runs failed, and nothing retried them. Do not go looking for an
`indeterminate` classification to confirm it: the observer's wait error went
straight through the run's final failure record, which writes `outcome:
failure` in `run.json` and produces no classification at all. And when the
second line was present it actively misled, because it contains `no such file
or directory` — an environmental marker — so a tail carrying it classified the
run `environmental`, pointing at the host rather than at the traced tree.

Two things identify the same event on a current install: the run finishes, and
the warning names the task and the request instead.

To see which version produced a run:

```sh
amplihack --version
```

## What not to do

- **Do not add a wait, a retry or a timeout around a run to work around it.**
  There are no timers in this path, by design, and adding one outside it hides
  the stall case that is worth reporting.
- **Do not stop processes by name or pattern.** No `pkill`, `pkill -f`,
  `killall` or `kill $(pgrep ...)`. Stop only a process id you started and
  recorded, and wait for it. A pattern-based stop has taken out unrelated runs
  on a shared host.
- **Do not read the stderr warning as a sign that evidence is missing.**
  Missing evidence has its own messages, and they fail the run. A warning plus
  `coverage: established` means the record is complete.

## Related

- [Vanished observed tasks in the Linux run observer](../reference/run-isolation-vanished-observed-tasks.md)
- [Recipe run and auto-drive checkout isolation](../reference/recipe-checkout-isolation.md#required-recording)
- [Recipe runner logging](../reference/recipe-runner-logging.md#git-command-record)
