---
title: How a Test's Rotator Ends with Its Test
type: explanation
updated: 2026-11-09
related: issue #2095, issue #1690, issue #1592
---

# How a Test's Rotator Ends with Its Test

Some integration tests start a real rotator with
`amplihack profile rotate start`. That rotator is detached with `setsid`, so
before #2095 it outlived a test process that was killed. This page explains
why it leaked, how the test harness now ties the rotator to the test process,
and why the other options were rejected. For the steps a contributor follows,
see [Keep Tests from Leaking Rotator Daemons](../howto/keep-tests-from-leaking-rotator-daemons.md#tie-a-started-rotator-to-the-test).

## Contents

- [Where the leak came from](#where-the-leak-came-from)
- [The lifetime pipe](#the-lifetime-pipe)
- [When the pipe is armed](#when-the-pipe-is-armed)
- [What the watcher does when the pipe closes](#what-the-watcher-does-when-the-pipe-closes)
- [What the fixture does on drop](#what-the-fixture-does-on-drop)
- [Why not PDEATHSIG, a subreaper or a fake spawner](#why-not-pdeathsig-a-subreaper-or-a-fake-spawner)
- [What does not change](#what-does-not-change)
- [Tests](#tests)

## Where the leak came from

Three test files start a real rotator:

- `issue_1592_daemon.rs`
- `issue_1746_daily_archive_rotator.rs`
- `issue_1592_root_rotator.rs`

Each runs `profile rotate start`. The call path is `daemon::start` →
`launch_core` → `daemon::spawn`, which starts
`amplihack profile rotate watch --spawned` in a new session with `setsid`.
The watcher's parent becomes PID 1, so nothing ties it to the test.

| How the test ends | Does the watcher leak? | Why |
|-------------------|------------------------|-----|
| The test completes | No | The test runs `profile rotate stop` itself. |
| The test panics | No, with one exception | A `Guard` or `Reaper` stops the watcher during unwinding. In `issue_1592_root_rotator.rs` there is a gap between `start` and the creation of its `Guard`; a panic there leaks. |
| SIGKILL, `abort`, or a cancelled `cargo test` | Yes | No destructor runs, so nothing stops the watcher. |

The leak check from #1690 (`scripts/check-leaked-rotators.sh`) reports such
a watcher after the run. It does not prevent one.

## The lifetime pipe

The fixture in `crates/amplihack-cli/tests/support/profile_rotate.rs` creates
a pipe with `std::io::pipe()` when it is built. The test process keeps the
write end. Every command the fixture builds through `base(..)` (that is,
`cmd(..)` and `cmd_no_root_env(..)`) receives the read end and the
variable `AMPLIHACK_TEST_ROTATOR_LIFETIME_FD`, which holds the read end's fd
number.

The watcher reads from that fd on a thread of its own. Nothing is ever
written, so the read blocks until the write end closes. The kernel closes the
write end when the test process ends for any reason: a normal return, a
panic, a signal, SIGKILL or `abort`. The read then returns EOF and the
watcher stops.

```text
test process (holds write end)
  └─ amplihack profile rotate start      (inherits read end)
       └─ setsid: rotate watch --spawned  (inherits read end, reads it)

test process exits → kernel closes write end → read() returns EOF → watcher stops
```

Details of the hand-over:

- The write end has `FD_CLOEXEC` set from creation and is never duplicated,
  so no child holds it. A child holding the write end would keep the pipe
  open after the test ends.
- The read end keeps `FD_CLOEXEC` in the test process. A `pre_exec` closure
  in the child clears it with `fcntl(fd, F_SETFD, 0)` just before `exec`. The
  closure captures only the fd number and makes only that call, which is
  async-signal-safe. The test binary runs tests in parallel, so clearing the
  flag in the parent would let unrelated children inherit the fd.
- There is no `dup2` to a fixed fd number, so the cargo jobserver fds and the
  exec-error pipe that `std::process::Command` uses are never overwritten.
- The variable does not start with `AMPLIHACK_ROTATE`, so `daemon::spawn`,
  which removes `AMPLIHACK_ROTATE*` from the watcher's environment, passes it
  on. A source-scan test asserts this.
- The `start` CLI process also holds a read end while it runs, and its
  children may inherit one. A read end cannot delay EOF, so this is harmless.
  Do not "fix" it by passing the write end instead.

## When the pipe is armed

`rotate/lifetime.rs` decides whether to watch the fd. The pure function
`armed_fd` returns the fd only when all five conditions hold:

| # | Condition | Production value |
|---|-----------|------------------|
| 1 | `AMPLIHACK_TEST_ROTATOR_LIFETIME_FD` is set and is valid UTF-8. | Never set. |
| 2 | Its value parses, without trimming, as an `i32` of at least `3`. | Not applicable. |
| 3 | `AMPLIHACK_TEST_HOME_GUARD` is exactly `1`. | Never set. |
| 4 | `fstat` on the fd reports a FIFO. | Not applicable. |
| 5 | `fcntl(fd, F_GETFD)` shows `FD_CLOEXEC` clear. | Not applicable. |

Condition 5 separates an fd inherited across `exec` from one the process
opened itself. A probe child that inherits the variable but not the fd could
otherwise find an unrelated pipe at the same number and take ownership of it.

If any condition fails, `arm()` returns without doing anything. It does not
print, log, panic or exit, and the watcher behaves exactly as it did before
#2095. Values such as `""`, `"x"`, `"-1"`, `"2"`, `" 5"` and
`"99999999999"` are all ignored, as is a guard of `0` or `true`.

Because a failed arming check is silent, nothing in the rotate log shows
whether a watcher was armed. The integration test
`a_killed_test_takes_its_rotator_with_it` (D7) is the only check that a
rotator started by the fixture arms: if arming stopped working, the watcher
would outlive the killed helper and that test would fail.

## What the watcher does when the pipe closes

`watch()` in `rotate/daemon.rs` calls `lifetime::arm(&stop)` once,
directly after it registers its signal handlers. This runs in every mode, so
a plain `rotate watch` child is covered as well as a `--spawned` one.

Once armed, the watcher:

1. Sets `FD_CLOEXEC` on the fd, so its own probe children (the `claude`
   processes it runs to read usage) do not hold the read end.
2. Starts a thread that reads into a fixed 64-byte buffer:
   - `Interrupted`: reads again.
   - Data: ignored; reads again.
   - EOF or any other error: sets the existing `stop` flag (`PROBE_CANCEL`).

Setting `stop` is the same thing the SIGTERM handler does. The watcher then
leaves by its normal path: in-flight probes are cancelled,
`finish_refreshes` runs and `rotate.pid` is removed. No log line, flag,
default or exit code was added, so the rotate log reads the same as after a
`profile rotate stop`.

## What the fixture does on drop

`Fixture` now implements `Drop`. It sends no signal, and it never panics:

1. It drops the write end. This must come before any wait, or the wait would
   never end.
2. If the fixture is armed and `rotate.pid` exists, it reads the pid and
   requires it to be greater than 1.
3. It opens a pidfd for that pid with `pidfd_open`. `ESRCH` means the
   watcher has already gone, and `Drop` returns.
4. It reads `/proc/<pid>/environ` and requires two exact entries, compared
   as bytes: `HOME=<fixture home>` and
   `AMPLIHACK_TEST_ROTATOR_LIFETIME_FD=<fd>`. Both are set by the fixture on
   every command it builds and pass through `daemon::spawn` unchanged, so
   the check holds for a `cmd_no_root_env(..)` start and under a symlinked
   `TMPDIR`, where the resolved profile root can differ from `fx.root`. If
   either entry is missing or does not match, `Drop` treats the pid as
   reused by another process and returns without waiting. The environment
   is never printed.
5. It waits with `poll(pidfd, POLLIN, -1)`, retrying only on `EINTR`. If
   `poll` fails with any other error, it prints one line to stderr, naming
   no pid or path, because the `TempDir` may then be removed under a running
   watcher.

Steps 2 to 5 use pidfds and `/proc`, so they are compiled on Linux only. On
other unix systems `Drop` only closes the write end, and the integration
tests below do not run.

A command built by the fixture must be spawned while the fixture is alive.
Its `pre_exec` closure holds the read end's bare fd number, which may belong
to another descriptor once the fixture has been dropped.

The `TempDir` is removed only after this, so the watcher never runs against
a deleted profile root. The exception is a mismatch in step 4: the wait is
skipped, and if the watcher is in fact still running, the `TempDir` is
removed under it. Matching on the fixture's own `HOME` keeps that case to
pid reuse.
The wait has no timeout. A watcher that does not exit
hangs the test, and the CI job timeout is the backstop; a timer would hide
the bug instead of reporting it.

The existing `Reaper`, `Guard` and `stop_until_gone` helpers are unchanged.

## Why not PDEATHSIG, a subreaper or a fake spawner

| Option | Why it was rejected |
|--------|---------------------|
| `prctl(PR_SET_PDEATHSIG)` | It fires when the parent exits. The watcher's parent is the `start` CLI, which exits at once, not the test. |
| `PR_SET_CHILD_SUBREAPER` on the test binary | It changes process handling for the whole multi-threaded test binary and every test in it. |
| An injected spawner in tests | It would not run `daemon::spawn`, which is the path that leaks. |
| A profile-root-under-`TMPDIR` check | `TMPDIR` differs between hosts and lanes, so the check would be fragile. The two test-only conditions and the FIFO checks are enough. |

A pipe owned by the test process ends with that process in every case,
because the kernel closes the write end.

## What does not change

- `profile rotate start`, `rotate autostart`, launch autostart and
  `daemon::spawn` behave exactly as before: same flags, defaults, log lines
  and detach model.
- A production rotator never sees the variable or the guard, so `arm()`
  always returns without effect.
- The tests for surviving the starter's exit and an endpoint outage run
  unchanged on armed fixtures. Their starter is the `start` command, which
  exits while the test process, and so the write end, stays alive.
- Only `an_unarmed_rotator_outlives_its_starter` uses `Fixture::unarmed()`,
  because it needs a rotator that outlives the test process itself.

## Tests

| File | What it checks |
|------|----------------|
| `src/commands/profile/rotate/tests_lifetime.rs` | `armed_fd` rejects every bad variable, guard and fd case; a closed fd (`i32::MAX`), a regular file and a pipe with `FD_CLOEXEC` set are all refused; the writer from `std::io::pipe()` has `FD_CLOEXEC` set; data leaves `stop` false and dropping the writer sets it. |
| `src/commands/profile/rotate/tests_source_scan.rs` | `lifetime.rs` is a planned rotate file; it declares the variable as `AMPLIHACK_TEST_ROTATOR_LIFETIME_FD`, which does not start with `AMPLIHACK_ROTATE`. |
| `tests/issue_2095_test_rotator_lifetime.rs` | `a_killed_test_takes_its_rotator_with_it`: a helper test starts an armed rotator and is SIGKILLed; the watcher exits and its pid file is removed. `an_unarmed_rotator_outlives_its_starter`: the same with an unarmed fixture; after the helper is reaped, the watcher writes a new `idle: auto-switch is off` log line and is still alive with its pid file present. `the_fixture_names_the_variable_the_watcher_reads`: the fixture's copy of the variable name is the same literal. Linux only. |

The integration tests use no sleep or deadline. They wait on the helper's
stdout line, `wait()`, a pidfd `poll(-1)` and inotify events.
