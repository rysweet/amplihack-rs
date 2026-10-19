# CI Pipeline Reference

> [Home](../index.md) > Reference > CI Pipeline

The `CI` workflow (`.github/workflows/ci.yml`) validates every pull request and
`main` push for the `amplihack-rs` Rust workspace. It pins the Rust toolchain,
runs the workspace suite with [cargo-nextest](https://nexte.st), frees runner
disk before the heavy test job, and scopes caches so required coverage stays
reliable without exhausting GitHub-hosted runners.

For the broader resource contract (concurrency, timeouts, matrix boundaries) see
[CI Resource Discipline](ci-resource-discipline.md).

## Job graph

```
push / pull_request / merge_group
   ├─ check (Lint & Format) ───── required
   ├─ test (Test) ─────────────── required
   ├─ install-smoke (Install Smoke Test)
   └─ cross-compile (Build <target>) × 4

release ── (tag push only) needs: [test, cross-compile]
```

The validation jobs **fan out from the trigger in parallel** — `check`, `test`,
`install-smoke`, and the four `cross-compile` legs have no `needs:` dependency on
each other, so none idles waiting on `check`. Only `release` declares
`needs: [test, cross-compile]`, and it runs only on `v*` tags after `test` and
every `cross-compile` leg succeed.

Running `test` concurrently with `check` (rather than behind it) roughly halves
the required-check critical path; a `check` failure still blocks the merge
independently, so correctness is unchanged. See
[CI Wall-Clock Parallelization](ci-wallclock-parallelization.md) for the timing
rationale and the tag-conditional release-optimization profile.

## Toolchain pinning

The workspace pins its Rust toolchain in `rust-toolchain.toml` at the repository
root, so local `cargo`, pre-commit, and CI all resolve to the **same** compiler
and lint set. A new stable release cannot introduce surprise clippy/fmt failures
because the pinned channel does not move until the file is bumped (this replaces
the `@stable` drift that forced the emergency lint fix in PR #878).

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.97.0"
components = ["rustfmt", "clippy"]
```

Every job in **every** build workflow installs the toolchain with a
version-pinned `dtolnay/rust-toolchain@1.97.0` action that matches the file.
This includes `ci.yml` (build, test, clippy, and the cross-compile matrix), the
`publish-snapshot.yml` release matrix, and the **Auto Release**
(`release.yml`) cross-build matrix. Cross-compile legs add
`targets: ${{ matrix.target }}` on top so the target's `rust-std` is installed
into the pinned toolchain.

`rust-toolchain.toml` is authoritative: at build time `rustup` always resolves
to the file's channel, regardless of which channel the action selected. That
override is exactly why the action ref and the file **must** name the same
version — a mismatch is not merely slower, it can be fatal on cross-compile
legs:

> **Why `@stable` breaks cross builds (issue #935).** When a workflow used
> `dtolnay/rust-toolchain@stable` with `targets: ${{ matrix.target }}`, the
> action installed the _stable_ toolchain and added the cross target's
> `rust-std` **to stable**. At build time `rust-toolchain.toml` overrode
> resolution back to `1.97.0` — a toolchain that never received the cross
> target's `rust-std` — so `cargo build` failed with
> `error[E0463]: can't find crate for std`. The `aarch64-unknown-linux-gnu` leg
> (built on `x86_64` `ubuntu-latest`) failed on every `main` push. Pinning the
> action to `@1.97.0` installs `rust-std` into the same toolchain that is used
> at build time, resolving the error. The build matrix in `ci.yml` never hit
> this because it was already pinned to `@1.97.0`.

The rule: any `dtolnay/rust-toolchain@<ref>` that supplies `targets:` for a
cross build **must** equal the `rust-toolchain.toml` channel. Prefer keeping
_all_ build-job refs pinned to the file's channel so target `rust-std` always
lands in the resolved toolchain.

### Auto Release cross-build legs (`release.yml`)

The **Auto Release** workflow (`.github/workflows/release.yml`) builds a
per-target artifact matrix. Its native-arch legs run on a host whose
architecture already matches the target, but two legs cross-build:

| Target                       | Runner          | Kind        |
| ---------------------------- | --------------- | ----------- |
| `aarch64-unknown-linux-gnu`  | `ubuntu-latest` | cross-arch  |
| `x86_64-apple-darwin`        | `macos-latest`  | cross-arch  |

Both cross legs install the toolchain with `dtolnay/rust-toolchain@1.97.0` and
`targets: ${{ matrix.target }}`, matching `rust-toolchain.toml`:

```yaml
# .github/workflows/release.yml (cross-build matrix step)
- uses: dtolnay/rust-toolchain@1.97.0
  with:
    targets: ${{ matrix.target }}
```

Because the action ref equals the file's channel, the target's `rust-std` is
added to the **same** `1.97.0` toolchain that `rust-toolchain.toml` resolves to
at build time. This is exactly what fixed the every-`main`-push failure in which
both cross legs aborted with `error[E0463]: can't find crate for std` while the
native legs passed (issue #939). It is the same drift class first resolved for
`publish-snapshot.yml` in #935/#948; `release.yml` is now pinned and fully
covered by the regression guard below — there is no remaining tracked drift.

### Release tags point at a stamp commit

`main` keeps the base version (`MAJOR.MINOR.0`); the patch number comes from
the latest `vMAJOR.MINOR.*` tag. The `release` job runs
`scripts/stamp-release-version.sh X` on the released SHA, which sets the
version in `Cargo.toml`, the workspace-member entries of `Cargo.lock` and
`package.json`, commits the result as `chore(release): stamp vX` on a detached
HEAD and tags that commit (or the released SHA itself when the manifests
already say X). Nothing is pushed to `main`. `cargo install --git … --tag vX`
therefore reports X, while `--rev <main SHA>` still reports `MAJOR.MINOR.0`;
the self-heal [downgrade guard](../features/self-heal-asset-restage.md#never-an-implicit-downgrade-1526)
covers those builds. Guarded by
`crates/amplihack-cli/tests/issue_1526_release_version_stamp.rs`.

The workflow token is read-only; only the `release` job has `contents: write`.
No checkout persists credentials. In the `release` job the token is passed
only to the steps that query origin for an existing tag and push the new one,
never to the step that runs the stamp script. Release artifacts are built from
the released `main` SHA, so they differ from the tagged stamp commit only in
the manifests.

### Bumping the pinned toolchain

Bump deliberately, in its own PR:

1. Edit `channel` in `rust-toolchain.toml`.
2. Update every `dtolnay/rust-toolchain@<version>` ref in the build workflows to
   match — the four refs in `ci.yml`, the ref in `publish-snapshot.yml`, and the
   cross-build ref in `release.yml`.
3. Run `cargo clippy -- -D warnings && cargo fmt --check && cargo nextest run
   --workspace --locked` locally, fix any new lints, and open the PR.

Keep the file and the workflow action refs in lockstep so the review stays
focused on lint fallout rather than mixing it with feature work.

### Regression guard: toolchain-ref invariant

To stop this drift class from recurring, a static check asserts that every
build-job `dtolnay/rust-toolchain@<ref>` in `.github/workflows/*.yml` equals the
`rust-toolchain.toml` channel:

```bash
scripts/check-toolchain-refs.sh
```

The script reads the pinned `channel` from `rust-toolchain.toml`, scans the
workflows for `dtolnay/rust-toolchain@<ref>` occurrences that provide a
`targets:` input (i.e. build/cross legs), and exits non-zero on any mismatch —
for example, if a leg reverts to `@stable`. Toolchain steps with **no**
`targets:` input (such as the lint-only job in `invisible-char-scan.yml`, which
runs on `@stable` by design) do not install per-target `rust-std`, are not
exposed to the `E0463` drift, and are outside the invariant. Run the guard locally before pushing; it is also wired
into CI and pre-commit, so a mismatched or floating ref fails fast instead of
surfacing as an `E0463` in a release build.

The guard's allowlist (`ALLOWLIST`) is **empty**: every targets-bearing ref —
across `ci.yml`, `publish-snapshot.yml`, and `release.yml` — must pin to the
`rust-toolchain.toml` channel, with no tracked exceptions. A clean run reports:

```text
check-toolchain-refs: OK — all targets-bearing toolchain refs pinned to @1.97.0
```

and exits `0`. Add an entry to `ALLOWLIST` only to track a genuinely
out-of-scope, in-progress pinning follow-up; allowlisted drift is surfaced as a
visible `WARNING` (never silently ignored) and must be removed once the target
workflow is pinned.

## Shell strictness: no early-exit pipeline stages

GitHub's `shell: bash` runs a step body as `bash --noprofile --norc -eo pipefail`,
but that strictness stops at the step: a step whose body is `bash tests/foo.sh`
starts a **child** shell with neither flag, and `foo.sh` sets its own. Recipe
steps, by contrast, run their bodies under `set -euo pipefail` directly. The two
environments therefore disagree, and a script can be green in CI while returning
a silent empty value in production.

The shape that exploits the gap is a pipeline stage that stops reading before its
producer is done:

```bash
SLUG="$(printf '%s' "$TASK_DESCRIPTION" | head -c 65536 | tr … | sed …)"
```

`head` stops once satisfied, leaving `printf` writing into a closed pipe. The
producer dies of `SIGPIPE` — or, where `SIGPIPE` is **ignored** (a disposition
that survives `exec` and is common in CI), bash's `printf` reports
`write error: Broken pipe` and returns 1. `pipefail` promotes either outcome to
the pipeline's status, the command substitution yields `""`, and under `set -e`
the step dies. This is the mechanism behind #1426.

Whether it fires at a given input size is a **race on the 64 KB pipe buffer**.
The same code was green on bash 5.3.9 and red on the runner's 5.2.21, and an
earlier 300-run attempt to reproduce a sibling failure came back clean. Execution
cannot establish absence, so the invariant is asserted **statically**:

```bash
bash tests/issue_1434_no_early_exit_pipelines.sh
```

The guard derives its script list from `.github/workflows/*.yml` rather than from
a path glob — a `tests/` glob misses the 17 scripts under
`amplifier-bundle/recipes/tests/` — and fails on any `| head`, `| tail`, or
`grep -m` stage in them. It first runs itself against known-bad and known-good
fixtures, so a detector that has stopped discriminating fails loudly instead of
passing vacuously. #1429's `D4-no-early-exit` applies the same rule to the
branch-name derivation inside `workflow-worktree.yaml`.

**Fix by removing the early exit, never by suppressing the status.** Use a shell
substring (`${VAR:0:N}`), a single `awk`, or any consumer that reads all of its
input. `|| true` silences the crash and leaves the empty value, which is the
worse half of the bug. A whole-line comment is ignored by the guard, and a line
that genuinely needs an early exit can carry `# early-exit-ok: <reason>`.

## Test execution

The `test` job runs the workspace suite with `cargo-nextest`, a parallel runner
that schedules each test as its own process: faster startup, more even core use,
and lower peak disk than `cargo test` because it does not hold every test binary
resident at once.

```yaml
test:
  env:
    CARGO_PROFILE_TEST_DEBUG: "0"
  steps:
    - uses: actions/checkout@v4
    - uses: jlumbroso/free-disk-space@<sha>   # see "Disk management"
    - uses: dtolnay/rust-toolchain@1.97.0
    - uses: taiki-e/install-action@<sha>      # tool: nextest (SHA-pinned)
    - uses: Swatinem/rust-cache@v2
      with:
        cache-targets: false
        save-if: ${{ github.ref == 'refs/heads/main' }}
    # Install pinned recipe runner here (see the dependency contract below).
    - run: cargo nextest run --workspace --locked
    - run: cargo test --workspace --doc --locked
```

- **`CARGO_PROFILE_TEST_DEBUG: "0"`** drops debug info from the many test
  binaries, shrinking link time and on-disk size; the suite asserts behavior,
  not backtraces.
- **Pinned recipe runner:** before nextest, the `Install pinned recipe runner`
  step validates `claude-plugin/recipe-runner.rev` as exactly one immutable
  40-hex revision and installs it with `cargo install --git ... --rev "$rev"
  --locked`. Its install and build roots are `$RUNNER_TEMP/recipe-runner` and
  `$RUNNER_TEMP/recipe-runner-target`. CI publishes the bin directory through
  `GITHUB_PATH` and the absolute executable through `RECIPE_RUNNER_RS_PATH`.
  Invalid pins or installation failures stop the job; terminal-state fixtures
  require this real executable and retain their missing-runner failure.
- **`cargo test --workspace --doc --locked`** runs after nextest because nextest
  does not execute doctests — the explicit step preserves that coverage.
- **`cache-targets: false`** — the workspace links large native artifacts (Kuzu,
  cxx); restoring `target/` can exhaust runner disk before tests start, so
  target output is never cached (issue #744). nextest lowers peak disk; caching
  `target/` here would fight that.
- **One process locally, one per test in CI.** Every file in
  `crates/amplihack-cli/tests/` is a module of one test binary,
  `cli_integration` (crate root `crates/amplihack-cli/tests/main.rs`; the
  package sets `autotests = false`). A former target is selected with a module
  filter, for example
  `cargo test -p amplihack-cli --test cli_integration cargo_cxx_pin::`.
  nextest runs each test in its own process, but a local `cargo test` runs all
  of them as threads of one process. A test that changes `HOME`, `PATH` or the
  working directory in-process therefore runs its body in a child process of
  the binary (`support::quiet_process::in_quiet_process`) and makes the change
  there with `child_set_var`, `child_remove_var` or `child_set_current_dir`,
  which panic anywhere else. Stubs that tests exec are written with
  `support::exec_stub`, so this process never holds a write descriptor on them
  (ETXTBSY). Guards in
  `tests/integration/test_harness_binary_path_contract_test.rs` fail on a `.rs`
  file at any depth that no module declaration reachable from `main.rs` loads,
  on a `#[cfg]` on a `mod` line in `main.rs`, on a test target other than
  `cli_integration` and `lbug_ffi` (from `cargo metadata`), and on a
  `set_var`, `remove_var` or `set_current_dir` call outside
  `support/quiet_process.rs`. Feature-gated modules such as `signal_*` build
  only with their feature, so pass `--features signal` to run them locally.

## Integration test binary convention

Integration tests that shell out to a built binary resolve its path with the
Cargo-provided environment variable, never a hardcoded `target/…` string:

```rust
let bin = env!("CARGO_BIN_EXE_amplihack");        // amplihack CLI
let bin = env!("CARGO_BIN_EXE_amplihack-hooks");  // hooks binary
```

`CARGO_BIN_EXE_<name>` is set for every `[[test]]` target owned by the package
that produces the binary, points at the exact artifact for the active profile,
and forces Cargo to build the binary **before** the test runs — eliminating the
`amplihack binary not found at target/debug/amplihack` transient. Each such test
is therefore compiled as part of a `[[test]]` target in the owning package's
`Cargo.toml` (`bins/amplihack` for `amplihack`, `bins/amplihack-hooks` for
`amplihack-hooks`).

For `bins/amplihack`, every file in `tests/integration/` is a **module of one
test binary**: the single `integration` target, whose crate root is
`tests/integration/main.rs`. Do not add a new `[[test]]` entry per file. Add the
file to `tests/integration/` and declare it in `main.rs` with
`#[path = "<file>.rs"] mod <name>;`. Each separate test target is another
binary that has to be linked, and sccache cannot cache test binaries, so the
CI Test job pays for each one on every run. Merging the 74 former targets
into one cut link work in the uncacheable tail roughly in half (measured
locally: 235 -> 162 links, 872 s -> 324 s of link time). Run one former
target's tests with:

```bash
cargo test -p amplihack --test integration ci_speedup_optimization::
cargo nextest run -p amplihack -E 'binary(integration) and test(/^ci_speedup_optimization::/)'
```

The package's own `bins/amplihack/tests/` directory follows the same rule.
Each file there is a module of the `amplihack_bin` target, whose crate root is
`bins/amplihack/tests/main.rs`, and the package sets `autotests = false` so
Cargo does not also build each file as a target of its own. A module keeps the
name of its former target:

```bash
cargo test -p amplihack --test amplihack_bin spec_conformance_process::
```

`every_amplihack_bin_test_file_is_wired_into_a_test_binary`, in
`tests/integration/test_harness_binary_path_contract_test.rs`, fails on any
`.rs` file under that directory, at any depth, that no module declaration
reachable from `main.rs` loads, and on a `#[cfg]` placed on a `mod` line, in
any reached file, whose module lies inside that directory. A gate belongs
inside the module file instead. The
`tests/integration` guard applies the same checks to the `integration` root,
and the test-target list is read from `cargo metadata`, not parsed from
`Cargo.toml`.

## Disk management

The `test` job frees runner disk with `jlumbroso/free-disk-space` before
installing the toolchain, deterministically reclaiming tens of gigabytes and
replacing the earlier hand-rolled `sudo rm -rf …` step (issue #744).

```yaml
- uses: jlumbroso/free-disk-space@<sha>
  with:
    tool-cache: false      # keep the Rust toolchain cache we depend on
    android: true
    dotnet: true
    haskell: true
    docker-images: true
    large-packages: false  # slow apt purge, not needed for the win
    swap-storage: false
```

The step is scoped to `test` (highest peak footprint). `install-smoke` and
`cross-compile` build the release profile without the test artifacts and have
not hit the disk ceiling. If a future dependency pushes them against the limit,
add the same step rather than caching `target/`.

## Caching

All jobs use `Swatinem/rust-cache@v2` with two shared rules:

- **`save-if: ${{ github.ref == 'refs/heads/main' }}`** — only `main` writes the
  cache; PR runs restore but never save, preventing fork/PR cache poisoning and
  per-branch bloat.
- **`cache-targets: false`** — only the registry and Git dependency caches are
  stored, keeping restores small and disk-safe.

`install-smoke` shares the `x86_64-unknown-linux-gnu` cache key with the Linux
build leg; each `cross-compile` leg keys by `matrix.target`. CI always passes
from a cold cache — caching is an optimization only.

## Recipe runner pin ancestry

The `Recipe runner pin` workflow (`.github/workflows/recipe-runner-pin.yml`)
runs one job, `Recipe runner pin ancestry`, which calls
`scripts/check-recipe-runner-pin.sh` on `pull_request`, `merge_group` and
`push` to `main`. It fails when `claude-plugin/recipe-runner.rev` moves to a
runner commit that is an ancestor of, or has diverged from, the pin it replaces
in `github.com/rysweet/amplihack-recipe-runner`. A forward move or an equal pin
passes.

Two commits are read for each event. The baseline decides whether the pin has
changed and starts the range searched for a rollback trailer. The comparison
commit supplies the pin that the new pin is ordered against:

| Event | Baseline commit | Comparison commit |
| --- | --- | --- |
| `pull_request` | Merge-base of the pull request head and its base branch | Tip of the base branch (`pull_request.base.sha`) |
| `merge_group` | `merge_group.base_sha` | `merge_group.base_sha` |
| `push` | The push event's `before` SHA | The push event's `before` SHA |

For a pull request this means that a branch which leaves the pin alone is
never flagged after `main`'s pin moves on, while a stale branch that changes
the pin is judged against `main` as it is now. For example, a branch cut when
the pin was `a70d522` that moves it to `b4a82e7` fails if `main` has since
moved to `4d5187e`, because `b4a82e7` is an ancestor of `4d5187e`.

A push whose `before` SHA is all zeros, as when a branch is created, has no
baseline and passes with a notice. If the pin is unchanged against the
baseline, the check passes without contacting the runner repository. A missing
or malformed head pin, an unknown SHA or an unreachable runner repository fails
the check whatever the commit messages say. An absent or malformed pin on the
baseline passes with a notice, so that a broken pin on `main` cannot block the
change that repairs it. The job runs with `contents: read`, no secrets and
`persist-credentials: false`, and push runs are never cancelled.

An intentional rollback needs a `Recipe-Runner-Rollback` trailer with a
non-empty reason on a commit in the range from the baseline to the head:

```text
fix(runner): roll back to b4a82e7 while rysweet/amplihack-recipe-runner#163 is investigated

Recipe-Runner-Rollback: 4d5187e breaks Windows path quoting; see rysweet/amplihack-recipe-runner#163
```

The script reads trailers with Git's own trailer parser,
`git log --format='%(trailers:key=Recipe-Runner-Rollback,valueonly)'`, which
recognises only the trailer block in the final paragraph of a message. A
`Recipe-Runner-Rollback:` line anywhere else in the message is ignored, and the
check fails as if no trailer were present. When a trailer is found, the check
passes and writes both SHAs, the relationship and every reason to the log and
the step summary. The trailer is an audit record, not an approval; code review
remains the control.

Four limits apply. On push the check reports after the merge has happened. A
squash merge keeps the trailer only if it survives in the final paragraph of
the squashed commit message, and no override is inferred when it does not. The
`merge_group` trigger, and the same trailer caveat for merge-queue commits, are
inactive until the repository has a merge queue; [Merge Flow](merge-flow.md)
explains why none is available today. A push whose `before` commit is absent
after a force-push exits 2, because there is no base to compare against; rerun
or re-push to clear it.

The check was added under issue #1593. `main`'s pin went from `a70d522` to
`b4a82e7` and then to `4d5187e`, a descendant of `b4a82e7`; it was not rolled
back on `main`. The `Argument list too long` failures that prompted the check
returned locally when a runner built from `4d5187e`, which contains
rysweet/amplihack-recipe-runner#161 and
rysweet/amplihack-recipe-runner#162, was replaced in `~/.cargo/bin`
by one built from `b4a82e7`. The [installer downgrade
guard](recipe-runner-validation.md#installer-downgrade-guard) prevents that
replacement; this check prevents the equivalent change reaching `main`.

## Required checks and merge governance

Branch protection on `main` requires **Lint & Format** (`check`), **Test**
(`test`), and **Build x86_64-unknown-linux-gnu** (the Linux x86_64
`cross-compile` leg). The three remaining `cross-compile` legs
(`aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`)
still run on every PR for signal but are not required-to-merge, so scarce
macOS/cross-arch runners cannot block a merge by sitting queued. That coverage
is **relocated, not dropped**: it must pass in the merge queue and on tagged
release builds before any artifact publishes.

**Recipe runner pin ancestry**, described
[above](#recipe-runner-pin-ancestry), becomes a required check only when an
administrator adds it to branch protection by hand. Branch protection matches
the job name `Recipe runner pin ancestry`, not the job ID `recipe-runner-pin`,
so the job's `name:` in `.github/workflows/recipe-runner-pin.yml` must stay
exactly as written.

A GitHub **merge queue** (`merge_group:` trigger) batches approved PRs and runs
the required checks once per batch instead of once per serialized merge.

> Configuring branch protection, required checks, and the merge queue needs
> repository-admin rights and is applied through repository settings, not this
> workflow file.

## Action pinning and token permissions

Third-party actions that execute during a job are pinned to a full 40-character
commit SHA with a trailing version comment (`taiki-e/install-action`,
`jlumbroso/free-disk-space`), so a moved tag cannot change what runs. First-party
GitHub actions and the widely used `Swatinem/rust-cache` / `dtolnay/rust-toolchain`
keep their major-version / channel tags.

The workflow declares a least-privilege `permissions: contents: read` at the top
level; only `release` escalates to `contents: write`, and only on `v*` tags.
Because CI triggers on `pull_request` (not `pull_request_target`), fork PRs run
read-only with no secret access — the explicit block makes that visible and
stops a future job from inheriting broad default permissions.

## Related references

- [CI Wall-Clock Parallelization](ci-wallclock-parallelization.md) — parallel
  fan-out job graph and tag-conditional release optimization.
- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
- [Contributing (Rust)](../../CONTRIBUTING_RUST.md) — local build, test, and PR
  process.
