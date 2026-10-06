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
is therefore registered as an explicit `[[test]]` in the owning package's
`Cargo.toml` (`bins/amplihack` for `amplihack`, `bins/amplihack-hooks` for
`amplihack-hooks`).

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

## Required checks and merge governance

Branch protection on `main` requires **Lint & Format** (`check`), **Test**
(`test`), and **Build x86_64-unknown-linux-gnu** (the Linux x86_64
`cross-compile` leg). The three remaining `cross-compile` legs
(`aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`)
still run on every PR for signal but are not required-to-merge, so scarce
macOS/cross-arch runners cannot block a merge by sitting queued. That coverage
is **relocated, not dropped**: it must pass in the merge queue and on tagged
release builds before any artifact publishes.

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

## Release tags point at a stamped commit (#1526)

The Auto Release workflow (`.github/workflows/release.yml`) puts tag `vX` on a
one-commit child of the released SHA in which the manifests say X, so a
`cargo install --git … --tag vX` build reports X. The released SHA is the
commit the run was triggered on: `main` for push runs, or whatever ref a
`workflow_dispatch` run was started from.

```
main ── A ── B                   Cargo.toml: version = "1.0.0"
              \
               S  ← tag v1.0.7   chore(release): stamp v1.0.7
```

The stamp commit changes only version lines: `version` under
`[workspace.package]` in `Cargo.toml`, the `version` of every `Cargo.lock`
`[[package]]` with no `source` line (the workspace members), and the
top-level `"version"` in `package.json`.

### Release job steps

