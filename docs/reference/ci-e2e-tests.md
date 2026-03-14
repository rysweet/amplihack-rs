# CI E2E Test Job — Reference

The `test-e2e` job in `.github/workflows/ci.yml` runs integration tests that
require `recipe-runner-rs` to be on `PATH`. These tests are excluded from the
standard `test` job because `recipe-runner-rs` is a separate binary not
available in a stock Rust/CI environment.

## Contents

- [Overview](#overview)
- [Job definition](#job-definition)
- [Why a separate job](#why-a-separate-job)
- [Tests covered](#tests-covered)
- [Running locally](#running-locally)
- [Stabilisation path](#stabilisation-path)
- [Related](#related)

---

## Overview

```
CI pipeline
├── check          — cargo fmt + clippy
├── test           — cargo test --workspace --locked (needs: check)
├── test-e2e       — installs recipe-runner-rs, then cargo test --workspace -- --ignored (needs: check, test)
└── cross-compile  — builds for all release targets (needs: check; does not wait for test-e2e)
```

`test-e2e` runs after **both** `check` and `test` pass. This ordering ensures
runner minutes are not consumed by E2E tests when unit tests have already
failed. `test-e2e` does not block `cross-compile` or `release`.

---

## Job definition

```yaml
test-e2e:
  name: E2E Tests (recipe-runner-rs)
  runs-on: ubuntu-latest
  needs: [check, test]
  continue-on-error: true
  permissions:
    contents: read

  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable

    - name: Cache Cargo registry
      uses: actions/cache@v4
      with:
        path: |
          ~/.cargo/registry
          ~/.cargo/git
        key: ${{ runner.os }}-cargo-e2e-${{ hashFiles('**/Cargo.lock') }}
        restore-keys: |
          ${{ runner.os }}-cargo-e2e-

    - name: Install recipe-runner-rs
      run: |
        cargo install \
          --git https://github.com/rysweet/amplihack-recipe-runner \
          --rev <PINNED_SHA> \
          --locked \
          recipe-runner-rs

    - uses: Swatinem/rust-cache@v2

    - name: Run ignored (E2E) tests
      run: cargo test --workspace --locked -- --ignored
```

> **Private repository:** If `amplihack-recipe-runner` is a private repository,
> `cargo install --git` requires authentication. Store a GitHub PAT with `repo`
> scope as a repository secret (e.g. `RECIPE_RUNNER_TOKEN`) and expose it via
> the `GH_TOKEN` environment variable on the install step. **Never embed the
> token in the YAML file.** Verify repo visibility before filing the PR:
>
> ```sh
> gh repo view rysweet/amplihack-recipe-runner --json visibility
> ```

> **`<PINNED_SHA>`** is replaced at PR-creation time with the HEAD commit SHA
> of `amplihack-recipe-runner` recorded when the PR was created. Pin the SHA
> to prevent supply-chain drift. Update the pin when a new compatible version
> of `recipe-runner-rs` is released.

### continue-on-error: true

`continue-on-error: true` prevents a `test-e2e` failure from blocking merges
while the tests are stabilising. This flag is intentionally temporary. Once all
covered tests pass on three consecutive main-branch runs, open a follow-up PR
to remove it.

### permissions: contents: read

The job requires only read access to the repository. No secrets, no write
permissions, no deployment access.

---

## Why a separate job

The standard `test` job runs `cargo test --workspace --locked`, which skips
`#[ignore]`-marked tests. Those tests are marked `#[ignore]` because they
invoke `recipe-runner-rs` as a subprocess — a binary that is not in the
standard CI environment.

Splitting into a dedicated job keeps two concerns separate:

| Concern | Job |
|---------|-----|
| Unit and fast integration tests | `test` |
| Tests that require `recipe-runner-rs` on PATH | `test-e2e` |

The `test` job remains unchanged. Adding `test-e2e` cannot break existing
test runs.

---

## Tests covered

All `#[ignore]`-marked tests in the workspace are run by this job. The PR that
adds this job **must** enumerate the 6 specific test names by running:

```sh
cargo test --workspace -- --list --ignored
```

The table below must be populated before the PR is filed. Passing all 6 is a
hard gate for filing the CI PR (`local-e2e-validator` component). This table
is also the audit trail used when removing `continue-on-error: true`.

| Crate | Test name | What it validates |
|-------|-----------|------------------|
| `amplihack-cli` | `[POPULATE: run cargo test --workspace -- --list --ignored]` | `recipe run` end-to-end via real `recipe-runner-rs` subprocess |
| `amplihack-cli` | `[POPULATE]` | `run_doctor()` smoke test requiring a real environment |
| `amplihack-cli` | `[POPULATE]` | _(populate from output above)_ |
| `amplihack-cli` | `[POPULATE]` | _(populate from output above)_ |
| `amplihack-cli` | `[POPULATE]` | _(populate from output above)_ |
| `amplihack-cli` | `[POPULATE]` | _(populate from output above)_ |

> **Before filing the PR:** Replace every `[POPULATE]` row with the actual
> test function names and descriptions. `ws3_e2e_ci_patch.py` must complete
> this table as part of the patching step.

---

## Running locally

```sh
# Check if recipe-runner-rs is already installed
which recipe-runner-rs

# Install from source if not present
cargo install \
  --git https://github.com/rysweet/amplihack-recipe-runner \
  recipe-runner-rs

# Run only the ignored tests
cargo test --workspace -- --ignored

# Run all tests (unit + ignored)
cargo test --workspace -- --include-ignored
```

> Run the ignored tests **twice** on a fresh branch to confirm they are not
> flaky before adding them to CI.

---

## Stabilisation path

1. `continue-on-error: true` is set at job creation — failures are visible in
   the Actions UI but do not block merges.
2. **You must file a follow-up GitHub issue immediately after the PR merges**,
   titled: **"Remove continue-on-error from test-e2e once tests stabilise"**.
   Without this issue, broken E2E tests will be silently ignored indefinitely.
   Use this issue body as a template:

   > `continue-on-error: true` was set in `test-e2e` to allow stabilisation.
   > Remove it once all 6 covered tests pass on three consecutive `main`-branch
   > runs. See docs/reference/ci-e2e-tests.md §Tests-covered for the full list.

3. When all tests pass on three consecutive main-branch runs, open a PR that
   removes `continue-on-error: true`. This makes the E2E job a hard gate.

---

## Related

- [Parity Test Scenarios](./parity-test-scenarios.md) — Python↔Rust comparison test harness
- [amplihack recipe](./recipe-command.md) — CLI reference for the `recipe` subcommand
- [Run a Recipe End-to-End](../howto/run-a-recipe.md) — How-to for recipe execution
- [ci.yml](../../.github/workflows/ci.yml) — Full workflow definition
