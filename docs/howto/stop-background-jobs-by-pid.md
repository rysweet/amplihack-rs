---
title: "Stop Background Jobs by PID"
description: "Stop a background job you started by its recorded PID or process group, never by a command-line pattern, and time-box test runs with the test runner's own options."
last_updated: 2026-11-13
review_schedule: quarterly
owner: rysweet
doc_type: howto
related:
  - ./wait-without-orphan-processes.md
  - ../reference/recipe-context-transport.md
  - ../concepts/prompt-text-out-of-argv.md
---

# Stop Background Jobs by PID

On 13 November an agent in a live auto-drive ran
`pkill -f "vitest.mjs"` to stop a hung test run. The pattern also matched the
recipe runner driving that agent, because the task text on the runner's command
line contained the word "vitest", and the drive ended with SIGTERM
([#2582](https://github.com/rysweet/amplihack-rs/issues/2582)). This guide
shows how to stop only the processes you started. It is written for recipe
authors and for anyone running agents.

## Contents

- [The guidance paragraph](#the-guidance-paragraph)
- [Stop a job by its recorded PID](#stop-a-job-by-its-recorded-pid)
- [Stop a job and its children by process group](#stop-a-job-and-its-children-by-process-group)
- [Time-box a test run](#time-box-a-test-run)
- [Commands not to use](#commands-not-to-use)
- [Where agents are told](#where-agents-are-told)

## The guidance paragraph

Agents receive this paragraph, worded identically wherever it appears:

> Stop a background job you started by the PID you recorded with `$!`, or by
> its process group with `kill -- -PGID`. Never use `pkill -f`,
> `pgrep -f PATTERN | xargs kill`, `kill $(pgrep -f PATTERN)` or `killall <name>`: a
> pattern matches every process whose command line contains the same words,
> including the recipe runner and other agents. Time-box test runs with the
> test runner's own options (for example nextest `slow-timeout` with
> `terminate-after`, or vitest `--testTimeout`) instead of killing the run
> afterwards.

## Stop a job by its recorded PID

Record `$!` as soon as the job starts, and use only that number:

```bash
npx vitest run src/ui > vitest.log 2>&1 &
vitest_pid=$!

# ... later, if it must be stopped:
kill "$vitest_pid"
wait "$vitest_pid"
echo "exit=$?"
# Output: exit=143
```

`wait` reaps the job, so no zombie is left. Exit status 143 is 128 plus
SIGTERM (15).

## Stop a job and its children by process group

A test runner usually starts workers. To stop all of them, start the job in its
own process group and signal the group. In a script, which is non-interactive
bash, turn on job control with `set -m` first; each background job then gets
its own process group, and that group's ID is `$!`:

```bash
set -m
npx vitest run src/ui > vitest.log 2>&1 &
pgid=$!

kill -- -"$pgid"
wait "$pgid"
echo "exit=$?"
# Output: exit=143
```

The minus sign before the number tells `kill` to signal the whole group. Only
processes in that group receive the signal; nothing is chosen by name.

`setsid` also works. A background job in non-interactive bash is not a group
leader, so `setsid` does not fork: the job calls `setsid()` itself, becomes the
leader of a new group, and its group ID is `$!`. `kill -- -"$!"` then stops it
and its children.

## Time-box a test run

Use the limits the test runner provides, so a hung test fails by itself and
nothing has to be killed afterwards:

| Runner | Option | Effect |
| --- | --- | --- |
| cargo nextest | `slow-timeout = { period = "60s", terminate-after = 3 }` in `.config/nextest.toml` | Ends a test that runs past three periods |
| vitest | `npx vitest run --testTimeout 30000` | Fails a test that runs past 30 seconds |
| pytest (with pytest-timeout) | `pytest --timeout 60` | Fails a test that runs past 60 seconds |

Plain `cargo test` has no per-test time limit. `cargo test -- --test-threads 4`
limits how many tests run at once, which keeps a hang from starving the host,
but it does not end a hung test. Use nextest when a run needs a time limit.

## Commands not to use

| Command | Why |
| --- | --- |
| `pkill -f <pattern>` | Matches the whole command line of every process the user owns, including the recipe runner and other agents |
| `pgrep -f <pattern> \| xargs kill` | The same match, in two steps |
| `kill $(pgrep -f <pattern>)` | The same match, in one line |
| `killall <name>` | Kills every process with that name, including other sessions' copies |

amplihack keeps recipe context off the runner's command line when the runner
supports it (see [Recipe Context Transport Reference](../reference/recipe-context-transport.md)),
but other processes still have arbitrary text in their argv. The guidance
applies whatever the runner version.

## Where agents are told

The paragraph above appears in:

- `amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md`, next to the guidance on
  waiting;
- the skills `default-workflow`, `smart-test`, `auto-drive-to-merge` and
  `quality/testing-code`;
- the agent steps that run shell or test commands in the recipes
  `default-workflow`, `auto-drive-to-merge`, `autodrive-merge-round`,
  `consensus-test-fix`, `workflow-precommit-test` and `smart-orchestrator`.

It is guidance only. No hook refuses pattern kills and no check scans for the
text.

## Related

- [Wait Without Leaving Orphan Processes](./wait-without-orphan-processes.md) - Waiting for background work with `wait $pid`
- [Recipe Context Transport Reference](../reference/recipe-context-transport.md) - Keeping recipe context off the runner's argv
- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The threat model
