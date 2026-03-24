# Parity Test Scenarios — Reference

The parity test harness (`tests/parity/validate_cli_parity.py`) compares the
Python and Rust `amplihack` CLIs by running identical scenarios against both
implementations and diffing their outputs.

This document describes every scenario tier file, the test cases each contains,
and what behaviour each tier validates.

## Contents

- [Running parity tests](#running-parity-tests)
- [Scenario file format](#scenario-file-format)
  - [Case fields](#case-fields)
  - [Comparison targets](#comparison-targets)
  - [Environment variable expansion](#environment-variable-expansion)
- [Tier files](#tier-files)
  - [tier1.yaml — Mode detection](#tier1yaml--mode-detection)
  - [tier2-install.yaml — Install command](#tier2-installyaml--install-command)
  - [tier2-plugin.yaml — Plugin command](#tier2-pluginyaml--plugin-command)
  - [tier3-memory.yaml — Memory command](#tier3-memoryyaml--memory-command)
  - [tier4-recipe-run.yaml — Recipe run](#tier4-recipe-runyaml--recipe-run)
  - [tier5-e2e.yaml — End-to-end launch](#tier5-e2eyaml--end-to-end-launch)
  - [tier5-gap-tests.yaml — Known gaps](#tier5-gap-testsyaml--known-gaps)
  - [tier5-launcher.yaml — Launcher flags](#tier5-launcheryaml--launcher-flags)
  - [tier5-live-recipe.yaml — Live recipe execution](#tier5-live-recipeyaml--live-recipe-execution)
  - [tier5-malformed-yaml.yaml — Error handling](#tier5-malformed-yamalyaml--error-handling)
  - [tier6-qa-bugfixes.yaml — QA regressions](#tier6-qa-bugfixesyaml--qa-regressions)
  - [tier7-launcher-parity.yaml — Launcher gaps](#tier7-launcher-parityyaml--launcher-gaps)
  - [tier8-env-vars.yaml — Environment variable injection](#tier8-env-varsyaml--environment-variable-injection)
  - [tier9-copilot-control-plane.yaml — Copilot control plane](#tier9-copilot-control-planeyaml--copilot-control-plane)
  - [tier10-pre-tool-use-hook.yaml — Copilot pre-tool-use hook](#tier10-pre-tool-use-hookyaml--copilot-pre-tool-use-hook)
  - [tier11-xpia-fail-closed.yaml — XPIA fail-closed behavior](#tier11-xpia-fail-closedyaml--xpia-fail-closed-behavior)
  - [tier12-xpia-malformed-output.yaml — XPIA malformed output](#tier12-xpia-malformed-outputyaml--xpia-malformed-output)
  - [tier13-xpia-timeout.yaml — XPIA timeout behavior](#tier13-xpia-timeoutyaml--xpia-timeout-behavior)
  - [tier14-xpia-logging.yaml — XPIA hook logging](#tier14-xpia-loggingyaml--xpia-hook-logging)
  - [tier15-session-start-hook.yaml — Copilot session-start hook](#tier15-session-start-hookyaml--copilot-session-start-hook)
  - [tier16-user-prompt-submit-hook.yaml — Copilot user-prompt-submit hook](#tier16-user-prompt-submit-hookyaml--copilot-user-prompt-submit-hook)
  - [tier17-session-stop-hook.yaml — Copilot session-stop hook](#tier17-session-stop-hookyaml--copilot-session-stop-hook)
  - [tier18-post-tool-use-hook.yaml — Copilot post-tool-use hook](#tier18-post-tool-use-hookyaml--copilot-post-tool-use-hook)
  - [tier19-pre-compact-legacy-runtime.yaml — PreCompact legacy runtime](#tier19-pre-compact-legacy-runtimeyaml--precompact-legacy-runtime)
  - [Related](#related)

---

## Running parity tests

```sh
# Run all cases in a single tier
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier8-env-vars.yaml

# Run a specific case by name
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier8-env-vars.yaml \
  --case env-var-agent-binary-is-claude

# Keep sandbox directories for post-mortem inspection
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier8-env-vars.yaml \
  --keep-sandboxes

# Side-by-side tmux panes (requires tmux)
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier1.yaml \
  --observable
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

Historically documented Python launcher behaviours that had not yet been ported
to Rust. The current cases now pass and serve as regression coverage for those
formerly divergent seams.

**Expected result:** All cases pass; comments may describe the historical gap
that motivated the regression.

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

Historically tracked launcher-level gaps around injected child-process flags,
session env handling, and alias parity. The current tier is green and now acts
as launcher regression coverage rather than a gap ledger.

**Expected result:** All cases pass.

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

All current tier-8 cases pass. They remain important because they verify the
launcher env contract that nested workflows and tool wrappers rely on.

**Example run:**

```sh
python tests/parity/validate_cli_parity.py \
  --scenario tests/parity/scenarios/tier8-env-vars.yaml

# Expected output:
# PASS env-var-agent-binary-is-claude
# PASS env-var-amplihack-home-contains-amplihack
```

---

### tier9-copilot-control-plane.yaml — Copilot control plane

Validates the dedicated `amplihack copilot` runtime surface beyond the generic
launcher contract. These cases compare:

- default Copilot launch argv
- explicit-args override behavior
- generated `.github/hooks` artifacts
- staged `~/.copilot/copilot-instructions.md`
- local plugin registration under `~/.copilot/installed-plugins/`
- Copilot MCP config staging under `~/.copilot/github-copilot/mcp.json`

**Expected result:** All cases pass. This tier now serves as regression coverage
for the dedicated Copilot control-plane contract.

---

### tier10-pre-tool-use-hook.yaml — Copilot pre-tool-use hook

Exercises the staged `.github/hooks/pre-tool-use` wrapper end-to-end through the
`amplihack copilot` launch path. The current cases compare:

- safe Bash input that should pass with an empty JSON response
- blocked Bash input (`git commit --no-verify ...`) that should return a deny payload

**Expected result:** All cases pass. This tier acts as outside-in regression
coverage for the staged Copilot pre-tool-use hook contract.

---

### tier11-xpia-fail-closed.yaml — XPIA fail-closed behavior

Exercises the staged Copilot `pre-tool-use` wrapper when the Rust-backed XPIA
binary is unavailable or misbehaves. The current cases compare:

- missing `xpia-defend` on `PATH`
- fake `xpia-defend` that returns exit code `2` despite valid-looking JSON

**Expected result:** All cases pass. This tier acts as regression coverage for
the fail-closed XPIA contract that must hold even under degraded runtime conditions.

---

### tier12-xpia-malformed-output.yaml — XPIA malformed output

Exercises the staged Copilot `pre-tool-use` wrapper when `xpia-defend` returns
syntactically or structurally invalid output. The current cases compare:

- fake `xpia-defend` that prints non-JSON output and exits `0`
- fake `xpia-defend` that exits `0` but produces no stdout

**Expected result:** All cases pass. This tier extends the fail-closed parity
matrix from "binary missing/internal error" into malformed-output behavior.

---

### tier13-xpia-timeout.yaml — XPIA timeout behavior

Exercises the staged Copilot `pre-tool-use` wrapper when `xpia-defend` hangs.
For deterministic test runtime, the sandbox copy of the Python hook lowers the
bridge timeout before invoking the real validation flow. The current tier
compares:

- fake `xpia-defend` that sleeps past the shortened timeout and must yield a deny payload

**Expected result:** All cases pass. This tier extends degraded-runtime parity
from missing/malformed binaries into timeout handling.

---

### tier14-xpia-logging.yaml — XPIA hook logging

Exercises the staged Copilot `pre-tool-use` wrapper's Rust-backed audit logging.
Instead of comparing raw log lines with timestamps, the scenarios normalize the
last log entry into a stable JSON summary. The current cases compare:

- allowed command logging with custom `session_id` passthrough
- denied command logging with threat count and risk metadata

**Expected result:** All cases pass. This tier extends outside-in parity into
the staged hook's security audit trail.

---

### tier15-session-start-hook.yaml — Copilot session-start hook

Exercises the staged `.github/hooks/session-start` wrapper end to end under both
supported hook-engine modes. The current cases compare:

- `AMPLIHACK_HOOK_ENGINE=rust`: wrapper forwards stdin JSON into `amplihack-hooks session-start`
- `AMPLIHACK_HOOK_ENGINE=python`: wrapper forwards stdin JSON into `session_start.py`

**Expected result:** All cases pass. This tier extends runtime parity from
artifact staging into actual session-start wrapper execution.

---

### tier16-user-prompt-submit-hook.yaml — Copilot user-prompt-submit hook

Exercises the staged `.github/hooks/user-prompt-submit` wrapper end to end
under both supported hook-engine modes. The current cases compare:

- `AMPLIHACK_HOOK_ENGINE=rust`: wrapper runs native `user-prompt-submit` plus the Python `workflow_classification_reminder.py`
- `AMPLIHACK_HOOK_ENGINE=python`: wrapper runs both Python hook scripts in order

**Expected result:** All cases pass. This tier extends runtime parity into the
multi-hook composition contract used for prompt-time workflow guidance.

---

### tier17-session-stop-hook.yaml — Copilot session-stop hook

Exercises the staged `.github/hooks/session-stop` wrapper end to end under
both supported hook-engine modes. The current cases compare:

- `AMPLIHACK_HOOK_ENGINE=rust`: wrapper runs native `stop` and `session-stop`
  subcommands with the same stdin payload
- `AMPLIHACK_HOOK_ENGINE=python`: wrapper runs both `stop.py` and
  `session_stop.py` with forwarded stdin

**Expected result:** All cases pass. This tier extends runtime parity into the
multi-hook shutdown contract used for power-steering and session-finalization
behavior.

---

### tier18-post-tool-use-hook.yaml — Copilot post-tool-use hook

Exercises the staged `.github/hooks/post-tool-use` wrapper end to end under
both supported hook-engine modes. The current cases compare:

- `AMPLIHACK_HOOK_ENGINE=rust`: wrapper `exec`s native `post-tool-use` with the
  original stdin payload
- `AMPLIHACK_HOOK_ENGINE=python`: wrapper `exec`s `post_tool_use.py` with the
  same stdin payload and argv forwarding

**Expected result:** All cases pass. This tier extends runtime parity into the
post-tool execution path rather than only checking staged artifact contents.

---

### tier19-pre-compact-legacy-runtime.yaml — PreCompact legacy runtime

Exercises the direct `PreCompact` hook runtime against the legacy
conversation-payload shape used by the Python hook. The case bypasses the
top-level CLI and invokes the Python hook script and native Rust hooks binary
directly, then compares normalized session artifacts:

- `CONVERSATION_TRANSCRIPT.md`
- `ORIGINAL_REQUEST.md`
- `compaction_events.json`
- transcript copy creation under `transcripts/`

**Expected result:** All cases pass. This tier extends parity beyond Copilot
wrappers into the non-Copilot hook runtime used before context compaction.

---

## Related

- [validate_cli_parity.py](../../tests/parity/validate_cli_parity.py) — Harness source
- [Environment Variables](./environment-variables.md) — Reference for all variables injected during launch
- [Agent Binary Routing](../concepts/agent-binary-routing.md) — Why `AMPLIHACK_AGENT_BINARY` exists
- [Bootstrap Parity](../concepts/bootstrap-parity.md) — Design principles behind Python↔Rust parity
