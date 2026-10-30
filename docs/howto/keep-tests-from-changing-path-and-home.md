---
title: Keep Tests from Changing PATH and HOME
type: howto
updated: 2026-10-30
related: issue #1765, issue #1699, issue #1506
---

# Keep Tests from Changing PATH and HOME

A unit test that calls `std::env::set_var("PATH", ..)` or
`std::env::set_var("HOME", ..)` changes the variable for every test in the
same test binary, not only for itself. Issue #1765 found tests that did this
to point a function at a fake binary, and timeout tests that failed on a
loaded host because they measured the host's speed rather than the timeout.

This guide is for contributors who write tests. It explains why the change
breaks other tests, how to give a child process its `PATH` and `HOME`
without changing the test process, how to test a timeout so that host load
cannot fail it, and which tests still change the variables.

## Contents

- [Why a process-wide change breaks other tests](#why-a-process-wide-change-breaks-other-tests)
- [Set the value on the child](#set-the-value-on-the-child)
- [Bound a timeout without measuring speed](#bound-a-timeout-without-measuring-speed)
- [Keep capture tests separate from timeout tests](#keep-capture-tests-separate-from-timeout-tests)
- [Sites that remain](#sites-that-remain)
- [Related](#related)

## Why a process-wide change breaks other tests

libtest runs the tests of one binary on parallel threads of one process.
`PATH` and `HOME` belong to the process, so a value set by one test is seen
by every other thread until it is restored:

- A test that starts a program by bare name (`git`, `npm`, `gh`) looks it up
  in the temporary `PATH`. It fails with `No such file or directory (os
  error 2)`, or it runs the other test's fake.
- A child started by any thread copies the environment at that moment, so a
  child started by an unrelated test while `HOME` is temporary uses the
  wrong home.
- Code that reads `HOME` in process, such as a config loader, sees the
  temporary directory.

`env_lock()` in `crates/amplihack-cli/src/test_support.rs` does not prevent
this. It serialises tests that change variables against each other. Tests
that only read `PATH` or `HOME`, or that start a program by name, never take
the lock, so they still run while the value is changed. In amplihack-utils,
`find_falls_back_to_npm_global_when_not_on_path` once set
`PATH=/nonexistent-just-for-this-test`, and 15 unrelated `artifact_guard`
tests failed. `crates/amplihack-utils/tests/no_global_path_mutation.rs` now
fails any unit test in that crate that sets or removes `PATH`.

The opposite mistake causes equal harm. A child that inherits the test
process's environment unchanged gets the developer's `HOME` and, on a
machine with login profiles, `AMPLIHACK_CLAUDE_PROFILE_ROOT`. That is how a
test install rewrote every profile's hooks in #1699. Every child a test
starts must run with a temporary `HOME` and without that variable.

## Set the value on the child

`Command::env` and `Command::env_remove` change only the child's
environment. On Unix, when `PATH` is set this way, `Command` also uses it to
find a program given by bare name. A test therefore needs a way to reach the
`Command` that production code builds. Choose the form by what the code
does:

| Production code | Form | Examples |
|-----------------|------|----------|
| Builds a `Command` with setup the test must keep (scrubbing, arguments, stdio, `setsid`) | [Configure hook](#configure-hook) | `ensure_mermaid_cli_with`, `version_probe_succeeds`, `git_clone_framework_repo_with`, `detect_version_with`, `resolve_checkout_repo_in_with`, `poll_tmux_sessions_with`, `run_claude_reflection_with` |
| Calls only `Command::new` and `args` | [Prepared `Command`](#prepared-command) | `run_provider_command_with_timeout` |
| Searches the `PATH` directories itself | [`PATH` value](#path-value) | `find_preferred_rustyclawd_binary_in`, `build_install_command_with_path` |
| Reads neither variable | [No seam](#code-that-reads-neither-variable) | The merge-guard test in `pre_tool_use_tests.rs` |

### Configure hook

Move the body of the function into a private inner function that takes
`configure: &dyn Fn(&mut Command)`. Call the hook after the existing setup
and immediately before the child is spawned. The original function keeps
its signature and passes a no-op, so production children are built exactly
as before:

```rust
fn version_probe_succeeds(bin: &str, configure: &dyn Fn(&mut Command)) -> bool {
    let mut cmd = Command::new(bin);
    amplihack_utils::litellm_proxy::scrub_inference_environment(&mut cmd);
    cmd.arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure(&mut cmd);
    run_with_timeout(cmd, VERSION_PROBE_TIMEOUT)
        .map(|status| status.success())
        .unwrap_or(false)
}

pub(super) fn ensure_mermaid_cli() -> Result<Outcome> {
    ensure_mermaid_cli_with(&|_| {})
}
```

`ensure_mermaid_cli_with` passes the hook to all four children it starts:
the `mmdc --version` probe, the `npm --version` probe, `npm install -g` and
the re-probe. The flow tests in
`crates/amplihack-cli/src/commands/install/tests/mermaid_cli_tests.rs` use
one hook for all of them:

```rust
use crate::commands::profile::ROOT_ENV;
use std::path::Path;
use std::process::Command;

fn hermetic<'a>(bin: &'a Path, home: &'a Path) -> impl Fn(&mut Command) + 'a {
    move |cmd: &mut Command| {
        cmd.env("PATH", bin).env("HOME", home).env_remove(ROOT_ENV);
    }
}

let outcome = ensure_mermaid_cli_with(&hermetic(&bin_dir, &home)).unwrap();
assert_eq!(outcome, Outcome::AlreadyPresent);
```

Follow these rules for every hook:

- Keep the inner function private, or `pub(super)` when a sibling `tests`
  module needs it. Production code reaches it only through the original
  function with a no-op. The hook runs after `scrub_inference_environment`,
  so a hook could restore a credential the scrub removed; only tests may
  pass one.
- A test hook sets `HOME` to the test's temporary directory and removes
  `ROOT_ENV`. When the code starts a program by bare name, the hook also
  sets `PATH` to the stub directory.
- Do not append the parent's `PATH`. With `PATH` set to the stub directory
  alone, a missing stub fails to spawn. With the parent's `PATH` appended,
  the test would run the developer's installed `git`, `npm` or `gh` with
  their credentials.
- Write each fake with `#!/bin/sh` and call every program in it by absolute
  path (`/bin/sleep`, `/bin/cat`). A `#!/usr/bin/env bash` fake, or a bare
  `cat`, fails under a `PATH` that holds only the stub directory.
- Take `ROOT_ENV` from `crate::commands::profile::ROOT_ENV` in
  amplihack-cli, `crate::rotator_status::ROOT_ENV` in amplihack-utils and
  `amplihack_utils::rotator_status::ROOT_ENV` in amplihack-hooks. Do not
  write the variable's name as a new literal.

### Prepared `Command`

When production code does nothing to the `Command` beyond `new` and `args`,
split the run into a function that takes the built `Command` and the
timeout:

```rust
fn run_provider_command(binary: &str, args: &[&str]) -> std::io::Result<Output> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    run_provider_command_with_timeout(cmd, PROVIDER_COMMAND_TIMEOUT)
}
```

The test builds the `Command` itself, sets `HOME` and removes `ROOT_ENV` on
it, and passes its own timeout. Because the timeout is an argument, no
test-only environment variable is needed to shorten it.

### `PATH` value

When the code searches the `PATH` directories itself instead of starting a
child, pass the value in as `Option<OsString>`. Production passes
`std::env::var_os("PATH")`; the test passes a value it built:

```rust
fn find_preferred_rustyclawd_binary() -> Option<PathBuf> {
    find_preferred_rustyclawd_binary_in(env::var_os("PATH"))
}
```

Inside the `_in` function, split the value with
`amplihack_utils::launch_target::split_path_var_of(path_var, RelativeEntries::Drop)`.
`Drop` discards empty and relative elements, which a shell would read as the
current directory. In amplihack-utils, use only `split_path_var_of`:
`this_crate_splits_the_path_in_exactly_one_place` in
`no_global_path_mutation.rs` fails a new `split_paths(` call.

`build_install_command_with_path(installed_exe, parent_path)` in
`crates/amplihack-cli/src/update/check.rs` follows the same form. Its tests
pass `Some(OsString::from("/tmp/stale-uvx:/usr/bin"))` and check the `PATH`
the install command would give its child.

### Code that reads neither variable

If the code under test reads neither `PATH` nor `HOME`, delete the change.
`auto_drive_merge_is_refused_for_bare_and_absolute_gh` in
`crates/amplihack-hooks/src/pre_tool_use/tests/pre_tool_use_tests.rs` used
to reorder `PATH`, under a name that said so, although the merge guard reads
no `PATH`. The test now keeps its cases, a bare `gh` and one named by its
absolute path, and has a comment: if the guard ever resolves `gh` through
`PATH`, add a `PATH` case through a seam, not through `set_var`.

## Bound a timeout without measuring speed

Before #1765, three of the four timeout tests ran a fake that slept 2 s and
asserted that the call returned within 1 s. The clone test ran a fake that
slept 5 s and asserted a 2 s bound; its background child wrote the orphan
marker after 1 s, and the test looked for the marker 1.2 s after the call
returned. These bounds measured the host's speed as well as the timeout.
In the workspace run reported in #1765, with a load average of about 177 on
72 CPUs, the mermaid test failed its 1 s bound although the probe's timeout
worked. On such a host, starting `sh`, waiting out the timeout, terminating
the child and reaping it can take longer than 1 s. In amplihack-cli,
`terminate_tree_gracefully` in `crates/amplihack-cli/src/util.rs` reads
every process's entry in `/proc` on each poll to find descendants (#1506),
so its cost grows with the number of processes on the host.

Write the test so that host speed cannot decide it:

1. The fake waits far longer than the bound. Its last command is
   `exec /bin/sleep 60`.
2. The test asserts that the call returned in under 30 s, with the elapsed
   time in the message.
3. The test then asserts that the call reported failure.
4. The test uses the production timeout, or passes one to the seam. It adds
   no environment variable to shorten the timeout.

With a working timeout the call returns in about the timeout plus the
termination time, far below 30 s even on a heavily loaded host. With a
broken timeout the call waits for the fake, about 60 s, and fails the
bound. The failure assertion catches a call that returned early for
another reason, such as a spawn error.

`exec` makes `sleep` the direct child instead of a child of `sh`.
`detect_version` kills only the direct child. Without `exec`, the `sleep`
would survive the test by up to 60 s and keep the threads that read its
output waiting.

```rust
#[cfg(target_os = "linux")]
#[test]
fn version_probe_is_bounded_for_hanging_binaries() {
    use crate::commands::profile::ROOT_ENV;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    let fake = temp.path().join("mmdc");
    std::fs::write(&fake, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = temp.path().to_path_buf();

    let started = Instant::now();
    let ok = version_probe_succeeds(fake.to_str().unwrap(), &|cmd: &mut Command| {
        cmd.env("HOME", &home).env_remove(ROOT_ENV);
    });
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the probe must stop a hanging binary long before it exits: {elapsed:?}"
    );
    assert!(!ok, "a timed-out probe must report failure");
}
```

The four tests and what each asserts after the bound:

| Crate | Test | Timeout | Failure assertion |
|-------|------|---------|-------------------|
| amplihack-cli | `commands::install::mermaid_cli::tests::version_probe_is_bounded_for_hanging_binaries` | `VERSION_PROBE_TIMEOUT` (500 ms) | Returns `false`. |
| amplihack-cli | `commands::workflow::tests::provider_command_execution_is_bounded` | 500 ms, passed by the test | Returns `Err` whose text contains `timed out`. |
| amplihack-cli | `commands::install::clone::tests::git_clone_times_out_and_reaps_child` | `GIT_CLONE_TIMEOUT` under `cfg(test)` (250 ms) | The error contains `timed out` and `started`, and the orphan marker is absent. |
| amplihack-utils | `binary_finder::tests::detect_version_is_bounded_for_hanging_binaries` | `VERSION_DETECTION_TIMEOUT` (500 ms) | Returns `None`. |

Check the provider's error text, not its `io::ErrorKind`. The kind depends
on whether an `io::Error` appears in the error chain, while every timeout
message in `util.rs` contains `timed out after`.

None of the four tests takes `env_lock()`, because none changes the process
environment.

### Check the process-group kill

`git_clone_framework_repo` starts `git` in its own process group with
`setsid` and kills the whole group on timeout. The clone test's fake starts
a background child that writes a marker file if it survives:

```sh
#!/bin/sh
(/bin/sleep 5; echo orphan > '<marker>') & echo started; exec /bin/sleep 60
```

After the call returns, the test waits until 6 s after the call started (no
wait if that time has passed), then asserts that the marker does not exist.
The 5 s delay gives the kill almost 5 s to finish before a surviving child
would write the marker, so a kill delayed by load is not reported as a
failure. The whole test takes about 6 s; the 30 s bound applies to the call.

### Show that the bound can fail

When you write or change a bound test, raise the timeout it uses to 120 s
by hand, run the test alone and confirm that it fails on the 30 s bound.
Then set the timeout back by hand. Do not use `git checkout -- <file>` or
`git restore <file>` for this: both discard every uncommitted change in the
file, including the test you are writing.

```bash
# After raising VERSION_PROBE_TIMEOUT to Duration::from_secs(120):
cargo test -p amplihack-cli --lib -- --exact \
  commands::install::mermaid_cli::tests::version_probe_is_bounded_for_hanging_binaries \
  --test-threads=1
# The test fails after about 60 s on the "long before it exits" assertion.

# After setting the timeout back by hand:
git diff -U0 HEAD -- crates | grep -E '^\+.*from_secs\(120\)' || echo reverted
```

The value to raise for each test is `VERSION_PROBE_TIMEOUT`, the provider
test's 500 ms argument, the `cfg(test)` `GIT_CLONE_TIMEOUT` and
`VERSION_DETECTION_TIMEOUT`. The last command checks staged and unstaged
changes against `HEAD`. It prints `reverted` when no added line contains
`from_secs(120)`, and prints the offending line otherwise.

## Keep capture tests separate from timeout tests

A test that checks the child's environment needs the fake to finish its
write. A fake that writes `$HOME` and then sleeps can be killed by a 500 ms
timeout on a loaded host before the write happens, and the test then fails
for a reason unrelated to the environment. Use two tests:

- a timeout test whose fake only sleeps, as above;
- a capture test whose fake records what it saw and exits 0.

The capture tests are
`commands::install::mermaid_cli::tests::version_probe_child_gets_test_home_without_profile_root`
and
`commands::workflow::tests::provider_command_child_gets_test_home_without_profile_root`.
Both fakes record `$HOME` and whether the profile-root variable is set.
The provider test builds its script with `format!` from this string; the
mermaid test uses the same script but quotes the path with `sh_quoted`:

```rust
"#!/bin/sh\nif [ -n \"${{{ROOT_ENV}+x}}\" ]; then r=set; else r=unset; fi\nprintf '%s\\n%s\\n' \"$HOME\" \"$r\" > '{capture_text}'\nexit 0\n"
```

The hook sets the child's environment in one line:

```rust
cmd.env("HOME", temp.path()).env_remove(ROOT_ENV)
```

Each test then asserts that the recorded lines are the temporary `HOME`
and `unset`.

Follow these rules for capture fakes:

- Record only `$HOME` and whether the profile-root variable is set, as
  the word `set` or `unset`. Never record its value, and never run `env`
  or `printenv`: assertion messages and fake output appear in public CI
  logs.
- Build the script text from `ROOT_ENV`.
- Assert that any path placed inside single quotes in a script contains no
  `'` and no newline.
- The `unset` line proves the removal only when the parent process has the
  variable set, as it does on a machine with login profiles. Do not set it
  in the test process to strengthen the check; that is the process-wide
  change this guide removes.

## Sites that remain

The #1765 sweep fixed every `PATH` site whose test could reach the lookup
through the file it calls. It also fixed four `HOME` sites: the `set_home`
call in `with_mermaid_env` in `mermaid_cli_tests.rs`, and the `HOME` change
and its two restore lines in `poll_tmux_sessions_parses_multiple_sessions`
in `crates/amplihack-cli/src/commands/fleet/tests.rs`. In both tests only
the child needed the temporary `HOME`, so the hook now sets it there.

`fleet/tests.rs` no longer changes `PATH` anywhere. Its eight `PATH` sites,
and the `HOME` and `AZLIN_PATH` changes in the same tests, went through
`get_azlin_path`, which read all three from the process. The fleet module
now has an `AzlinLookup` value holding them. `get_azlin_path` builds one
with `AzlinLookup::from_env()`, and the tests call the `_with` forms of
`run_setup`, `run_status`, `run_snapshot` and `run_watch` with values
pointing at their own temporary directories. The TUI dry-run test passes
its reasoner backend in through `run_tui_dry_run_with` instead of placing a
fake `claude` on `PATH`. The `HOME` sites still in that file are in tests
of code that reads `HOME` itself to find `~/.amplihack/fleet`; they are in
the second group below.

The remaining sites are listed by file, with line numbers and the group
each falls into, in
[#1787](https://github.com/rysweet/amplihack-rs/issues/1787).
They fall into three groups:

| Group | Why the site stays |
|-------|--------------------|
| `PATH` read outside the called file | The lookup happens in another file or crate, the test drives a whole command flow or the binary, or the fix would change a `pub` signature used outside the crate. |
| `HOME` read in process | The code under test reads `HOME` itself, so no child is involved. The function must take the home directory as a parameter before the test can stop setting it. |
| Helper bodies | `set_home`, `EnvGuard`, `HomeGuard` and similar helpers stay while any test calls them. |

Until those sites are fixed, a test in the same binary can still start a
program while another test has `PATH` set to a temporary directory. Fakes
that call every program by absolute path are not affected. The `azlin`
stub in `crates/amplihack-cli/src/commands/fleet/tests_issue_1570_admiral.rs`
called a bare `cat`, and the admiral tests failed whenever they ran beside
the `fleet/tests.rs` tests that set `PATH`. Those tests no longer set it,
which removes the cause; the stub still calls `/bin/cat`, so that a test
added later with a process-wide `PATH` cannot break it in the same way.

Not every remaining test meets the following requirements yet. When you
change one of these tests, make sure that it:

- holds its crate's environment lock for the whole change;
- restores the value through a guard, so that a panic cannot leave it set;
- removes `ROOT_ENV` from every child it starts.

A crate's unit tests share one test binary, so each crate has its own lock:
`env_lock()` in that crate's `src/test_support.rs`, for example in
amplihack-cli, amplihack-utils, amplihack-hooks and amplihack-memory. An
integration test under `tests/` is a separate binary and uses a lock
defined in its own file, such as `ENV_LOCK` in
`crates/amplihack-remote/tests/remote_contract.rs`.

Two searches give an approximate list of the remaining calls:

```bash
# Direct calls. A call split across lines is missed.
git grep -nE '(set_var|remove_var)\(\s*"(PATH|HOME)"' -- crates bins

# Helper calls. Guards that set other variables, such as CODEX_HOME, are
# listed too, because EnvGuard::set takes an array that often spans lines.
git grep -nE '\b(set_home|HomeGuard::set|IsolatedHome::new|EnvGuard::set|EnvVarGuard::set)\(' \
  -- crates bins | grep -vE '\bfn '
```

[#1787](https://github.com/rysweet/amplihack-rs/issues/1787) is the
complete list. Do not add a new call of either
kind for `PATH` or `HOME`. A new test that needs a different `PATH` or
`HOME` uses one of the forms in
[Set the value on the child](#set-the-value-on-the-child).

## Related

- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - `isolated(..)`, the profile-root refusal and the real-root guard (#1699)
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The other stops `isolated(..)` sets
- [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md#testing) - The flow tests that use the configure hook
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
