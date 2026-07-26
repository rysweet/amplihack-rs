# CI Wall-Clock Parallelization Reference

> [Home](../index.md) > Reference > CI Wall-Clock Parallelization

The `CI` workflow (`.github/workflows/ci.yml`) runs its validation jobs as a
**fan-out from the trigger**, not a chain behind the lint job. Combined with a
tag-conditional release-optimization profile, this cuts the required-check
critical path roughly in half without changing what a `main` push or a tagged
release actually ships.

This page documents two coupled behaviors introduced together:

1. **Job parallelization** — `test`, `install-smoke`, and every `cross-compile`
   leg start immediately instead of waiting on `check` (Lint & Format).
2. **Conditional release optimization** — non-tag runs build the release
   profile with LTO **off** and 16 codegen units for speed; tag (`v*`) runs keep
   full LTO with a single codegen unit so shipped binaries are unchanged.

For the pinned toolchain, disk, and cache contract see
[CI Pipeline Reference](ci-pipeline.md). For the broader concurrency/timeout
contract see [CI Resource Discipline](ci-resource-discipline.md).

## Parallel job graph

```
push / pull_request / merge_group
   ├─ check          (Lint & Format)            ── required
   ├─ test           (Test)                     ── required
   ├─ install-smoke  (Install Smoke Test)
   └─ cross-compile  (Build <target>) × 4
         ├─ Build x86_64-unknown-linux-gnu      ── required
         ├─ Build aarch64-unknown-linux-gnu
         ├─ Build x86_64-apple-darwin
         └─ Build aarch64-apple-darwin

release  (Release)   ── tag push only, needs: [test, cross-compile]
```

`check`, `test`, `install-smoke`, and the four `cross-compile` legs have **no
`needs:` dependency on each other** — they are scheduled together the moment the
workflow triggers. Only `release` declares `needs: [test, cross-compile]`, and
it runs exclusively on `refs/tags/v*`.

### Why `test` no longer waits on `check`

The `check` job compiles the whole workspace for clippy (~15 min under the
no-artifact-cache policy). Gating the expensive `test` job behind it serialized
the required-check critical path to:

```
check (~15m) ─▶ test (~35m)  ≈ 50 min
```

Under strict "branches must be up to date" protection, a fast-moving `main`
would re-queue the branch before it could merge, so PRs could churn without ever
landing. Running `test` **concurrently** with `check` drops the critical path to
the longer single job:

```
max( check ~15m , test ~35m )  ≈ 35 min
```

`concurrency: cancel-in-progress: true` still supersedes stale runs, so a new
push cancels the previous run's jobs. A `check` failure now only costs redundant
`test`/`build` compute for that run — never correctness, because every required
check must still pass before merge.

### Correctness is preserved

Removing `needs: check` changes **scheduling only**, not the merge gate. Branch
protection still requires **Lint & Format** (`check`), **Test** (`test`), and
**Build x86_64-unknown-linux-gnu** to be green. A lint or format regression
fails `check` independently and blocks the merge exactly as before; the other
jobs simply do not idle waiting to discover that.

## Conditional release optimization

The workspace release profile is the slowest-to-build configuration and exists
purely to optimize **shipped** binaries:

```toml
# Cargo.toml
[profile.release]
lto = true
strip = true
panic = "unwind"
codegen-units = 1
```

`install-smoke` and the non-tag `cross-compile` legs build the release profile
only to *verify* it compiles and links — they neither ship nor benchmark the
artifact. Both override the two expensive knobs via Cargo's environment-variable
profile overrides:

| Variable                              | Effect                                   |
| ------------------------------------- | ---------------------------------------- |
| `CARGO_PROFILE_RELEASE_LTO`           | Overrides `[profile.release] lto`        |
| `CARGO_PROFILE_RELEASE_CODEGEN_UNITS` | Overrides `[profile.release] codegen-units` |

### `install-smoke` — always fast

The smoke test asserts only that `cargo install` succeeds and the binary
launches, so it is unconditionally fast:

```yaml
install-smoke:
  name: Install Smoke Test
  env:
    CARGO_PROFILE_RELEASE_LTO: "false"
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16"
```

### `cross-compile` — fast on branches, full on tags

