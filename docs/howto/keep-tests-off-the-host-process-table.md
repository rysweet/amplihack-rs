---
title: Keep Tests off the Host Process Table
type: howto
updated: 2026-11-11
related: issue #2196, issue #2042, issue #2040, issue #1760, issue #1690, issue #2175
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

Since #2196 the unit tests inside the crate (`cargo test -p amplihack-cli
--lib`) also read only a fake table. They do not start a binary, so they
cannot use the environment variable. A test that needs fake processes
installs a per-thread scope; every other lib test reads one shared, empty
fake table. Both are described in
[Lib tests: the per-thread scope](#lib-tests-the-per-thread-scope). In a lib
test build, `proc_root()` never returns `/proc`.

## Contents

- [How the seam works](#how-the-seam-works)
- [Write a test that runs `amplihack profile`](#write-a-test-that-runs-amplihack-profile)
- [Keep the fake tree after `env_clear()`](#keep-the-fake-tree-after-env_clear)
- [Add fake processes with `FakeProc`](#add-fake-processes-with-fakeproc)
- [Show a process the test started](#show-a-process-the-test-started)
- [Lib tests: the per-thread scope](#lib-tests-the-per-thread-scope)
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

In a lib test build, a per-thread override is checked before the variable,
and the shared fake table replaces `/proc` as the last step; see
[Lib tests: the per-thread scope](#lib-tests-the-per-thread-scope).

In every other build, if any condition fails, the code reads the real `/proc`. A debug build, which
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

## Lib tests: the per-thread scope

Unit tests under `crates/amplihack-cli/src/` call profile and rotation code
directly, in one process, on many threads at once. Setting
`AMPLIHACK_TEST_PROC_ROOT` there would change the table for every test in the
binary, so lib tests never set it (nor `PROC_ROOT_ENV`). A test that needs
fake processes installs a table for its own thread instead. A test that
installs nothing reads the shared default described below.

### The order `proc_root()` resolves in

| Order | Source | Present in |
|-------|--------|------------|
| 1 | The calling thread's override, set by `proc_scope_at` or `fake_proc_scope` | lib test builds only (`#[cfg(test)]`) |
| 2 | `AMPLIHACK_TEST_PROC_ROOT`, accepted under the rules in [How the seam works](#how-the-seam-works) | every build |
| 3 | The shared default fake table, `shared_default_root()` | lib test builds only (`#[cfg(test)]`) |
| 4 | `/proc` | every build except lib test builds |

In a lib test build, step 3 takes every outcome of step 2 other than an
accepted tree: an unset variable, an empty one, and a refused one. A refused
variable prints its stderr line once, ending in `reading the shared fake
table` rather than `reading /proc`. So `proc_root()` cannot return `/proc` in
a lib test, whether or not the test asked for a fake table.

Release, debug and integration-test binaries compile only steps 2 and 4 and
resolve exactly as before. The override, its guard, the shared default and
the changed stderr text are all `#[cfg(test)]`; a release binary contains
neither the string `amplihack-fake-proc` nor `shared fake table`.

This is the same resolver and the same `FakeProc` builder that #2042 added,
not a second seam. The shared default asserts nothing about the code under
test and never panics on its account: it only gives code that does not ask
for a table an empty one to read.

### The shared default

`crate::test_support::proc::shared_default_root() -> &'static Path` is
created on first use and kept for the life of the test process:

| Property | Value |
|----------|-------|
| Location | A randomly named directory directly under `std::env::temp_dir()` (the seam accepts trees only there, so not under the scratch base, which can fall back to `/var/tmp`), never under `target/`, never a fixed or pid-derived name |
| Mode | 0700 at creation, owned by the effective user |
| Contents | `self/environ` only, written by `seed_proc_tree` |
| Checked | Once, by the same `check_tree` that guards the variable; a refusal panics naming the path and the reason |
| Lifetime | One per test process, shared by every thread, never deleted during the run |

A small directory stays in `TMPDIR` after the run. That is expected.

It is returned as `&'static Path`, not as a `FakeProc`, so no test can add an
entry to it. A test that needs a process installs its own scope. Because the
default holds only `self/environ`, liveness reports nothing live, the rotator
is not running, and no session holds any profile.

### Choose the injection point

Use the first of these that the code under test offers:

1. A root the code already takes: `autostart::Deps.proc_root`, or a
   function with a `proc_root: &Path` argument. Pass `scope.path()`, or
   `fake.path()` for a fixture that holds a `FakeProc`. A test that reads
   only its own process may pass `Path::new("/proc")` to the `*_at` variant:
   its own pid is not another process.
2. A scan function the code lets the test replace: `cycle::Ctx.scan` is a
   `fn(&Path) -> Liveness` (`rotate/cycle_ctx.rs`), set to `scan_system` in
   production. A test sets it to a function that returns the liveness it
   needs, and no table is read at all.
3. Otherwise, a scope on the test's thread. Code that calls `proc_root()`
   itself (`recover_auto_switch`, `swap_check`, `change_log::record`,
   `shared_dirs::ensure_for`, `owner::describe`) then reads the fake table.

There is no other seam: no process-table trait, no second fake, no extra
environment variable and no `State` field. A test that needs none of these,
because it expects no live process, uses the shared default and installs
nothing.

### `fake_proc_scope` and `proc_scope_at`

Both live in `crate::test_support::proc`
(`crates/amplihack-cli/src/test_support_proc.rs`, declared from the
`#[cfg(test)]` module `src/test_support.rs`). The module includes the same
`tests/support/fake_proc.rs` builder the integration tests use, so there is
one builder for both.

| Item | What it does |
|------|--------------|
| `fake_proc_scope() -> FakeProcScope` | Creates a fresh `FakeProc` (0700, `self/environ` seeded) and installs it as this thread's override. |
| `proc_scope_at(&FakeProc) -> ProcRootOverride` | Installs an existing `FakeProc` as this thread's override. Use it inside a thread the test spawns, or for a fixture that already holds a `FakeProc`. It takes a `FakeProc`, not a path, so only a tree built by the one fake builder can be installed. |
| `FakeProcScope::path()` | The tree's root, for an explicit `proc_root` argument. |
| `shared_default_root() -> &'static Path` | The [shared default](#the-shared-default). Tests rarely call it; `proc_root()` returns it when nothing else applies. |
| `FakeProcScope::fake` | The `FakeProc`, for `process(pid)`, `remove(pid)` and `self_cgroup(..)`. |
| `FakeProcScope::running_session(pid, config_dir) -> FakeEntry` | Seeds a Claude session that uses `config_dir`; see [below](#seed-a-running-session). |

`ProcRootOverride` restores the previous override when it is dropped, so
scopes nest, but only if they are dropped in the reverse of the order they
were installed. If an outer guard is dropped first, the inner guard later
restores the outer tree's path after that tree has been deleted. Let guards
drop at the end of their block, and do not call `drop` on an outer one early.
A test needs one scope; `env()` already installs it (see below). The guard
is `#[must_use]`, is neither `Clone` nor `Copy`, and is not
`Send`: it cannot be moved to another thread, where it would restore the
wrong thread's value. It is `Sync`, so a fixture that holds one can still be
lent to a scoped thread by reference. `FakeProcScope` holds the guard before the tree, so the
override is removed before the directory is deleted.

A test with no fixture installs the scope itself and keeps it to the end:

```rust
use crate::test_support::proc::fake_proc_scope;

#[test]
fn a_profile_with_no_session_is_not_live() {
    let _proc = fake_proc_scope();
    // Code under test that calls proc_root() on this thread reads _proc.path().
    assert!(!is_live("work"));
}
```

`rotate::tests_swap::Env` carries a `proc: FakeProcScope` as its last field,
so every test built on `env()` already reads an empty fake table. Such a test
seeds processes through `e.proc` and does not call `fake_proc_scope()` as
well.

The names `is_live`, `switch` and `run_auto_switch` in this section stand for
the function under test. The tests converted for #2196 call the real
functions; see the `tests_swap` module for current examples.

### Seed a running session

`running_session` writes one entry for a pid at or above `FAKE_PID_BASE`:

| File | Contents |
|------|----------|
| `cmdline` | `claude` |
| `status` | `State: S` and `Uid:` set to the effective uid |
| `environ` | `CLAUDE_CONFIG_DIR=<config_dir>` |

It writes no `exe` link. Add `.exe_link(..)` on the returned `FakeEntry`
where an owner check needs one. It panics on a pid below `FAKE_PID_BASE`, so
no lib test can name a pid a host process might hold.

A test that refuses a switch while a session holds a profile seeds the
session, checks the refusal, then removes the entry and checks that the
refusal goes away. The second half proves the result came from the fake
entry and not from anything else:

```rust
let pid = FAKE_PID_BASE + 1;
e.proc.running_session(pid, &e.root.join("work"));
assert!(switch(&e, "work").is_err(), "a live session holds `work`");

e.proc.fake.remove(pid);
assert!(switch(&e, "work").is_ok(), "nothing holds `work` once the entry is gone");
```

No lib test starts a process to stand for a session. The
`tests_switch_candidates` sessions are fake entries. The lease in
`tests_login_check` is an entry in the auto-switch ledger, not a process:
no pid is checked or signalled, and the liveness scan behind it reads the
test's empty fake table.

### Threads

The override belongs to one thread and is not inherited by threads that
thread starts.

- **A thread the test spawns.** Install the scope inside the closure. Borrow
  the tree under `std::thread::scope`, so it outlives the threads and the
  test waits for them by `join`:

  ```rust
  let scope = fake_proc_scope();
  let fake = &scope.fake;
  std::thread::scope(|s| {
      for _ in 0..2 {
          s.spawn(|| {
              let _proc = proc_scope_at(fake);
              run_auto_switch(&config);
          });
      }
  });
  ```

  Because `FakeProcScope` is not `Send`, a struct that holds one (such as
  `Env`) cannot be moved into a thread. Borrow it (`&Env`) inside
  `thread::scope` instead; the compiler points at every place that needs
  this.

- **A thread production code spawns.** That thread sees no override, so it
  reads the shared default: an empty table, never `/proc`. If a test needs
  a fake process to be visible on such a thread, inject at a layer the code
  already takes, `cycle::Ctx.scan` or a `*_at(proc_root, ..)` function,
  rather than adding a process-wide override. The PR that converts such a
  test names it.

### A refused tree panics

`override_proc_root` runs the same checks as the variable (a directory
inside the canonical temp dir and not the temp dir itself, owned by the
effective user, not writable by others), skipping only the home-guard check.
A tree that fails them panics at once, naming the path and the reason:

```text
override_proc_root(/tmp/.tmpAbC123): others can write the directory
```

This differs from the variable on purpose. A refused variable falls back with
a stderr line (to `/proc` in a child binary, to the shared default in a lib
test), because a child process cannot fail its parent's test. A refused
override is a mistake in the test, so it stops the test. The panic is a
precondition on the helper's argument; it is not a check on the code under
test. Build trees with `FakeProc`, which always passes.

`procfs/tests_root.rs` still builds refused trees by hand to test this, and
calls the internal `override_proc_root(&Path)` directly. Tests of the refused
variable call `procfs::resolve()` and never set the process-wide variable,
since under `cfg(test)` `proc_root()` would hide the refusal behind the
shared default.

The precedence test runs a single lib test in a child process with the
variable set on `Command`, checks that the thread override wins, then drops
it and checks that the variable wins. It never changes the parent process's
environment: other lib tests read the shared default without an environment
lock.

### Guard against bypassing the resolver

`src/test_support_proc_guard.rs` runs as part of `cargo test --lib`. It
parses every Rust source under `src/`, checks inline `#[cfg(test)]` modules
and `#[test]` functions, and checks helpers in `tests/`, `tests*.rs`,
`*_tests.rs` and `test_support*.rs` files. It rejects filesystem reads that
enumerate a literal `/proc` root or name a literal numeric host pid, even
when the path is passed through local bindings (including typed bindings),
module or block constants, `Path` constructors,
`format!`, raw strings or an assertion. Pure validation inputs and diagnostic
strings do not count as reads. The guard's negative samples include
`fs::read_to_string("/proc/1/stat")` in both inline and separate test code,
typed `let root: &str = "/proc"` bindings, and constant aliases declared
before or after the test. Bindings follow block scope and shadowing.

This source guard complements the integration guard; its coverage does not
depend on whether a particular lib test happened to run during a trace.
It permits `/proc/self/*` and global kernel files such as `loadavg` and
`meminfo`. Dynamic pid paths still require an ownership review: only the
test itself or a child it holds may be read. Arbitrarily computed paths and
interprocedural data flow need the strace check below.

The release-layout opaque-process test uses `fake_proc_scope()` and the real
`classify_opaque_at(root, pid)` classifier. It supplies process `stat` and `cgroup` files and a synthetic
boot-time file, then checks exact classifications, including malformed and
missing data. It never enumerates the host table.

### Run the lib tests

Plain `cargo test` runs them:

```bash
cargo test -p amplihack-cli --lib
cargo test -p amplihack-cli --lib -- --test-threads=1
```

Run both over the whole lib suite: the first catches a test that depends on
another thread's override, the second a test that depends on running
alongside others.

Some hosts put a local wrapper in front of `cargo` that skips the profile and
rotation lib tests. That wrapper is not part of this repository, and no
setting in the repository reads `AMPLIHACK_RUN_PROC_TESTS`. Where the wrapper
is installed, `AMPLIHACK_RUN_PROC_TESTS=1` tells it to run them. Since these
tests read only fake tables, the wrapper's skip can be removed.

To confirm a module reads no host process, build the lib test binary first,
then trace that binary alone on one thread. Tracing `cargo` itself would mix
cargo's own `/proc` reads into the output.

```bash
bin=$(cargo test -p amplihack-cli --lib --no-run --message-format=json \
  | jq -r 'select(.profile.test == true and .target.kind == ["lib"]) | .executable')
strace -f -e trace=openat,connect "$bin" commands::profile::rotate --test-threads=1 \
  2>&1 | grep -E '"/proc/[0-9]+|/proc"|connect\('
```

`/proc/self/...` and `/proc/loadavg` may appear. `/proc/<pid>` may appear
only for a pid inside the traced tree: `strace -f` prefixes each line from a
child or thread with `[pid N]`, and those pids, with the test binary's own,
are the ones allowed. They come from tests that read their own pid through
`Path::new("/proc")` and from the exceptions in `util.rs` and
`recipe/process_tree.rs`. A `/proc/<pid>` for any other pid is a host
process and a failure, as is any tmux or session socket `connect`.

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
| `/proc/1/cgroup` and `/.dockerenv` in `docker::DockerDetector::is_in_docker`, and `docker info` in `is_running`, reached through `profile::docker_env_active` | Reached only when `AMPLIHACK_USE_DOCKER` is truthy. Decides whether Docker mode is active so that `profile run` can refuse it; a session already inside a container is not refused. Tests pass the answer directly to `reject_docker_mode(active)` in `profile/mod.rs` and `run::run_in_mode(store, name, args, docker_active)` in `profile/run.rs` (see `profile/tests_run.rs`), so they read no host file and need no Docker daemon. `run_in_mode` checks Docker before it touches the profile store. |
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
| `running_session` panics | The pid is below `FAKE_PID_BASE`. | Use `FAKE_PID_BASE + n`. |
| A lib test panics with `override_proc_root(<path>): <reason>` | The tree failed the checks in [A refused tree panics](#a-refused-tree-panics). | Build it with `FakeProc` under `std::env::temp_dir()`. |
| A lib test does not see a session it seeded | The code ran on a thread with no override and read the empty shared default. | Install `proc_scope_at(&scope.fake)` inside a closure the test spawns. For a thread production code spawns, inject through `Ctx.scan` or a `*_at(proc_root, ..)` function. |
| A lib test that reads its own pid fails | The shared default has no entry for the test's pid. | Pass `Path::new("/proc")` to the `*_at` variant, or add the pid with `FakeProc::link_own`. |
| A lib test panics naming the shared default and a reason | The default could not be built or `check_tree` refused it. `check_tree` checks the created directory, not `std::env::temp_dir()`, so a `/tmp` at mode 1777 is accepted. The causes are: `TMPDIR` is missing or cannot be canonicalised; the directory cannot be created in it; or the created directory fails the owner or mode checks, as it can on a shared or network filesystem or where an ACL changes the mode. The panic names which. | Set `TMPDIR` to an existing local directory on a filesystem that keeps the owner and mode a directory is created with. |
| stderr has `AMPLIHACK_TEST_PROC_ROOT ignored (..); reading the shared fake table` | A lib test, or the shell it ran from, set the variable to a tree `check_tree` refuses. | Unset it; lib tests use scopes, not the variable. |
| A lib test reads a deleted tree or the wrong table after a nested scope | Guards were dropped out of order, so an inner guard restored an outer path that no longer exists. | Use one scope per test (`env()` or `fake_proc_scope()`, not both), and let guards drop at the end of their block. |
| `Env` cannot be sent to a thread | `FakeProcScope` is not `Send`. | Borrow `&Env` under `std::thread::scope`. |

## Related

- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - the `isolated(..)` stops that keep tests from starting a real rotator
- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - the profile root a test may write
- [Keep Tests Passing Under Host Load](./keep-tests-passing-under-host-load.md) - wait on events, not timers
- `crates/amplihack-cli/tests/issue_1760_fake_proc.rs` - the fake `/proc` builder for the `amplihack agent` tests; see [why it is separate](#add-fake-processes-with-fakeproc)
- [`AMPLIHACK_TEST_PROC_ROOT`](../reference/amplihack-env-vars.md) in the environment variable reference
