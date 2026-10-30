# amplihack wait — Command Reference

`amplihack wait` blocks for a fixed time, or until a process exits, without
starting any other process. Agents use it instead of `tail -f /dev/null` or
`sleep` loops, which leave orphaned processes behind when the agent's shell
goes away (#1794).

## Contents

- [Synopsis](#synopsis)
- [Options](#options)
- [Exit codes](#exit-codes)
- [Examples](#examples)
- [Why not tail on /dev/null](#why-not-tail-on-devnull)
- [uutils timeout and SIGTTIN](#uutils-timeout-and-sigttin)
- [Behaviour and limits](#behaviour-and-limits)
- [Related](#related)

## Synopsis

```
amplihack wait --seconds <N>
amplihack wait --until pid-exited:<PID> --deadline <SECS>
```

Exactly one of `--seconds` and `--until` must be given.

## Options

| Option | Value | Description |
|--------|-------|-------------|
| `--seconds <N>` | integer, `0` to `86400` | Sleep for `N` seconds in the current process, then exit 0. |
| `--until <CONDITION>` | `pid-exited:<PID>` | Return when process `PID` has exited. Requires `--deadline`. |
| `--deadline <SECS>` | integer, `1` to `86400` | Upper bound for `--until`. Valid only with `--until`. |

`<PID>` must be an integer from `1` to `2147483647`. `pid-exited` is the only
condition kind; the condition is case-sensitive.

A process counts as exited when `kill(PID, 0)` fails with `ESRCH`, or, on
Linux, when `/proc/<PID>/stat` reports state `Z` (zombie) or `X` (dead). Any
other result, including `EPERM` for a process owned by another user, counts as
running. The PID is checked at most every 250 ms. The process need not be a
child of `amplihack wait`.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | The time passed (`--seconds`), or the process exited (`--until`). |
| `2` | Invalid arguments, or the command is not supported on this platform. |
| `124` | The deadline passed while the process was still running. |

Code 124 is the code `timeout` uses, so scripts that already handle `timeout`
can treat both the same way. On deadline the command writes one line to
stderr:

```
amplihack wait: deadline of 600 s passed; pid 48211 still running
```

Exit 2 covers all of these:

- neither `--seconds` nor `--until`, or both;
- `--until` without `--deadline`, or `--deadline` without `--until`;
- a value outside the ranges above, or one that is not an integer;
- a malformed condition such as `pid-exited`, `pid-exited:abc`,
  `pid-exited:0` or `pid-exited:-1`;
- an unknown condition kind such as `file-exists:/tmp/x`, which is rejected
  with a message naming the supported form `pid-exited:<PID>`;
- the PID of `amplihack wait` itself;
- a non-Unix platform, where the command reports that it is unsupported.

`amplihack wait` writes nothing to stdout.

## Examples

### Pause between checks

Wait 60 seconds before checking CI again:

```bash
amplihack wait --seconds 60
gh pr checks
```

### Wait for background work to finish

Start a long build in the background and wait up to 30 minutes for it:

```bash
cargo build --release > build.log 2>&1 &
build_pid=$!
amplihack wait --until pid-exited:"$build_pid" --deadline 1800
```

`amplihack wait` does not report the build's own exit status. Collect it with
`wait "$build_pid"` in the same shell afterwards.

### Handle a deadline

```bash
amplihack wait --until pid-exited:"$build_pid" --deadline 600
case $? in
  0)   echo "build finished" ;;
  124) echo "build still running after 10 minutes"; kill "$build_pid" ;;
  *)   echo "wait failed" ;;
esac
```

## Why not tail on /dev/null

`tail -f /dev/null` never ends, because `/dev/null` never grows and `tail`
has nothing to notice. When an agent uses it to wait and its shell is then
closed, the `tail` process is left running with no parent and no one to stop
it. The same holds for `/dev/zero`, `/dev/random` and `/dev/urandom`.

The `pre-tool-use` hook therefore denies a shell tool call that would run
`tail` in follow mode on one of those four devices, and its reason points to
`amplihack wait`. See
[tail guard for idle devices](hook-specifications.md#tail-guard-for-idle-devices-1794) for
exactly which commands it matches.

A `sleep` loop has a similar problem: each iteration starts a new `sleep`
process, and a loop cut off by its shell can leave the last one behind.
`amplihack wait` sleeps in its own process and starts no children.

## uutils timeout and SIGTTIN

On hosts where `timeout` is the uutils (Rust coreutils) version, it can leave
its child running when the parent shell exits first, and a child that reads
from the terminal stops on `SIGTTIN` and then waits indefinitely rather than
until the deadline. Redirecting stdin from `/dev/null` does not prevent this
for programs that open the terminal themselves, such as `git` prompting for
credentials.

When you need `timeout`, run it in the foreground or in its own session:

```bash
timeout --foreground 600 cargo test --workspace
setsid timeout 600 cargo test --workspace
```

To wait on work that is already running, prefer
`amplihack wait --until pid-exited:<PID> --deadline <SECS>`, which needs no
terminal and leaves nothing behind when its shell exits.

## Behaviour and limits

- **No child processes.** The command sleeps with `std::thread::sleep` and
  checks the PID with `kill(PID, 0)`. It starts no processes and no async
  runtime, installs no signal handlers and reads no environment variables.
- **File access.** The only file it reads is `/proc/<PID>/stat`, on Linux.
  Other Unix platforms use `kill(PID, 0)` alone, so a zombie counts as
  running there until its parent reaps it.
- **PID reuse.** If the target exits and the system gives its PID to a new
  process before the next check, the command keeps waiting on the new
  process. `--deadline` bounds that wait. Checks are 250 ms apart, so this is
  unlikely but possible on a busy host.
- **Interrupting.** `Ctrl-C` or `SIGTERM` ends the command with the default
  signal behaviour; it does not exit 0 or 124.
- **Platform.** Unix only. Elsewhere it exits 2.

## Related

- [Hook specifications: tail guard for idle devices](hook-specifications.md#tail-guard-for-idle-devices-1794)
- [CLI reference](cli.md)
