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

Every CI job installs the toolchain with a version-pinned
`dtolnay/rust-toolchain@1.97.0` action that matches the file; cross-compile legs
add `targets: ${{ matrix.target }}` on top. `rust-toolchain.toml` is
authoritative: if the action pin and the file ever disagree, `rustup` uses the
file's channel, so a mismatch degrades to a slower run — never a broken one.

### Bumping the pinned toolchain

Bump deliberately, in its own PR:

1. Edit `channel` in `rust-toolchain.toml`.
2. Update the four `dtolnay/rust-toolchain@<version>` refs in `ci.yml` to match.
3. Run `cargo clippy -- -D warnings && cargo fmt --check && cargo nextest run
   --workspace --locked` locally, fix any new lints, and open the PR.

Keep the file and the four action refs in lockstep so the review stays focused
on lint fallout rather than mixing it with feature work.

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
