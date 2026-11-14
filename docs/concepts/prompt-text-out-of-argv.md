---
title: "Why Prompt Text Stays Out of argv"
description: "Why amplihack never puts the operating contract, a task prompt or a recipe brief in an agent process's command line, and the threat model behind the rule."
last_updated: 2026-11-13
review_schedule: quarterly
owner: rysweet
doc_type: explanation
related:
  - ../reference/agent-prompt-channels.md
  - ../howto/check-agent-argv-for-prompt-text.md
  - ../SYSTEM_PROMPT_APPEND.md
  - ./large-payload-delivery.md
  - ../reference/recipe-context-transport.md
---

# Why Prompt Text Stays Out of argv

amplihack used to start every Claude session with its operating contract in
argv, as `--append-system-prompt '<text>'`. Since issue
[#2030](https://github.com/rysweet/amplihack-rs/issues/2030), no agent process
that amplihack starts has prompt, brief or contract text in
`/proc/<pid>/cmdline`. This page explains why. For the channel each launch path
uses now, see [Agent Prompt Channels Reference](../reference/agent-prompt-channels.md).

## The incident

On 7 November at 22:33:18Z, one command run to clean up stray sleeps:

```bash
pkill -f 'tail -f /dev/null'
```

`pkill -f` matches against the whole command line. The contract contains the
sentence "Never use `tail -f /dev/null` ... as a sleep", and that sentence was
in the argv of every amplihack-launched `claude` process. The command matched 63
of 79 of them and ended 34 sessions across the factory.

The contract was not wrong to mention `tail -f /dev/null`. The defect was that
any text in the contract was also a pattern every session would match. The same
is true of a task prompt in `-p <text>` or a recipe brief: `pgrep -f`,
`pkill -f`, `killall -r` and monitoring tools that search command lines all
treat that text as part of the process's name.

## Two problems, one channel

Text in argv causes two separate problems:

- **Accidental matches.** A pattern-based kill or search meant for one process
  matches every process whose prompt mentions the same words. This is what
  happened on 7 November.
- **Exposure.** `/proc/<pid>/cmdline` is readable by every user on the host, and
  it appears in `ps`, crash reports and process accounting. A prompt that names
  a private repository, an issue body or a customer is visible to all of them.

Both are fixed by the same rule: the text goes to the child on stdin or in a
file that only the session's user can read, and argv holds at most a path.

## The threat model

The rule protects against:

- other users on the same host reading prompts from the process table;
- any process, run by anyone, that selects processes by matching their command
  line.

It does not protect against a process running as the same user. That process
can read the contract file, the per-session files and `/proc/<pid>/environ`,
and can attach a debugger. Files are mode 0600 inside 0700 directories so that
other users cannot read them; there is no attempt to hide them from their owner.
A short window remains in which a same-user process could swap a file between
amplihack's check and the agent's read, or change `private/` between its mode
check and its use. That is accepted for the same reason: exploiting it needs
write access to the amplihack home.

Contract text in an environment variable was considered and rejected, because
the same user can read `/proc/<pid>/environ`, and the variable would also be
inherited by every child.

## What is left behind after a crash

Per-session files are removed when the agent exits, whether it succeeds, fails
or is stopped by a signal that amplihack forwards. If amplihack itself is
killed with SIGKILL, it cannot run any cleanup, and its 0700 per-session
directory stays in the temporary directory. Only its owner can read it, so this
is accepted rather than handled by a sweeper.

## What Copilot and Codex get

Copilot and interactive Codex cannot read a prompt from stdin or a file flag.
They receive a fixed pointer sentence, "Read and follow the complete
instructions in the file <path>", which holds a path and no task text.
`codex exec` reads its prompt from stdin. Neither CLI receives amplihack's
operating contract, as before #2030.

The same holds when a caller passes the prompt through the wrapper:
`amplihack codex -- exec <text>` becomes `exec -` with the text on stdin, and
`amplihack copilot -- -p <text>` becomes `-p` and the pointer sentence.
Codex options take values differently under each `exec` subcommand, so the
rewrite reads them with one table per subcommand, checked against Codex's
saved help text, and refuses a command line where it cannot tell which word
is the prompt rather than leave the prompt on argv. Two
passthrough forms are left alone and listed as exceptions: an interactive
first prompt (`amplihack claude -- '<text>'`, `amplihack codex -- '<text>'`)
and `amplihack amplifier -- run <text>`. See
[What argv still contains](../reference/agent-prompt-channels.md#what-argv-still-contains).

## Why fail instead of falling back

Before #2030, prompt delivery chose argv for any prompt of 4096 bytes or fewer,
and used argv as the last resort for larger ones. Each fallback looked harmless
on its own, and together they meant that most prompts were in argv most of the
time.

Now no automatic path selects argv:

- automatic delivery tries stdin, then a pointer sentence naming a private
  file, then a prompt-file flag, and returns an error if none is available;
- a Claude Code older than 2.1.319 has no `--append-system-prompt-file`, so the
  launch fails with a message naming the version, instead of sending the
  contract as text;
- a launcher that amplihack does not recognise gets an error, not argv.

There is no override that brings argv back. `AMPLIHACK_PROMPT_DELIVERY=argv`
used to be one, and it was a trap: the variable is inherited by every nested
step, so a single export in a shell profile put every prompt below it back in
the process table. The value is now unrecognised, logs a warning and behaves
as `auto`.

## Why one contract file per install

The contract is the same for every session, so it is written once to
`<amplihack home>/private/system-prompt-append.md` and shared. Writing it at
launch, rather than at install:

- lets an upgraded binary replace the file at its first launch;
- lets remote and fleet hosts write their own copy from their own binary;
- keeps it out of `install::essential_files`, which would arm the restage path
  described in [System Prompt Append](../SYSTEM_PROMPT_APPEND.md#there-is-no-install-side-gap-for-this-feature).

The bytes still come from the binary. amplihack compares the file with the
compiled-in contract and rewrites it if they differ, but never reads it as a
source of content, so editing the file does not change what the agent gets.

Task prompts, briefs and agent system prompts differ per session, so they go to
a new 0700 directory per session, which is removed when the session ends.

## Why the wrapper's own argv is a separate step

Recipe agent steps run `amplihack claude -- -p <brief> --system-prompt <agent md>`.
amplihack rewrites those arguments for the `claude` child it starts, so the
agent process is clean. The `amplihack claude` wrapper process was started by
the recipe runner with the brief in its argv, and amplihack cannot change its
own argv after the fact. That part is fixed in the runner, which is a separate
repository with its own review, and takes effect when amplihack's pinned
runner revision moves. The two are tracked as separate steps so that the
agent processes are clean without waiting for the runner.

## Recipe context on the runner's argv

On 13 November an agent in a live auto-drive ran `pkill -f "vitest.mjs"` to
stop a hung test run. `amplihack recipe run` was passing every recipe context
value to `recipe-runner-rs` as `--set key=value`: 153 argv entries and
12,397 bytes, including the task text. The task text contained "vitest", so the
pattern matched the runner and ended the drive
([#2582](https://github.com/rysweet/amplihack-rs/issues/2582)). This is the
#2030 failure one level up the process tree.

The same rule now applies to recipe context. When the runner reports the
`context_file_v1` capability, amplihack writes the context to a 0600 file in
the run's private runtime folder and passes only `--context-file <path>`.
When it does not, amplihack keeps the `--set` arguments unchanged, because a
runner without the option has no other way to receive the context.

The fallback is a compatibility path, not a quiet weakening. The choice
between the two is made from the capability report alone, and a run that
should use the file and cannot write it fails rather than putting the values
on argv, for the same reason as in [Why fail instead of falling back](#why-fail-instead-of-falling-back).

The pinned runner does not yet report `context_file_v1`, and it still passes
each agent step's prompt to the agent launcher with `-p`. Both are runner
changes, tracked in `rysweet/amplihack-recipe-runner`. Until they are released
and the pin moves, context values and task text remain in some argv on a host.
Agents are therefore also told to stop their own jobs by PID and never by
pattern; see [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md).

Rules, file locations and errors are in the
[Recipe Context Transport Reference](../reference/recipe-context-transport.md).

## Related

- [Recipe Context Transport Reference](../reference/recipe-context-transport.md) - How recipe context reaches the runner without argv (#2582)
- [Stop Background Jobs by PID](../howto/stop-background-jobs-by-pid.md) - The agent guidance against pattern kills
- [Agent Prompt Channels Reference](../reference/agent-prompt-channels.md) - Channels, files, the version gate and errors
- [Check Agent Processes for Prompt Text](../howto/check-agent-argv-for-prompt-text.md) - Run the `/proc` check
- [System Prompt Append](../SYSTEM_PROMPT_APPEND.md) - What the contract is for
- [Large Payload Delivery to Child Processes](./large-payload-delivery.md) - The `E2BIG` side of the same delivery layer
