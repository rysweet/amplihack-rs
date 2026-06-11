# How to Validate No-Python Compliance

Confirm that the `amplihack` repository ships no Python implementation assets
and that the `amplihack` binary operates correctly in an environment where no
Python interpreter is on `PATH`. This is the acceptance test for AC9 in issue
#77 and the final migration guard.

## Before you start

You need:

- The amplihack-rs repository checked out
- `cargo` available unless you reuse an already-built binary with
  `AMPLIHACK_PROBE_BIN`
- `bash` (the probe script uses bash features)

## Run the probe

```sh
# Build debug binary and run all smoke tests in a Python-free environment
./scripts/probe-no-python.sh

# Build release binary instead
./scripts/probe-no-python.sh --release

# Reuse an already-built binary instead of building inside the probe
AMPLIHACK_PROBE_BIN="$PWD/target/debug/amplihack" ./scripts/probe-no-python.sh

# Reuse wins over --release when the supplied binary is executable
AMPLIHACK_PROBE_BIN="$PWD/target/debug/amplihack" ./scripts/probe-no-python.sh --release

# Verify no tracked Python source or package metadata exists
./scripts/check-no-python-assets.sh
```

Expected output when all tests pass:

```
==> Building amplihack-rs...
    binary: /home/user/src/amplihack-rs/target/debug/amplihack

==> Stripping python/python3 from PATH...
    removed: /usr/bin (contains python3)

==> Verifying no Python interpreter on PATH...
    OK — no python/python3 on PATH

==> Running binary smoke tests (Python-free PATH)...
  smoke: --version ... PASS
  smoke: --help exits 0 ... PASS
  smoke: fleet --help ... PASS
  smoke: doctor --help ... PASS
  smoke: recipe --help ... PASS
  smoke: TC-04 index-code --help ... PASS
  smoke: TC-05 query-code --help ... PASS
  smoke: TC-06 query-code stats (empty DB) ... PASS
  smoke: TC-07 index-scip --help ... PASS
  smoke: TC-08 index-code + query-code populated graph ... PASS

==> Results: 10 passed, 0 failed
PASS: All smoke tests passed with no Python interpreter on PATH (AC9).
```

When `AMPLIHACK_PROBE_BIN` points to an executable file, the probe skips the
internal build step. In that reuse mode, do not expect the
`==> Building amplihack-rs...` banner; expect the probe to use the supplied
binary and continue with Python-free `PATH` stripping and the same smoke tests.

Exit code 0 from both scripts means the binary is Python-free on all tested
paths and the repository has no tracked Python source/package assets.

## Reuse an existing binary

`scripts/probe-no-python.sh` supports an optional `AMPLIHACK_PROBE_BIN`
environment variable for callers that already built `amplihack`:

```sh
cargo build --locked
AMPLIHACK_PROBE_BIN="$PWD/target/debug/amplihack" ./scripts/probe-no-python.sh
```

When `AMPLIHACK_PROBE_BIN` is set to an executable file, the probe uses that
binary directly and skips its internal `cargo build`. The reusable binary is
resolved before the script removes Python directories from `PATH`, so the smoke
tests still run in the Python-free environment.

`AMPLIHACK_PROBE_BIN` is explicit caller input. If it points to a stale binary,
the probe validates that stale binary. If it is unset, the probe preserves the
standalone behavior and builds `target/debug/amplihack`, or
`target/release/amplihack` when `--release` is supplied.

If `AMPLIHACK_PROBE_BIN` is set but does not point to an executable file, the
script prints a warning to stderr and falls back to the standalone cargo build
path. A valid `AMPLIHACK_PROBE_BIN` takes precedence over `--release`; passing
`--release` does not force a release build when an executable reusable binary is
provided.

## What the probe tests

| Test case | What it verifies |
|-----------|-----------------|
| TC-01: `--version` | Binary loads and reports its version without Python |
| TC-02: `--help` | Top-level help page renders without Python |
| TC-03: `fleet --help` | Fleet subcommand help page renders without Python |
| `doctor --help` | Doctor subcommand is registered and accessible |
| `recipe --help` | Recipe subcommand is registered and accessible |
| TC-04: `index-code --help` | Code-graph indexing help renders without Python |
| TC-05: `query-code --help` | Code-graph query help renders without Python |
| TC-06: `query-code stats` smoke | `query-code stats` against an empty temp database terminates without crashing or invoking Python |
| TC-07: `index-scip --help` | SCIP indexing help renders without Python |
| TC-08: `index-code` + `query-code` populated graph | Imports a tiny code graph, then verifies `query-code stats`, `search`, and `callers` without invoking Python |

