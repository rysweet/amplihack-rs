# CI Pipeline Reference

> [Home](../index.md) > Reference > CI Pipeline

The `CI` workflow (`.github/workflows/ci.yml`) validates every pull request and
`main` push for the `amplihack-rs` Rust workspace. It pins the Rust toolchain,
runs the workspace test suite with [cargo-nextest](https://nexte.st), frees
runner disk before heavy jobs, and keeps build caches scoped so required
coverage stays reliable without exhausting GitHub-hosted runners.

This reference describes the pipeline's structure and behavior. For the broader
resource contract (concurrency, timeouts, matrix boundaries) see
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
`cross-compile` matrix legs fan out from `check` and run in parallel. `release`
runs only on `v*` tags after `test` and every `cross-compile` leg succeed.

| Job | Name | Runner | Purpose |
| --- | --- | --- | --- |
| `check` | Lint & Format | `ubuntu-latest` | Repository guards, `cargo fmt --check`, `cargo clippy -- -D warnings` |
| `test` | Test | `ubuntu-latest` | `cargo nextest run` over the workspace plus doctests |
| `install-smoke` | Install Smoke Test | `ubuntu-latest` | `cargo install --path bins/amplihack` and binary sanity checks |
| `cross-compile` | Build `<target>` | `ubuntu-latest` / `macos-latest` | Release build + artifact upload per target |
| `release` | Release | `ubuntu-latest` | Package artifacts and publish the GitHub Release |

## Toolchain pinning

The workspace pins its Rust toolchain in `rust-toolchain.toml` at the repository
root. `rustup` reads this file automatically for every local and CI command, so
developers and CI resolve to the **same** compiler and lint set. This stops
clippy drift: a new stable release cannot introduce surprise lint failures,
because the pinned channel does not move until the file is bumped.

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.97.0"
components = ["rustfmt", "clippy"]
```

Every CI job installs the toolchain with a version-pinned action that matches
the file, and cross-compile legs add their target on top:

```yaml
- uses: dtolnay/rust-toolchain@1.97.0
  with:
    components: clippy, rustfmt   # check job only
- uses: dtolnay/rust-toolchain@1.97.0
  with:
    targets: ${{ matrix.target }} # cross-compile legs
```

`rust-toolchain.toml` is authoritative. If an action pin and the file ever
disagree, `rustup` overrides the job with the file's channel, so a mismatch
degrades to a slower run — never a broken one.

### Bumping the pinned toolchain

Bump the toolchain deliberately, in its own PR:

1. Edit `channel` in `rust-toolchain.toml` to the new version.
2. Update the four `dtolnay/rust-toolchain@<version>` refs in
   `.github/workflows/ci.yml` to the same version.
3. Run `cargo clippy -- -D warnings && cargo fmt --check && cargo nextest run
   --workspace --locked` locally, fix any new lints, and open the PR.

Keep the file and the four action refs in lockstep. A version-only PR keeps the
review focused on lint fallout rather than mixing it with feature work.

## Test execution

The `test` job runs the workspace suite with `cargo-nextest`, a parallel test
runner that schedules each test as its own process. Compared with `cargo test`
it starts faster, uses runner cores more evenly, and holds a lower peak disk
footprint because it does not need every test binary resident at once.

```yaml
test:
  name: Test
  runs-on: ubuntu-latest
  needs: check
  timeout-minutes: 45
  env:
    CARGO_PROFILE_TEST_DEBUG: "0"
  steps:
    - uses: actions/checkout@v4
    - uses: jlumbroso/free-disk-space@<sha>  # see "Disk management"
      with:
        tool-cache: false
        android: true
        dotnet: true
        haskell: true
        docker-images: true
        large-packages: false
        swap-storage: false
    - uses: dtolnay/rust-toolchain@1.97.0
    - uses: taiki-e/install-action@<sha>     # SHA-pinned, see "Action pinning"
      with:
        tool: nextest
    - uses: Swatinem/rust-cache@v2
      with:
        cache-targets: false
        cache-on-failure: true
        save-if: ${{ github.ref == 'refs/heads/main' }}
    - run: cargo nextest run --workspace --locked
    - run: cargo test --workspace --doc --locked
```

Key settings:

- **`CARGO_PROFILE_TEST_DEBUG: "0"`** — drops debug info from test binaries,
  shrinking link time and on-disk size. The workspace already sets
  `debug = "line-tables-only"` in `Cargo.toml`; forcing `0` for the CI test run
  removes even line tables, which the suite does not need to assert behavior.
- **`cargo nextest run --workspace --locked`** — runs every unit and
  integration test. `--locked` keeps `Cargo.lock` authoritative.
- **`cargo test --workspace --doc --locked`** — nextest does not execute
  doctests, so a dedicated step preserves that coverage. Running it after
  nextest reuses the already-built dependency graph.
- **`cache-targets: false`** — the workspace links large native artifacts
  (Kuzu, cxx). Restoring `target/` can exhaust runner disk before tests start,
  so target output is never cached for this job.

Nextest honors `.config/nextest.toml` when present. The repository ships that
file only if a test needs a serial group to avoid contention on a shared
resource (fixed port, `$HOME`, single-writer path); by default no file is
present and nextest runs at full concurrency. Adding a serial group there does
not change `cargo test` behavior, so the pre-commit hook (which still uses
`cargo test`) is unaffected.

## Integration test binary convention

Integration tests that shell out to the built binaries resolve their path with
the Cargo-provided environment variable, never a hardcoded `target/…` string:

```rust
// amplihack CLI
let bin = env!("CARGO_BIN_EXE_amplihack");
// hooks binary
let bin = env!("CARGO_BIN_EXE_amplihack-hooks");
```

`CARGO_BIN_EXE_<name>` is set by Cargo for every `[[test]]` target that belongs
to the package owning the binary, and it points at the exact artifact for the
current profile (debug or release). Because the variable is only defined when
the binary is a build dependency of the test, referencing it guarantees Cargo
builds the binary **before** the test runs — eliminating the
`amplihack binary not found at target/debug/amplihack` transient that occurred
when a test raced ahead of a separate build step.

Most binary-invoking integration suites already follow this convention. The
`amplihack`-owned suites — `cli_golden_tests.rs`, `cli_launch_test.rs`,
`recipe_e2e_test.rs`, `fleet_probe.rs`, `kuzu_path_notice_test.rs`,
`security_hygiene_test.rs`, `no_python_probe_test.rs`, and
`doctor_node_remediation_test.rs` — resolve the `amplihack` binary this way, and
`hook_dispatch_test.rs` resolves `amplihack-hooks`.

Two of these still carry a soft path lookup that this work tightens:

- `no_python_probe_test.rs` uses `option_env!("CARGO_BIN_EXE_amplihack")` with a
  hardcoded `target/debug/amplihack` fallback.
- `doctor_node_remediation_test.rs` uses
  `std::env::var_os("CARGO_BIN_EXE_amplihack")` with the same fallback.

Both are switched to the hard `env!("CARGO_BIN_EXE_amplihack")` form. This is
safe: both files are already `[[test]]` targets in `bins/amplihack`, so `env!`
resolves at compile time and the `target/debug/...` fallback (which breaks under
release profiles and cross builds) is removed.

Package ownership determines where the env var resolves. Each file is registered
as an explicit `[[test]]` target in the `Cargo.toml` of the package that owns the
binary:

- `amplihack` tests live in `bins/amplihack/Cargo.toml` and resolve
  `CARGO_BIN_EXE_amplihack`.
- `hook_dispatch_test.rs` lives in `bins/amplihack-hooks/Cargo.toml` (package
  `amplihack-hooks-bin`) and resolves `CARGO_BIN_EXE_amplihack-hooks`. It is
  already wired there; only its internal path constant changes to `env!`.

`skip_update_check_flag_test.rs` is the one previously unwired orphan: adding its
`[[test]]` entry to `bins/amplihack/Cargo.toml` both lets the env var resolve and
brings the file into the compiled test set, so its assertions now run in CI.

## Disk management

The `test` job frees runner disk with `jlumbroso/free-disk-space` before
installing the toolchain. This reclaims tens of gigabytes deterministically and
replaces the earlier hand-rolled `sudo rm -rf …` step, so the disk-exhaustion
transients tracked in issue #744 no longer recur.

```yaml
- uses: jlumbroso/free-disk-space@<sha>
  with:
    tool-cache: false      # keep the Rust toolchain cache we depend on
    android: true          # remove Android SDK/NDK
    dotnet: true           # remove .NET
    haskell: true          # remove GHC
    docker-images: true    # prune preloaded images
    large-packages: false  # skip slow apt purge; not needed for the win
    swap-storage: false    # keep swap
```

Toggles are chosen to maximize reclaimed space while keeping anything the build
depends on. `tool-cache: false` preserves the hosted Rust toolchain cache;
`large-packages` and `swap-storage` stay off because they add minutes without
materially helping this workspace.

Disk freeing is scoped to the `test` job, which links the full workspace test
profile and has the highest peak footprint. The `install-smoke` and
`cross-compile` jobs build the release profile without the test artifacts and
have not hit the disk ceiling, so they skip the step to save wall-clock time. If
a future dependency pushes those jobs against the limit, add the same
`free-disk-space` step to them rather than caching `target/`.

## Caching

All jobs use `Swatinem/rust-cache@v2` with two shared rules:

- **`save-if: ${{ github.ref == 'refs/heads/main' }}`** — only `main` writes the
  cache. Pull-request runs restore but never save, which prevents fork/PR runs
  from poisoning the shared cache and stops per-branch cache bloat.
- **`cache-targets: false`** — `target/` output is never cached (see
  [Test execution](#test-execution)); only the registry and Git dependency
  caches are stored, keeping restores small and disk-safe.

The `install-smoke` job and the `x86_64-unknown-linux-gnu` build leg share the
cache key `x86_64-unknown-linux-gnu`, so once the Linux build populates the
dependency cache the smoke test hydrates instead of rebuilding from scratch.
Each `cross-compile` leg keys its cache by `matrix.target` to keep
per-architecture dependency graphs isolated.

CI always passes from a cold cache; caching is an optimization only.

## Required checks and merge governance

Branch protection on `main` requires these status checks:

| Required check | Job |
| --- | --- |
| Lint & Format | `check` |
| Test | `test` |
| Build x86_64-unknown-linux-gnu | `cross-compile` (Linux x86_64 leg) |

The three additional `cross-compile` legs — `aarch64-unknown-linux-gnu`,
`x86_64-apple-darwin`, and `aarch64-apple-darwin` — still run on every pull
request for signal, but they are not required-to-merge status checks. Scarce
macOS and cross-arch runners therefore cannot block a merge by sitting queued.
Cross-architecture and Darwin coverage is **relocated, not dropped**: it runs
in the merge queue and on tagged release builds, where every target must pass
before an artifact publishes.

A GitHub **merge queue** batches approved PRs and runs the required checks once
per batch instead of once per serialized merge, so a strict "branches
up-to-date" policy no longer forces a full CI cycle per PR.

> Configuring branch protection, required checks, and the merge queue requires
> repository-admin rights and is applied through repository settings, not this
> workflow file.

## Action pinning

Third-party actions that execute during a job are pinned to a full 40-character
commit SHA with a trailing version comment, so a moved tag cannot change what
runs:

```yaml
- uses: taiki-e/install-action@<40-char-sha>       # vX.Y.Z
- uses: jlumbroso/free-disk-space@<40-char-sha>    # vX.Y.Z
```

First-party GitHub actions (`actions/checkout`, `actions/upload-artifact`,
`actions/download-artifact`) and the widely used `Swatinem/rust-cache` and
`dtolnay/rust-toolchain` continue to use their major-version / channel tags.

## Token permissions

The workflow declares a least-privilege `GITHUB_TOKEN` at the top level and
grants write access only where a job needs it:

```yaml
# top level — applies to every job unless overridden
permissions:
  contents: read

jobs:
  # ...
  release:
    permissions:
      contents: write   # create the GitHub Release + upload assets
```

With `contents: read` as the default, the `check`, `test`, `install-smoke`, and
`cross-compile` jobs receive a read-only token: they can check out the
repository but cannot push commits, edit issues, or publish releases. Only
`release` escalates to `contents: write`, and only on `v*` tags. Because CI
triggers on `pull_request` (not `pull_request_target`), fork PRs already run
with a read-only token and no access to repository secrets; the explicit
top-level block makes that guarantee visible and stops a future job from silently
inheriting broad default permissions.

## Reproducing CI locally

The pinned toolchain means local commands match CI exactly. From the workspace
root:

```bash
# Toolchain (installed automatically by rustup from rust-toolchain.toml)
rustc --version            # rustc 1.97.0

# Lint & format (mirrors the check job)
cargo fmt --check
cargo clippy -- -D warnings

# Fast test run (mirrors the test job)
cargo install cargo-nextest --locked   # one-time
CARGO_PROFILE_TEST_DEBUG=0 cargo nextest run --workspace --locked
cargo test --workspace --doc --locked  # doctests

# Install smoke test
cargo install --path bins/amplihack --locked
amplihack --version
```

`cargo test --workspace --locked` still works and remains what the pre-commit
hook runs; nextest is the faster path used in CI and is fully optional locally.

## Configuration reference

| Setting | Location | Value | Effect |
| --- | --- | --- | --- |
| Rust channel | `rust-toolchain.toml` | `1.97.0` | Pins compiler + lints for local and CI |
| Toolchain components | `rust-toolchain.toml` | `rustfmt`, `clippy` | Ensures format/lint tools present |
| `CARGO_PROFILE_TEST_DEBUG` | `test` job env | `0` | Removes debug info from test binaries |
| Test runner | `test` job | `cargo nextest run` | Parallel, lower-peak-disk test execution |
| Doctests | `test` job | `cargo test --doc` | Preserves doctest coverage nextest skips |
| `cache-targets` | all jobs | `false` | Never cache `target/`; avoids disk exhaustion |
| `save-if` | all jobs | `main` only | Only `main` writes cache; prevents PR cache poisoning |
| Disk freeing | `test` job | `jlumbroso/free-disk-space` | Deterministic reclaim; supersedes manual `rm` |
| Token permissions | top level / `release` | `contents: read` / `contents: write` | Least-privilege default; write only for releases |
| Binary path in tests | integration tests | `env!("CARGO_BIN_EXE_*")` | Correct per-profile path; forces build ordering |

## Measured impact

The baseline pipeline (`cargo test --workspace --locked`, manual `rm -rf` disk
free, unpinned `@stable` toolchain) had a measured critical path of **~42m36s**
through `check → test`.

The current pipeline (nextest + `CARGO_PROFILE_TEST_DEBUG=0` + deterministic
disk free + pinned toolchain) shortens the `test` job and removes the
disk-exhaustion and clippy-drift failure classes, cutting the critical path
accordingly. Per-job wall-clock is visible in each Actions run summary and is
reproduced by the local commands above; because runner timings vary, this
reference records only the sourced baseline rather than fixed per-job targets.
Toolchain pinning also eliminates the emergency lint-fix churn (for example the
drift addressed in PR #878) by holding the compiler steady between deliberate
bumps.

## Related references

- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
- [Contributing (Rust)](../../CONTRIBUTING_RUST.md) — local build, test, and PR
  process.
