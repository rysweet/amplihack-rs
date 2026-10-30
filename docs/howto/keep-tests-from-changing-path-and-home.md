---
title: Keep Tests from Changing PATH and HOME
type: howto
updated: 2026-10-30
related: issue #1765, issue #1787, issue #1699
---

# Keep Tests from Changing PATH and HOME

A unit test that calls `std::env::set_var("PATH", ..)` or
`std::env::set_var("HOME", ..)` changes the variable for every test in the
same test binary, not only for itself. This guide is for contributors who
write tests. It explains how to give a child process its `PATH` and `HOME`
without changing the test process, and how to test a timeout so that host
load cannot fail it.

## Contents

- [The rule](#the-rule)
- [Set the value on the child](#set-the-value-on-the-child)
- [Bound a timeout without measuring speed](#bound-a-timeout-without-measuring-speed)
- [Keep capture tests separate from timeout tests](#keep-capture-tests-separate-from-timeout-tests)
- [Tests that still change the variables](#tests-that-still-change-the-variables)
- [Related](#related)

## The rule

libtest runs the tests of one binary on parallel threads of one process, and
`PATH` and `HOME` belong to the process. While one test has changed them,
any other test that starts a program by bare name looks it up in the wrong
`PATH`, any child started by any thread copies the wrong `HOME`, and code
that reads `HOME` in process sees the wrong directory. `env_lock()` does not
prevent this: tests that only read the variables, or start a program by
name, never take the lock.

The opposite mistake causes equal harm. A child that inherits the test
process's environment unchanged gets the developer's `HOME` and, on a
machine with login profiles, `AMPLIHACK_CLAUDE_PROFILE_ROOT` (#1699).

So:

- Do not call `set_var` or `remove_var` for `PATH` or `HOME` in a test.
- Every child a test starts runs with a temporary `HOME` and without
  `ROOT_ENV`, set on the child's `Command`.

`crates/amplihack-utils/tests/no_global_path_mutation.rs` fails any unit
test in amplihack-utils that sets or removes `PATH`.

## Set the value on the child

`Command::env` and `Command::env_remove` change only the child's
environment. On Unix, a `PATH` set this way is also used to find a program
given by bare name. A test needs a way to reach the `Command` that
production code builds. Choose the form by what the code does:

| Production code | Form |
|-----------------|------|
| Builds a `Command` with setup the test must keep (scrubbing, arguments, stdio, `setsid`) | [Configure hook](#configure-hook) |
| Calls only `Command::new` and `args` | [Prepared `Command`](#prepared-command) |
| Searches the `PATH` directories itself | [`PATH` value](#path-value) |

If the code under test reads neither variable, delete the change.

### Configure hook

Add a `_with` variant that takes `configure: &dyn Fn(&mut Command)` and
calls it after the existing setup, immediately before the child is spawned.
The original function keeps its signature and passes a no-op, so production
children are built exactly as before. A function that starts several
children passes the same hook to each.

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

The test passes a hook that sets the child's environment:

```rust
use crate::commands::profile::ROOT_ENV;

let outcome = ensure_mermaid_cli_with(&|cmd: &mut Command| {
    cmd.env("PATH", &bin_dir).env("HOME", &home).env_remove(ROOT_ENV);
})
.unwrap();
```

Rules for every hook:

- Keep the `_with` function private, or `pub(super)` when a sibling `tests`
  module needs it. Production reaches it only through the original function
  with a no-op. The hook runs after any scrubbing, so only tests may pass
  one.
- A test hook sets `HOME` to the test's temporary directory and removes
  `ROOT_ENV`. When the code starts a program by bare name, it also sets
  `PATH` to the stub directory alone. Do not append the parent's `PATH`:
  the test would then run the developer's real `git`, `npm` or `gh`.
- Write each fake with `#!/bin/sh` and call every program in it by absolute
  path (`/bin/sleep`, `/bin/cat`).
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

The test builds the `Command`, sets `HOME` and removes `ROOT_ENV` on it, and
passes its own timeout. No test-only environment variable is needed to
shorten the timeout.

### `PATH` value

When the code searches the `PATH` directories itself instead of starting a
child, pass the value in as `Option<OsString>`. Production passes
`std::env::var_os("PATH")`; the test passes a value it built:

```rust
fn find_preferred_rustyclawd_binary() -> Option<PathBuf> {
    find_preferred_rustyclawd_binary_in(env::var_os("PATH"))
}
```

Split the value with
`amplihack_utils::launch_target::split_path_var_of(path_var, RelativeEntries::Drop)`.
`Drop` discards empty and relative elements, which a shell would read as the
current directory.

## Bound a timeout without measuring speed

A test that runs a fake sleeping 2 s and asserts the call returned within
1 s measures the host's speed as well as the timeout, and fails on a loaded
host. Write the test so that host speed cannot decide it:

1. The fake waits far longer than the bound. Its last command is
   `exec /bin/sleep 60`. `exec` makes `sleep` the direct child, so killing
   the child kills the `sleep`.
2. The test asserts that the call returned in under 30 s, with the elapsed
   time in the message.
3. The test then asserts that the call reported failure, which catches a
   call that returned early for another reason, such as a spawn error.
4. The test uses the production timeout, or passes one to the seam. It adds
   no environment variable to shorten the timeout.

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

To check that a process-group kill also stopped a background child, have the
fake start one that writes a marker file after a few seconds, and assert
after that delay that the marker does not exist. Do not signal a pid read
from a file: the pid may have been reused by then.

When you write or change a bound test, raise the timeout it uses to 120 s by
hand, run the test alone, confirm that it fails on the 30 s bound, and set
the timeout back by hand. Do not use `git checkout -- <file>` for this; it
discards the test you are writing.

## Keep capture tests separate from timeout tests

A test that checks the child's environment needs the fake to finish its
write, and a short timeout on a loaded host can kill the fake first. Use two
tests: a timeout test whose fake only sleeps, and a capture test whose fake
records what it saw and exits 0:

```rust
"#!/bin/sh\nif [ -n \"${{{ROOT_ENV}+x}}\" ]; then r=set; else r=unset; fi\nprintf '%s\\n%s\\n' \"$HOME\" \"$r\" > '{capture_text}'\nexit 0\n"
```

The test asserts that the recorded lines are the temporary `HOME` and
`unset`. Rules for capture fakes:

- Record only `$HOME` and whether the profile-root variable is set. Never
  record its value, and never run `env` or `printenv`: fake output appears
  in public CI logs.
- Build the script text from `ROOT_ENV`.
- Assert that any path placed inside single quotes in a script contains no
  `'` and no newline.
- Do not set the profile-root variable in the test process to strengthen
  the check; that is the process-wide change this guide removes.

## Tests that still change the variables

Some tests still change `PATH`, `HOME` or `AZLIN_PATH` for the whole
process. [#1787](https://github.com/rysweet/amplihack-rs/issues/1787) lists
them. Find the current sites with:

```bash
git grep -nE '(set_var|remove_var)\(\s*"(PATH|HOME|AZLIN_PATH)"' -- crates bins
git grep -nE '\b(set_home|HomeGuard::set|IsolatedHome::new|EnvGuard::set|EnvVarGuard::set)\(' \
  -- crates bins | grep -vE '\bfn '
```

Do not add a new one. When you change one of these tests, move it to a form
in [Set the value on the child](#set-the-value-on-the-child). If it must
stay for now, hold the crate's `env_lock()` for the whole change, restore
the value through a guard, and remove `ROOT_ENV` from every child it starts.

## Related

- [Keep Tests out of the Real Profile Root](./keep-tests-out-of-the-real-profile-root.md) - `isolated(..)`, the profile-root refusal and the real-root guard (#1699)
- [Keep Tests from Leaking Rotator Daemons](./keep-tests-from-leaking-rotator-daemons.md) - The other stops `isolated(..)` sets
- [Best-Effort Mermaid CLI Provisioning](../features/mermaid-cli-best-effort-install.md#testing) - The flow tests that use the configure hook
- [Developing amplihack](./develop-amplihack.md) - Build and test setup
