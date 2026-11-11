---
title: Keep Tests Passing Under Host Load
type: howto
updated: 2026-11-11
related: issue #1973, issue #1765, issue #1961, issue #2019, issue #1977, issue #1998, issue #2036, issue #2017
---

# Keep Tests Passing Under Host Load

A test that measures wall-clock time also measures how busy the host is. On a
loaded host such a test fails, and the next run fails on a different subset,
so a push can take several attempts. This guide is for contributors who write
or change tests in `amplihack-cli`, `amplihack-hooks` and `amplihack-utils`.
It explains how to write a test that host speed cannot decide, which shared
helpers and test knobs to use, and how to validate the changed behavior.

## Contents

- [The rule](#the-rule)
- [The two hang guards](#the-two-hang-guards)
- [Assert the final state](#assert-the-final-state)
- [Wait on the condition](#wait-on-the-condition)
- [Wait on an event](#wait-on-an-event)
- [Change a test input](#change-a-test-input)
- [Test that a timeout fires](#test-that-a-timeout-fires)
- [Inject a clock](#inject-a-clock)
- [Run a private copy of the binary](#run-a-private-copy-of-the-binary)
- [Use a debug-only knob](#use-a-debug-only-knob)
- [Wait for the version probe to exit](#wait-for-the-version-probe-to-exit)
- [Recipe runner tests](#recipe-runner-tests)
- [Fleet TUI tests](#fleet-tui-tests)
- [Rotator tests](#rotator-tests)
- [Checks that something does not happen](#checks-that-something-does-not-happen)
- [Review event waits](#review-event-waits)
- [What not to do](#what-not-to-do)
- [Validate a fix](#validate-a-fix)
- [Related](#related)

## The rule

No test passes or fails because of how long something took. A test ends when
an event happens, and asserts the outcome of that event. Remove a wall-clock
dependency with the first method in this list that works:

1. Assert the final state.
2. Wait on the event itself: a join, a channel message, a barrier, a gate, a
   child's exit, a readiness file or a pidfd.
3. Wait on the condition with `wait_until`, under the 10-minute ceiling.
4. Change a test input: a stub, a time scale, a function parameter or an
   explicit mtime.
5. Inject a clock, when the product measures the time itself.
6. Isolate shared state: a per-test directory, counter or environment.
7. Add a debug-only knob, as a last resort. A new knob replaces a deadline
   with the event it was standing in for, as `AMPLIHACK_TEST_PROBE_WAIT=exit`
   replaces the probe's 3 s deadline with the candidate's exit. A new knob
   never lengthens a window.

Product timeouts and budgets keep their defaults in every build. A test
changes only its own inputs. Never fix a load failure by raising a timeout,
sleep, delay or retry count.

Two older knob uses, added by #1973 before this rule, do lengthen a window:
the identity knobs at 600 s in `Fixture::base`, and
`AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` at 600 s in `issue_1760_fixture`.
They are exceptions, described in [Knobs](#knobs). Do not copy them.

Wall-clock dependencies to look for:

| Dependency | Example before #1973 and #2019 |
|------------|--------------------------------|
| An exit timeout | "amplihack did not exit within 60 s" |
| A budget measured from real time | the 30 s identity budget, measured from the start of a rotate cycle |
| A fixed sleep used to order two events | `sleep(Duration::from_millis(1200))` before writing a file |
| An `elapsed < N` assertion | `elapsed < 5 s` after `--skip-update-check`; `start.elapsed() < 3 s` in the `limit_resume_fs` racers; `< 25 s` in three recipe tests |
| A product timeout no test input reaches | the 10 s limit on each tmux call of `amplihack agent`; the 3 s `--version` probe in `launch_target` |
| A lock wait or request deadline measured inside the product | the 5 s `FsGuard` lock timeout, timed with real sleeps; the `usage_direct` refresh deadline, run against a slow fake server |
| A sleep followed by "nothing happened" | sleeping 2.3 s, then asserting an orphan's marker file is absent |
| A quiet-period read | reading a PTY until no bytes arrive for 500 ms |
| A binary that another build replaces mid-run | `current_exe()/../../amplihack`, re-linked by cargo while a test runs it |

## The two hang guards

A test that waits on an event must still end if the event never comes. Two
guards do that, and no test adds a third:

| Guard | Value | Where | Why it exists |
|-------|-------|-------|---------------|
| `CEILING` | 600 s | `crates/amplihack-cli/tests/support/wait.rs`, included with `#[path]` by the `amplihack-cli` lib tests (`crate::test_support::wait`), `amplihack-hooks`'s `limit_resume_fs` and the `amplihack` `integration` target | Ends every wait on a child process, a channel, a PTY or a pidfd if the event never comes. |
| `HANG_GUARD` | 120 s | `commands::profile::tests_usage_direct` only | Bounds the real socket timeouts of a request whose deadline is held still by a fake clock. Without it a request to a gated fake server could block forever. |

A guard may only end a test that has hung. It never decides a pass:

- No test asserts that something finished within a guard.
- A guard's failure message names the event it was waiting for. A
  `HANG_GUARD` failure contains the word `hung`.
- Do not lower a guard to make a hang fail sooner. A real hang taking 10
  minutes to fail is the price of never failing a working test on a stalled
  host.

One helper nests the ceiling: [`stop_until_gone`](#stop_until_gone) calls
`output_within` inside a `wait_until`, so a real hang there takes about 20
minutes to fail. It still uses only `CEILING`. Do not add another nesting.

`limit_resume_fs` is in `amplihack-hooks`, so it reaches `CEILING` by
including `wait.rs` directly with `#[path]` in its integration test.
It does not define a local guard.

## Assert the final state

When a test counts polls or log lines produced in a time window, assert the
state that the behaviour leaves behind instead. For example,
`skip_update_check_flag::launch_with_skip_update_check_completes_without_npm_subprocess_overhead`
used to assert that a launch took under 5 s. It now puts a recording `npm`
stub on `PATH` and asserts that the stub never ran.

This test is a module of the `amplihack` package's `integration` target,
rooted at `tests/integration/main.rs`, which cannot `use` anything under
`crates/amplihack-cli/tests/`. `main.rs` pulls the two shared helper files in
with `#[path]`, the same way it already includes
`tests/support/isolated_amplihack.rs`:

```rust
// tests/integration/main.rs
#[path = "../../crates/amplihack-cli/tests/support/wait.rs"]
mod wait;
#[path = "../../crates/amplihack-cli/tests/support/exec_stub.rs"]
mod exec_stub;
```

Both files therefore use only `std` and refer to no other `support` module.
The test then reaches them as `crate::wait` and `crate::exec_stub`. From the
committed test:

```rust
let record = tmpdir.path().join("npm-calls");
let record_text = record.to_str().expect("temporary paths are UTF-8");
assert!(
    !record_text.contains(['\'', '\n']),
    "cannot single-quote {record_text:?} for /bin/sh"
);
// The record path is written into the stub, so the stub records even if
// launch gives npm a cleared environment.
crate::exec_stub::write_executable(
    &tmpdir.path().join("npm"),
    format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{record_text}'\nexit 1\n"),
    0o755,
);

// PATH contains only the stub dir: the recording npm is the only npm.
let isolated_path = tmpdir.path().to_string_lossy().to_string();

let mut cmd = crate::isolated_amplihack::isolated(&bin);
cmd.env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
    .args(["launch", "--skip-update-check"])
    .env("PATH", &isolated_path)
    .env("AMPLIHACK_NONINTERACTIVE", "1")
    .stdin(std::process::Stdio::null());
let out = crate::wait::output_within("`amplihack launch --skip-update-check`", &mut cmd);

let calls = std::fs::read_to_string(&record).ok();
assert!(
    calls.is_none(),
    "FAIL: `amplihack launch --skip-update-check` ran npm ({}):\n{}\n\
     Check that maybe_print_npm_update_notice respects the skip parameter.\n\
     stderr: {}",
    out.status,
    calls.unwrap_or_default(),
    String::from_utf8_lossy(&out.stderr)
);
```

The test proves only that a launch with the flag completes without running
npm. Its runs have no TTY on stdin, so `is_noninteractive()` already skips
the check. The unit test
`tool_update_check::tests::check_wanted_only_without_skip_in_an_interactive_session`
shows that the flag alone suppresses it.

When you remove an `elapsed` check, the behaviour it guarded must still be
checked another way. Name that check in the PR's per-test table.

## Wait on the condition

`crates/amplihack-cli/tests/support/wait.rs` holds the shared wait helpers.
Integration tests in `crates/amplihack-cli/tests/` reach it as
`crate::support::wait`; lib tests in `amplihack-cli` reach it as
`crate::test_support::wait`; the `amplihack` package's `integration` target
reaches it as `crate::wait`.

| Item | What it does |
|------|--------------|
| `CEILING` | `Duration::from_secs(600)`. The single ceiling for every wait. |
| `wait_until(what, cond)` | Calls `cond` every 50 ms until it returns `true`. Panics at `CEILING`. |
| `output_within(what, &mut cmd)` | Runs `cmd` with stdout and stderr piped and drained on helper threads, and returns its `Output`. Kills and reaps the child at `CEILING`, then panics. |
| `wait_child(what, &mut child)` | Waits for a child you spawned yourself. Kills and reaps it at `CEILING`, then panics. |

`what` names the condition in words. A timeout panic reads like this:

```text
timed out after 600.0s waiting for: the rotator pid file to appear
stderr (last 4 KiB): ...
```

The panic includes at most the last 4 KiB of stderr and never the child's
environment.

Example from `issue_1592_root_rotator.rs`:

```rust
use crate::support::wait;

wait::wait_until("the rotator to log its first cycle", || {
    f.log().contains(" cycle ")
});
```

`wait_until` polls a cheap condition such as a file or a log line. Do not use
it to rerun a heavy check, such as a whole command, on an interval. Wait for
that command's exit instead.

Older local helpers (`wait_for`, `wait_bounded`, `run_bounded`) in
`issue_1592_daemon`, `issue_1592_root_rotator`, `issue_1606_detach` and
`issue_1606_config_cli` are either gone or one-line wrappers around these.
Do not add a new local wait helper with its own ceiling.

`issue_1760_fixture::wait_until` takes only the condition and uses the same
10-minute `CEILING`; it returns `false` when the condition never held.

## Wait on an event

When the test controls both sides, wait on the event itself instead of
polling. These helpers exist for that.

### Channels and barriers

Threads that race start together on a `std::sync::Barrier` and send their
outcome over an `mpsc` channel. The test collects every outcome with
`recv_timeout(CEILING)` and asserts on the outcomes, never on how long the
race took. See [Racing threads](#racing-threads).

### `Gate`

`crate::test_support::test_gate::Gate` holds one side of a test at a known
point until the test lets it go. A fake server's handler calls
`gate.arrive_and_wait()`. The test calls `gate.wait_arrived(what)` to learn
that the handler has been reached, makes its assertions, then calls
`gate.open()`:

```rust
use crate::test_support::test_gate::Gate;

let gate = Gate::new(); // an Arc
let fake = FakeServer::start({
    let gate = gate.clone();
    move |_req| {
        gate.arrive_and_wait();
        Response::json(200, TOKEN_BODY)
    }
});

let reader = std::thread::spawn(move || read_with(&ep, &req, &ctx));
gate.wait_arrived("the token handler to be reached");
assert!(lock_path.exists(), "the refresh must hold the lock while the POST is open");
gate.open();
let r = reader.join().expect("reader thread panicked");
```

`wait_arrived` is bounded by `CEILING`. A gate used out of order, such as
`open()` twice or `wait_arrived` after `open()`, panics with a message that
contains `gate`. `tests_usage_direct` and `rotate::tests_daemon` share it.

### `PtyReader`

`PtyReader::spawn(master)` owns a PTY master, reads it with blocking reads on
its own thread and sends each chunk on an `mpsc` channel. When the read
reports EOF or EIO, because the child closed its side, the thread sends an
explicit end-of-stream message. `reader.wait_for(predicate)` blocks on
`recv_timeout()` until the output so far satisfies `predicate` and returns
it, or returns `Closed` with the captured output once the stream has ended.
Each call uses one absolute `CEILING` deadline; incoming chunks do not reset
it. Reaching the deadline fails as hung and includes the output tail. See
[Fleet TUI tests](#fleet-tui-tests).

### `wait_pid_gone` and `KillOnDrop`

On Linux, `crate::test_support::pidfd_wait` waits for a process the test did
not spawn, such as an orphan left behind by a recipe step:

| Item | What it does |
|------|--------------|
| `parse_pid(text)` | Accepts ASCII digits only. Rejects `0`, `1` and the test's own pid. A missing or bad pid file fails the test. |
| `PidFd::open(pid)` | Opens a pidfd. Call it while the process is known to be alive, so a later reuse of the pid cannot point it at another process. |
| `wait_pid_gone(&pidfd, what)` | Blocks until the process exits, bounded by `CEILING`. |
| `KillOnDrop(pidfd)` | Sends `SIGKILL` to that one process through the pidfd when dropped. Descendants require separate ownership. |

`ESRCH` from `PidFd::open` counts as "gone" only after the teardown under
test has run. Before that it is a failure. These helpers need Linux 5.3 or
later and are `cfg(target_os = "linux")`.

## Change a test input

When the product reads time through a seam, give the seam a slower or fixed
input instead of measuring real time.

| Input | Example |
|-------|---------|
| Time scale | `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE`. `issue_1618_wait_ends::ac5` uses `3`, so the wait's deadline is about 640 s away and cannot end the wait before the login changes. The test moves the login into place itself when the wait writes its first `progress` line (see below). `issue_1801_capacity_wait` uses `10`. A scale never bounds a failing run: if the change does not end the wait, `run_watching` kills the step at `CEILING`. |
| A function parameter | `mermaid_cli::version_probe_succeeds_within(bin, configure, timeout)`. `version_probe_succeeds` calls it with `VERSION_PROBE_TIMEOUT`; the environment-capture test passes 600 s. `codex_plugin` tests call `native_with_timeout` with 600 s. |
| An explicit mtime | The blarify staleness test sets `src/app.py` to now minus 60 s and `blarify.json` to now with `File::set_modified`, instead of sleeping 1 s between writes. |
| A stub | A recording `npm` stub, or the rotator fixture's `FAKE_CLAUDE`. |
| A per-test directory | `issue_1811_governor` gives each test its own governor state directory, so parallel tests never share admissions. |

### Ordering two events

Do not order two events with a fixed sleep. Wait for a line or file that
proves the first event happened, then act. The #1618 fixture's
`run_watching(c, on_line)` runs a step under `CEILING` and calls `on_line`
with each stderr line as it is written. A helper thread reads the lines and
the test thread receives them with `recv_timeout`, so a step that hangs with
stderr still open is killed and reaped at `CEILING`, and the test fails with
`waiting for the step: timed out after …` and the stderr tail.

`issue_1618_wait_ends::ac5` creates the credentials file on the wait's first
`progress` line. That line comes 60 virtual seconds into the wait, after the
first poll has recorded the old login, which the wait needs before it can
see a change. A change made before that poll would be taken as the old
state, and the wait would run to the reset.

`issue_1801_capacity_wait` uses `run_during_wait(f, c, then)`, built on
`run_watching`:

1. It reads the child's stderr line by line.
2. On the line `WAIT_PREFIX + "start reason=usage_limit"` it records the
   current second and signals a writer thread.
3. The writer waits for the signal under `CEILING` and panics with
   `the wait never started` if it does not come.
4. The writer waits until the clock has moved past the recorded second, then
   runs `then`.
5. The writer is joined and its panic, if any, fails the test.

Both return the child's `Output`, with stderr rebuilt from the lines read.

### Starting children together

`issue_1811_governor` starts several children that must contend for the
governor at the same time. Each child's stub writes `ready.N` into the test's
state directory and then blocks reading a FIFO. The test waits for every
`ready.N` file, then opens the FIFO for writing and closes it, which releases
all children at once. It asserts the number of children admitted, not when
they were admitted.

### Comparing times from one clock

`issue_1743_autodrive_budget` compares ledger values with each other, never
with the test's own `Instant::now()`. Every value it compares comes from the
product's clock. Where the test needs to know when a stub step started and
ended, the stub writes start and end marks to files, and the test asserts the
order of those marks.

### Stub files

A stub written in place can fail to start with `ETXTBSY` when another thread
forks while the file is still open for writing. Every stub in `amplihack-cli`,
`amplihack-hooks` and the `amplihack` `integration` target is written by one
function:
`crates/amplihack-cli/tests/support/exec_stub.rs::write_executable(path, body, mode)`.

| Test location | How to reach it |
|---------------|-----------------|
| `crates/amplihack-cli/tests/` and the `amplihack` `integration` target | `exec_stub::write_executable(path, body, mode)`. |
| Unit tests in `amplihack-cli` (`install::clone`'s `fake_git`, `recipe::run`'s `write_stub`, `mermaid_cli`, `codex_plugin`, `path_conflicts::tests`, `fleet::tests`, `fleet::tests_issue_1570_reasoner`) | `crate::test_support::write_executable(path, body)`, a wrapper that includes `exec_stub.rs` with `#[path]` and passes mode `0755`. |
| Unit tests in `amplihack-hooks` (`session_start::blarify`) | The same wrapper in `crates/amplihack-hooks/src/test_support.rs`, which includes `exec_stub.rs` with `#[path]`. |

Other crates still write some stubs in place: `amplihack-utils`
(`launch_target` tests), `amplihack-remote` (`script/prompt_file_tests.rs`)
and `amplihack-memory` (`scip_indexing`). They are not covered by this rule
yet; a follow-up issue tracks moving them to `write_executable`.

A short-lived `sh` writes `<path>.stub-tmp.<pid>`, closes it, sets the mode
and renames it over `path`. The test process never holds a write descriptor
that a fork could inherit, and rewriting a stub that is still running
replaces the directory entry instead of failing with `ETXTBSY`.
`write_executable` asserts `mode & 0o7000 == 0`, so no stub is setuid,
setgid or sticky. There is no retry: a stub that fails to start is a bug.
`output_within` does not retry a spawn that fails with `ETXTBSY` either.

To find a stub writer in `amplihack-cli` or `amplihack-hooks` that bypasses it:

```bash
grep -rn 'set_permissions' crates/amplihack-cli/src crates/amplihack-hooks/src | grep '0o7'
# Output: only write_executable and production code
```

Quote every path in a stub, pass paths as positional arguments, and never
interpolate test input into `sh -c`.

## Test that a timeout fires

Some tests exist to prove that a timeout fires: the recipe runner's hang
test, a lock that gives up. Such a test:

1. Uses an injected clock and advances it, when the product takes one.
2. Otherwise keeps a short product timeout as the trigger only.
3. Asserts the outcome: the "timed out" error, and that every process the
   step started is gone (see [Recipe runner tests](#recipe-runner-tests)).
4. Never asserts an upper bound on elapsed time. A timeout that fires late
   on a busy host is correct behaviour.

`tests_execute.rs` and `tests_teardown.rs` used to assert that the runner
returned within 25 s or 8 s; those assertions are gone. Review remaining
elapsed bounds individually. For a fake that hangs, see
[Bound a timeout without measuring speed](./keep-tests-from-changing-path-and-home.md#bound-a-timeout-without-measuring-speed).

## Inject a clock

When the product itself measures time, such as a lock wait or a request
deadline, a test cannot change that through a parameter or a time scale. The
product reads time through a `Clock` instead, and the test passes a fake one.

### The `Clock` trait

`amplihack_utils::clock` defines the trait and the only implementation that
production code uses:

```rust
use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep(&self, d: Duration);
}

/// Real time: `Instant::now()` and `std::thread::sleep`.
pub struct SystemClock;
```

No environment variable, global or config setting selects a clock. Production
code always passes `SystemClock`, and no fake ships in a library. Each test
module that needs a fake defines its own. A `*_with(.., clock)` function
takes all of its time from the clock it is given.

These seams take a clock:

| Seam | Default path | Test path |
|------|--------------|-----------|
| `amplihack_utils::secure_files` lock wait | `lock_exclusive_timeout(file, timeout)` calls the `_with` form with `&SystemClock` | `lock_exclusive_timeout_with(file, timeout, &clock)` |
| `amplihack_hooks::limit_resume::guard::FsGuard` | `FsGuard::new(root)` uses `LOCK_TIMEOUT` (5 s) and `SystemClock`; `with_lock_timeout(root, t)` uses `SystemClock` | `FsGuard::with_clock(root, timeout, Arc::new(clock))` |
| `amplihack-cli` `profile::usage_direct` read and refresh | `read(ep, req)` and `refresh_expired(ep, req)` pass `Ctx::system()`: `SystemClock` and the process-wide refresh counter | `read_with(ep, req, &ctx)` and `refresh_expired_with(ep, req, &ctx)`, where `ctx` is `Ctx::new(&clock, &refreshing)` |
| `amplihack-cli` `profile::usage_direct` stop path | `wait_for_refreshes(limit)` calls `wait_for_refreshes_on(&REFRESHING, limit, &SystemClock)` | `wait_for_refreshes_on(&counter, limit, &clock)` |

On non-unix targets `lock_exclusive_timeout` and its `_with` form return
`ErrorKind::Unsupported`. They fail closed and never fall back to no lock.

The existing call signatures and defaults do not change in any build. Unit
tests pin each one:

| Default | Value | Pinned by |
|---------|-------|-----------|
| `LOCK_TIMEOUT` | 5 s | `limit_resume_fs::default_lock_timeout_is_five_seconds` |
| `MAX_STEP` | 10 s | `usage_direct` unit test |
| `REFRESH_BUDGET` | 3 s | `usage_direct` unit test |
| Expiry margin | 60 s | `usage_direct` unit tests at 59 s (refreshed) and 61 s (not refreshed) |
| `wait_for_refreshes` poll | 20 ms (`usage_direct::POLL`) | `tests_wait_for_refreshes` |
| `PER_CANDIDATE_PROBE_TIMEOUT`, `TOTAL_PROBE_BUDGET` | 3 s, 10 s | `launch_target` unit tests |

### Frozen and advancing fakes

A fake keeps virtual time as a base `Instant` plus an `AtomicU64` offset in
nanoseconds. No third-party crate is used. There are three kinds:

| Fake | `now()` | `sleep(d)` | Use it when |
|------|---------|------------|-------------|
| Advancing | base + offset | adds `d` to the offset and returns at once | the rule under test is about time, such as "a commit fails only after 5 s" |
| Frozen | base + offset | calls `std::thread::yield_now()`, and leaves the offset alone | elapsed time is not the subject, and a slow host must not run a deadline out |
| `PanicOnSleep` | base + offset | panics | the code under test must return without waiting at all |

A test can also call `advance(d)` on the advancing and frozen fakes to move
virtual time from a fake HTTP handler, so "the token endpoint took 300 ms"
costs no real time.

The frozen fake's `sleep()` yields instead of sleeping. Under contention a
lock wait spins on `yield_now` for as long as another thread holds the lock.
That holder's critical section is short and bounded, so the spin is too; the
fake's doc comment says so.

A frozen clock makes a lock wait unbounded, because virtual time never reaches
the timeout. That is why `FsGuard::with_clock` is documented as test-only, and
why every test that uses a frozen clock collects its results under
[`CEILING`](#the-two-hang-guards).

### The default lock timeout

`limit_resume_fs::default_lock_timeout_is_five_seconds` uses virtual time
only. It holds the lock through another handle, then commits with an
advancing clock:

```rust
assert_eq!(LOCK_TIMEOUT, Duration::from_secs(5));
assert_eq!(FsGuard::new(root).lock_timeout(), LOCK_TIMEOUT);

let clock = AdvancingClock::new(); // an Arc
let mut g = FsGuard::with_clock(root, LOCK_TIMEOUT, clock.clone());
assert!(matches!(g.commit(/* .. */), Err(CommitError::Io)));

// Still waiting at 4.9 s: a retry began at or after 4.9 s.
let (last_start, _) = *clock.sleeps.lock().unwrap().last().unwrap();
assert!(last_start >= Duration::from_millis(4900) && last_start < LOCK_TIMEOUT);
// Gave up only once virtual time reached 5 s.
assert!(clock.elapsed() >= LOCK_TIMEOUT);
```

No real time passes, and the old assertion that the failure took between
4.5 s and 8 s of real time is gone.

### Racing threads

`eight_racing_waiters_for_one_session_wake_it_once` and
`eight_racing_hooks_on_one_event_commit_exactly_once` start eight threads on a
`Barrier`, give each a frozen clock, and send every outcome over an `mpsc`
channel. The test collects them with `recv_timeout(CEILING)` and asserts:

- exactly one wake or commit, and `resumes == 1`;
- every other outcome is `Duplicate` or already handled;
- no outcome is a lock timeout or `Busy`.

The last check replaces the old `start.elapsed() < 3 s`. Its purpose was to
prove that no racer waited out the lock timeout; asserting that no racer
timed out proves the same thing, and with a frozen clock a timeout cannot
happen by accident. The racers contain no real sleep.

### Deadlines in `usage_direct`

`Deadline` reads the injected clock, and the socket step of each request comes
from the pure functions `get_step` and `post_step`. Unit tests check those
functions directly, so the property that a refresh `POST` keeps its full step
needs no slow server. Tests about the deadline move time only through the
fake clock, for example by calling `clock.advance(300 ms)` from a fake handler
instead of sleeping, and assert the outcome, never the elapsed time.

A stalled server is a `Gate` that the test never opens before the deadline.
The test advances the fake clock past the deadline, then opens the gate. The
real ureq socket timeouts are derived from the time left on the injected
clock and bounded by `HANG_GUARD`, so they act only as triggers and cannot
change the outcome. `RETURN_SLACK`, `COUNTER_WAIT` and the old real
`SEND_MARGIN` are gone; the virtual margins are named `V_SEND_MARGIN` and
`V_REFRESH_T`.

These tests run on virtual time and gates or channels only:

- `a_held_lock_is_refresh_busy_and_left_alone`
- `a_set_stop_flag_takes_no_lock_and_sends_no_post`
- `a_401_refreshes_once_writes_back_and_reads_again`
- `a_second_401_after_the_refresh_is_not_refreshed_again`
- `a_401_refresh_and_retry_share_one_deadline`
- `a_refresh_with_the_budget_left_saves_the_rotated_token`
- `a_refresh_answer_with_no_refresh_token_keeps_the_old_one`
- `a_slow_refresh_within_the_deadline_still_reads`
- `a_token_expiring_within_60_seconds_is_refreshed_before_the_first_get`
- `the_refresh_counter_covers_a_refresh_in_progress`

### A counter per test

`usage_direct` counts refreshes in progress so that `wait_for_refreshes` can
wait for them before the process exits. In production that is the
process-wide `REFRESHING`. Tests that ran in parallel shared it, so one test's
refresh could make another test's `wait_for_refreshes` wait.

Each test passes its own counter:

```rust
let counter = AtomicUsize::new(0);
let clock = FakeClock::new(); // moves only through advance()
let req = request_guarded(&login, &[], true);

let r = read_with(&endpoints(&fake), &req, &Ctx::new(&*clock, &counter));

assert_eq!(counter.load(Ordering::SeqCst), 0);
```

`read_uses_the_process_wide_counter` pins the default path: it calls `read()`
with a gated handler and asserts that `refreshes_in_progress()` is at least
1 before it opens the gate. `wait_for_refreshes` has a matching test that
pins it to `REFRESHING`.

### `wait_for_refreshes_on`

```rust
pub(crate) fn wait_for_refreshes_on(
    counter: &AtomicUsize,
    limit: Duration,
    clock: &dyn Clock,
) -> bool
```

It returns `true` when `counter` reaches 0, and `false` when `limit` has
passed on `clock`. A counter already at 0 returns `true` at once without
calling `sleep`, which a `PanicOnSleep` clock proves. On `false` the stop
path behaves as before and does not remove a lock it does not own:
`a_timed_out_wait_removes_no_lock_it_does_not_own` times the wait out on
virtual time with a foreign lock (another inode) in place, and asserts that
the lock is still there and that releasing the old inode leaves it alone.

`rotate::tests_daemon` uses it for the #2036 case, and proves the order, not
only the outcome. A gated token handler holds the refresh. The stopper runs
what `finish_refreshes` runs, `wait_for_refreshes_on` on the process-wide
count (`usage_direct::refresh_counter()`, test-only) with the stop's grace,
on a clock that delegates to `SystemClock` and reports its first `sleep` over
a channel. It sleeps only while the count is above zero, so the report means
the stop is waiting on the held refresh. The test receives it, asserts the
stopper has not finished, opens the gate, asserts the stop returns `true`,
joins the reader, then asserts that no lock file remains.

## Run a private copy of the binary

Some `cli_integration` failures under load were not timeouts. Cargo can
re-link `target/debug/amplihack` while sibling tests are running it, and a
test then fails with `NotFound`, `cp: cannot stat` or a failed runner check.
A test is ready to run the binary once the binary is built and nothing can
replace it.

`crates/amplihack-cli/tests/support/amplihack_bin.rs::amplihack_bin()` gives
that guarantee. On its first call in a test process it:

1. Returns `AMPLIHACK_TEST_BINARY` unchanged if it is set, without copying.
   Do not point it at `target/debug/amplihack`: cargo can re-link that file
   during the run, which is the race the private copy prevents.
2. Otherwise runs `cargo build -p amplihack --bin amplihack` once.
3. Creates a private `tempfile::TempDir` with mode `0700` under
   `CARGO_TARGET_TMPDIR`, or under `env::temp_dir()` when that is unset.
4. Copies `amplihack`, and `amplihack-hooks` when `symlink_metadata` shows it
   is a regular file, into that directory. Each copy is written to
   `.name.tmp` with `create_new`, set to mode `0755`, and renamed into place.

Every later call returns the same path. Nothing re-links the copy while the
test process runs, and the directory is removed when the process exits.
`amplihack-hooks` is copied beside it because the binary
finds its hooks binary as a sibling of its own path.

Use it for every path to the binary, including the ones a test hands to a
script:

```rust
use crate::support::amplihack_bin::amplihack_bin;

let bin = amplihack_bin();
std::os::unix::fs::symlink(&bin, plugin_bin.join("amplihack"))?;
cmd.env("VALIDATION_TEST_CLI", &bin);
```

`codex_frontdoor_contract/fixture.rs` uses it too. Do not build the path from
`std::env::current_exe()` and `../../amplihack`. That path is the shared
build output, and cargo may replace it during the run.

### Python bytecode in the source tree

A second race is not fixed yet. When a test or hook runs Python against files
under `amplifier-bundle/`, Python can write a `__pycache__` directory into the
source tree. The install copy skips git-ignored directories, but the install
completeness check (`real_relative_dirs` in
`crates/amplihack-cli/src/commands/install/verification.rs`) walks every
directory in the source. A concurrent install test then reports the
`__pycache__` directory as missing from the staged bundle and fails.

No test sets `PYTHONDONTWRITEBYTECODE`, because `amplifier-bundle/` on
`main` holds no Python source and nothing in the tree can write a
`__pycache__` there. The race appears once Python modules are added under the
bundle, as the recipe tests in #1554 do. The fix, making the completeness
check skip the same directories as the copy (`is_excluded_dir`) and running
those Python tests with `PYTHONDONTWRITEBYTECODE=1`, is tracked in
[#2017](https://github.com/rysweet/amplihack-rs/issues/2017); the related
staging problem is in
[#1554](https://github.com/rysweet/amplihack-rs/issues/1554).

Until #2017 ships, check by hand before a load run that the tree is clean:

```bash
find amplifier-bundle -name __pycache__
# Output: (nothing)
```

If it prints anything, delete those directories before you start. A clean
tree only removes this one cause of failure. It does not prove the install
check is sound: the completeness check fails open in other cases, so a
passing install test is not evidence that every directory was staged.

## Use a debug-only knob

Add a knob only when no test input can reach the limit. Every knob follows
the pattern in
`crates/amplihack-cli/src/commands/profile/limit_wait/clock.rs`, and new
seconds knobs read their value with `crate::debug_knob::secs`:

- The name starts with `AMPLIHACK_TEST_`. The rotator daemon removes every
  `AMPLIHACK_ROTATE*` variable before it starts, so a knob with that prefix
  would never reach the daemon.
- The name and its parser exist only under `#[cfg(debug_assertions)]`. The
  `#[cfg(not(debug_assertions))]` twin returns the constant, so a release
  binary contains neither the name nor the parser.
- A seconds value is whole seconds. The parser accepts ASCII digits only,
  checked before parsing, in the range `1..=3600`. Anything else (unset,
  empty, `0`, `+5`, ` 5`, Unicode digits, a value above `u64::MAX`) gives the
  default. It never panics and never logs the raw value.
- The knob has a test for the unset default and for invalid values.
- The PR gives the reason no test input could do the job.

### Knobs

| Variable | Default | Read by | Purpose |
|----------|---------|---------|---------|
| `AMPLIHACK_TEST_IDENTITY_BUDGET_SECS` | 30 s (`IDENTITY_BUDGET`) | `cred_identity::identity_budget()`, used by `Budget::available` in `login_identity.rs` | Total time a rotate cycle may spend on account identity reads. |
| `AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS` | 5 s (`IDENTITY_READ_TIMEOUT`) | `cred_identity::identity_read_timeout()`, used by `read_with` | Time one identity read may take. |
| `AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` | 10 s (`tmux::CHILD_TIMEOUT`) | `agent::tmux::child_timeout()`, used for every tmux call, the `systemd-run` scope launch in `server.rs` and the git call in `dirty.rs` | Time each child process of `amplihack agent` may take. |
| `AMPLIHACK_TEST_DETACH_START_SECS` | 15 s | `recipe run --detach` | How long the detach command waits for the pane to start the run. |
| `AMPLIHACK_TEST_PROBE_WAIT` | unset (3 s per candidate, 10 s in total) | `amplihack_utils::launch_target::probe_wait_mode()` | When `exit`, the `--version` probe waits for the candidate to exit. See [Wait for the version probe to exit](#wait-for-the-version-probe-to-exit). |

The two identity knobs and `AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` come
from #1973. The fixtures set them to 600 s, which lengthens a product window
instead of replacing it with an event. They are older exceptions to
[the rule](#the-rule), kept until each is replaced by an event wait. A new
knob must not follow them.

The hand-fix lock bound in `handfix_plan.rs` keeps passing the
`IDENTITY_BUDGET` constant in every build, so the budget knob never changes
it. Its 45 s bound also covers one identity read, so in a debug build the
read-timeout knob can stretch it: with the knob at the fixtures' 600 s, a
debug build can hold the lock for about 30 s + 3 × 600 s. This is part of
the #1973 exception above. In release builds,
and in debug builds with `AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS` unset,
the bound is 45 s.

### Agent child timeout in `issue_1760_fixture`

The `amplihack agent` tests run the binary against a fake tmux, a shell
script found first on `PATH`. The 10 s limit on each tmux call is inside the
binary, and no test input reaches it: on a loaded host a slow fake call ended
as `failed: tmux: …` or as an unknown session state, and the supervisor then
wrote a different result and an extra log line. `Fx::cmd` therefore sets
`AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` to 600 s for every run, one of the
#1973 exceptions listed under [Knobs](#knobs).
The one test of the limit itself,
`issue_1760_agent_tmux_scope::a_systemd_run_timeout_is_treated_as_a_failure`,
removes the knob from its command, so the product's 10 s limit applies and
the fake's 30 s sleep still outlasts it.

Check that a release binary contains no test knob:

```bash
cargo build --release -p amplihack-cli --bin amplihack
strings target/release/amplihack \
  | grep -c -e AMPLIHACK_TEST_IDENTITY_ -e AMPLIHACK_TEST_AGENT_ -e AMPLIHACK_TEST_PROBE_WAIT
# Output: 0
```

The full list of test-only variables is in the
[environment variable reference](../reference/amplihack-env-vars.md).

## Wait for the version probe to exit

`amplihack_utils::launch_target` picks a launch target by running each
candidate with `--version`. Each probe may take 3 s
(`PER_CANDIDATE_PROBE_TIMEOUT`) and all probes together 10 s
(`TOTAL_PROBE_BUDGET`). Tests that launch `amplihack` against a stub agent
binary missed that 3 s window on a loaded host, and the launch then picked no
target.

No test input reaches the probe, so a debug-only knob changes which event
ends it:

```bash
AMPLIHACK_TEST_PROBE_WAIT=exit
```

When the knob is set to exactly `exit`:

- `resolve_from_candidates` reads it once per call. Nothing caches it in a
  global.
- The `TOTAL_PROBE_BUDGET` check is skipped.
- `probe_version` calls `run_capped_output_to_exit`, which waits for the
  candidate to exit and still applies the byte cap on its output.

The candidate limit, the byte cap, the rejection reasons and the candidate
order all still apply. The knob replaces the deadline with the candidate's
exit; it does not set a longer deadline. Any other value, including
`Exit`, `exit ` and `1`, leaves the 3 s and 10 s limits in place. The value is
never logged.

The exit wait is a second capture function, `run_capped_output_to_exit`,
beside the production `run_capped_output_with_timeout`. It is compiled only
in debug builds: in a release build `ProbeWait::Exit`, the knob's name, its
reader and the `to_exit` modules do not exist, so a release binary has one
capture path. The exit wait reads each pipe once per wake, so a grandchild
that writes without pause cannot hold it away from the exit check. Once the
child has exited it reads, once, the bytes each pipe holds at that moment
(`FIONREAD`) and returns, without waiting for an EOF a grandchild may
withhold. Folding the two functions into one is left to a follow-up issue,
because it would change the release capture path that every launch uses.

`AMPLIHACK_TEST_PROBE_WAIT` is a test-only knob and exists only in debug
builds. Do not set it in a shell: with it set, a debug `amplihack` has no
bound on the `--version` probe, and a candidate that hangs on `--version`
hangs the launch.

`isolated(..)` sets the knob on every child after `env_clear`, in both
`tests/support/isolated_amplihack.rs` and the `amplihack-cli` test support,
without widening its allow-list. These tests depend on it:

| Test | Cause confirmed |
|------|-----------------|
| `issue_1659_limit_resume_gate` (2 tests) | the 3 s probe of the stub agent |
| `issue_1870_*` (3 tests) | the 3 s probe of the stub agent |
| `codex_frontdoor_contract` (2 tests) | the 3 s probe, and the binary path (now `amplihack_bin()`) |
| `claude_code_plugin_manifest::shell` | the 3 s probe of the stub agent |

Each of these tests waits for its child with `support::wait::output_within`
or `wait_child`, bounded only by `CEILING`.

## Recipe runner tests

### `fixture_environment()` and `EnvGuard`

Each `commands::recipe::run` test module has a `fixture_environment()`
helper. A test takes the crate's `home_env_lock()` first and then calls it:

```rust
let _guard = crate::test_support::home_env_lock()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
let mut fixture_env = fixture_environment();
fixture_env.assign("RECIPE_RUNNER_RS_PATH", &runner);
fixture_env.assign("AMPLIHACK_HOME", &amplihack_home);
fixture_env.clear("AMPLIHACK_STEP_TIMEOUT");
```

The helper returns a `crate::test_support::EnvGuard`. `EnvGuard::assign`
and `EnvGuard::clear` record a variable's value the first time the guard
touches it, and the guard restores every recorded variable when it is
dropped, panics included. There is no manual restore block, and no bare
`set_var` or `remove_var` remains in `tests_execute.rs`.

In `tests_teardown.rs` the helper also gives each fixture its own temp
`HOME`, a `TempDir` named `amplihack-recipe-home-*`. The fixture owns it:
dropping the fixture restores the variables and then removes the directory,
so no recipe test reads the real `~/.amplihack` and none leaves a home
directory behind.

### A runner that hangs

The timeout fixtures block on shell builtin FIFO reads. Their
`unix_process::FifoWorker` runs the executor on a worker thread and receives
its result with `recv_timeout(CEILING)`, independently of the product's
one-second timeout. Passing requires a "timed out" error; the process-tree
test also waits for every process carrying its unique exec-time environment
token to disappear and asserts that the orphan marker is absent.

The supervisor opens nonblocking read and write handles before starting the
worker, and writes nothing until drop. It stays alive through the descendant
assertions, so releasing a survivor cannot hide broken termination. On
success or panic, drop writes one newline per builtin read, allowing blocked
shells to exit without product termination code. A tree fixture opens its
FIFO descriptor before forking, so late descendants retain the release.
Worker-held handles also retain buffered release lines if cleanup starts
before the runner does. Drop waits up to `CEILING` for an outstanding worker
to return before restoring the test's environment and removing its tempdir;
a worker still stuck after cleanup is reported without an unbounded join.
There is no `done` file or elapsed-time pass threshold.

### Teardown leaves no orphan

`tests_teardown.rs` proves that teardown kills a step's orphans:

1. The runner starts an orphan shell that writes its own pid and blocks on
   a builtin `read` opening a FIFO. There is no separate `cat` descendant.
2. The test waits for the pid file, parses it with `parse_pid`, opens a
   pidfd while the orphan is alive, and wraps it in `KillOnDrop`.
3. Teardown runs.
4. The test calls `wait_pid_gone` and asserts the orphan's marker file does
   not exist.

The orphan's `KillOnDrop` signals the same shell that owns the blocked FIFO
open, so failure cleanup cannot leave a separate reader behind. The scoped
teardown test also owns an unrelated sibling `cat` directly and reaps it on
every path. The orphan stub has no fallback writer. The 2.3 s sleep and the
`< 8 s` assertion are gone.

## Fleet TUI tests

The fleet TUI tests in `tests/integration/no_python_probe_test.rs` run the
TUI in a PTY. `FleetTuiProbe` hands the PTY master to a
[`PtyReader`](#ptyreader) and keeps a second handle for keystrokes. Every key
a test sends is followed by a wait for the screen text that key produces:

```rust
let mut probe = FleetTuiProbe::new(&bin, None);
probe.wait_for_text("q quit");                      // the first frame
probe.send_and_wait_for(b"d", "Prepared proposal for vm-1/claude-1:");
probe.send_and_wait_for(b"\x1b", "[fleet]");        // Esc back to Fleet
let cleaned = probe.finish();                       // q, read to EOF, reap
```

- `send_and_wait_for` looks only at the text drawn after the key, so text
  already on the screen cannot satisfy it. Choose text that the key's new
  state adds, such as the active tab label (`[detail]`, `[fleet]`) or a
  status line.
- `send_and_wait_for_frame` waits for a whole frame drawn after the key. Use
  it when the key removes text, such as closing the help overlay.
- A lone `Esc` is sent on its own and its effect awaited before the next key,
  so the TUI cannot read it as the start of an escape sequence.
- Files the TUI's helpers write, such as the azlin shim's logs and the task
  queue, are written before the status line that reports them, so they are
  read once after that line appears.
- `finish` sends `q`, reads to the end of the stream and reaps the child.

The probe holds its child in `KillOnDrop`: a failed test kills the TUI, the
PTY closes and the reader thread ends. Each output or EOF wait has an
absolute `CEILING` deadline; a hung TUI fails that wait and is killed by the
probe's drop guard.

`read_until_quiet`, `drain(duration)`, `poll_until_output_contains`,
`poll_file_for_content`, `wait_for_exit` and their 20 ms and 50 ms sleeps are
gone. So is the fixture's 2 s delay on the second VM: the frame drawn while
that VM is polled is captured however long the poll takes.

## Rotator tests

`crates/amplihack-cli/tests/support/profile_rotate.rs` gives every rotator
test load-safe defaults.

### Identity knobs in `Fixture::base`

`Fixture::base` clears the environment and then sets both identity knobs to
`600`. This is one of the #1973 exceptions listed under [Knobs](#knobs). Every command a rotate test runs therefore has a 10-minute identity
budget, and a slow host no longer produces "identity read budget used up".

A test of budget or read-timeout exhaustion must remove both knobs from its
command, or set a small value:

```rust
let mut cmd = f.cmd(&["profile", "rotate"]);
cmd.env_remove("AMPLIHACK_TEST_IDENTITY_BUDGET_SECS")
    .env_remove("AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS");
```

Otherwise the test would wait 10 minutes, or stop checking the limit at all.

### `IdentityEndpoint`

The fake identity server binds `127.0.0.1:0` and gives each connection its
own thread. A read error or early EOF ends that connection only, so a slow
client no longer panics the server's single worker and breaks every later
read. It never echoes request data. Dropping the endpoint stops and joins the
acceptor.

### `stop_until_gone`

`profile rotate stop` refuses with a documented message when the daemon does
not stop within 10 s, which can happen on a loaded host.
`f.stop_until_gone(pid)` retries `stop` through `output_within` while it gets
that refusal, panics on any other failure, then waits until `pid` is gone. It
returns the last successful `Output` and whether any attempt was refused.

The `rotate: …` lines come from the `stop` command's stdout
(`control.rs`), not from the daemon log. Without a refusal, the one `stop`
prints `rotate: stopped (pid N)`. After a refusal the daemon may exit before
the retried `stop` runs, so that `stop` prints any one of
`rotate: stopped (pid N)`, `rotate: not running` or
`rotate: removed a stale pid file`. Assert on stdout in both branches, and
in both assert that the pid file is gone:

`f.assert_stopped(&out, refused, pid)` makes those checks:

```rust
let (out, refused) = f.stop_until_gone(pid);
f.assert_stopped(&out, refused, pid);
```

A test that runs `stop` from a shell whose root resolves elsewhere uses
`f.stop_until_gone_by(pid, || f.cmd_no_root_env(&["profile", "rotate", "stop"]))`.

`stop_until_gone` has already waited for `pid` to exit before it returns.
Each attempt is an `output_within` call with its own ceiling, inside a
`wait_until` with another, so a real hang can take about 20 minutes to fail.
This is the one known nesting of `CEILING` (see
[The two hang guards](#the-two-hang-guards)).

It signals nothing itself and never reads a pid from the pid file to signal.
`a_stubborn…` still asserts a single refusal and does not use it.

### Holding the first probe

`f.hold_probes()` makes `FAKE_CLAUDE`'s `/usage` call wait on a marker file;
`f.release_probes()` removes the marker. A drop guard removes it if the test
panics. `issue_1592_daemon::start_status_and_stop` uses it to check that
`start` returns before the first cycle, without timing it:

1. Hold probes.
2. Run `profile rotate start` through `output_within`.
3. Wait for a probe environment record.
4. Assert there is no ` cycle ` line in the log yet.
5. Release probes.

## Checks that something does not happen

A test that a thing does not happen cannot wait for it. Run the actor to an
event first, then assert that the thing did not happen:

| Check | Event to wait on first |
|-------|------------------------|
| "no `POST` was sent" | join the reader thread |
| "the lock was left alone" | join the thread, or receive its "done" message |
| "the orphan was killed" | `wait_pid_gone` on its pidfd |
| "the step wrote nothing" | reap the step's exit with `wait_child` |

A sleep followed by "nothing happened" is not allowed: on a loaded host the
sleep can end before the actor has done anything, and the test passes while
proving nothing. Prefer a positive marker, a later event that proves the
earlier one was skipped.

Sleeps that remain because the rule under test is about time, such as
`Fixture::wait_past_damping`, are listed in the PR that keeps them, with a
follow-up issue.

## Review event waits

Review tests for success conditions based on real elapsed time. Prefer an
observable event, and bound missing events with the shared hang guard.
There is no repository source scanner or shrinking exception allowlist:
review the behavior and synchronization of each changed test directly.

## What not to do

- Do not mark a test `#[ignore]`, pass `--skip`, or exclude it in the
  pre-push hook.
- Do not raise a timeout, sleep, delay or retry count to make a test pass.
- Do not add retries, lower parallelism or split the test run in the hook.
- Do not delete a test or weaken an assertion. Removing an `elapsed` check is
  allowed only when another check proves the behaviour.
- Do not add a fixed sleep to order two events, or before a negative check.
- Do not add a hang guard beyond `CEILING` and `HANG_GUARD`, and do not
  assert that something finished within one.
- Do not select a fake clock from an environment variable, global or config
  setting. Pass it through a `_with` function or constructor.
- Do not change a product timeout's default.
- Do not write a stub with `fs::write` and `set_permissions`. Use
  `write_executable`.
- Do not call `set_var` in a test. Use `EnvGuard` (`assign`, `clear`) under
  the crate's lock, or pass the value as a parameter.
- Do not pause, cap or renice other sessions to make a run pass.
- Do not signal a process by `pkill -f`, `killall` or a pid read from a file
  without a pidfd. Signal only processes the test spawned, or a pidfd opened
  while the process was known to be alive.

## Validate a fix

Run the tests affected by the change and the repository's normal required
checks. Synthetic CPU load runs and three consecutive workspace passes are
not required. Do not generate host load as part of this validation.

For timing policy, use the same fixture with an injected clock and change
only the configured setting. Check both the allowed completion and the
configured escalation limit. Verify the test fails if the implementation
ignores the setting.

For event waits, check the success event, stream closure and missing-event
paths. A repeated redraw must not reset the hang deadline. When replacing a
running stub, wait for an explicit readiness event from its original body
before replacing the path; bound readiness and exit waits and clean up the
child on failure.

Record the commands and results in the PR. Push normally so the repository's
hooks run. A passing run alone does not establish a load failure's cause:
explain the synchronization or wall-clock dependency that the change fixes.

## Related

- [Keep Tests from Changing PATH and HOME](./keep-tests-from-changing-path-and-home.md) - Testing a timeout without measuring host speed
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The stops `isolated(..)` sets
- [Give Test Logins a Fake Account Identity](./give-test-logins-a-fake-account-identity.md) - The rotate fixture's identity server
- [Wait Without Leaving Orphan Processes](./wait-without-orphan-processes.md) - Waiting from the shell
- [AMPLIHACK_* Environment Variable Audit](../reference/amplihack-env-vars.md) - Every test-only variable
- [CI Pipeline Reference](../reference/ci-pipeline.md) - How CI runs the tests with `cargo nextest`