TC-06 creates a fresh temporary code-graph database path for the empty-database
smoke test. TC-08 creates a temporary workspace, writes a tiny Blarify-style
JSON graph, imports it with `index-code`, queries it with `query-code stats`,
`query-code search`, and `query-code callers`, and cleans up via an `EXIT`
trap. Failing `python` and `python3` shims are placed first on `PATH` during
TC-08 so any interpreter dependency is caught.

## How the probe strips Python

The script iterates over every directory in `$PATH`. It drops any directory
that contains an executable named `python` or `python3`. The cleaned `PATH` is
exported before the smoke tests run.

If Python is still reachable after stripping (e.g. via a `pyenv` shim in an
unconventional location), the probe prints a `FAIL` message and exits 1 before
running any smoke tests.

## Add a new smoke test

Open `scripts/probe-no-python.sh` and add a `run_smoke` call in the smoke test
section:

```bash
# Template — add after the last run_smoke call
run_smoke "my new test"  "${BINARY}" my-subcommand --some-flag
```

`run_smoke` takes a label and a command. It runs the command with stdout and
stderr suppressed, records pass/fail, and increments the counters. The overall
exit code reflects the aggregate result.

## Interpret a FAIL result

A failing probe means at least one smoke test returned a non-zero exit code
while Python was absent from `PATH`. Common causes:

| Symptom | Likely cause |
|---------|-------------|
| `--version` FAIL | Binary was not built or is not executable |
| `fleet --help` FAIL | A `fleet` subcommand path panics at startup |
| `index-code --help` FAIL | The `index-code` subcommand is not registered in the CLI router |
| `query-code --help` FAIL | The `query-code` subcommand is not registered in the CLI router |
| `query-code stats` smoke FAIL | The `query-code` path crashes or tries to invoke Python when opening a fresh code-graph database |
| `index-scip --help` FAIL | The `index-scip` subcommand is not registered in the CLI router |
| `index-code + query-code` populated graph FAIL | Native graph import/querying broke or invoked Python during indexing, stats, search, or callers queries |

Run the failing command manually with Python stripped:

```sh
export PATH=$(echo "$PATH" | tr ':' '\n' | grep -v python | tr '\n' ':')
./target/debug/amplihack query-code --db-path /tmp/test_kuzu stats
```

Check the stderr output for the panic message or error.

## Run in CI

Add the probe as a CI step after `cargo build`:

```yaml
# GitHub Actions example
- name: Build amplihack
  run: cargo build --workspace --locked

- name: Validate no-Python compliance
  env:
    AMPLIHACK_PROBE_BIN: ${{ github.workspace }}/target/debug/amplihack
  run: ./scripts/probe-no-python.sh
```

For standalone use, the probe still builds the binary itself, so no separate
build step is required before it:

```yaml
- name: Validate no-Python compliance
  run: ./scripts/probe-no-python.sh
```

If you want standalone validation of the release binary, pass `--release`:

```yaml
- name: Validate no-Python compliance (release)
  run: ./scripts/probe-no-python.sh --release
```

If CI already built the release binary, reuse it explicitly:

```yaml
- name: Validate no-Python compliance (release)
  env:
    AMPLIHACK_PROBE_BIN: ${{ github.workspace }}/target/release/amplihack
  run: ./scripts/probe-no-python.sh
```

The repository asset guard is lightweight and should run early in CI:

```yaml
- name: Validate no tracked Python assets
  run: ./scripts/check-no-python-assets.sh
```

## Related

- [No-Python Compliance (AC9)](../concepts/kuzu-code-graph.md#security-model) — why this matters
- [`scripts/probe-no-python.sh`](https://github.com/rysweet/amplihack-rs/blob/main/scripts/probe-no-python.sh) — the probe script itself
- [Parity Test Scenarios](../reference/parity-test-scenarios.md) — full parity test matrix
