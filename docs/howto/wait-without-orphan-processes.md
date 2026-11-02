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
- [What the guard allows](#what-the-guard-allows)
- [Where agents are told](#where-agents-are-told)
- [Known limits](#known-limits)
- [Tests](#tests)

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
that runs `tail` with a follow option on a device that never ends. The answer
is an enforced deny, the same shape as the tmux guard's, so Claude Code,
Copilot and Codex all stop the call. The full rules are in the
[hook specification](../reference/hook-specifications.md#tail-follow-guard).

The guard denies a command when all of these hold:

1. `tail` is the program that runs, after wrappers are removed. The guard
   sees through `timeout`, `nohup`, `env`, `nice`, `exec`, `command`, `time`,
   `sudo`, variable assignments, `setsid` and `stdbuf`.
2. A follow option is set: `-f`, `-F`, `--follow` or `--follow=name` or
   `--follow=descriptor`. Shortened forms such as `--f`, `--fo` and
   `--foll=name` count too, because `tail` accepts any unique start of a
   long option name. Short options may be grouped, and are read left to
   right until an option that takes a value (`-n`, `-c`, `-s`), so `-fn0`
   follows and `-n5f` does not. The old form `-5f` also follows.
3. A file operand is one of these devices, after repeated `/` and `/./` are
   collapsed:

   | Device |
   |--------|
   | `/dev/null` |
   | `/dev/zero` |
   | `/dev/random` |
   | `/dev/urandom` |
   | `/dev/full` |
   | `/dev/stdin` |
   | `/dev/tty` |
   | `/dev/fd/0` |
   | `/proc/self/fd/0` |

The guard also looks inside the command given to `sh -c`, `bash -c`,
`bash -lc`, `zsh -c`, `dash -c` and `ksh -c`, and inside a heredoc fed to one
of those shells as its script (not to a script file, as in
`bash run.sh <<'EOF'`), up to three nested shells deep. A fourth nested shell is
not checked, and its command is allowed.

Examples of denied commands:

| Command | Why |
|---------|-----|
| `timeout 10 tail -f /dev/null` | `timeout` is removed; `tail -f` on `/dev/null` |
| `tail -f /dev/null &` | Running in the background does not matter |
| `tail -F /dev/null` | `-F` follows |
| `tail -fn0 /dev/null` | `-f` comes before `-n` in the group |
| `tail --follow=name //dev//null` | Collapses to `/dev/null` |
| `tail -f --pid=1234 /dev/null` | `--pid` does not exempt the command; see below |
| `setsid stdbuf -oL tail -f /dev/null` | Both wrappers are removed |
| `bash -lc 'tail -f /dev/null'` | The `-c` command is checked |
| `bash <<'EOF'`, `tail -f /dev/null`, `EOF` on separate lines | The heredoc is run by `bash` |

`--pid=PID` makes `tail` stop when that process ends, but the command is
still denied. A `tail` that ends with a PID is rarely needed, and
`wait $pid` does the same job without a follower.

The deny message names the supported ways to wait. It never repeats the
command:

> Blocked: this command runs `tail -f` or `tail -F` on a device that never ends, such as /dev/null. Used as a wait, the tail can outlive its `timeout` and spin on the host until it is killed (#1794). To wait for a fixed time, run `amplihack wait --seconds N`. To wait for work to finish, run it in the background and wait on its PID (`cmd & pid=$!; wait $pid`). Do not use `tail -f /dev/null` or `timeout N tail …` as a sleep.

No environment variable, flag or setting turns the guard off, and it does
not depend on XPIA.

## What the guard allows

| Command | Why |
|---------|-----|
| `tail -f build.log` | A regular file, not a device |
| `tail -n 5 /dev/null` | No follow option |
| `tail -n5f /dev/null` | `5f` is the value of `-n` |
| `tail -f` or `tail -f -` | No device operand; stdin is followed |
| `echo 'tail -f /dev/null'` | The text is an argument to `echo` |
| `git commit -m 'stop using tail -f /dev/null'` | The text is an argument to `git` |
| `cat > Dockerfile <<'EOF'`, a `CMD ["tail","-f","/dev/null"]` line, `EOF` | `cat` is not a shell, so the heredoc is not run |
| `cat > compose.yaml <<EOF`, a `command: tail -f /dev/null` line, `EOF` | As above |
| `docker run -d alpine tail -f /dev/null` | `tail` runs in the container as its entry point; `docker` is the program |
| `kubectl exec pod -- tail -f /dev/null` | `kubectl` is the program |
| `sleep 30` | `sleep` is not guarded |

Unlike the tmux guard, the tail-follow guard fails open. When the command
cannot be fully parsed, when the input is over 64 KiB, or when the nesting or
work limits are reached, it allows the command. A missed deny leaves at worst
one orphaned `tail`, while a false deny would block ordinary work. The tmux
guard fails closed, because a missed deny there kills the user's tmux server.

When both guards match, the tmux guard answers.

## Where agents are told

Agents are told to use `amplihack wait --seconds N`, or a background command
and `wait $pid`, and never `tail -f /dev/null` or `timeout N tail …`, in two
places:

| Place | Reaches |
|-------|---------|
| `step-13-local-testing` in `amplifier-bundle/recipes/workflow-precommit-test.yaml`, in the "Waiting" bullet that follows "Any long command" | Every agent CLI that runs the default workflow |
| `amplifier-bundle/context/SYSTEM_PROMPT_APPEND.md` | Claude sessions started by amplihack |

`AGENTS.md` does not carry this guidance; see
[issue #1333](https://github.com/rysweet/amplihack-rs/issues/1333).

## Known limits

The guard is a guard rail against agent mistakes, not a sandbox. It does not
see:

- a device given by expansion, such as `tail -f "$DEV"`, `tail -f $(echo /dev/null)`
  or `tail -f /dev/nul*`;
- `tail` run by `xargs` or `find -exec`;
- `tail` run by `eval` or `env -S`;
- `tail` run through a multi-call binary, such as `busybox tail -f /dev/null`;
- `tail` in a script file run by a later command, such as `bash wait.sh`;
- a path with `..`, such as `/dev/../dev/null`, or a symbolic link to a
  device;
- `sh -c` nesting deeper than three nested shells;
- input over 64 KiB, or input the shell tokeniser cannot fully parse;
- `sleep`, and `tail -f` on a regular file that is never written.

It does not end `tail` processes that are already running. To find any left
from before the guard, list them by exact match and check each before ending
it:

```bash
pgrep -xa tail | grep -F ' -f /dev/null'
```

## Tests

| Test | Runs | Checks |
|------|------|--------|
| `crates/amplihack-hooks/src/pre_tool_use/tests/tail_follow_guard_tests.rs` | `cargo test -p amplihack-hooks tail_follow` | Each denied and allowed form above, including `setsid -w`, `-f` and `-c` and `stdbuf -i`, `-o` and `-e` with joined and separate values; each fail-open limit, including that three nested shells are checked and four are allowed; that an argv array with a non-string element is allowed; the deny shape, that the command is never echoed, and that the guard reads no environment variable |
| `crates/amplihack-hooks/src/pre_tool_use/tests/pre_tool_use_tests.rs` | `cargo test -p amplihack-hooks pre_tool_use` | A deny through `PreToolUseHook::process` for tool names `Bash`, `bash`, `shell` and `exec_command`, the `cmd` field, the argv array form, and a Copilot `preToolUse` payload |
| `crates/amplihack-cli/tests/issue_1794_wait.rs` | `cargo test -p amplihack-cli --test cli_integration issue_1794_wait::` | On Linux, `amplihack wait --seconds 2` takes at least 2 s, exits `0` with empty stdout and stderr, and never has a child process; no process is left with its PID as parent or process group; the invalid values above exit `2` |
| `tests/integration/default_workflow_decomposition_test.rs` | `cargo test -p amplihack --test integration default_workflow_decomposition::` | Step 13's prompt and `SYSTEM_PROMPT_APPEND.md` both contain `amplihack wait --seconds` and `tail -f /dev/null` |

The wait test runs under `isolated(..)`, so it uses a temporary `HOME` and
does not inherit `AMPLIHACK_CLAUDE_PROFILE_ROOT`.

## See also

- [Hook specifications: tail-follow guard](../reference/hook-specifications.md#tail-follow-guard)
- [CLI reference](../reference/cli.md)
- [XPIA configuration: check order](../reference/xpia-configuration.md)
- [Keep Tests from Prompting for Git Credentials](./keep-tests-from-prompting-for-git.md)
