# Parity Test Scenarios — Reference

The parity scenario files are retained as historical migration fixtures. The
Python-based parity harness has been retired; native Rust tests and
`scripts/probe-no-python.sh` now guard the shipped implementation.

This document describes every scenario tier file, the test cases each contains,
and what behaviour each tier validates.

## Contents

- [Running parity checks](#running-parity-checks)
- [Scenario file format](#scenario-file-format)
  - [Case fields](#case-fields)
  - [Comparison targets](#comparison-targets)
  - [Environment variable expansion](#environment-variable-expansion)
- [Tier files](#tier-files)
  - [tier1.yaml — Mode detection](#tier1yaml-mode-detection)
  - [tier2-install.yaml — Install command](#tier2-installyaml-install-command)
  - [tier2-plugin.yaml — Plugin command](#tier2-pluginyaml-plugin-command)
  - [tier3-memory.yaml — Memory command](#tier3-memoryyaml-memory-command)
  - [tier4-recipe-run.yaml — Recipe run](#tier4-recipe-runyaml-recipe-run)
  - [tier5-e2e.yaml — End-to-end launch](#tier5-e2eyaml-end-to-end-launch)
  - [tier5-gap-tests.yaml — Known gaps](#tier5-gap-testsyaml-known-gaps)
  - [tier5-launcher.yaml — Launcher flags](#tier5-launcheryaml-launcher-flags)
  - [tier5-live-recipe.yaml — Live recipe execution](#tier5-live-recipeyaml-live-recipe-execution)
  - [tier5-malformed-yaml.yaml — Error handling](#tier5-malformed-yamlyaml-error-handling)
  - [tier6-qa-bugfixes.yaml — QA regressions](#tier6-qa-bugfixesyaml-qa-regressions)
  - [tier7-launcher-parity.yaml — Launcher gaps](#tier7-launcher-parityyaml-launcher-gaps)
  - [tier8-env-vars.yaml — Environment variable injection](#tier8-env-varsyaml-environment-variable-injection)
- [Related](#related)

---

## Running parity checks

```sh
# Run native tests that cover migrated behavior
cargo test --workspace --locked

# Verify the CLI works without Python on PATH
scripts/probe-no-python.sh

# Verify no Python implementation/package assets are tracked
scripts/check-no-python-assets.sh
```

---

## Scenario file format

Each YAML file contains a top-level `cases:` list. Each entry is one test case.

```yaml
cases:
  - name: example-case
    argv: ["launch"]
    timeout: 15
    env:
      PATH: "${SANDBOX_ROOT}/bin:${PATH}"
      AMPLIHACK_NONINTERACTIVE: "1"
    setup: |
      mkdir -p bin
      cat > bin/claude <<'SCRIPT'
      #!/usr/bin/env bash
      printf '%s\n' "$@" > "${SANDBOX_ROOT}/claude_args.txt"
      SCRIPT
      chmod +x bin/claude
    compare:
      - exit_code
      - stdout
      - fs:claude_args.txt
```

### Case fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Unique identifier for the test case |
| `argv` | yes | Argument list passed to the CLI (without the binary name) |
| `timeout` | no | Seconds before the case is killed (default: 30) |
| `env` | no | Extra environment variables for both Python and Rust runs |
| `setup` | no | Shell script run once per engine before execution, in `$SANDBOX_ROOT` |
| `compare` | yes | List of comparison targets (see below) |
| `cwd` | no | Working directory relative to `$SANDBOX_ROOT` |
| `stdin` | no | String piped to stdin of both engines |

### Comparison targets

| Target | What is compared |
|--------|-----------------|
| `exit_code` | Process exit code |
| `stdout` | Captured stdout, newline-normalised |
| `stderr` | Captured stderr, newline-normalised |
| `fs:<path>` | Content of `$SANDBOX_ROOT/<path>` after execution |

### Environment variable expansion

`${SANDBOX_ROOT}` in `env:` values and `setup:` scripts is expanded to the
absolute path of the per-engine temporary directory. Use double-quoted
`"${SANDBOX_ROOT}"` in shell redirections to handle paths with spaces.

`${PATH}` expands to the inherited `PATH` at harness startup. `${HOME}` expands
to the user's home directory.

---

## Tier files

### tier1.yaml — Mode detection

Validates `amplihack mode detect`, `mode to-plugin`, and `mode to-local`
commands. Tests both the dry-run and confirmation paths. Filesystem comparisons
verify that `.claude/` layout changes match between Python and Rust.

**Expected result:** All cases pass (no known gaps).

---

### tier2-install.yaml — Install command

Validates `amplihack install` and `amplihack uninstall` in sandboxed home
directories. Covers first-install, idempotent re-install, and clean removal.
Filesystem comparisons check hook files, the uninstall manifest, and binary
symlinks.

**Expected result:** All cases pass (no known gaps).

---

### tier2-plugin.yaml — Plugin command

Validates `amplihack plugin list`, `plugin install`, and `plugin uninstall`.
Covers installing a plugin from a local directory, listing installed plugins,
and removing them.

**Expected result:** All cases pass (no known gaps).

---

### tier3-memory.yaml — Memory command

Validates `amplihack memory show`, `memory add`, and `memory clear`. Tests both
empty-state and populated-state scenarios.

**Expected result:** All cases pass (no known gaps).

---

### tier4-recipe-run.yaml — Recipe run

Validates `amplihack recipe run` with local YAML recipe files. Tests
single-step recipes, multi-step recipes, and recipes with environment variable
interpolation.

**Expected result:** All cases pass (no known gaps).

---

### tier5-e2e.yaml — End-to-end launch

Validates full-path launch scenarios using a stub `claude` binary that captures
its arguments and environment. Covers the complete `EnvBuilder` chain output.

**Expected result:** All cases pass (no known gaps).

---

### tier5-gap-tests.yaml — Known gaps

Documents Python launcher behaviours not yet ported to Rust. Cases in this file
are **expected to show divergence**. Kept as a living record of outstanding
parity work.

**Expected result:** Divergence is expected and documented in YAML comments.

---

### tier5-launcher.yaml — Launcher flags

Validates `--resume`, `--continue`, `--skip-permissions`, and `--skip-update-check`
flag behaviour via stub binaries that capture command-line arguments.

**Expected result:** All cases pass (no known gaps).

---

### tier5-live-recipe.yaml — Live recipe execution

Validates recipe runner execution against a real recipe YAML, asserting stdout
line patterns and exit codes. Requires a working `amplifier` binary on `PATH`
and is skipped when `AMPLIHACK_SKIP_LIVE_TESTS=1`.

**Expected result:** All cases pass when live dependencies are present.

---

### tier5-malformed-yaml.yaml — Error handling

Validates error handling when recipe YAML is syntactically invalid, semantically
incorrect, or references missing steps. Compares exit codes and stderr error
messages between Python and Rust.

**Expected result:** All cases pass (no known gaps).

---

### tier6-qa-bugfixes.yaml — QA regressions

Regression tests for specific bugs fixed during QA cycles. Each case includes
a comment referencing the original issue. Cases are added here when a bug fix
is confirmed on both Python and Rust.

**Expected result:** All cases pass (no known gaps).

---

### tier7-launcher-parity.yaml — Launcher gaps

Documents launcher-level gaps: flags that the Python launcher injects into the
child process (`--dangerously-skip-permissions`, `--model`) that the Rust
launcher did not inject at the time of writing. See [GitHub Issue #25].

**Expected result:** Divergence is expected and documented in YAML comments.

---

### tier8-env-vars.yaml — Environment variable injection

Validates that the Rust launcher correctly injects `AMPLIHACK_AGENT_BINARY` and
`AMPLIHACK_HOME` into the child process environment. Each case uses a stub
`claude` binary that captures specific environment variables to a file, which is
then compared between Python and Rust.

| Case | What it validates |
|------|------------------|
| `env-var-agent-binary-is-claude` | `AMPLIHACK_AGENT_BINARY=claude` is set for `amplihack claude` |
| `env-var-amplihack-home-contains-amplihack` | `AMPLIHACK_HOME` is set to a path containing `.amplihack` |

#### Expected divergence: AMPLIHACK_AGENT_BINARY

> **The `env-var-agent-binary-is-claude` case will show a mismatch between
> Python and Rust outputs. This divergence is intentional.**
>
> - **Rust launcher**: always sets `AMPLIHACK_AGENT_BINARY` before exec.
> - **Python launcher**: does not set `AMPLIHACK_AGENT_BINARY`.
>
> The `fs:agent_binary.txt` comparison will therefore show an empty file from
> the Python run against `claude` from the Rust run. This is not a regression —
> it documents a known behavioral gap. The Rust implementation reflects the
> correct intended behavior. The Python implementation is expected to be updated
> to match.

This divergence is also recorded as a YAML comment in `tier8-env-vars.yaml`
alongside the `env-var-agent-binary-is-claude` case definition.

Both cases set `AMPLIHACK_NONINTERACTIVE=1` in the `env:` block to prevent
bootstrap prompts from interfering with the captured output.

**Current verification:**

```sh
cargo test -p amplihack --test cli_launch --locked
scripts/probe-no-python.sh
```

---

## Related

- [No-Python Validation](../howto/validate-no-python.md) — Runtime and repository checks
- [Environment Variables](./environment-variables.md) — Reference for all variables injected during launch
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — Why `AMPLIHACK_AGENT_BINARY` exists
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — Design principles behind Python↔Rust parity
