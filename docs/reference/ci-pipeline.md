# CI Pipeline Reference

> [Home](../index.md) > Reference > CI Pipeline

The `CI` workflow (`.github/workflows/ci.yml`) validates every pull request and
`main` push for the `amplihack-rs` Rust workspace. It pins the Rust toolchain,
runs the workspace suite with [cargo-nextest](https://nexte.st), frees runner
disk before the heavy test job, and scopes caches so required coverage stays
reliable without exhausting GitHub-hosted runners.

For the broader resource contract (concurrency, timeouts, matrix boundaries) see
[CI Resource Discipline](ci-resource-discipline.md). For the rule that decides
which commits reach a published release, see
[A release follows green CI on the same commit](#a-release-follows-green-ci-on-the-same-commit).

## Job graph

```
push / pull_request / merge_group
   ├─ check (Lint & Format) ───── required
   ├─ test (Test) ─────────────── required
   ├─ install-smoke (Install Smoke Test) ── required
   └─ cross-compile (Build <target>) × 4 ── required (all 4 legs)

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

CI's `aarch64-unknown-linux-gnu` check runs natively on `ubuntu-24.04-arm`.
It uses the runner's C/C++ toolchain, avoiding the GNU cross-toolchain package
installation that failed when Ubuntu's package mirrors could not be resolved.
The target and required check name remain `aarch64-unknown-linux-gnu`.

The two macOS legs of the `ci.yml` build matrix also run
`cargo check --locked --tests -p amplihack-cli -p amplihack-utils -p amplihack-hooks`.
The release build compiles neither `cfg(test)` nor `debug_assertions` code,
so without this step no macOS job compiles the Unix test fixtures or
`amplihack-utils`' debug-only probe capture path (`binary_finder::to_exit`).
The step uses the dev profile, which keeps `debug_assertions` on; with
`--release` that code would not be compiled.

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

### A release follows green CI on the same commit

**Auto Release** (`.github/workflows/release.yml`) publishes a release only
after the `CI` workflow has passed **on the exact commit being released**. It is
not started by the push to `main`; it is started by CI finishing:

```yaml
# .github/workflows/release.yml
on:
  workflow_run:
    workflows: [CI]
    types: [completed]
  workflow_dispatch:
```

Auto Release previously ran on `push: branches: [main]` — a sibling of CI on the
same event rather than a consumer of its result — so it built and published
while CI was still deciding. Two releases went out over red CI and were then
installed on hosts: `v1.2.163` (Auto Release run 43619141276, CI run
43619141356 failed) and `v1.2.164` (Auto Release run 43647760173, CI run
43647760163 failed). Issue #2740.

#### What admits a run

The `version-bump` job carries the whole admission rule. `build` and `release`
reach it through `needs:`, so when `version-bump` is skipped nothing downstream
starts. All five clauses, read from `github.event.workflow_run`, must hold:

| Clause                                           | What it excludes                                                             |
| ------------------------------------------------ | ---------------------------------------------------------------------------- |
| `conclusion == 'success'`                        | A CI run that failed, was cancelled, or timed out.                           |
| `head_branch == 'main'`                          | CI on a topic branch.                                                        |
| `event == 'push'`                                | CI from `pull_request`, from `merge_group`, CI's twice-weekly `schedule`, and CI's own `workflow_dispatch` run (the `simulate_sccache_missing` fallback test). |
| `head_repository.full_name == github.repository` | A CI run whose commit came from a fork.                                      |
| no `[skip ci]` in `head_commit.message`          | A commit whose author asked for no automation.                               |

`workflow_run` is a privileged trigger — the run receives repository secrets and
a write-capable token — and `ci.yml` runs on `pull_request` in a public
repository. These clauses are therefore a security boundary, not a convenience
filter: they are joined with `&&`, and the only other way into the release jobs
is `github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'`.

Passing all five clauses admits the run, not the commit. One more condition is
checked inside `version-bump`'s `bump` step, because it cannot be expressed as
a job-level `if:`: on the `workflow_run` path the tested commit must still be
the tip of `origin/main`. A CI re-run on a superseded commit satisfies every
clause in the table above, so this is the check that stops it — see
[What this guarantees](#what-this-guarantees).

The release's own tag push does not start a second release. The tag is pushed
with `secrets.GITHUB_TOKEN` (`release.yml`), and GitHub creates no workflow run
for a push authenticated with that token, so `ci.yml`'s `push: tags: ["v*"]`
trigger does not fire and no `workflow_run` event reaches this workflow. Were
the push ever moved to a personal access token, the loop would still be closed
structurally: a tag push's `head_sha` is the stamp commit, which already
carries its series tag, and `version-bump` short-circuits on a commit that has
already been released (see
[Release tags point at a stamp commit](#release-tags-point-at-a-stamp-commit)).
It then reports the existing version rather than computing a new one, and the
`release` job finds that tag already on the remote and skips stamping and
pushing. The tip comparison stops that path sooner still, since a stamp commit
is a child of `main`'s tip rather than the tip itself. No new tag is created
and no version number is consumed.

#### Which commit is built

`version-bump` checks out `github.event.workflow_run.head_sha` — the commit CI
tested — rather than the current head of `main`, and republishes it as its `sha`
output. `build` and `release` check out that output, so the commit is resolved
in one place and the three jobs cannot disagree about it. Before stamping, the
`release` job compares `git rev-parse HEAD` with that output and stops if they
differ.

Each run records where it came from in its job summary, so a disputed release
can be traced back to the CI run that admitted it:

```text
Admitted by CI run 43712904881 · conclusion success
commit 5bc4defc4 · branch main
```

A manually dispatched run has no triggering CI run to name, so it records the
run its lookup matched instead, by URL:

```text
Admitted by workflow_dispatch · validated by CI run https://github.com/rysweet/amplihack-rs/actions/runs/43712904881
commit 5bc4defc4 · branch main
```

Either path therefore names the CI run that admitted the release.

#### What this guarantees

- **Red CI publishes nothing.** When CI on a commit fails, is cancelled or times
  out, `version-bump` is skipped, `build` and `release` never start, and no tag
  or release is created. The version number is not consumed either, so the same
  commit can be released once its CI is green.
- **A later push decides for itself.** The concurrency group is keyed on the CI
  run's repository and branch
  (`${{ github.workflow }}-${{ github.event.workflow_run.head_repository.full_name || github.repository }}-${{ github.event.workflow_run.head_branch || github.ref_name }}`)
  with `cancel-in-progress: false`. One release is in flight per branch and a
  second one queues behind it instead of cancelling it, so a stale run can
  never stop a newer commit's release part-way through. #1356's invariant holds
  without cancellation: nothing is written to `main` or to the tag namespace
  before the `release` job, so a run that never reaches that job leaves nothing
  behind either way. Queueing cannot resurrect an old commit, because a queued
  run that finally starts finds that `main` has moved past its commit and
  publishes nothing (next bullet). A later commit's own CI therefore decides
  whether that commit ships.
- **A commit `main` has moved past publishes nothing.** Re-running CI on an
  older commit produces a CI completion that satisfies every admission clause
  above: it succeeded, on `main`, from a `push`, in this repository. Before any
  tag is created and before the `version` output is written, `version-bump`
  compares the commit CI tested with the current tip of `origin/main` and stops
  when they differ:

  ```text
  CI tested 5bc4defc4 but origin/main is now 8a51b29e8
  no release published for a commit that main has moved past
  ```

  Without that comparison the older tree is published under a *higher* version
  number and marked `latest`, and every host running `amplihack update`
  installs code older than the code it already has. A rising version number
  does not imply a later commit; this comparison is what binds the two
  together. The comparison sits just after the next-patch arithmetic, so a
  skipped run's log prints a `next patch:` line immediately above the skip
  message; that arithmetic is local to the step and persists nothing.

  The job then writes no `version` output, `build` is skipped by
  `if: needs.version-bump.outputs.version != ''`, `release` skips as a
  dependant of a skipped job, and `sccache-prune` — which runs under
  `always()` — skips on its own `needs.build.result != 'skipped'` clause.

  The tip is read from the checkout's `origin/main`, falling back to a single
  `git ls-remote origin refs/heads/main`; if neither resolves, the job fails
  visibly rather than skipping quietly, so an infrastructure fault is never
  mistaken for a superseded commit. The cost of that failure is bounded — the
  next push to `main` releases normally.
- **Every CI completion creates an Auto Release run.** Most skip every job
  immediately — pull-request CI, merge-group CI, failed CI, the twice-weekly
  schedule run, and CI's own `workflow_dispatch` run. The release's own `v*` tag
  push creates no CI run at all, since it is pushed with `GITHUB_TOKEN`. A
  skipped "Auto Release" entry in the Actions list is the expected shape, not a
  fault.
- **Publication trails the merge by one CI duration.** A release appears once CI
  on that commit has completed, rather than beside it. Nothing waits or polls
  for it: the Auto Release run is created by CI's completion event, so the delay
  is CI's own duration and no step sleeps or retries.

#### Releasing by hand

`workflow_dispatch` is held to the same rule, and to one more: it runs only
from `main`. `version-bump`'s dispatch clause is
`github.event_name == 'workflow_dispatch' && github.ref == 'refs/heads/main'`,
so a dispatch from a tag or a topic branch skips every job. Choose the ref in
the Actions UI or on the command line; there are no workflow inputs and no SHA
to type:

```bash
gh workflow run release.yml --ref main
```

`version-bump` resolves that ref to a SHA and, before computing any version,
asks for a successful `ci.yml` run on that exact SHA — one started by a `push`
whose branch was `main`. Because the dispatch is restricted to
`refs/heads/main`, the SHA it resolves is already the tip of `main`, which is
why the tip-of-`main` comparison described above is scoped to the
`workflow_run` path and not repeated here. To ask the same question first, read
the SHA from the remote rather than from a possibly stale local branch, since
that is what the runner resolves:

```bash
gh api "repos/rysweet/amplihack-rs/actions/workflows/ci.yml/runs?head_sha=$(git rev-parse origin/main)&status=success&per_page=100" \
  --jq '[.workflow_runs[] | select(.event == "push" and .head_branch == "main")] | length'
# Output: 1
```

A count of `0` ends the job with
`::error::no successful CI run built <sha>; not publishing a release` — before
any tag arithmetic, so a rejected dispatch cannot disturb the version series.
The check runs once, with no retry, sleep or polling; it reads the API only.

The lookup keeps `push` runs on `main` and discards every other event. A
pull-request run reports the PR head commit as its `head_sha`, but no checkout
in `ci.yml` sets `ref:`, so it builds `refs/pull/N/merge` — the head merged into
the base — and is evidence about a different tree from the one being released.
A `merge_group` run is excluded without losing a reachable case: a merge-queue
commit that actually lands on `main` gets its own push CI run on `main`, and
that is the run this lookup finds. Both conditions are needed — `.event ==
"push"` alone would admit a push run from a fork branch, and `.head_branch ==
"main"` alone would admit a fork branch that happens to be named `main` — so
keep them joined with `and`, here and in the workflow.

The check is a safety control rather than an authorization control — anyone who
can push to `main` can let CI pass and then dispatch. Restricting the dispatch
to `refs/heads/main` hardens a maintainer-error path, such as dispatching from
a stale tag or a topic branch, rather than an outsider path. What the check
rules out is publishing a commit that CI did not build and pass on.

#### Keeping the trigger attached

`workflows: [CI]` matches `ci.yml`'s `name: CI` as a literal string. Renaming
that workflow detaches Auto Release silently: no run is created and nothing
reports an error — releases simply stop. Change `name:` in `ci.yml` and
`workflows:` in `release.yml` in the same commit.

`ci.yml` has no `paths` or `paths-ignore` filter, and the admission rule depends
on that: `conclusion == 'success'` is read as the full job set having passed.
Adding a path filter to `ci.yml` would make a success conclusion compatible with
a commit on which nothing ran, weakening admission without any edit to
`release.yml`.

Two further properties follow from the trigger itself rather than from any job:

- `workflow_run` trigger definitions are read from the **default branch**, so a
  change to this trigger cannot be exercised from a pull request. The first run
  after the merge is its first live test; before that, validation is static —
  `actionlint` or a YAML parse. No test checks the trigger or the concurrency
  expression: `ci_workflows_define_concurrency_boundaries` only asserts that
  each workflow file has a top-level `concurrency:` key at all.
- `github.ref` is always `refs/heads/main` on the `workflow_run` path, whatever
  CI ran on; on `workflow_dispatch` it is the ref the operator selected.
  Conditions that need the released branch read
  `github.event.workflow_run.head_branch` instead. For the same reason the
  `sccache-prune` job also requires `needs.build.result != 'skipped'`, so it
  prunes once per release rather than once per CI completion.

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
the manifests. That SHA is the commit CI passed on (see
[A release follows green CI on the same commit](#a-release-follows-green-ci-on-the-same-commit)),
so every published asset is built from source that CI tested.

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
  Tests that use the shared `pinned_runner` helper accept this executable
  because its receipt names the pin, so they do not build the runner again
  (see [Run Tests Against the Pinned Recipe Runner](../howto/run-tests-against-the-pinned-recipe-runner.md)).
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

### sccache binary

Jobs that set `RUSTC_WRAPPER: sccache` get the binary from the Actions cache
(key `sccache-bin-v0.19.0-${{ runner.os }}-${{ runner.arch }}`, via
`.github/actions/sccache-bin`) and check it against the sha256 pinned in
`.github/scripts/sccache-bin.sh` and against `--version`. Only on a miss does
`Setup sccache` download it, once; a binary that passes is saved. A cached
binary that fails the checks produces a warning naming the cache entry to
delete, because the key is immutable and a later save cannot replace it. With no
usable binary the job writes `RUSTC_WRAPPER=`, skips the disk-cache steps and
`sccache --show-stats`, prints
`::warning::sccache binary unavailable (cache miss and download failed); building without RUSTC_WRAPPER`
and builds uncached. There is no retry, sleep or timeout: a failed download is
handled by building uncached, not by waiting (issue #2105). Test the fallback
with `gh workflow run ci.yml --ref <branch> -f simulate_sccache_missing=true`;
the dispatch input is declared in `.github/workflows/ci.yml`.

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

> **Updated 9 Nov 2026:** this section used to say only 3 checks were
> required and that the other build legs ran in a merge queue. Both were
> wrong. [Branch protection on main](../contributing/branch-protection.md) is
> the record; where this section disagrees with it, that page wins.

Branch protection on `main` requires these 7 checks, all from `ci.yml`:
**Lint & Format** (`check`), **Test** (`test`), **Install Smoke Test**, and all
four `cross-compile` legs: **Build x86_64-unknown-linux-gnu**,
**Build aarch64-unknown-linux-gnu**, **Build x86_64-apple-darwin** and
**Build aarch64-apple-darwin**. The "require branches to be up to date"
(strict) setting is off.

**Recipe runner pin ancestry**, described
[above](#recipe-runner-pin-ancestry), becomes a required check only when an
administrator adds it to branch protection by hand. Branch protection matches
the job name `Recipe runner pin ancestry`, not the job ID `recipe-runner-pin`,
so the job's `name:` in `.github/workflows/recipe-runner-pin.yml` must stay
exactly as written.

`ci.yml` also triggers on `merge_group:`, but no merge queue is in use: GitHub
does not offer one on a user-owned repository. See
[Why not a GitHub merge queue?](merge-flow.md#why-not-a-github-merge-queue).

> Configuring branch protection, required checks, and the merge queue needs
> repository-admin rights and is applied through repository settings, not this
> workflow file.

Branch protection governs what reaches `main`. A separate rule governs what
reaches a published release: on the automatic path **Auto Release** starts only
from a successful `CI` run on a `main` push and builds the commit that run
tested, so the seven required checks also stand between such a commit and its
release. A `workflow_dispatch` release is held to a successful CI run on the
same commit, but not to `main`: see
[A release follows green CI on the same commit](#a-release-follows-green-ci-on-the-same-commit).

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

That holds for `ci.yml`. **Auto Release** is a different case: it is started by
`workflow_run`, a privileged trigger, so its run has repository secrets and a
write-capable token even when the CI run that started it was a read-only fork
pull request. Nothing from the fork is checked out or executed, because
`version-bump` admits a run only on a successful `main` push from this
repository; those clauses are what keeps the privilege from reaching fork
content, and they are listed in
[What admits a run](#what-admits-a-run).

`release.yml` scopes its own escalations per job rather than at the top level:
`version-bump` takes `actions: read` so it can query `ci.yml` run history on the
manual dispatch path, `build` takes `actions: read` so the sccache composite
actions can list cache entries, `release` takes `contents: write` to push the
tag and publish, and `sccache-prune` alone takes `actions: write`.

## Related references

- [A release follows green CI on the same commit](#a-release-follows-green-ci-on-the-same-commit)
  (on this page) — the rule that decides which commits **Auto Release**
  publishes, and how to release by hand.
- [CI Wall-Clock Parallelization](ci-wallclock-parallelization.md) — parallel
  fan-out job graph and tag-conditional release optimization.
- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
- [Contributing (Rust)](../../CONTRIBUTING_RUST.md) — local build, test, and PR
  process.
