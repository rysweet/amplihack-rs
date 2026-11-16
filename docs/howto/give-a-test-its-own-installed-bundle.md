---
title: Give a test its own installed bundle
description: How Rust tests and shell scenarios that drive amplihack recipe run set up a private, trusted installed bundle, and how to tell a missing one from a real failure.
doc_type: howto
last_updated: 2026-11-16
related_issues: [2528, 1517, 1551, 2658]
---

# Give a test its own installed bundle

Every `amplihack recipe run` selects one bundle at start and takes its
recipes, sub-recipes, agents and tools from it. A run with no installed
bundle stops with E4 before any step, so a test that drives `recipe run`
has to provide one.

This guide is for contributors writing tests. It covers the three fixture
forms, the file modes the bundle trust checks require, how to start the
recipe, and how to recognise a test that passes for the wrong reason because
its bundle was missing.

The rules those fixtures satisfy are in
[Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md).

## Contents

- [Which form to use](#which-form-to-use)
- [Integration tests that start the binary](#integration-tests-that-start-the-binary)
- [In-crate tests that call run_recipe](#in-crate-tests-that-call-run_recipe)
- [Shell scenarios](#shell-scenarios)
- [Modes the trust checks require](#modes-the-trust-checks-require)
- [Start the recipe by name](#start-the-recipe-by-name)
- [Resolve the real runner from the pin](#resolve-the-real-runner-from-the-pin)
- [Recognise a missing bundle](#recognise-a-missing-bundle)
- [Never the real home](#never-the-real-home)

## Which form to use

| Your test | Fixture | Bundle contents |
|-----------|---------|-----------------|
| Starts the `amplihack` binary from `crates/amplihack-cli/tests/` | `crate::support::installed_bundle` | Empty `recipes/` and `agents/` |
| Calls `recipe::run` in process from inside the crate | `crate::test_support::installed_bundle_home()` | Empty `recipes/`, `agents/` and `tools/` |
| Is a shell scenario under `tests/gadugi/` | A copy of the repository's `amplifier-bundle` | The real recipes and tools |

The first two give an empty bundle, which is all a run needs when the test
names its recipe by path and no step reaches a bundled tool. Use the copy
when the behaviour under test is in a shipped recipe or tool.

## Integration tests that start the binary

`crates/amplihack-cli/tests/support/installed_bundle.rs` writes
`<home>/amplifier-bundle` with empty `recipes/` and `agents/` directories at
mode 0755, whatever the umask, and is safe to call more than once:

```rust
// crates/amplihack-cli/tests/issue_1976_real_runner_nested_run.rs
.env("HOME", &home)
.env("RECIPE_RUNNER_RS_PATH", &runner)
.env(
    "AMPLIHACK_HOME",
    crate::support::installed_bundle::home(home.join(".amplihack")),
)
```

`home(path)` creates the bundle and returns `path`, so it reads as one
argument to `.env("AMPLIHACK_HOME", ..)`. When the fixture sets
`AMPLIHACK_HOME` elsewhere, call `ensure` for its effect alone:

```rust
// crates/amplihack-cli/tests/issue_1808_run_cleanup/fixture.rs
crate::support::installed_bundle::ensure(&self.home.join(".amplihack"));
```

Both take a path under the test's temporary `HOME`. A bundle elsewhere works
too, but keeping it under `HOME` means the temporary directory's own cleanup
removes it.

## In-crate tests that call run_recipe

`crate::test_support::installed_bundle_home()` creates a temporary home with
`recipes/`, `agents/` and `tools/`, points `AMPLIHACK_HOME` at it, and clears
`AMPLIHACK_RUN_BUNDLE`, `AMPLIHACK_RUN_BUNDLE_CHOICE` and
`AMPLIHACK_STATE_HOME` so that the test does not inherit the bundle of a
recipe run it is running inside. It returns the temporary directory and an
`EnvGuard`:

```rust
// crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs
// Issue #2528: every run selects a bundle; give it a private installed one.
let (_installed, _installed_env) = crate::test_support::installed_bundle_home();

let result = super::run_recipe(/* .. */);
```

That comment and that one binding are all a test in that file needs. Do not add
`clear_outer_run_env()` beside them: the file's own `fixture_environment()`
captures every `OUTER_RUN_IDENTITY_VARS` key and removes it already, so a
second call clears nothing and restores nothing.

Three rules come with it:

- **Name both bindings.** `let _ = installed_bundle_home();` drops the
  temporary directory and the `EnvGuard` immediately, which deletes the
  bundle and restores `AMPLIHACK_HOME` before `run_recipe` is called, so the
  test falls back to the developer's real `~/.amplihack`. Write
  `let (_installed, _installed_env) = ..`.
- **Declare it last.** Bindings drop in reverse order, so declaring it after
  any other environment guard lets it restore `AMPLIHACK_HOME` before the
  outer guard restores the rest.
- **Hold the environment lock.** It changes process environment variables, so
  the test must already hold `env_lock()` or `home_env_lock()`, as for any
  `EnvGuard`.

`in_process_installs_remove_the_profile_root` and the other source
assertions described in
[Keep tests out of the real profile root](./keep-tests-out-of-the-real-profile-root.md)
are the pattern to follow when adding a check of this kind.

## Shell scenarios

A scenario that runs the real `recipe run` installs a copy of the
repository's bundle under the private `HOME` it already uses. From
`tests/gadugi/run-autodrive-crusty-rereview-scenario.sh`:

```bash
AMP_HOME="$WORK/home/.amplihack"
mkdir -p "$AMP_HOME"
cp -a "$REPO_ROOT/amplifier-bundle" "$AMP_HOME/amplifier-bundle" \
  || { echo "FAIL: could not install a bundle under $AMP_HOME"; echo "SCENARIO_FAILED"; exit 1; }
chmod 0755 "$WORK/home" "$AMP_HOME"
find "$AMP_HOME" -type d -exec chmod 0755 {} +
find "$AMP_HOME" -type f -exec chmod go-w {} +
[ -f "$AMP_HOME/amplifier-bundle/recipes/autodrive-crusty-round.yaml" ] \
  || { echo "FAIL: the installed bundle has no autodrive-crusty-round recipe"; echo "SCENARIO_FAILED"; exit 1; }
```

Then pass the home to the command under test, keeping the scenario's cleared
environment:

```bash
( cd "$WORK" && env -i HOME="$WORK/home" PATH="$SYS_PATH" TMPDIR="$WORK/tmp" \
    AMPLIHACK_SKIP_AUTO_INSTALL=1 AMPLIHACK_HOME="$AMP_HOME" \
    RECIPE_RUNNER_RS_PATH="$WORK/bin/recipe-runner-rs" \
    "$REAL" recipe run autodrive-crusty-round --dry-run )
```

Three details matter:

- **Copy `amplifier-bundle`, not the repository root.** A home that holds a
  `.git` entry is treated as a checkout: the run skips it, prints a note and
  moves to the next candidate home.
- **Keep `env -i`.** The scenario's `HOME` is a world-readable temporary
  directory, and the cleared environment keeps tokens, `SSH_AUTH_SOCK` and
  the host's `AMPLIHACK_*` values out of it. Add only the variables the case
  needs.
- **Check the copy.** Assert that the recipe the case runs is in the copy, as
  above, so a layout change fails with its own message rather than as a
  recipe-not-found error later.

The repository's gadugi scenarios run in CI. Do not run them on a shared
development host; to check one invocation by hand, reproduce just that
command under a private `HOME` in a scratch directory and remove it
afterwards.

## Modes the trust checks require

The bundle is checked entry by entry before it is copied, and the run is
refused when an entry is writable by the group or by other users, or is owned
by another user (#1551). A fixture must therefore set modes explicitly rather
than leave them to the umask: CI often runs under `umask 002`, which leaves
new files group-writable.

- Rust fixtures: both helpers call `set_permissions` with `0o755` on the
  bundle and its subdirectories. `installed_bundle_home()` sets the home it
  creates to 0755 as well; `installed_bundle::ensure` sets no mode on the
  `.amplihack` directory it is handed, which is safe only because that
  directory sits under a `tempfile` temporary `HOME` created at 0700. A
  fixture that puts the bundle under a directory of its own making must set
  that directory's mode itself.
- Shell fixtures: `find -type d -exec chmod 0755 {} +` and
  `find -type f -exec chmod go-w {} +`, plus the enclosing `HOME` and
  `.amplihack` directories. `find -type d` and `-type f` do not follow
  symlinks, so the bundle's links are copied and left as links.

A failure here reads:

```
untrusted bundle entry /tmp/.tmpXXXX/home/.amplihack/amplifier-bundle/tools/workflow_context.sh: mode 0664 (group or other writable); fix with chmod go-w /tmp/.tmpXXXX/home/.amplihack/amplifier-bundle/tools/workflow_context.sh
```

Fix the fixture's modes. Never relax the check, and never `chmod -R 0777` or
`a+w` a fixture bundle to make a run start.

## Start the recipe by name

With `AMPLIHACK_HOME` pointing at the fixture bundle, start a bundled recipe
by its bare name, as a run outside a checkout does:

```bash
"$REAL" recipe run autodrive-crusty-round --dry-run
```

A path into the repository's own `amplifier-bundle/recipes/` is refused with
E14 while the run uses the fixture bundle, because the recipe and its tools
would come from different bundles. A path into a bundle the test does not
select is refused the same way. Two paths are still accepted: a path into the
selected bundle itself, and, in a nested run, a path into the home that run
inherited, which is how an auto-drive round starts. A standalone recipe file
outside every `amplifier-bundle/recipes/` directory is accepted as before,
and takes its tools from the fixture bundle; that is why an empty bundle is
enough for most tests.

## Resolve the real runner from the pin

A test that needs steps to run needs a real `recipe-runner-rs`. Resolve it
through the shared `tests/support/pinned_runner.rs` helper, which returns a
receipt-checked binary at the revision in `claude-plugin/recipe-runner.rev`.
`crates/amplihack-cli/tests/main.rs` includes that file as a module, so a
test in the `cli_integration` target calls it as
`crate::pinned_runner::pinned_runner`:

```rust
// crates/amplihack-cli/tests/issue_2528_one_bundle.rs
fn real_runner() -> PathBuf {
    crate::pinned_runner::pinned_runner(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// The real runner at the pin; the test name stays for the call sites.
pub(crate) fn runner(_test: &str) -> PathBuf {
    real_runner()
}
```

Keep the wrapper's signature when you change how it finds the binary. The
lookup moves; no call site does. `runner` has thirteen of them, which this
lists:

```bash
grep -nE '(^|[^_[:alnum:]])runner\(' \
  crates/amplihack-cli/tests/issue_2528_one_bundle.rs \
  crates/amplihack-cli/tests/issue_2528_no_cleanup_child.rs \
  | grep -v 'fn runner'
```

Dropping the unused parameter instead would mean editing every one of them,
and the panic message it once carried is what told you which test was missing
a runner.

Do not search `$PATH` for `recipe-runner-rs` and do not accept an override
without the receipt check: either makes the test run whatever binary of that
name the host happens to offer first, and makes results depend on the host
rather than on the pin. See
[Run tests against the pinned recipe runner](./run-tests-against-the-pinned-recipe-runner.md).

## Recognise a missing bundle

With no installed bundle, the run exits 1 before any step with E4:

```
no installed amplifier-bundle found; searched: /home/alice/src/amplihack-rs (git checkout, skipped), /tmp/.tmpXXXX/home/.amplihack. To use this checkout's bundle, pass --bundle checkout
```

This is the failure that makes a test pass for the wrong reason. The #1517
crusty re-review scenario asserted that a depth-4 round was stopped by the
recursion limit; with no installed bundle both of its cases exited 1 before
the limit was reached, and the assertion on a non-zero status held while the
behaviour under test never ran.

Two habits catch it:

- assert on the message, not only on the status: the depth case now requires
  `depth 4 of max 3` in the output, which E4 cannot satisfy;
- when a run in a test is expected to reach a step, assert that it did, for
  example by checking a file the stand-in runner writes.

If a test starts failing with E4 after a rebase, the fixture is missing one
of the three forms above, not the production code.

## Never the real home

A fixture bundle always lives under a temporary directory. The Rust helpers
take a path you supply, so pass one under the test's temporary `HOME`;
`installed_bundle_home()` uses `tempdir_outside_framework_roots()` itself.
Set `AMPLIHACK_WORK_ROOT` for the run's work root as well, so bundle
snapshots are written under the temporary root and removed with it.

No test reads or writes the real `~/.claude` or `~/.amplihack`. The related
guides describe the guards that enforce this for profiles, `PATH` and `HOME`.

## Related

- [Recipe run bundle selection and pinning](../reference/recipe-run-bundle.md): the rules these fixtures satisfy, and the full error list
- [One bundle per recipe run](../concepts/one-bundle-per-recipe-run.md): why a run needs one bundle
- [Run tests against the pinned recipe runner](./run-tests-against-the-pinned-recipe-runner.md): resolving a real runner at the pinned revision
- [Keep tests out of the real profile root](./keep-tests-out-of-the-real-profile-root.md): the profile-root controls and guard
- [Keep tests from changing PATH and HOME](./keep-tests-from-changing-path-and-home.md): giving a child its own `PATH` and `HOME`
- [Keep tests passing under host load](./keep-tests-passing-under-host-load.md): waiting on conditions instead of on the clock