The cross-compile matrix keys the two knobs off the git ref. Non-tag runs (PR,
`main` push, merge queue) verify each target compiles with optimization off; tag
(`v*`) runs restore full optimization so the artifacts uploaded for a release
are byte-for-byte the fully optimized build:

```yaml
cross-compile:
  name: Build ${{ matrix.target }}
  env:
    CARGO_PROFILE_RELEASE_LTO: >-
      ${{ startsWith(github.ref, 'refs/tags/v') && 'true' || 'false' }}
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS: >-
      ${{ startsWith(github.ref, 'refs/tags/v') && '1' || '16' }}
```

| Trigger                          | `LTO`   | `codegen-units` | Purpose                    |
| -------------------------------- | ------- | --------------- | -------------------------- |
| `pull_request` / `merge_group`   | `false` | `16`            | Fast compile-verify        |
| `push` to `main`                 | `false` | `16`            | Fast compile-verify        |
| `push` tag `refs/tags/v*`        | `true`  | `1`             | Full-optimization artifact |

The condition keys on the server-evaluated `github.ref` (`refs/tags/v*`), not on
attacker-controllable head-ref content, so there is no injection surface.

## Shipped artifacts are unchanged

The **Auto Release** workflow (`.github/workflows/release.yml`) does **not** set
`CARGO_PROFILE_RELEASE_LTO` or `CARGO_PROFILE_RELEASE_CODEGEN_UNITS`. Its `build`
matrix runs `cargo build --release --locked --target <target>` with no profile
overrides, so it inherits the full `Cargo.toml` release profile (`lto = true`,
`codegen-units = 1`). Released
`.tar.gz` binaries therefore keep full optimization regardless of the CI
speed-ups above.

The tag path in `ci.yml`'s own `cross-compile` job likewise flips back to
`lto = true` / `codegen-units = 1`, so any artifact produced by a `v*` build —
in either workflow — is the fully optimized binary.

| Path                          | Effective release profile        |
| ----------------------------- | -------------------------------- |
| CI non-tag (PR/main)          | LTO off, 16 codegen units (fast) |
| CI tag `v*` (`cross-compile`) | LTO on, 1 codegen unit (full)    |
| `release.yml` `build`         | LTO on, 1 codegen unit (full)    |

## Required checks (unchanged)

Parallelization did not rename or remove any required check. Branch protection on
`main` continues to require these `name:` values:

| Job             | Check name                        | Required |
| --------------- | --------------------------------- | -------- |
| `check`         | `Lint & Format`                   | ✅       |
| `test`          | `Test`                            | ✅       |
| `install-smoke` | `Install Smoke Test`             |          |
| `cross-compile` | `Build x86_64-unknown-linux-gnu`  | ✅       |
| `cross-compile` | `Build aarch64-unknown-linux-gnu` |          |
| `cross-compile` | `Build x86_64-apple-darwin`       |          |
| `cross-compile` | `Build aarch64-apple-darwin`      |          |

The three non-required `cross-compile` legs still run on every PR for signal;
their coverage is enforced in the merge queue and on tagged release builds before
any artifact publishes.

## Verifying the behavior

Because the change is workflow-only, validate it by inspecting the YAML rather
than running a build:

```bash
# No downstream job should gate on check (fan-out, not chain):
grep -n 'needs:' .github/workflows/ci.yml
#   → exactly one real dependency — the `release` job's
#     `needs: [test, cross-compile]`. The other three matches are
#     `# No needs: check` comments on test / install-smoke / cross-compile
#     explaining why each runs in parallel with Lint & Format.
#
# To see *only* the real dependency (anchored, comments excluded):
grep -nE '^\s*needs:' .github/workflows/ci.yml   # → just the release job

# Conditional cross-compile optimization keyed on tag ref:
grep -n 'CARGO_PROFILE_RELEASE' .github/workflows/ci.yml

# release.yml must NOT pin the release profile (inherits full Cargo.toml):
grep -n 'CARGO_PROFILE_RELEASE' .github/workflows/release.yml   # → no matches
```

## Related references

- [CI Pipeline Reference](ci-pipeline.md) — pinned toolchain, `cargo-nextest`
  test job, disk freeing, caching, required checks.
- [CI Resource Discipline](ci-resource-discipline.md) — concurrency, timeout,
  matrix, and cache boundaries.