Once every build artifact exists (#1356), the `Stamp version and create tag`
step of the `release` job runs, in this order:

1. **Look up `vX` on origin** with `git ls-remote --exit-code --tags`.
   - Exit 2 (no such tag): continue to step 2.
   - Exit 0: a re-run finds the tag its earlier attempt pushed. The step
     accepts the tag, and goes straight to publishing, only if it points at
     the released SHA itself, or at a commit whose first parent is the
     released SHA **and** whose subject is exactly `chore(release): stamp vX`.
     Any other `vX` fails the job, so these artifacts are never attached to
     someone else's tag.
   - Any other exit fails the job.
2. **Check `HEAD`** is the released SHA.
3. **Stamp and stage.** Run `bash scripts/stamp-release-version.sh "$VERSION"`,
   stage `Cargo.toml`, `Cargo.lock` and `package.json` by name, and fail if
   anything else is staged. Staging by name keeps the downloaded
   `artifacts/` and `release-assets/`, which are also in the working tree,
   out of the commit.
4. **Commit** `chore(release): stamp vX`. When the manifests already say X
   (the first release of a `MAJOR.MINOR` series), nothing is staged, no commit
   is made, and the tag goes on the released SHA itself.
5. **Tag and push.** Create the annotated tag `vX` and push `refs/tags/vX`
   only. No branch is pushed and nothing is forced. This is the last git step
   before `softprops/action-gh-release` creates the release.

No git failure in the step is ignored: there is no `|| true`, and the
`ls-remote` exit status is checked explicitly.

`version-bump` treats the triggering commit as already released when a series
tag points at it, or at its stamp commit (first parent is the commit, subject
is `chore(release): stamp vX`). A re-run of the same SHA therefore rebuilds the
same X instead of taking a new patch number.

### Permissions

Unlike `ci.yml`, `release.yml` grants `contents: write` at the top level,
because the `release` job pushes the tag and creates the release. Every job
inherits it, including the `build` matrix. No checkout sets
`persist-credentials: false`; the tag push uses the credentials the `release`
checkout persists. No job pushes a branch.

The binaries are built from the released SHA with
`AMPLIHACK_RELEASE_VERSION=X`. The tagged tree differs from that SHA only in
the version lines.

### What a failed or cancelled run leaves behind

- **Any failure up to and including the tag push** (tag lookup, stamp,
  commit, tag or push): the job stops, so this run pushes no tag and creates
  no release, and it burns no version number (#1356). The stamp commit only
  ever existed in the runner's checkout.
- **Failed or cancelled after the tag push.** If
  `softprops/action-gh-release` fails, or the run is cancelled before it
  finishes, tag `vX` exists with no release. This window was already there
  before #1526 (tag, then release) and is no wider. Re-running the same run
  publishes the release, because step 1 recognises the tag as its own. A
  later run for a newer commit bumps past `vX` instead, and `vX` stays a tag
  with no release. Closing the window would need the workflow to delete tags,
  which it does not do.

### Which builds report which version

| Build | Reports |
| --- | --- |
| Release binaries attached to the GitHub release | X (`AMPLIHACK_RELEASE_VERSION`) |
| `cargo install --git https://github.com/rysweet/amplihack-rs --tag vX amplihack` | X (the stamped manifests) |
| `--tag` build of a tag cut before #1526 merged | The base version at that commit: `v0.18.39` reports `0.18.0`. The first release of a series, such as `v1.0.0`, reports its own version |
| `cargo install --git https://github.com/rysweet/amplihack-rs --rev <SHA> amplihack`, or any build of a `main` commit, including the one a release was cut from | The base version in `Cargo.toml` |

Tags cut before #1526 merged are not re-tagged; the stamp applies from the
first release the updated workflow cuts. The last row is what #1526 hit, and
the stamp does not change it. Instead, self-heal never lets such a build
re-stage over a newer install; see
[Never an implicit downgrade](../features/self-heal-asset-restage.md#never-an-implicit-downgrade-1526).

Release tags are not ancestors of `main`, except the first release of a series
and tags cut before #1526 merged. For a stamped tag, `git rev-parse 'v1.0.7^'`
gives the commit the release was cut from.

### `scripts/stamp-release-version.sh`

Bash and POSIX `awk` with `LC_ALL=C`. It needs no Rust toolchain, because the
`release` job does not install one. Run it from the repository root with
exactly one argument; a second run with the same X changes nothing. With 34
workspace members:

```sh
bash scripts/stamp-release-version.sh 1.0.7
git diff --shortstat
#  3 files changed, 36 insertions(+), 36 deletions(-)
```

The old version is the `[workspace.package]` version in `Cargo.toml`. In
`Cargo.lock` the script rewrites the `version` line of every `[[package]]`
block with no `source` line (the workspace members), and it refuses to run
unless every one of them is at the old version. A registry crate at the same
version, such as `anstream 1.0.0`, has a `source` line and keeps its version.
No `source`, `checksum` or `dependencies` line changes.

| Exit | Meaning |
| --- | --- |
| `0` | Stamped, or already at X. The checks still run first. |
| `1` | A [check](#checks-before-any-write) failed, and no file changed. Also any failure while writing; see [Temporary files](#temporary-files) for what that can leave. |
| `2` | Usage error: not exactly one argument, or X is not `MAJOR.MINOR.PATCH` (digits only, no leading zeros, no `v`, no pre-release such as `1.0.0-rc.1`, no build metadata such as `1.0.0+m`) |

#### Checks before any write

Every check runs before the first file is written, on every run. The script
exits `1` when:

- **Any of the three files** is missing or unreadable, is empty, contains a
  carriage return (CRLF line endings), or does not end with a newline; or a
  `<file>.stamp-tmp` already exists, as a file or a symlink (see
  [Temporary files](#temporary-files)).
- **`Cargo.toml`**: `[workspace.package]` does not have exactly one `version`
  line, that line is not exactly `version = "…"`, or its value is not
  `MAJOR.MINOR.PATCH`.
- **`Cargo.lock`**: there is no `[[package]]` without a `source` line; such a
  block has no `name` or `version` line; two of them share a name; one of them
  is at any version other than the old one; or a dependency entry names a
  workspace member with any version (`"amplihack-hooks 9.9.9"`), because that
  version would need stamping too.
- **`package.json`**: there is not exactly one top-level (two-space-indented)
  `"version"` line, the line has another form, or its value is not the old
  version.

A member already at X counts as "not at the old version", so a partly stamped
tree is refused, not finished:

```
stamp-release-version: error: Cargo.lock: unexpected path package amplihack-hooks at 1.0.7 (expected 1.0.0)
```

#### Temporary files

The script writes each new file to a sibling in the current directory
(`Cargo.toml.stamp-tmp`, `Cargo.lock.stamp-tmp`, `package.json.stamp-tmp`) and
then renames it over the original with `mv -f`.

- A leftover `<file>.stamp-tmp`, whether a file or a symlink (dangling or
  not), is refused up front. The script never follows or removes one it did
  not create; delete it after checking where it came from.
- Each temp file is created under `set -C` (noclobber), so a file planted
  after that check makes the run fail instead of being overwritten.
- The `EXIT` trap removes only the temp files this run created, so a failure
  while writing them leaves none behind.
- The renames are per file. A failure part-way through them can leave some
  manifests stamped. The `release` job stops there, so a partly stamped tree
  never reaches a pushed ref, and running the script again on that tree exits
  `1`.

#### Output

The status line goes to stdout: `stamped …` when files changed, `already at …`
on a second run.

```
stamped 1.0.7 (was 1.0.0): Cargo.toml 1, Cargo.lock 34, package.json 1
already at 1.0.7; nothing to do
```

Errors go to stderr, prefixed `stamp-release-version: error: ` and followed by
the `usage:` line where it applies. A rejected argument is printed with
`printf %q`, so it cannot start a new line:

```
stamp-release-version: error: v1.0.7 is not MAJOR.MINOR.PATCH (no "v" prefix, suffix or leading zeros)
usage: stamp-release-version <MAJOR.MINOR.PATCH>
```

Every line starts with fixed text, so nothing the script prints can be read as
a GitHub Actions workflow command (`::…`).

Guarded by `crates/amplihack-cli/tests/issue_1526_release_version_stamp.rs`,
which runs the script on copies of the real manifests.

## Related references

- [CI Wall-Clock Parallelization](ci-wallclock-parallelization.md) — parallel
  fan-out job graph and tag-conditional release optimization.
- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
- [Contributing (Rust)](../../CONTRIBUTING_RUST.md) — local build, test, and PR
  process.
