---
title: Keep Tests Passing Under Host Load
type: howto
updated: 2026-11-05
related: issue #1973, issue #1765, issue #1961
---

# Keep Tests Passing Under Host Load

A test that measures wall-clock time also measures how busy the host is. On a
loaded host such a test fails, and the next run fails on a different subset,
so a push can take several attempts. This guide is for contributors who write
or change tests in `amplihack-cli` and `amplihack-hooks`. It explains how to
write a test that host speed cannot decide, which shared helpers and test
knobs to use, and how to prove a fix under CPU load.

## Contents

- [The rule](#the-rule)
- [Assert the final state](#assert-the-final-state)
- [Wait on the condition](#wait-on-the-condition)
- [Change a test input](#change-a-test-input)
- [Use a debug-only knob](#use-a-debug-only-knob)
- [Rotator tests](#rotator-tests)
- [Checks that something does not happen](#checks-that-something-does-not-happen)
- [What not to do](#what-not-to-do)
- [Prove a fix under load](#prove-a-fix-under-load)
- [Limits of a CPU-only proof](#limits-of-a-cpu-only-proof)
- [Related](#related)

## The rule

No test passes or fails because of how long something took, unless the rule
under test is itself about time. Remove a wall-clock dependency with the first
method in this list that works:

1. Assert the final state.
2. Wait on the condition with `wait_until`, under the 10-minute ceiling.
3. Change a test input: a stub, a time scale, a function parameter or an
   explicit mtime.
4. Add a debug-only knob, as a last resort.

Product timeouts and budgets keep their defaults in every build. A test
changes only its own inputs.

Wall-clock dependencies to look for:

| Dependency | Example before #1973 |
|------------|----------------------|
| An exit timeout | "amplihack did not exit within 60 s" |
| A budget measured from real time | the 30 s identity budget, measured from the start of a rotate cycle |
| A fixed sleep used to order two events | `sleep(Duration::from_millis(1200))` before writing a file |
| An `elapsed < N` assertion | `elapsed < 5 s` after `--skip-update-check` |
| A product timeout no test input reaches | the 10 s limit on each tmux, `systemd-run` and git call of `amplihack agent`, run against a shell-script fake tmux |

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
`crate::support::wait`; the `amplihack` package's `integration` target
reaches it as `crate::wait` through the `#[path]` line shown in
[Assert the final state](#assert-the-final-state).

| Item | What it does |
|------|--------------|
| `CEILING` | `Duration::from_secs(600)`. The single ceiling for every wait. |
| `wait_until(what, cond)` | Calls `cond` every 50 ms until it returns `true`. Panics at `CEILING`. |
| `output_within(what, &mut cmd)` | Runs `cmd` with stdout and stderr piped and drained on helper threads, and returns its `Output`. Retries a spawn that fails with `ExecutableFileBusy` up to 50 times, 20 ms apart. Kills and reaps the child at `CEILING`, then panics. |
| `wait_child(what, &mut child)` | Waits for a child you spawned yourself, with `try_wait`. Kills and reaps it at `CEILING`, then panics. |

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

A real hang now takes up to 10 minutes to fail. That is the price of never
failing a working test on a stalled host. A ceiling longer than any expected
stall is the point; do not shorten it to make a hang fail sooner.

Older local helpers (`wait_for`, `wait_bounded`, `run_bounded`) in
`issue_1592_daemon`, `issue_1592_root_rotator`, `issue_1606_detach` and
`issue_1606_config_cli` are either gone or one-line wrappers around these.
Do not add a new local wait helper with its own ceiling.

`issue_1760_fixture::wait_until` takes only the condition and uses the same
10-minute `CEILING`; it returns `false` when the condition never held.

## Change a test input

When the product reads time through a seam, give the seam a slower or fixed
input instead of measuring real time.

| Input | Example |
|-------|---------|
| Time scale | `AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE`. `issue_1618_wait_ends::ac5` uses `3`, so the wait's deadline is about 640 s away and cannot end the wait before the login changes. The test moves the login into place itself when the wait writes its first `progress` line (see below). `issue_1801_capacity_wait` uses `10`. A scale never bounds a failing run: if the change does not end the wait, `run_watching` kills the step at `CEILING`. |
| A function parameter | `mermaid_cli::version_probe_succeeds_within(bin, configure, timeout)`. `version_probe_succeeds` calls it with `VERSION_PROBE_TIMEOUT`; the environment-capture test passes 600 s. `codex_plugin` tests call `native_with_timeout` with 600 s. |
| An explicit mtime | The blarify staleness test sets `src/app.py` to now minus 60 s and `blarify.json` to now with `File::set_modified`, instead of sleeping 1 s between writes. |
| A stub | A recording `npm` stub, or the rotator fixture's `FAKE_CLAUDE`. |

Tests that check a timeout itself keep their short timeout and a generous
upper bound. `version_probe_is_bounded_for_hanging_binaries` still uses
500 ms and asserts under 30 s; see
[Bound a timeout without measuring speed](./keep-tests-from-changing-path-and-home.md#bound-a-timeout-without-measuring-speed).

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

### Stub files

A stub written in place can fail to start with `ETXTBSY` when another thread
forks at the same moment. Which fix applies depends on where the test lives:

| Test location | Fix |
|---------------|-----|
| `crates/amplihack-cli/tests/` and the `amplihack` `integration` target | `exec_stub::write_executable(path, body, mode)`. |
| Unit tests inside a crate that has a `test_support` module (`mermaid_cli` and `codex_plugin` in `amplihack-cli`, `session_start::blarify` in `amplihack-hooks`) | `crate::test_support::write_executable(path, body)`, a wrapper that includes `exec_stub.rs` with `#[path]` and passes mode `0755`. |

There is one writer, `crates/amplihack-cli/tests/support/exec_stub.rs`. A
short-lived `sh` writes `<path>.stub-tmp.<pid>`, sets the mode and renames it
over `path`. The test process never holds a write descriptor that a fork
could inherit, and rewriting a stub that is still running replaces the
directory entry instead of failing with `ETXTBSY`.
| Anywhere neither is possible | Retry the spawn once when it fails with `ErrorKind::ExecutableFileBusy`. |

Quote every argument in the stub and never interpolate test input into
`sh -c`.

## Use a debug-only knob

Add a knob only when no test input can reach the limit. Every knob follows
the pattern in
`crates/amplihack-cli/src/commands/profile/limit_wait/clock.rs`, and new
knobs read their value with `crate::debug_knob::secs`:

- The name starts with `AMPLIHACK_TEST_`. The rotator daemon removes every
  `AMPLIHACK_ROTATE*` variable before it starts, so a knob with that prefix
  would never reach the daemon.
- The name and its parser exist only under `#[cfg(debug_assertions)]`. The
  `#[cfg(not(debug_assertions))]` twin returns the constant, so a release
  binary contains neither the name nor the parser.
- The value is whole seconds. The parser accepts ASCII digits only, checked
  before parsing, in the range `1..=3600`. Anything else (unset, empty, `0`,
  `+5`, ` 5`, Unicode digits, a value above `u64::MAX`) gives the default.
  It never panics and never logs the raw value.
- The knob has a test for the unset default and for invalid values.
- The PR gives the reason no test input could do the job.

### Knobs

| Variable | Default | Read by | Purpose |
|----------|---------|---------|---------|
| `AMPLIHACK_TEST_IDENTITY_BUDGET_SECS` | 30 s (`IDENTITY_BUDGET`) | `cred_identity::identity_budget()`, used by `Budget::available` in `login_identity.rs` | Total time a rotate cycle may spend on account identity reads. |
| `AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS` | 5 s (`IDENTITY_READ_TIMEOUT`) | `cred_identity::identity_read_timeout()`, used by `read_with` | Time one identity read may take. |
| `AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` | 10 s (`tmux::CHILD_TIMEOUT`) | `agent::tmux::child_timeout()`, used for every tmux call, the `systemd-run` scope launch in `server.rs` and the git call in `dirty.rs` | Time each child process of `amplihack agent` may take. |
| `AMPLIHACK_TEST_DETACH_START_SECS` | 15 s | `recipe run --detach` | How long the detach command waits for the pane to start the run. |

The hand-fix lock bound in `handfix_plan.rs` keeps passing the
`IDENTITY_BUDGET` constant in every build, so the budget knob never changes
it. Its 45 s bound also covers one identity read, so in a debug build the
read-timeout knob can stretch it: with the knob at the fixtures' 600 s, a
debug build can hold the lock for about 30 s + 3 × 600 s. In release builds,
and in debug builds with `AMPLIHACK_TEST_IDENTITY_READ_TIMEOUT_SECS` unset,
the bound is 45 s.

### Agent child timeout in `issue_1760_fixture`

The `amplihack agent` tests run the binary against a fake tmux, a shell
script found first on `PATH`. The 10 s limit on each tmux call is inside the
binary, and no test input reaches it: on a loaded host a slow fake call ended
as `failed: tmux: …` or as an unknown session state, and the supervisor then
wrote a different result and an extra log line. `Fx::cmd` therefore sets
`AMPLIHACK_TEST_AGENT_CHILD_TIMEOUT_SECS` to the 600 s ceiling for every run.
The one test of the limit itself,
`issue_1760_agent_tmux_scope::a_systemd_run_timeout_is_treated_as_a_failure`,
removes the knob from its command, so the product's 10 s limit applies and
the fake's 30 s sleep still outlasts it.

Check that a release binary contains no test knob:

```bash
cargo build --release -p amplihack-cli --bin amplihack
strings target/release/amplihack | grep -c -e AMPLIHACK_TEST_IDENTITY_ -e AMPLIHACK_TEST_AGENT_
# Output: 0
```

The full list of test-only variables is in the
[environment variable reference](../reference/amplihack-env-vars.md).

## Rotator tests

`crates/amplihack-cli/tests/support/profile_rotate.rs` gives every rotator
test load-safe defaults.

### Identity knobs in `Fixture::base`

`Fixture::base` clears the environment and then sets both identity knobs to
`600`. Every command a rotate test runs therefore has a 10-minute identity
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

A test that a thing does not happen cannot wait on a condition. Prefer a
positive marker: a later event that proves the earlier one was skipped. When
no marker exists, keep a short window. Load can make such a window miss a
fault but never fail a working test.

Kept windows:

| Test | Window |
|------|--------|
| `issue_1592_daemon::watch_follows_the_auto_switch_toggle` | 3 s with no cycle after the toggle is turned off |
| `issue_1592_daemon::a_foreign_process…` | 200 ms before checking that the foreign process is still alive |

Sleeps that remain because the rule under test is about time, such as
`Fixture::wait_past_damping`, are listed in the PR that keeps them.

## What not to do

- Do not mark a test `#[ignore]`, pass `--skip`, or exclude it in the
  pre-push hook.
- Do not add retries, lower parallelism or split the test run in the hook.
- Do not delete a test or weaken an assertion. Removing an `elapsed` check is
  allowed only when another check proves the behaviour.
- Do not add a fixed sleep to order two events.
- Do not change a product timeout's default.
- Do not pause, cap or renice other sessions to make a run pass.
- Do not signal a process by `pkill -f`, `killall` or a pid read from a file.
  Signal only processes the test spawned.

## Prove a fix under load

The pre-push hook runs `cargo test`, which shares one process per test
target; CI runs `cargo nextest`, which does not. Build first, then run the
test binaries directly so the cargo lock does not queue them.

1. Build the test binaries without running them and note each `Executable`
   path:

   ```bash
   export TMPDIR=<scratch dir>/tmp/issue-1973
   mkdir -p -m 0700 "$TMPDIR"
   cargo test --no-run -p amplihack-cli --lib --test cli_integration 2>&1 | grep Executable
   cargo test --no-run -p amplihack-hooks --lib 2>&1 | grep Executable
   cargo test --no-run -p amplihack --test integration 2>&1 | grep Executable
   ```

   Replace `<scratch dir>` with a directory on a file system with room for
   the tests' temporary files. Keep `/tmp` in the path: some pre-push tests
   walk up from their temporary directory and expect `/tmp` in it.

   `skip_update_check_flag` and `recipe_e2e` are modules of the `amplihack`
   package's `integration` target. In the commands below, `$CLI_INTEGRATION`,
   `$CLI_LIB`, `$HOOKS_LIB` and `$INTEGRATION` stand for the `Executable`
   paths these commands print.

2. Confirm the filters select exactly the tests listed in the PR (R1), and
   that none of them is ignored. Save R1, one `name: test` line per test, as
   `$TMPDIR/r1-cli.txt`, then compare:

   ```bash
   filters=(issue_1592_ issue_1606_ issue_1618_ issue_1634_
     issue_1760_ issue_1801_capacity_wait issue_1569_auto_switch
     codex_frontdoor_contract)
   "$CLI_INTEGRATION" --list "${filters[@]}" | grep ': test$' | sort \
     | diff - <(sort "$TMPDIR/r1-cli.txt")
   "$CLI_INTEGRATION" --list --ignored "${filters[@]}" | grep ': test$'
   ```

   The `diff` must print nothing, and the `--ignored` listing must print
   nothing. Do the same for each other binary with its own filters.

3. Run the binaries under load with the script below. It stops before
   starting if `/` or the file system holding `$TMPDIR` is more than 97% full. It then starts one busy
   loop per CPU at the tests' own nice level, logs `uptime` every minute,
   runs all binaries at the same time with the default test threads, and
   stops everything when they finish or when it is interrupted:

   ```bash
   #!/usr/bin/env bash
   set -u
   for mount in / "$TMPDIR"; do
     used=$(df --output=pcent "$mount" | tail -n 1 | tr -dc 0-9)
     if [ "$used" -gt 97 ]; then
       echo "stop: $mount is ${used}% full" >&2
       exit 2
     fi
   done

   out="$TMPDIR/proof-$(date -u +%Y%m%dT%H%M%SZ)"
   mkdir -p -m 0700 "$out"
   pids=()
   cleanup() {
     trap - EXIT INT TERM
     [ "${#pids[@]}" -gt 0 ] && kill "${pids[@]}" 2>/dev/null
     wait 2>/dev/null
   }
   trap cleanup EXIT
   trap 'cleanup; exit 130' INT
   trap 'cleanup; exit 143' TERM

   for _ in $(seq "$(nproc)"); do ( while :; do :; done ) & pids+=($!); done
   ( while :; do uptime; sleep 60; done ) >"$out/uptime.log" & pids+=($!)

   tests=()
   "$CLI_INTEGRATION" >"$out/cli.log" 2>&1 & tests+=($!)
   "$CLI_LIB" install:: >"$out/cli-lib.log" 2>&1 & tests+=($!)
   "$HOOKS_LIB" blarify >"$out/hooks.log" 2>&1 & tests+=($!)
   "$INTEGRATION" skip_update_check recipe_e2e >"$out/integration.log" 2>&1 & tests+=($!)
   pids+=("${tests[@]}")

   rc=0
   for t in "${tests[@]}"; do wait "$t" || rc=1; done
   grep -h '^test result' "$out"/*.log
   exit "$rc"
   ```

   The handlers kill only the processes the script started, the test
   binaries included, and an interrupt ends the script instead of returning
   to the `wait` loop.

4. Run it twice. Each `test result` line must show `0 failed` and
   `0 ignored`. Put the commands, the `uptime` readings and the summaries in
   the PR, after removing tokens, email addresses and home-directory paths.
   Do not attach full logs.

5. Push normally, without `--no-verify`. The pre-push hook's full run is
   further evidence.

## Limits of a CPU-only proof

Some failures on the build host came from a host-wide stall, which CPU load
alone may not reproduce. The ceilings are therefore set long enough to
outlast a stall rather than tuned to the load used in the proof. A test that
does not fail under load in the baseline run is fixed only where the code
shows a wall-clock dependency, and its PR row says "not reproduced; cause not
confirmed".

## Related

- [Keep Tests from Changing PATH and HOME](./keep-tests-from-changing-path-and-home.md) - Testing a timeout without measuring host speed
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The stops `isolated(..)` sets
- [Give Test Logins a Fake Account Identity](./give-test-logins-a-fake-account-identity.md) - The rotate fixture's identity server
- [Wait Without Leaving Orphan Processes](./wait-without-orphan-processes.md) - Waiting from the shell
- [AMPLIHACK_* Environment Variable Audit](../reference/amplihack-env-vars.md) - Every test-only variable
- [CI Pipeline Reference](../reference/ci-pipeline.md) - How CI runs the tests with `cargo nextest`
