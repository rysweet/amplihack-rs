---
title: Keep Tests off the Host Process Table
type: howto
updated: 2026-11-11
related: issue #2042, issue #2040, issue #1760, issue #1690, issue #2175
---

# Keep Tests off the Host Process Table

`amplihack profile` commands look at running processes. Auto-switch liveness
reads `/proc/<pid>/status`, `environ` and `cmdline` to find Claude sessions
that use a profile. `profile rotate` reads `/proc/<pid>/cmdline` and `exe` to
find a running rotator. `sessions` reads `/proc` to find live sessions.

Before #2042, every `cli_integration` test that ran one of these commands
scanned the real host's process table. On a busy host that meant hundreds of
test children each reading every other process's `cmdline` and `environ`, and
a test result that depended on whatever else was running.

Now every `amplihack profile`, `sessions` and `cleanup` command a test starts
reads a fake process table that the test owns. This guide shows how to write such a test, how to add fake
processes, and what the guard test checks. It is for contributors who write
tests in `crates/amplihack-cli/tests/`.

## Contents

- [How the seam works](#how-the-seam-works)
- [Write a test that runs `amplihack profile`](#write-a-test-that-runs-amplihack-profile)
- [Keep the fake tree after `env_clear()`](#keep-the-fake-tree-after-env_clear)
- [Add fake processes with `FakeProc`](#add-fake-processes-with-fakeproc)
- [Show a process the test started](#show-a-process-the-test-started)
- [What the guard test checks](#what-the-guard-test-checks)
- [The strace check](#the-strace-check)
- [What still reads the real `/proc`](#what-still-reads-the-real-proc)
- [Troubleshooting](#troubleshooting)

## How the seam works

All `/proc` reads in profile and sessions code go through one function,
`commands::agent::procfs::proc_root()`. It returns `/proc`, or the directory
named in `AMPLIHACK_TEST_PROC_ROOT` when every one of these holds (rule S27 in
`procfs::resolve`, in `crates/amplihack-cli/src/commands/agent/procfs/root.rs`):

| Condition | Why |
|-----------|-----|
| `AMPLIHACK_TEST_HOME_GUARD` is set and not empty | Only the test harness sets it. |
| The path, after `canonicalize`, lies inside the canonical temp dir and is not the temp dir itself | A user's real directories are never used. |
| It is a directory owned by the effective user | Another user cannot plant a tree. |
| No other user can write to it | Another user cannot change a tree after it is checked. |

If any condition fails, the code reads the real `/proc`. A debug build, which
is what every test runs, also prints one line to stderr, once per process:

```text
amplihack: AMPLIHACK_TEST_PROC_ROOT ignored (others can write the directory); reading /proc
```

The reason is one of: the test home guard is not set, the path does not
exist, the path is not inside the temp dir, the path is not a directory,
another user owns the directory, or others can write the directory. An unset
or empty variable reads `/proc` with no line. Release builds print nothing.
A test that does not look at stderr can still miss the line, which is why
the guard test described below also checks the test side.

The seam is compiled into every build, release included. It gives no new
power: using it needs the home guard and a private directory you already own.

#2042 routes 10 call sites in 9 files through `proc_root()`. Each one passed
a literal `Path::new("/proc")` to a function that already takes a root:

| Call site under `crates/amplihack-cli/src/commands/` | Reads |
|------------------------------------------------------|-------|
| `profile/auto_switch/liveness.rs` (`scan`) | `status`, `environ`, `cmdline`, `cwd` of each pid; `self/environ` |
| `profile/rotate/control.rs` (`owner::describe`) | the owner marker pid's `cmdline` and `exe` |
| `profile/rotate/owner.rs` (`describe`) | the same, for `rotate owner` |
| `profile/rotate/daemon.rs` (`state_at` in `current_state`) | the rotator pid's `cmdline` and `exe` |
| `profile/rotate/autostart.rs` (`Deps { proc_root }`) | the rotator pid |
| `profile/rotate/change_log.rs` (`caller`) | the parent pid's `cmdline` |
| `profile/rotate/ledger.rs` (`foreign_rotator_at`) | the rotator pid named in the pid file |
| `profile/rotate/mod.rs` (`foreign_note_for`) | the same, for the foreign-rotator note |
| `sessions/live.rs` (`claude_config_dirs`) | `environ` of each pid, for live Claude sessions |
| `sessions/live.rs` (`process_dirs` error path) | nothing; the error names the root that was read |

The other `daemon.rs` site, in `watch` (`owner::supervised` and
`owner::unit_name`, which read `self/cgroup`), already calls `proc_root()`.
Agent code (`agent/pass.rs`, `register.rs`, `wrapper.rs`) was routed for
#1760.

`#2040` adds one cached process snapshot per production pass. That snapshot
reads through the same `proc_root()`, so tests stay on the fake tree.

## Write a test that runs `amplihack profile`

Start the binary with `isolated(..)` from `tests/support/isolated_amplihack.rs`.
Besides the temp `HOME`, the home guard and the autostart opt-out, it sets
`AMPLIHACK_TEST_PROC_ROOT` (`PROC_ROOT_ENV`) to a shared empty tree:

```rust
use crate::isolated_amplihack::isolated;

let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.args(["profile", "auto-switch", "status"]);
let out = cmd.output().unwrap();
assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
```

The shared tree comes from `shared_empty_proc_root()`. It is created once per
test binary at `<shared_temp_home()>/fake-proc` and removed at exit. It holds
exactly one file:

```text
fake-proc/            mode 0700
└── self/             mode 0700
    └── environ       mode 0600, contents b"AMPLIHACK_FAKE=1\0"
```

With this tree:

- the liveness scan succeeds and finds no processes, so no profile is live
  (without `self/environ` the scan would report "unsupported", and every
  profile would count as live);
- `rotate status` finds no rotator and reports it stopped or stale;
- nothing reads another process on the host.

The shared tree is never written after it is created, because parallel tests
share it. A test that needs processes makes its own tree with
[`FakeProc`](#add-fake-processes-with-fakeproc) and sets it with `.env(..)`
after `isolated(..)`; the later value wins.

## Keep the fake tree after `env_clear()`

A fixture that calls `env_clear()` after `isolated(..)` loses the variable,
and the child falls back to the real `/proc`. Call `reapply_proc_root` straight
after `env_clear()`:

```rust
use crate::isolated_amplihack::{isolated, reapply_proc_root, HOME_GUARD_ENV};

let tmp = tempfile::tempdir().unwrap();
let mut cmd = isolated(env!("CARGO_BIN_EXE_amplihack"));
cmd.env_clear();
reapply_proc_root(&mut cmd);
cmd.env("PATH", std::env::var_os("PATH").unwrap())
    .env("TMPDIR", std::env::temp_dir())
    .env("HOME", tmp.path().join("home"))
    .env(HOME_GUARD_ENV, "1")
    .args(["profile", "rotate", "status"]);
```

`reapply_proc_root` sets `AMPLIHACK_TEST_PROC_ROOT` to the shared empty tree.
To use your own tree instead, set `PROC_ROOT_ENV` to `fake.path()` after it.

Keep `TMPDIR` pointing at `std::env::temp_dir()`. The seam accepts only a tree
inside the child's temp dir, so a child with a different `TMPDIR` rejects the
tree and reads the real `/proc`.

`HOME` must also lie under the temp dir, as `tmp` does here. With the home
guard set, amplihack ignores a `HOME` outside it; see
[Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md).

The base fixture in `tests/support/profile_rotate.rs` already sets
`PROC_ROOT_ENV`, `TMPDIR` and the home guard after `env_clear()`, and its
`run` fails a test whose child prints the refusal line
(`assert_proc_root_honoured`). Its `with_proc(&FakeProc)` method points every later
command of that fixture at a test's own tree. Keep the `FakeProc` alive while
those commands run: the seam reads the real `/proc` for a tree that is gone,
so the fixture panics if the tree has been removed. A test that keeps a guard
which runs a command on drop declares the `FakeProc` before the guard, so the
tree is dropped last.

## Add fake processes with `FakeProc`

`crates/amplihack-cli/tests/support/fake_proc.rs` (Linux only) builds a
private fake process table for one test.

The agent tests already have a fake `/proc` builder, `Proc` with `add_proc`
and `remove_proc` in `tests/issue_1760_fake_proc.rs`. `FakeProc` is a
separate builder because the two cover different code:

| | `issue_1760_fake_proc.rs` | `FakeProc` |
|-|---------------------------|------------|
| Used by | `amplihack agent` tests | `amplihack profile` and `sessions` tests |
| Writes | `stat` with `ppid` and start time, `status`, `comm`, `cmdline`, `environ`, `cwd`, and `sessions/<pid>.json` files | `cmdline`, `status`, `environ`, `exe` link, `self/environ`, `self/cgroup` |
| `/proc/self` | a link to the fake `SELF_PID`, an `amplihack` process with a chosen parent | a real directory holding `environ` and, if set, `cgroup` |
| Pids | fixed pids such as `5000` and `SELF_PID = 999_999`, which a host process can also hold | `FAKE_PID_BASE..=i32::MAX` only, so no signal can reach a host process |
| Real processes | none | `link_own(pid)` for a process the test started |

The agent code never signals the pids it reads, so the #1760 pids are safe
there. `profile rotate stop` does signal the rotator pid, so a profile test
needs the pid floor. Moving the #1760 tests onto the floor and changing
`/proc/self` from a link to a directory would rewrite those tests for no
gain, and neither suite needs the other's files, so the two builders stay
separate.

| Method | What it does |
|--------|--------------|
| `FakeProc::new()` | Creates a 0700 temp directory with `self/environ` = `AMPLIHACK_FAKE=1\0`. |
| `path()` | The tree's root, for `PROC_ROOT_ENV`. |
| `process(pid)` | Starts a static entry `<root>/<pid>/` and returns a `FakeEntry`. |
| `link_own(pid)` | Creates `<root>/<pid> -> /proc/<pid>` for a process the test started. See [below](#show-a-process-the-test-started). |
| `remove(pid)` | Deletes an entry. A missing entry counts as removed. |
| `self_cgroup(text)` | Writes `self/cgroup`, for rotation-owner tests. |

| `FakeEntry` method | Writes |
|--------------------|--------|
| `cmdline(&[..])` | `cmdline`, each argument followed by NUL |
| `status(state)` | `status` with `State:` set to the one-character state and `Uid:` set to the effective uid |
| `environ(&[(key, value)])` | `environ`, each `key=value` followed by NUL |
| `exe_link(path)` | `exe` as a symlink to `path` |

Static pids must lie in `FAKE_PID_BASE..=i32::MAX`, where
`FAKE_PID_BASE = 4_194_305` is one above the largest Linux `pid_max`. No real
process can have such a pid, so if code under test ever signals a fake pid,
both `pidfd_open` and `kill` fail with `ESRCH` and no host process is touched.
The upper bound stops the pid wrapping to a negative number, which `kill`
would treat as a process group. `process(pid)` panics on a pid outside the
range.

The builder also panics on any I/O error, on a NUL inside an argument or
environment value, and on `=` inside an environment key. It never reads a real
process's `environ`.

### Example: a live session for one profile

```rust
use crate::support::fake_proc::{FakeProc, FAKE_PID_BASE};
use crate::isolated_amplihack::{isolated, PROC_ROOT_ENV};

let fake = FakeProc::new();
let profile_dir = home.join(".amplihack/claude-profiles/work"); // absolute
fake.process(FAKE_PID_BASE)
    .cmdline(&["claude"])
    .status('S')
    .environ(&[("CLAUDE_CONFIG_DIR", profile_dir.to_str().unwrap())]);

let out = isolated(env!("CARGO_BIN_EXE_amplihack"))
    .env("HOME", &home)
    .env(PROC_ROOT_ENV, fake.path())
    .args(["profile", "auto-switch", "status"])
    .output()
    .unwrap();
// stdout reports `work` as live.
```

Use an absolute `CLAUDE_CONFIG_DIR`. For a relative one, liveness reads the
entry's `cwd`, which the builder does not write.

The same entry with `.status('Z')` is skipped as a zombie, and with the shared
empty tree `work` is not live.

### Example: a rotator that is ours, and one that is not

`rotate status` treats a rotator as ours only when its `exe` is the binary
under test:

```rust
fake.process(FAKE_PID_BASE + 1)
    .cmdline(&[bin, "profile", "rotate", "watch", "--spawned"])
    .status('S')
    .exe_link(bin);
fs::write(&pid_file, format!("{}\n", FAKE_PID_BASE + 1)).unwrap();
// `rotate status` prints running (pid 4194306).
```

Without `exe_link` the same entry reads as a foreign rotator. `rotate stop`
against either entry fails with `the rotator (pid 4194306) changed before it
was signalled` and keeps the pid file. Just before it signals, `stop` checks
the pid against the real `/proc` (`rotator_status::running_on_host`), not the
fake tree, so a fake tree can never choose a real pid to signal; no process
there holds a pid above `pid_max`.

## Show a process the test started

Some tests need a real process to be visible: `rotate start`, `status` and
`stop` against a daemon the test started, or a child the test spawned to hold
a profile. Give the fake tree a link to that one pid:

```rust
let mut child = Command::new("sleep").arg("600").spawn().unwrap();
fake.link_own(child.id());
// ... run `amplihack profile ...` with PROC_ROOT_ENV = fake.path() ...
// assert on the output
child.kill().unwrap(); // only after the assertions
child.wait().unwrap();
```

Clean up with `child.kill()` and then `child.wait()`, in that order, after the
last assertion. Waiting only at the end keeps the pid held, so it cannot be
reused while the test reads it. The build step must keep this order in the
fixtures and the seam test.

For a daemon started with `rotate start`, read its pid from the pid file the
test's own command wrote, or from the `started (pid N)` line, and link that.
The daemon is a grandchild after `setsid` and may be reparented, but it is
still one of the test's own descendants.

Rules for `link_own`:

- Pass only a pid the test started: `child.id()`, the pid from the test's own
  pid file, or the pid in a `started (pid N)` line the test's command printed.
  Never a literal, and never a pid read from the host.
- Keep a spawned `Child` unreaped until the assertions are done. A dead but
  unreaped child is a zombie: its `cmdline` is empty, so it is not a rotator,
  and liveness skips state `Z`. Once reaped, the link dangles. Either way the
  pid cannot be reused behind the link while the test still reads it.
- After the daemon exits, its link dangles and the state reads `Stale`. That
  is how `rotate stop`'s existing wait ends; no timer or extra wait is added.

The rotator's owner check calls `symlink_metadata` on `<root>/<pid>`, so it
sees the owner of the link, not of the process. That is acceptable because the
test owns the process anyway, the kernel still refuses to signal another
user's process, and liveness reads the real `Uid:` through the link. It is
also why rule (e) of the guard is a hard failure.

The rotation-owner check (`owner::alive`, behind `rotate owner` and the
`owner:` line of `rotate status`) is stricter: it refuses a `<pid>` entry
that is a symlink, and `tests_owner::a_proc_entry_that_is_a_symlink_is_not_alive`
pins that. A linked pid therefore never reads as a live owner. To check the
`(running)` owner line, write a static rotator entry at `FAKE_PID_BASE + n`
and name that pid in the owner marker.

A pid can only be linked after the test learns it, so a race between two
`rotate start`s with no rotator yet cannot be shown through the binary: the
second `start` checks the tree before the test can link the first daemon.
`tests_autostart::sixteen_concurrent_launches_start_exactly_one_rotator`
covers that race in a unit test. The integration test
`issue_1592_daemon::starts_beside_a_running_rotator_refuse` checks only that
two concurrent starts beside a running, linked rotator both refuse.

## What the guard test checks

`crates/amplihack-cli/tests/issue_2042_proc_root_guard.rs` spawns nothing. It
has seven tests; the scanner itself is `tests/support/proc_root_scan.rs`:

| Test | Checks |
|------|--------|
| 1 | `isolated(..)` sets `AMPLIHACK_TEST_PROC_ROOT` to a path that is not `/proc`, lies under the temp dir, is a 0700 directory owned by the effective user, and holds only `self/environ`. |
| 2 | `procfs/root.rs` reads the variable under the name `AMPLIHACK_TEST_PROC_ROOT`, and its doc comment names the exceptions (`/proc/loadavg`, `/proc/self`, `util.rs`, `rotator_down`, `running_on_host`, `recipe/process_tree.rs`). No other file under `src/` names the variable, apart from the resolver's unit tests. |
| 3 | No non-test file under `src/commands/profile/` or `src/commands/sessions/`, and not `src/commands/cleanup/procs.rs`, has a string literal that starts with `/proc` (`"/proc"`, `"/proc/{pid}/cmdline"` inside `format!`, and so on), apart from `"/proc/self/..."` and `"/proc/loadavg"`. Files whose name starts with `tests` are unit tests and are skipped. Each routed file must call `proc_root()`. A sample test pins what the matcher flags. `util.rs` is not checked; see [the exceptions](#what-still-reads-the-real-proc). |
| 4 | Scans every `crates/amplihack-cli/tests/**/*.rs`, including `support/`, with the rules below and prints each finding as `file:line rule`. |
| 5 | Runs the same scanner over in-memory samples: each bad sample gives exactly its own rule, and the good sample gives none. |
| 6 | The scanner recognises each form of function declaration listed below, and nothing else. |
| 7 | `assert_proc_root_honoured`, which the `profile_rotate` fixture's `run` calls on every child, fails on the refusal line. |

The scanner rules:

| Rule | Fails on |
|------|----------|
| (a) | A command for the amplihack binary with a `profile` argument that is not built by `isolated(..)` and does not set `PROC_ROOT_ENV`. |
| (b) | `env_remove("AMPLIHACK_TEST_PROC_ROOT")` or `env_remove(PROC_ROOT_ENV)`. |
| (c) | The variable set to `"/proc"`. |
| (d) | `env_clear()` in a function that builds an amplihack command, in a file that runs `profile` or `sessions`, with no call to `reapply_proc_root` and no `PROC_ROOT_ENV` after it. `sessions` counts because `sessions/live.rs` reads the process table through the same seam. |
| (e) | `link_own(..)` with an integer literal, or in a function that neither spawns a process nor runs `rotate start`. |

The wording of rule (e) is fixed here, and the guard must match it. If the
guard needs a different rule, change this table and the samples in test 5
together.

Run it on its own:

```bash
RUST_TEST_THREADS=8 cargo test -p amplihack-cli --test cli_integration issue_2042
```

The scanner matches text and splits functions at lines that declare one:
`fn`, after any visibility (`pub`, `pub(crate)`, `pub(super)`, `pub(in ..)`)
and any of `async`, `const` and `unsafe`. Lines before the first function
form a chunk of their own. Rules (a), (d) and (e) apply per function. It can be
wrong about code split oddly across lines. Fix a false finding by moving the
code so it reads plainly (for example, `link_own` next to its `spawn`), not by
adding an exception.

`crates/amplihack-cli/tests/issue_2042_fake_proc_seam.rs` checks the other
direction: through the real fixtures, `auto-switch status`, `switch`,
`rotate status` and `rotate stop` give one result with a marked fake process
and another with the empty tree. It asserts only on output and exit status.
`issue_2042_seam_dry_run_and_refusal.rs` does the same for `rotate --dry-run`,
and checks that a refused tree gives the stderr line above and an accepted
one does not.

## The strace check

The guard above reads source text, so it misses a path built some other way.
`crates/amplihack-cli/tests/issue_2042_host_proc_strace.rs` checks what the
kernel sees instead. It runs real fixture commands (`profile auto-switch
status`, `profile rotate status`, `profile switch`, `profile rotate
--dry-run`, `sessions archive`, `cleanup` and `recipe stop <run_id>`) under `strace -f -e
trace=%file`, and fails when any traced process opens `/proc` itself or names
`/proc/<pid>/...` for a pid outside the traced tree. Each test also checks
that the command read its fake tree, so an empty trace cannot pass.

Without `strace` the tests print a skip line and pass. CI installs `strace`
and sets `AMPLIHACK_TEST_REQUIRE_STRACE=1`, so there a missing `strace` fails
the run. To run it locally:

```bash
AMPLIHACK_TEST_REQUIRE_STRACE=1 cargo test -p amplihack-cli --test cli_integration issue_2042_host_proc_strace
```

When you route a new command through `proc_root()`, add it to this file.

### `recipe stop <run_id>`: reads only its own run's pids

`recipe stop` signals real processes, so it cannot read a fake tree (see
[What still reads the real `/proc`](#what-still-reads-the-real-proc)). By run
id it takes its pids from the attempt's
[run record](../reference/recipe-command.md#run-records), not from a scan of
`/proc`. Its strace case, `recipe_stop_by_run_id_reads_no_host_process_table`,
passes the run's own pids to `host_proc_reads` as an allowed set, because
`recipe stop` must read their `stat` to check their start times.

The test starts a foreground `recipe run` with the stub runner fixture from
`issue_1606_detach`, under a temporary `HOME` and work root, outside strace.
It reads the run id from the `early` pointer line and waits for the runner's
and child's pid files. The record already exists at that point, because
`recipe run` writes it before printing `early`. Only `recipe stop <run_id>`
runs under `strace -f`, with the fixture's environment.

The test fails if any traced process:

- opens `/proc` or `/proc/` as a directory;
- opens any `environ` at all, even of the run's own pids;
- opens any `/proc/<pid>/...` path for a pid outside the traced tree, the
  `recipe run` process, the runner and the stub's child. `/proc/self` and
  `/proc/thread-self` count as the traced tree.

It also fails unless the trace shows an open of `procs/<run_id>.json`, so an
empty trace cannot pass. Afterwards it checks that the stopped `recipe run`
exited non-zero and that the runner and its child are gone, by waiting on the
child's exit status. No step waits on a timer.

The same test fails on a build without run records, where `recipe stop`
lists `/proc`.

## What still reads the real `/proc`

| Reader | Why it stays |
|--------|--------------|
| `/proc/self/*` outside the routed files, for example `/proc/self/fd/0` in `profile/limit_wait/scratch.rs` | It describes the process itself, not other processes. The routed files read `<root>/self/environ` (`liveness.rs`) and `<root>/self/cgroup` (`owner.rs`) through the seam, which is why the fake trees write those two files. |
| `/proc/loadavg` | One file, no other process's data. |
| `proc_children` and `proc_stat_ppid` in `crates/amplihack-cli/src/util.rs` | They find the binary's own descendants to send `SIGTERM` and `SIGKILL` after a timeout. With a fake root, real descendants would be left running and fake pids would be signalled on the host. |
| `amplihack-utils::rotator_down` | Outside the CLI crate; unchanged. |
| `amplihack-utils::running_on_host`, called by `rotate stop` just before it signals | The pid it confirms is signalled, so a fake tree must not choose it. Discovery and `rotate status` still read the fake tree. |
| `recipe/process_tree.rs`, used by `recipe stop <session>` and `recipe run`'s teardown | Every pid it finds may be signalled, so a fake tree must not choose one. It lists `/proc` and reads `/proc/<pid>/stat` to build a tree from its roots, and reads no `environ`. `recipe stop <run_id>` does not use it: it reads only the `stat` of the pids in the run's record (`recipe/run_record.rs`). |
| A recorded run's `/proc/<pid>/stat` in `cleanup/run_dir.rs` and `eligibility.rs` | One named pid's start time, not a scan and not its `cmdline` or `environ`. |
| Governor and launch scans | Not reached by these tests. If one is, that call site alone moves to `proc_root()`. |

Agent code is not in this table: `agent/pass.rs`, `register.rs` and
`wrapper.rs` already read through `proc_root()`, routed for #1760.
`amplihack cleanup`'s scan of working folders, open files and `TMPDIR`
values (`cleanup/procs.rs`) reads through `proc_root()` too. It signals
nothing; the `issue_1808_run_cleanup` fixture writes its fake tree inside the
child's `TMPDIR` and links each process `live_process` starts.

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Guard finding `(a)` | The test builds the command with `Command::new`. | Use `isolated(..)`, or set `PROC_ROOT_ENV` yourself. |
| Guard finding `(d)` | `env_clear()` with no reapply. | Call `reapply_proc_root(&mut cmd)` straight after it. |
| A profile is live with the empty tree, or stderr has `AMPLIHACK_TEST_PROC_ROOT ignored (..)` | The child read the real `/proc`: its `TMPDIR` changed, the home guard is missing, or the tree is not private. The stderr line names which. | Keep `TMPDIR` at `std::env::temp_dir()` (or use `FakeProc::new_in` under the child's temp dir), keep the guard, create the tree with `FakeProc`. Fix the fixture; do not hide the line. |
| Every profile is live | The tree has no `self/environ`, so liveness reports "unsupported". | Use `FakeProc::new()` or the shared tree; both write it. |
| A fake rotator reads as foreign | The entry has no `exe` link. | Add `.exe_link(<binary under test>)`. |
| `rotate status` reports `Stale` for a daemon the test started | Its pid is not in the fake tree. | `fake.link_own(pid)` with the pid from the pid file. |
| `process(pid)` panics | The pid is below `FAKE_PID_BASE`. | Use `FAKE_PID_BASE + n`. |

## Related

- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - the `isolated(..)` stops that keep tests from starting a real rotator
- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - the profile root a test may write
- [Keep Tests Passing Under Host Load](./keep-tests-passing-under-host-load.md) - wait on events, not timers
- `crates/amplihack-cli/tests/issue_1760_fake_proc.rs` - the fake `/proc` builder for the `amplihack agent` tests; see [why it is separate](#add-fake-processes-with-fakeproc)
- [`AMPLIHACK_TEST_PROC_ROOT`](../reference/amplihack-env-vars.md) in the environment variable reference
