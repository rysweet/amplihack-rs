---
title: "Stop Background Jobs by PID"
description: "Stop a background job you started by its recorded PID or by a process group you created, never by name or command-line pattern, and time-box test runs with the test runner's own options."
last_updated: 2026-11-14
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

- [The guidance wording](#the-guidance-wording)
- [Stop a job by its recorded PID](#stop-a-job-by-its-recorded-pid)
- [Stop a job and its children by process group](#stop-a-job-and-its-children-by-process-group)
- [Time-box a test run](#time-box-a-test-run)
- [Commands not to use](#commands-not-to-use)
- [Where agents are told](#where-agents-are-told)

## The guidance wording

Agents receive the guidance in three wordings. All three say the same three
things: stop a job by the PID you recorded from `$!`; signal a process group
only if you created that group for the job with `set -m` or `setsid`; never
choose processes by name or by a command-line pattern.

**Skills.** The skills listed under [Where agents are told](#where-agents-are-told)
are delivered as files, so they contain the full wording with tool names and
examples:

> Stop a background job you started by the PID you recorded with `$!`, or by a
> process group you created for the job with `set -m` or `setsid`, using
> `kill -- -PGID`. Never use `pkill -f`, `pgrep -f` with `kill` or
> `xargs kill`, or `killall <name>`: a pattern matches every process whose
> command line contains the same words, including the recipe runner and other
> agents. Time-box test runs with the test runner's own options (for example
> nextest `slow-timeout` with `terminate-after`, or vitest `--testTimeout`)
> instead of killing the run afterwards.

**System-prompt fragment.** `amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md`
is written to a file, but it is limited to 25 lines, so it uses a two-line
bullet:

```markdown
- Stop only a job you started: by its `$!` PID, or a group you created with `set -m` or `setsid`.
  Never select processes by name or pattern (`pkill -f`, `killall`); it may match the runner.
```

**Recipe agent steps.** The copies inside recipe prompts name no tool, binary
or process:

> Stop only a process you started, by the PID you recorded from `$!`. If you
> stop a process group, it must be a group you created yourself for the job
> with `set -m` or `setsid`; never signal a group you did not create. Never
> choose processes to signal by name or by a command-line pattern. Use the test
> runner's own per-test time limit instead of stopping a run.

### Why the recipe copies name no tools

On the pinned recipe runner, the first 8 KiB of each agent step's prompt is on
the command line of the `amplihack claude` wrapper that the runner starts for
that step. A prompt that names `vitest`, `nextest`, `pkill` or `killall` puts
those words in the wrapper's argv, so a `pkill -f vitest` run by any agent on
the host would also end every wrapper whose prompt contains the paragraph. The
recipe copies therefore keep the rule and leave the examples to the skills, the
fragment and this guide, none of which is on argv.

### Why the group clause matters

The recipe runner leads its own session, and the agent processes it starts are
in that session's process group unless they create a new one. A bare
`kill -- -PGID` with a group ID read from `ps` can therefore signal the runner.
Signal a group only when you created it yourself, as shown in
[Stop a job and its children by process group](#stop-a-job-and-its-children-by-process-group),
so that its ID is the `$!` you recorded.

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
processes in that group receive the signal; nothing is chosen by name. Signal
only a group you created this way: a group ID taken from `ps` may belong to the
recipe runner's session.

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
supports it, and the bundled nested recipe calls pass agent text by file (see
[Recipe Context Transport Reference](../reference/recipe-context-transport.md)).
On the pinned runner, the runner's own argv and the first 8 KiB of each agent
wrapper's prompt still hold arbitrary text, and other processes on the host
have their own. The guidance applies whatever the runner version.

## Where agents are told

| Location | Wording |
| --- | --- |
| `amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md`, next to the guidance on waiting (quoted in [System Prompt Append](../SYSTEM_PROMPT_APPEND.md)) | Fragment bullet |
| Skills `auto-drive-to-merge`, `default-workflow`, `smart-test` and `quality/testing-code`, and their copies under `docs/claude/skills/` | Full wording |
| Recipe agent steps, listed below | Recipe paragraph |

The recipe paragraph appears in these ten agent steps:

| Recipe | Step IDs |
| --- | --- |
| `auto-drive-to-merge` | `autodrive-prerequisites`, `autodrive-check-unmerged` |
| `autodrive-merge-round` | `step-01-platform-facts`, `step-01b-crusty-evidence`, `step-02-merge-ready-assessment`, `step-04-address-blockers` |
| `consensus-test-fix` | `step7-fix-issues`, `step8-local-testing` |
| `workflow-precommit-test` | `step-12-run-precommit`, `step-13-local-testing` |

To list them in a checkout:

```bash
grep -n 'by the PID you recorded from' amplifier-bundle/recipes/*.yaml
# Output: ten lines, in the four recipes above
```

It is guidance only. No hook refuses pattern kills and no check scans for the
text.

## Related

- [Wait Without Leaving Orphan Processes](./wait-without-orphan-processes.md) - Waiting for background work with `wait $pid`
- [Recipe Context Transport Reference](../reference/recipe-context-transport.md) - Keeping recipe context off the runner's argv
- [Why Prompt Text Stays Out of argv](../concepts/prompt-text-out-of-argv.md) - The threat model
