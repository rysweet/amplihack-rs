---
title: Recipe runner validation
description: Runner capability schema, provider rules and managed source provenance.
type: reference
updated: 2026-10-18
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

Rust installation, launcher freshness checks and Claude shell bootstrap use
the same immutable `claude-plugin/recipe-runner.rev`, with Cargo `--rev` and
`--locked`. Updating that file changes both managed paths; Rust consumers must
be rebuilt because they embed the revision at compile time. User-owned override
compatibility remains independent of this managed source identity.

| Consumer | Revision source |
| --- | --- |
| Rust installer and launcher freshness | Shared file embedded at compile time; changing the pin requires rebuilding the consumer. |
| Claude plugin `install-runtime` | Shared file read from the plugin checkout at runtime. |
| Compatible user-owned override | No bundled-SHA requirement; provider-specific compatibility policy applies. |

A single Cargo receipt entry must identify the `recipe-runner-rs` package, expected
git repository, full bundled revision and `recipe-runner-rs` binary. Matching a
revision from one entry and a binary from another is invalid. The selected
executable must be the actual managed binary in that effective Cargo home;
a compatible binary shadowing it elsewhere fails validation.

Missing, malformed or mismatched receipts and unsuccessful Cargo installation
fail visibly. Freshness reinstalls a different managed revision and validates the
result before recording success. Shell bootstrap propagates failures before
writing success stamps. Failed builds or links preserve the prior usable managed
binary and its Cargo receipts; retry must complete delivery and validation before
stamping success. The managed revision is never selected from upstream `main`
or inferred from a capability report. If an older user-owned runtime lacks native validation,
bootstrap builds a private Cargo validator without replacing the user's runtime.
Receipts describe local installation provenance, not cryptographic attestation.

## Installer downgrade guard

`scripts/install-pinned-recipe-runner.sh`, `claude-plugin/bin/install-runtime`,
`amplihack install` and launcher freshness do not replace an installed runner
with an older revision. Each reads the installed revision from the Cargo
receipt (`.crates2.json`), never from `recipe-runner-rs --version` or
`recipe_runner.json`, and accepts it only when exactly one entry has the source
`git+https://github.com/rysweet/amplihack-recipe-runner` followed by `?` or `#`,
the `recipe-runner-rs` package and binary, and a 40-character lowercase SHA.
Ancestry comes from Cargo's git database and, failing that, from a fetch of the
two SHAs (into `~/.amplihack/state/recipe-runner.git` for Rust, a temporary
directory for the scripts). The installed runner is classified against the pin:

| Class | Installed revision | Without force |
| --- | --- | --- |
| `Unknown` | Not found in the receipt | Install the pin |
| `Equal` | Same as the pin | Keep; validate the existing binary (`install-runtime` instead reruns `cargo install`, which makes no changes at the same revision) |
| `Older` | Ancestor of the pin | Install the pin |
| `Newer` | Descendant of the pin | Keep and warn |
| `Diverged` | Neither ancestor nor descendant | Keep and warn |
| `Unverifiable` | Order could not be established | Keep and warn |

### Skip warnings

Each skip produces one warning naming both SHAs and the force variable. The
wording depends on the class:

| Class | Message |
| --- | --- |
| `Newer` | `kept recipe-runner-rs <installed> (newer than pinned <pin>); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to downgrade` |
| `Diverged` | `kept recipe-runner-rs <installed> (diverged from pinned <pin>); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to replace it` |
| `Unverifiable` | `kept recipe-runner-rs <installed> (could not verify order against pinned <pin>); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to replace it` |

Where the message appears depends on the installer. `amplihack install` prints
it to the terminal as an indented warning line; for a `Newer` runner:

```text
   ⚠️  kept recipe-runner-rs 77a0c1e2b3d4 (newer than pinned 4d5187eaf958); set AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1 to downgrade
```

`scripts/install-pinned-recipe-runner.sh` and
`claude-plugin/bin/install-runtime` write the same message text to standard
error. Launcher freshness does not print a terminal line; it emits the message
through `tracing::warn!`, so it appears only where launcher warnings are
logged.

### Skip record, force and overrides

The kept binary is still validated, and a further warning is given if it fails
the capability probe or provenance; it is kept regardless. The launcher records
the skip in `recipe_runner.json` with the real installed SHA, so later launches
do no git work while the pin and receipt are unchanged; `Diverged` and
`Unverifiable` are retried after the 24-hour freshness cooldown. Setting
[`AMPLIHACK_FORCE_RECIPE_RUNNER_DOWNGRADE=1`](environment-variables.md#amplihack_force_recipe_runner_downgrade),
or passing `--force` to the shell installer, installs the pin anyway and logs
"downgrade forced". A `RECIPE_RUNNER_RS_PATH` override is never replaced.

[`RECIPE_RUNNER_ANCESTRY_REPO`](environment-variables.md#recipe_runner_ancestry_repo)
replaces both ancestry sources in the two shell installers. It exists for the
test suite only; a value starting with `-`, or anything other than a local path
or an `https://` or `file://` URL, is rejected, and the result is
`Unverifiable`.

The guard is a control against accidental regression, not a supply-chain
defence: integrity still rests on the pinned SHA, HTTPS and
`cargo install --locked --rev`. Pin changes on `main` are checked separately by
the [recipe runner pin ancestry check](ci-pipeline.md#recipe-runner-pin-ancestry).

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
{"error":{"code":"RUNNER_CAPABILITY_MISSING","message":"recipe-runner-rs must advertise codex_exec for Codex; fix or unset the override","details":{"provider":"codex"}}}
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
