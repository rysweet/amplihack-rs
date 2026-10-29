---
title: Codex native plugin probe
description: Output caps, failure classification and error messages for the native Codex plugin inventory probe.
type: reference
updated: 2026-10-29
---

# Codex native plugin probe

Before `amplihack codex`, `amplihack install` or `amplihack update` registers the Amplihack plugin with Codex, it asks the selected Codex executable for its plugin inventory. This page describes how that probe runs, how a failed probe is classified, and the exact messages each failure produces.

For installation steps, see [Use Amplihack with Codex](../howto/install-codex-plugin.md). For the transaction and rollback model around the probe, see [Native Codex installation ownership](../concepts/codex-installation-ownership.md).

## Contents

- [Where the probe runs](#where-the-probe-runs)
- [Commands and output caps](#commands-and-output-caps)
- [Failure classification](#failure-classification)
- [Error messages](#error-messages)
- [Diagnosing a failure](#diagnosing-a-failure)
- [Output redaction](#output-redaction)
- [Source locations](#source-locations)

## Where the probe runs

Two commands reach the probe, and each wraps its errors in a different prefix:

| Command | When it reaches the probe | Error prefix |
|---|---|---|
| `amplihack codex` | Only when Codex is the selected provider (`install/mod.rs`, launch reconciliation) | `failed native Codex reconciliation` |
| `amplihack install`, `amplihack update` | Always, whatever the selected provider | `failed native Codex installation` |
| Implicit framework re-stage | Only when Codex is the selected provider | `failed native Codex installation` |

Both then follow the same path:

```
codex_plugin::install
  └─ install_unix → path_scope::with_scope
       └─ install_scoped (codex_plugin/transaction.rs)
            ├─ native(["plugin", "list", "--json"])
            └─ on failure:
                 ├─ path_scope::check()
                 ├─ if not freshness::codex_selected():
                 │    └─ print warning, skip the optional plugin
                 └─ else classify_probe_failure(probe error,
                         plugin_capability())   # `codex plugin --help`
```

The probe and the capability check both use the absolute Codex path that was selected for the session. Neither resolves `codex` through `PATH` a second time. Both run with `CODEX_HOME` set to the scoped Codex home, so the probe does not read or change any other Codex home.

`path_scope::check()` runs before and after every child process, including the capability check when it runs. If the trusted home scope changes while a child is running, the install stops before it creates or changes any file.

## Commands and output caps

Amplihack waits for Codex to exit; there is no time limit on any native plugin call. This page covers only `codex plugin …` calls. Other Codex checks, such as the launcher's version check, keep their own limits.

| Call | Command | Output cap | Stdin |
|---|---|---|---|
| Inventory probe | `codex plugin list --json` | 1 MiB per stream | null |
| Capability check | `codex plugin --help` | 64 KiB per stream | null |
| All other native calls (planning, registration, rollback, uninstall) | `codex plugin … --json` | 1 MiB per stream | null |

The first `plugin list` after a Codex update can be slow. On codex-cli 0.165.0 a cold run took 32.8 s. A fixed limit would report a slow but healthy Codex as a failure, so Amplihack does not apply one. There is no environment variable or configuration key that adds a limit.

Output beyond the cap is read and discarded, so a Codex that writes a lot cannot fill a pipe and stall, and memory use stays bounded.

A Codex that never exits blocks the command until you stop it. Ctrl-C reaches Codex as well as Amplihack. In unattended runs, such as CI jobs or detached sessions, wrap the Amplihack command in your own outer limit if you need one:

```bash
timeout 10m amplihack install
```

The capability check runs only after a failed probe, and only when Codex is the selected provider.

Every native call runs with stdin set to null. A child process cannot read keystrokes from your terminal while a probe is running.

## Failure classification

When the inventory probe succeeds, installation continues and no capability check runs.

When it fails and Codex is the selected provider, Amplihack runs `codex plugin --help` and treats native plugin support as present only if all of these hold:

- the command exits with status 0;
- one stdout line starts with `Usage:` and has `plugin` as its third word, so the binary's own name does not matter;
- one stdout line, after leading whitespace is removed, starts with `list` followed by whitespace.

Support is treated as absent only when the subcommand is clearly missing: the command exits with status 0 without plugin help, or exits with status 2 and stderr contains `unrecognized subcommand`. Older Codex releases without the `plugin` subcommand behave in one of these two ways. Any other exit is reported as a failed capability check, not as missing support. stderr is only compared with that fixed string and is never quoted.

When Codex is not the selected provider, the capability check does not run, because its result would not change the outcome. The optional plugin is skipped with a warning.

The outcome depends on whether Codex is the selected provider:

| Codex selected | Capability check | Outcome |
|---|---|---|
| Yes | Support present | Error. The probe's own failure (spawn failure, exit status or invalid JSON) is reported. |
| Yes | Support absent | Error: `selected Codex requires native plugin support; update Codex and retry`. |
| Yes | Check failed (spawn failure or any other exit) | Error. Both the probe failure and the check failure are reported. |
| No | Not run | Warning. The optional plugin is skipped and the command continues. |

A failed probe never counts as success. After a failed probe, installation either stops with an error or skips the optional plugin, and nothing is registered. A capability check that fails is never treated as "support present".

The "update Codex" advice appears only in the second row, where Codex has been shown to lack the `plugin list` subcommand.

## Error messages

The examples below show the errors as `amplihack codex` prints them, with a Codex executable at `/usr/local/bin/codex`. Under `amplihack install`, `amplihack update` or a re-stage, the text after the prefix is the same, but the prefix reads `failed native Codex installation` instead of `failed native Codex reconciliation`.

### Probe exited non-zero, plugin support present

```
error: failed native Codex reconciliation: selected Codex supports native plugins but the plugin inventory probe failed: Codex native plugin command failed (exit status: 1); check native plugin support and authentication
```

A non-zero exit with plugin support present usually means Codex is not authenticated or its configuration under `CODEX_HOME` cannot be read.

### Probe returned output that is not JSON

```
error: failed native Codex reconciliation: selected Codex supports native plugins but the plugin inventory probe failed: invalid native Codex plugin response; expected JSON inventory
```

### Codex lacks plugin support

```
error: failed native Codex reconciliation: selected Codex requires native plugin support; update Codex and retry
```

### Probe and capability check both failed

```
error: failed native Codex reconciliation: native plugin inventory probe failed; capability check also failed: native plugin capability check `plugin --help` failed with exit status: 1: Codex native plugin command failed (exit status: 1); check native plugin support and authentication
```

### Codex not selected

Only `amplihack install` and `amplihack update` print this warning, when a provider other than Codex is selected. `amplihack codex` never reaches it, because it runs the probe only when Codex is selected. The command continues and prints:

```
  ⚠️ Optional Codex plugin skipped: native plugin inventory probe failed: Codex native plugin command failed (exit status: 1); check native plugin support and authentication
```

The text after `native plugin inventory probe failed: ` is the probe's own failure, in the same form as the errors above. This line replaces the earlier `native plugin support unavailable` warning.

## Diagnosing a failure

Run the checks against the same executable and the same Codex home that Amplihack used. `which -a codex` is not enough on its own: `AMPLIHACK_CODEX_BINARY_PATH`, then `CODEX_BINARY_PATH`, take precedence over `PATH`. Amplihack uses `CODEX_HOME` when it is set and not empty, and `~/.codex` otherwise.

```bash
CODEX_BIN="${AMPLIHACK_CODEX_BINARY_PATH:-${CODEX_BINARY_PATH:-$(command -v codex)}}"
export CODEX_HOME="${CODEX_HOME:-$HOME/.codex}"
readlink -f "$CODEX_BIN"
"$CODEX_BIN" --version
# codex-cli 0.165.0
"$CODEX_BIN" plugin --help | head -n 1
# Usage: codex plugin [OPTIONS] <COMMAND>
time "$CODEX_BIN" plugin list --json < /dev/null > /dev/null
"$CODEX_BIN" login status
```

If none of the override variables is set and `command -v codex` finds nothing, Amplihack falls back to `~/.npm-global/bin`, `~/.cargo/bin` and `~/.local/bin`, in that order.

Then match the error to its cause:

| Message contains | Cause | Action |
|---|---|---|
| `exit status: N` | Codex rejected the command. | Check `"$CODEX_BIN" login status` and the configuration under `$CODEX_HOME`, then retry. |
| `invalid native Codex plugin response` | Codex wrote output that is not JSON to stdout. | Run `"$CODEX_BIN" plugin list --json` and inspect the output. |
| `requires native plugin support` | The selected Codex has no `plugin list` subcommand. | Update Codex, or point `AMPLIHACK_CODEX_BINARY_PATH` or `PATH` at a newer Codex. |
| `capability check also failed` | Codex is selected, and `plugin --help` did not start or exited with an unexpected status. | Check that the selected path is executable and not a stale symlink. |

## Output redaction

Error messages, printed warnings and tracing events from this path never include bytes from the child's stdout or stderr. They contain only:

- the program path and pid;
- the exit status;
- fixed text and errors generated by Amplihack.

The capability check compares stdout against fixed strings and returns a yes or no answer. It reads stderr only on exit status 2, to look for the fixed string `unrecognized subcommand`, and never quotes it. JSON parse errors are replaced with a fixed message, because `serde_json` errors can quote the input.

This keeps tokens, terminal escape sequences and other content that Codex writes out of Amplihack's logs and of any error that is pasted into an issue.

## Source locations

| Item | File |
|---|---|
| `native`, `plugin_capability`, `classify_probe_failure`, output caps | `crates/amplihack-cli/src/commands/install/codex_plugin/native.rs` |
| `run_output_limited` (output caps, no time limit) | `crates/amplihack-cli/src/util.rs` |
| `install_scoped` probe block | `crates/amplihack-cli/src/commands/install/codex_plugin/transaction.rs` |
| Error prefixes (`failed native Codex reconciliation`, `failed native Codex installation`) | `crates/amplihack-cli/src/commands/install/mod.rs` |
| Codex executable selection (`AMPLIHACK_CODEX_BINARY_PATH`, `CODEX_BINARY_PATH`, `PATH`, fallback directories) | `crates/amplihack-utils/src/launch_target.rs` |
| Regression tests | `probe_tests.rs`, `capability_tests.rs`, `classify_tests.rs` and `probe_install_tests.rs` in `crates/amplihack-cli/src/commands/install/codex_plugin/` |

The tests run fake `#!/bin/sh` Codex executables from temporary directories and cover:

- a slow Codex (`sleep 2` before answering) for both the probe and the capability check, which must still succeed;
- the 0.165.0 `plugin --help` format;
- top-level usage with exit status 2;
- a non-zero exit with plugin support present;
- a `plugin --help` that fails for a reason other than a missing subcommand;
- plugin help printed under another binary name;
- output that is not JSON;
- a Codex without plugin support;
- a failed probe when Codex is not selected, which must skip the plugin and must not run `plugin --help`.

Each fake writes a canary string and an ESC byte to stderr, and most also write a canary to stdout. Each test asserts that neither appears in the error. Run them with:

```bash
cargo test -p amplihack-cli codex_plugin
```
