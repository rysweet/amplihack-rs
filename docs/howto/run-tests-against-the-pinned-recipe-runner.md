---
title: Run Tests Against the Pinned Recipe Runner
type: howto
updated: 2026-11-14
related: issue #2658, issue #2582, issue #2290, issue #2306, issue #2096
---

# Run Tests Against the Pinned Recipe Runner

Some integration tests run a real `recipe-runner-rs`. They must run the
revision named in
[`claude-plugin/recipe-runner.rev`](../../claude-plugin/recipe-runner.rev),
not whatever happens to be installed in `~/.cargo/bin` or on `PATH`. This
guide is for contributors who run those tests, move the pin, or write a new
test that needs the runner.

The shared helper `tests/support/pinned_runner.rs` finds or builds the runner
at the pinned revision. It never uses a runner installed on the host.

## Contents

- [Which tests use the helper](#which-tests-use-the-helper)
- [Run the tests](#run-the-tests)
- [How the helper finds the runner](#how-the-helper-finds-the-runner)
- [Where the build is cached](#where-the-build-is-cached)
- [Use a runner you built yourself](#use-a-runner-you-built-yourself)
- [Use the helper in a new test](#use-the-helper-in-a-new-test)
- [Check that a test does not depend on the host](#check-that-a-test-does-not-depend-on-the-host)
- [Troubleshooting](#troubleshooting)

## Which tests use the helper

| Test target | Tests |
| --- | --- |
| `amplihack`, `--test integration` | `workflow_publish_pr_metadata::runner::actual_runner_*` (six tests) and `workflow_publish_pr_metadata::runner::persistent_awk_failure_still_serializes_success_after_exactly_one_creation`, in `tests/integration/issue_2306_publish/runner.rs` |
| `amplihack-cli`, `--test cli_integration` | `issue_2096_execution_isolation::recording_coverage::pinned_runner_shell_to_rust_to_absolute_git_keeps_required_coverage`, in `crates/amplihack-cli/tests/issue_2096_recording_coverage.rs` |

Both files call the same function, so the two targets share one build of the
runner.

## Run the tests

Run the publish tests:

```bash
cargo test -p amplihack --test integration workflow_publish_pr_metadata::runner::
```

Run the recording-coverage test:

```bash
cargo test -p amplihack-cli --test cli_integration issue_2096_execution_isolation::recording_coverage::
```

The `amplihack-cli` crate sets `autotests = false` and compiles every file in
`crates/amplihack-cli/tests/` into the one `cli_integration` target, so
`--test workflow_publish_pr_metadata` or `--test issue_2096_recording_coverage`
does not exist.

The first run on a clean target directory builds the runner. With
`--nocapture` it prints one line to stderr:

```text
building pinned recipe-runner-rs 2833a0b8bafb937f1cea19bdc4219e23ced76aef into <target-dir>/tmp/pinned-recipe-runner
```

`<target-dir>` is the absolute path of the cargo target directory.

Later runs reuse the build and print nothing about the runner. The build takes
a few minutes and needs network access to GitHub, unless the cargo git cache
already holds the pinned revision.

## How the helper finds the runner

```rust
pub(crate) fn pinned_runner(repo_root: &Path) -> PathBuf
```

The function reads the pin from `<repo_root>/claude-plugin/recipe-runner.rev`
and returns an absolute path to a `recipe-runner-rs` built at that revision.
It checks these sources in order and uses the first one that is set:

| Order | Source | Receipt checked |
| --- | --- | --- |
| 1 | `RECIPE_RUNNER_QA_BINARY` | `RECIPE_RUNNER_QA_RECEIPT` if set, otherwise `<bin>/../../.crates2.json` |
| 2 | `RECIPE_RUNNER_RS_PATH` | `<bin>/../../.crates2.json` |
| 3 | The test-private cache | `<cache>/<pin>/.crates2.json` |

`<bin>/../../.crates2.json` means the `.crates2.json` in the install root that
`cargo install --root <root>` wrote; the binary itself is in `<root>/bin/`.

Every source must pass the same receipt check. The receipt must:

- hold an `installs` key that starts with `recipe-runner-rs `;
- name the source `git+https://github.com/rysweet/amplihack-recipe-runner`,
  followed by `?`, `#` or `.git`;
- end with `#<pin>)`, where `<pin>` is the full 40-character revision;
- list `recipe-runner-rs` in that entry's `bins`.

The check confirms that the install matches the pin. It is a consistency
check, not proof of where the binary came from.

The helper does not search `PATH`, `CARGO_HOME/bin` or `~/.cargo/bin`. An
override that is relative, is not a regular file, or fails the receipt check
makes the test panic. The helper never falls back from a bad override to the
cache, and never skips the test.

The pin itself must be exactly 40 lowercase hexadecimal characters. Any other
content in `recipe-runner.rev` makes the helper panic before it builds a path
or a cargo command from it.

## Where the build is cached

The cache is in `CARGO_TARGET_TMPDIR`, which cargo sets to `<target-dir>/tmp`
for integration tests. With the default target directory that is
`target/tmp/pinned-recipe-runner/`; with `CARGO_TARGET_DIR` set it is
`$CARGO_TARGET_DIR/tmp/pinned-recipe-runner/`.

| Entry | Contents |
| --- | --- |
| `<pin>/` | The finished install root: `bin/recipe-runner-rs` and `.crates2.json` |
| `<pin>.partial/` | An install in progress; removed and redone if a build was interrupted |
| `<pin>.build/` | cargo's build directory for the runner; kept so that a rebuild is incremental |
| `<pin>.lock` | The lock file that serialises builds |

The directory is created with mode `0700`.

The build runs as:

```bash
cargo install --locked \
  --git https://github.com/rysweet/amplihack-recipe-runner \
  --rev <pin> \
  --root <cache>/<pin>.partial \
  --target-dir <cache>/<pin>.build \
  recipe-runner-rs
```

The helper uses `$CARGO`, the cargo that is running the tests, and falls back
to `cargo`. It removes `CARGO_INSTALL_ROOT`, `CARGO_TARGET_DIR` and
`CARGO_BUILD_TARGET_DIR` from the child's environment, so the build cannot
write to `~/.cargo/bin` or share the outer build's target directory and lock.

Builds happen at most once per pin and target directory:

- Inside one test binary, the result is kept in a `OnceLock`, so test threads
  call cargo at most once.
- Across test binaries, the helper holds a blocking `File::lock` on
  `<pin>.lock` while it checks the cache, builds, renames `<pin>.partial` to
  `<pin>` and checks the receipt. When the pre-push hook runs both targets at
  the same time, one builds and the other waits for the lock and reuses the
  result.

There are no sleeps, retries or timeouts. A failed build makes the test panic
with cargo's stderr.

Moving the pin needs no extra step. A new revision gets new cache entries, and
the next test run builds it.

To clear the cache, delete the directory. With the default target directory,
from the repository root:

```bash
rm -rf target/tmp/pinned-recipe-runner
```

With `CARGO_TARGET_DIR` set, delete
`"$CARGO_TARGET_DIR/tmp/pinned-recipe-runner"` instead.

## Use a runner you built yourself

CI installs the pinned runner with `scripts/install-pinned-recipe-runner.sh`.
The script installs into `$RUNNER_TEMP/recipe-runner`, so the binary is
`$RUNNER_TEMP/recipe-runner/bin/recipe-runner-rs` and the receipt is
`$RUNNER_TEMP/recipe-runner/.crates2.json`. Under GitHub Actions it then
exports `RECIPE_RUNNER_RS_PATH` through `GITHUB_ENV`, and the tests use that
install instead of building a second copy.

Outside GitHub Actions the script exports nothing, and without `RUNNER_TEMP`
it installs into a new temporary directory. To reuse an install locally, set
`RUNNER_TEMP` to a directory you choose and pass the binary path yourself:

```bash
export RUNNER_TEMP=~/.amplihack/work/recipe-runner-ci
mkdir -p "$RUNNER_TEMP"
scripts/install-pinned-recipe-runner.sh
RECIPE_RUNNER_RS_PATH="$RUNNER_TEMP/recipe-runner/bin/recipe-runner-rs" \
  cargo test -p amplihack --test integration workflow_publish_pr_metadata::runner::
```

The script never installs into `~/.cargo/bin`.

The install passes the receipt check only while it is at the current pin. If
that directory already holds a runner that is newer than the pin or has
diverged from it, the script keeps it rather than downgrading, and the receipt
check then fails. Run the script with `--force` to install the pin, or unset
`RECIPE_RUNNER_RS_PATH` to let the helper build its own copy.

To test a runner build that is not a plain `cargo install`, set
`RECIPE_RUNNER_QA_BINARY` to the binary and `RECIPE_RUNNER_QA_RECEIPT` to a
`.crates2.json` that names the pin.

## Use the helper in a new test

The helper lives in `tests/support/pinned_runner.rs` and is compiled into each
test target with `#[path]`.

In `tests/integration/main.rs`:

```rust
#[path = "../support/pinned_runner.rs"]
mod pinned_runner;
```

In `crates/amplihack-cli/tests/main.rs`:

```rust
#[path = "../../../tests/support/pinned_runner.rs"]
mod pinned_runner;
```

Call it with the repository root, then pass the path to the code under test:

```rust
let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
let runner = crate::pinned_runner::pinned_runner(&repo_root);

let output = Command::new(amplihack_binary())
    .env("RECIPE_RUNNER_RS_PATH", &runner)
    .args(["recipe", "run", recipe_path.to_str().unwrap()])
    .output()
    .unwrap();
```

Do not add your own lookup through `PATH` or `~/.cargo/bin`, and do not skip
the test when the runner is missing.

## Check that a test does not depend on the host

Run the tests with no host runner reachable: take `~/.cargo/bin` off `PATH`,
point `CARGO_HOME` at a fresh empty directory, and clear the overrides.

```bash
mkdir -p ~/.amplihack/work/pinned-runner-cargo-home
export CARGO="$(rustup which cargo)"
export RUSTUP_TOOLCHAIN="$(rustup show active-toolchain | cut -d' ' -f1)"
export CARGO_HOME=~/.amplihack/work/pinned-runner-cargo-home
export PATH="$(printf '%s' "$PATH" | tr ':' '\n' | grep -v '/.cargo/bin$' | paste -sd:)"
unset RECIPE_RUNNER_RS_PATH RECIPE_RUNNER_QA_BINARY RECIPE_RUNNER_QA_RECEIPT

"$CARGO" test -p amplihack --test integration workflow_publish_pr_metadata::runner:: -- --nocapture
"$CARGO" test -p amplihack-cli --test cli_integration issue_2096_execution_isolation::recording_coverage:: -- --nocapture
```

Both commands pass and resolve the runner under `CARGO_TARGET_TMPDIR`. A fresh
`CARGO_HOME` has no git cache, so a cold cache needs GitHub access in this
mode.

## Troubleshooting

The headings below quote the helper's panic messages. `<path>` is the
receipt's absolute path and `<pin>` is the full revision.

### "receipt <path> does not name recipe-runner-rs at #<pin>"

`RECIPE_RUNNER_RS_PATH` or `RECIPE_RUNNER_QA_BINARY` points at a runner
installed at another revision. With an install from
`scripts/install-pinned-recipe-runner.sh`, this happens when the pin has moved
since the install, or when the script kept a newer or diverged runner instead
of downgrading it. Run the script again with `--force` and the same
`RUNNER_TEMP`, or unset both variables to let the helper build its own copy.
Do not edit the receipt.

### "cargo install of recipe-runner-rs <pin> failed" followed by cargo's stderr

The build failed. Common causes:

- No network access and no cached copy of the pinned revision in
  `CARGO_HOME/git`. Run once with network access.
- The pin names a revision that does not exist upstream or does not compile
  with `--locked`. Check `claude-plugin/recipe-runner.rev`.
- The disk is full. `<pin>.build` needs one to two gigabytes.

After you fix the cause, run the tests again. The helper removes the
leftover `<pin>.partial` and builds again, reusing `<pin>.build`.

### The test seems to wait before it starts

Another test binary holds `<pin>.lock` and is building the runner. The waiting
binary continues when the build finishes. Check for a running
`cargo install ... recipe-runner-rs` process before you assume it has hung.

### "recipe-runner.rev is not a 40-character lowercase revision"

The pin file holds a short SHA, a branch name, upper-case hex or extra text.
Replace it with the full commit SHA on one line.

## Related

- [CI pipeline: pinned recipe runner](../reference/ci-pipeline.md) - How CI
  installs the pinned runner and sets `RECIPE_RUNNER_RS_PATH`
- [Publish metadata recovery diagnostics](../bugfixes/bugfix-2306-publish-metadata-recovery-diagnostics.md) -
  What the seven publish runner tests check
- [Keep Tests from Changing PATH and HOME](keep-tests-from-changing-path-and-home.md) -
  How tests give a child process its environment without changing their own
