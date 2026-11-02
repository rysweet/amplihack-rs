---
title: Wait Without Leaving Orphan Processes
type: howto
updated: 2026-11-01
related: issue #1794, issue #1733, issue #1773, issue #1764
---

# Wait Without Leaving Orphan Processes

Agents running recipe steps sometimes need to pause: to let a service start,
to poll a CI run, or to give a background build time to finish. Claude Code
refuses a plain foreground `sleep`, so agents improvised
`timeout 10 tail -f /dev/null` instead. On a host whose `timeout` is uutils
coreutils, the `tail` can outlive the wait. Each orphan then wakes on every
write to `/dev/null` anywhere on the host and spins. On one host, 55 of them
used about 29 CPUs (#1794).

This guide is for anyone writing recipe prompts or running agents, and for
contributors working on the PreToolUse hook.

## Contents

- [Wait for a fixed time](#wait-for-a-fixed-time)
- [Wait for background work to finish](#wait-for-background-work-to-finish)
- [Use `timeout` safely](#use-timeout-safely)
- [What the guard denies](#what-the-guard-denies)
- [Where agents are told](#where-agents-are-told)
- [Clean up tails left from before the guard](#clean-up-tails-left-from-before-the-guard)

## Wait for a fixed time

Use `amplihack wait`:

```bash
amplihack wait --seconds 30
echo "exit=$?"
# Output: exit=0
```

`amplihack wait` sleeps inside its own process. It starts no child process,
prints nothing and exits `0`. Ctrl-C, `SIGINT` and `SIGTERM` end it with the
default action. It runs for at most `N` seconds, never spins the CPU and
starts nothing that could outlive it.

| Option | Values | Notes |
|--------|--------|-------|
| `--seconds N` | Whole seconds, `0` to `86400` inclusive | Required. `0` returns at once. |

Anything else is a usage error. `amplihack wait` exits `2` and prints the
reason on stderr for a missing `--seconds`, a negative number, a fraction, a
non-number, a value over `86400` or an extra argument:

```bash
amplihack wait --seconds 1.5
echo "exit=$?"
# stderr: error: invalid value '1.5' for '--seconds <SECONDS>': invalid digit found in string
# Output: exit=2
```

The upper limit of one day catches mistakes such as a value given in
milliseconds.

`amplihack wait` skips the start-up work that other subcommands do, such as
staging framework assets on a first run, and shows no update notice.

## Wait for background work to finish

When the wait is for a command to finish, wait on the command itself rather
than for a fixed time. Start it in the background and wait on its PID:

```bash
cargo build --workspace --locked > build.log 2>&1 & pid=$!
wait $pid
echo "build exit=$?"
```

`wait $pid` returns as soon as the command ends and gives its exit status. To
poll a log while the command runs, check it between short waits:

```bash
cargo test --workspace --locked > test.log 2>&1 & pid=$!
while kill -0 "$pid" 2>/dev/null; do
  tail -n 3 test.log
  amplihack wait --seconds 30
done
wait $pid
echo "test exit=$?"
```

`tail -n 3` reads the end of the file once and exits, so it is not affected by
the guard.

## Use `timeout` safely

The host's `timeout` may be uutils coreutils rather than GNU coreutils. Check
which one you have:

```bash
timeout --version | head -1
# Output on a uutils host: timeout (uutils coreutils) 0.12.0
```

Plain `timeout N <command>` moves itself and its command into a new process
group. If the shell that started it ends first, the command can be left
running with PID 1 as its parent. Use one of these forms instead:

```bash
# Stay in the terminal's process group; Ctrl-C works as usual.
timeout --foreground 600 ./scripts/long-check.sh

# Run in a new session with no controlling terminal; -w waits for the result.
setsid -w timeout 600 ./scripts/long-check.sh
```

With `--foreground`, children of the command are not killed at the deadline,
so prefer `setsid -w` for a command that starts children of its own. The
trade-offs of each form, including how to stop a `setsid -w` run, are in
[Keep Tests from Prompting for Git Credentials](./keep-tests-from-prompting-for-git.md#run-a-long-command-by-hand-or-from-an-agent).

Never use `timeout N tail -f /dev/null` as a sleep. Use
`amplihack wait --seconds N`.

## What the guard denies

The `pre-tool-use` hook has a tail-follow guard. It denies a shell command
that runs `tail` with a follow option (`-f`, `-F`, `--follow` and their other
spellings) on a device that never ends, such as `/dev/null`. The answer is an
enforced deny, the same shape as the tmux guard's, so Claude Code, Copilot
and Codex all stop the call. The deny message names the supported ways to
wait and never repeats the command. No environment variable, flag or setting
turns the guard off, and it does not depend on XPIA.

The guard sees through wrappers such as `timeout`, `nohup`, `nice`,
`ionice`, `chrt`, `taskset`, `unbuffer`, `setsid` and `stdbuf`, and looks
inside `bash -c '…'`, a heredoc fed to a shell, and
`$(…)` or backtick substitutions. These are all denied:

```bash
timeout 10 tail -f /dev/null
bash -lc 'tail -f /dev/null'
x=$(timeout 5 tail -f /dev/null)
```

Text that only mentions the form is allowed, such as
`echo 'tail -f /dev/null'`, a commit message, or a Dockerfile written with
`cat <<'EOF'`. So are `tail -f build.log` and
`docker run -d alpine tail -f /dev/null`, where `tail` runs in the container.

Unlike the tmux guard, the tail-follow guard allows a command when it cannot
be sure, for example when the command cannot be fully parsed. A missed deny
leaves at worst one orphaned `tail`; a false deny would block ordinary work.

The full rules, examples, deny message and known limits, including the forms
the guard does not see (such as `bash <<< '…'`, `echo '…' | bash` and
`busybox tail`), are in the
[hook specification](../reference/hook-specifications.md#tail-follow-guard).

## Where agents are told

Agents are told to use `amplihack wait --seconds N`, or a background command
and `wait $pid`, and never `tail -f /dev/null` or `timeout N tail -f /dev/null`,
in the places below. Each recipe entry reaches only the agent that runs that
step, on whichever agent CLI it uses.

| Place | Reaches |
|-------|---------|
| `step-13-local-testing` in `amplifier-bundle/recipes/workflow-precommit-test.yaml`, in the "Waiting" bullet that follows "Any long command" | The agent running local testing in the default workflow |
| `step-18b-implement-feedback` in `amplifier-bundle/recipes/workflow-pr-review.yaml`, in the "Waiting" checklist item | The agent implementing review feedback, the step where the orphans in issue #1794 came from |
| `reflect-round-1`, `reflect-round-2` and `reflect-final` in `amplifier-bundle/recipes/smart-reflect-loop.yaml`, in the "Waiting" paragraph of the PR early-exit section | The reviewer agents that check PR CI status |
| `amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md` | Claude sessions started by amplihack, in every step |

Any other step relies on the PreToolUse hook: a Copilot or Codex agent there
learns the supported forms from the deny message on its first refused call.
`recipe_prompts_and_system_prompt_name_the_supported_ways_to_wait` in
`tests/integration/default_workflow_decomposition_test.rs` fails if any of
these places loses the guidance.

`AGENTS.md` does not carry this guidance; see
[issue #1333](https://github.com/rysweet/amplihack-rs/issues/1333).

## Clean up tails left from before the guard

The guard does not end `tail` processes that are already running. To find
any left from before the guard, list them by exact match and check each
before ending it:

```bash
pgrep -xa tail | grep -F ' -f /dev/null'
```

The list also shows the `tail -f /dev/null` entry point of any running
container, since container processes are visible on the host. Leave those
running: ending one stops its container. An orphan left by a tool call has
PID 1 or a user `systemd` as its parent (`ps -o ppid= -p <pid>`).

## See also

- [Hook specifications: tail-follow guard](../reference/hook-specifications.md#tail-follow-guard)
- [CLI reference](../reference/cli.md)
- [XPIA configuration: check order](../reference/xpia-configuration.md)
- [Keep Tests from Prompting for Git Credentials](./keep-tests-from-prompting-for-git.md)
