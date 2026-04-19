# How to Validate No-Python Compliance

Confirm that the `amplihack` binary operates correctly in an environment where
no Python interpreter is on `PATH`. This is the acceptance test for AC9 in
issue #77.

## Before you start

You need:

- The amplihack-rs repository checked out
- `cargo` available to build the binary
- `bash` (the probe script uses bash features)

## Run the probe

```sh
# Build debug binary and run all smoke tests in a Python-free environment
./scripts/probe-no-python.sh

# Build release binary instead
./scripts/probe-no-python.sh --release
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
  smoke: memory index --help ... PASS
  smoke: memory query --help ... PASS
  smoke: memory query smoke (empty db) ... PASS

==> Results: 8 passed, 0 failed
PASS: All smoke tests passed with no Python interpreter on PATH (AC9).
```

Exit code 0 means the binary is Python-free on all tested paths.

## What the probe tests

| Test case | What it verifies |
|-----------|-----------------|
| TC-01: `--version` | Binary loads and reports its version without Python |
| TC-02: `--help` | Top-level help page renders without Python |
| TC-03: `fleet --help` | Fleet subcommand help page renders without Python |
| TC-04: `doctor --help` | Doctor subcommand is registered and accessible |
| TC-05: `recipe --help` | Recipe subcommand is registered and accessible |
| TC-06: `memory index --help` | Memory index subcommand is registered and accessible |
| TC-07: `memory query --help` | Memory query subcommand is registered and accessible |
| TC-08: `memory query` smoke | `query-code stats` against an empty temp database exits without launching Python |

TC-08 uses `mktemp` to create a temporary LadybugDB database path, runs
`query-code stats` against it, and cleans up via an `EXIT` trap. The test
validates that the binary does not crash and does not invoke Python, even when
the database is empty (LadybugDB auto-creates the schema on first open).

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
| `memory index --help` FAIL | The `index-scip` subcommand is not registered in the CLI router |
| `memory query` smoke FAIL | The `query-code` binary panics when LadybugDB creates a new empty database |

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
- name: Validate no-Python compliance
  run: ./scripts/probe-no-python.sh
```

The probe builds the binary itself, so no separate build step is required
before it. If you want to validate the release binary, pass `--release`:

```yaml
- name: Validate no-Python compliance (release)
  run: ./scripts/probe-no-python.sh --release
```

## Related

- [No-Python Compliance (AC9)](../concepts/kuzu-code-graph.md#security-model) — why this matters
- [`scripts/probe-no-python.sh`](https://github.com/rysweet/amplihack-rs/blob/main/scripts/probe-no-python.sh) — the probe script itself
- [Parity Test Scenarios](../reference/parity-test-scenarios.md) — full parity test matrix
