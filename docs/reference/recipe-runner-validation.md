---
title: Recipe runner validation
type: reference
updated: 2026-10-05
---

# Recipe runner validation

Installation, freshness checks and Claude shell bootstrap share native Rust
validation. Capability compatibility and managed provenance are separate checks.

## Configuration and provider policy

| Setting | Contract |
| --- | --- |
| `RECIPE_RUNNER_RS_PATH` | User-owned override. Codex rejects missing, nonexecutable or incompatible selections; Claude/Copilot preserve nonexistent-path discovery fallback and legacy custom runners. |
| `CARGO_HOME` | Effective Cargo home for managed binary and `.crates2.json`; defaults to `~/.cargo`. |
| `claude-plugin/recipe-runner.rev` | Bundled full immutable managed source revision; shared by Rust and Claude bootstrap. |
| `AMPLIHACK_AGENT_BINARY` | Existing provider routing; selecting Codex requires `codex_exec`. |

Custom version strings and newer compatible explicit builds are accepted without
requiring the bundled SHA. This does not classify them as managed installations.

## Capability report

`recipe-runner-rs --capabilities` exits zero and writes exactly one UTF-8 JSON
object, with optional surrounding whitespace. It performs no recipe execution,
network/update check or model invocation. A supported Codex report is:

```json
{"capabilities":["codex_exec"],"schema_version":1,"version":"0.4.0"}
```

| Property | Requirement |
| --- | --- |
| `schema_version` | Integer `1`, not a Boolean or string. |
| `version` | Nonblank string; independent of source revision. |
| `capabilities` | Array containing only strings. Codex requires exact membership of `codex_exec`; Claude/Copilot accept `[]`. |

Property order and compatible additional fields are accepted. Duplicate property
names, trailing data, invalid UTF-8, malformed JSON, wrong types, failed probes
and unsupported schemas fail validation. Probes have bounded duration and output.
Unsupported Codex platforms advertise an empty array rather than claiming support.
Python and Node are optional; semantic parsing uses existing native JSON support.

## Managed provenance

A single Cargo receipt entry must identify the `recipe-runner-rs` package, expected
git repository, full bundled revision and `recipe-runner-rs` binary. Matching a
revision from one entry and a binary from another is invalid. The selected
executable must be the actual managed binary in that effective Cargo home;
a compatible binary shadowing it elsewhere fails validation.

Missing, malformed or mismatched receipts and unsuccessful Cargo installation
fail visibly. Freshness reinstalls a different managed revision and validates the
result before recording success. Shell bootstrap propagates failures before
writing success stamps. If an older user-owned runtime lacks native validation,
bootstrap builds a private Cargo validator without replacing the user's runtime.
Receipts describe local installation provenance, not cryptographic attestation.

## Internal validation endpoint

`amplihack internal validate-recipe-runner` is an internal integration command,
hidden from ordinary help. It accepts these typed arguments:

| Argument | Requirement |
| --- | --- |
| `--binary` | Absolute path to the exact selected executable. |
| `--provider` | Explicit `claude`, `copilot` or `codex`; not inferred from ambient routing. |
| `--cargo-home`, `--repository`, `--revision` | All present for managed validation, or all absent for user-owned validation. Revision is the full immutable SHA. |

Arguments are validated before spawning. The endpoint probes the selected binary
once and reads the receipt in managed mode; it never installs software or changes
configuration or stamps. Success exits zero with one schema-1 JSON result containing
`provider`, resolved `binary`, parsed `runner` report and `provenance`.
Managed provenance contains `kind: "managed"`, `package`, `repository`, `revision`
and `cargo_home`; user-owned provenance contains `kind: "explicit"`.

Operational failures exit 1 with empty stdout and one JSON error on stderr:

```json
{"error":{"code":"RUNNER_CAPABILITY_MISSING","message":"recipe-runner-rs must advertise codex_exec for Codex; fix or unset the override","details":{"provider":"codex","stage":"capability"}}}
```

Argument parsing retains the CLI parser's nonzero convention. Diagnostics exclude
raw prompts, credentials, configuration contents and unbounded subprocess output.
Public CLI errors remain human-readable.

| Error code | Meaning |
| --- | --- |
| `INVALID_VALIDATION_REQUEST` | Invalid argument combination. |
| `RUNNER_PROBE_FAILED` | Probe failed or exceeded its bounds. |
| `RUNNER_REPORT_INVALID` | Capability report is malformed, ambiguous or uses an unsupported schema. |
| `RUNNER_CAPABILITY_MISSING` | Required provider capability is absent. |
| `RUNNER_RECEIPT_MISSING` | Managed receipt is absent. |
| `RUNNER_PROVENANCE_MISMATCH` | Receipt is invalid or same-entry package/source/revision/binary identity does not match. |
| `RUNNER_BINARY_SHADOWED` | Selected executable differs from the managed binary. |

See [Codex usage](../howto/install-codex-plugin.md),
[Claude plugin installation](../howto/install-claude-code-plugin.md) and
[runner execution](./rust-runner-execution.md).
