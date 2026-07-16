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
check (Lint & Format) ── required
   ├─ test (Test) ─────────────── required
   ├─ install-smoke (Install Smoke Test)
   └─ cross-compile (Build <target>) × 4
         └─ (tag push only) release
```

`check` gates every downstream job. `test`, `install-smoke`, and the four
`cross-compile` legs fan out from it in parallel. `release` runs only on `v*`
tags, after `test` and every `cross-compile` leg succeed.

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
   match —    the four refs in `ci.yml`, the ref in `publish-snapshot.yml`, and the
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
    - run: cargo nextest run --workspace --locked
    - run: cargo test --workspace --doc --locked
```

- **`CARGO_PROFILE_TEST_DEBUG: "0"`** drops debug info from the many test
  binaries, shrinking link time and on-disk size; the suite asserts behavior,
  not backtraces.
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

## Related references

- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
- [Contributing (Rust)](../../CONTRIBUTING_RUST.md) — local build, test, and PR
  process.
