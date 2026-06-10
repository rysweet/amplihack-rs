---
title: Framework bundle compatibility reference
description: Compatibility contract for install/update framework bundles, smart-orchestrator recipes, and stale bundle repair.
last_updated: 2026-06-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Framework bundle compatibility reference

`amplihack install` and `amplihack update` validate the framework bundle before
accepting it as an install source and after staging it under `AMPLIHACK_HOME`.
Startup self-heal does not run a separate compatibility scan; when the
version-stamp safety net re-runs install, that install path inherits the same
validation.

The validator protects users from mixed-version installs where the binary is
current but `~/.amplihack/amplifier-bundle/` still contains stale recipes from an
older release. This specifically covers issue
[#734](https://github.com/rysweet/amplihack-rs/issues/734), where a stale
monolithic `smart-orchestrator.yaml` remained installed after v0.11.0 and failed
during `parse-decomposition`.

## Compatibility contract

A framework bundle is compatible only when its `amplifier-bundle/` tree satisfies
the current smart-orchestrator contract.

Required files:

| Path | Required role |
| --- | --- |
| `amplifier-bundle/recipes/smart-orchestrator.yaml` | Composable parent recipe |
| `amplifier-bundle/recipes/smart-classify-route.yaml` | Classification, preflight, decomposition, activation, and session setup |
| `amplifier-bundle/recipes/smart-execute-routing.yaml` | Q&A, operations, development, investigation, parallel, fallback, and adaptive routing |
| `amplifier-bundle/recipes/smart-reflect-loop.yaml` | Goal-seeking reflection loop |
| `amplifier-bundle/recipes/smart-validate-summarize.yaml` | Outside-in validation, summary, and completion |
| `amplifier-bundle/recipes/_recipe_manifest.json` | Recipe manifest containing non-empty hash entries for `smart-orchestrator` and all four companion recipes |

The parent recipe must be the composable smart-orchestrator:

```yaml
name: "smart-orchestrator"
steps:
  - id: "smart-classify-route"
    type: "recipe"
    recipe: "smart-classify-route"
  - id: "smart-execute-routing"
    type: "recipe"
    recipe: "smart-execute-routing"
  - id: "smart-reflect-loop"
    type: "recipe"
    recipe: "smart-reflect-loop"
  - id: "smart-validate-summarize"
    type: "recipe"
    recipe: "smart-validate-summarize"
```

The validator checks structure and required references, not line count. Future
valid changes may add comments, context keys, or metadata without breaking
compatibility, as long as the parent recipe still delegates to the four required
sub-recipes.

The recipe manifest must also contain entries for:

```text
smart-orchestrator
smart-classify-route
smart-execute-routing
smart-reflect-loop
smart-validate-summarize
```

Each entry must have a non-empty hash value. The validator uses this as a
bundle-manifest sanity check; it does not require one exact recipe file hash.

This is also not an exact content-hash pin. The install path still uses
source-derived manifest verification to prove required directories were copied,
but smart-orchestrator compatibility is a structural/content contract. A bundle
may pass file-presence manifest checks and still fail this validator when the
recipe content is stale.

## Rejected stale patterns

The installer rejects any candidate or staged bundle whose
`smart-orchestrator.yaml` matches known incompatible monolithic behavior.

Rejected examples include:

| Pattern | Why it is rejected |
| --- | --- |
| A single large monolithic `smart-orchestrator.yaml` that does not delegate to the four sub-recipes | It bypasses the v0.11.0 composable smart-orchestrator contract. |
| `resolve-bundle-asset helper-path` inside `smart-orchestrator.yaml` as part of old orchestration-helper execution | This belongs to the stale monolithic recipe path and must not remain installed. |
| Python `importlib` orchestration in `smart-orchestrator.yaml` | The recipe is stale and predates the Rust-native/composable workflow split. |
| References to `orch_helper.py` as a current smart-orchestrator dependency | `orch_helper.py` no longer exists and must not be restored. |

These stale-marker checks are scoped to
`amplifier-bundle/recipes/smart-orchestrator.yaml`. Historical documentation,
tests, or bugfix notes may still mention `orch_helper.py`, `importlib`, or old
helper-path behavior as examples of stale failures; those references do not
make the bundle incompatible.

`helper-path` remains a valid named bundle asset, but it resolves to the native
multitask wrapper:

```text
helper-path -> amplifier-bundle/bin/multitask-orchestrator.sh
```

Do not remap `helper-path` to `orch_helper.py`. Do not reintroduce
`orch_helper.py` to make stale recipes pass validation.

## Source selection behavior

Every local framework source candidate is validated before use.

Default source resolution for `amplihack install`:

| Priority | Candidate | Compatibility behavior |
| --- | --- | --- |
| 1 | `AMPLIHACK_HOME` | Explicit user-configured source. Skipped when stale or incompatible. |
| 2 | Current-working-directory walk-up | Finds the checkout the user is installing from. Used only when compatible. |
| 3 | Executable-path walk-up | Finds in-tree development binaries under `target/`. Used only when compatible. |
| 4 | Compile-time workspace root | Fallback for `cargo run`-style invocations. Used only when compatible. |
| 5 | `~/.amplihack` | Prior staged install location. Skipped when stale or incompatible. |
| 6 | Network download | Used when no compatible local bundle is available or `--force-refresh` is active. |

An incompatible local candidate does not win the resolution race merely because
it exists. The installer reports the skipped candidate and continues to the next
source. If no compatible source can be found or downloaded, install fails.

`amplihack update` runs the new binary as:

```sh
amplihack install --force-refresh
```

The hidden `--force-refresh` flag bypasses local source selection and downloads
fresh framework assets. The downloaded source is still validated before staging,
and the staged destination is validated after copy.

## Staging behavior

Framework staging is fail-closed:

1. Validate the selected source bundle.
2. Copy the bundle into the install staging area.
3. Validate the staged `AMPLIHACK_HOME/amplifier-bundle/`.
4. Run source-derived install completeness verification.
5. Persist uninstall manifest and version-success metadata only after the bundle
   and mapped assets are valid.

If a stale monolithic `smart-orchestrator.yaml` exists at the destination before
install, a successful install replaces it with the canonical composable recipe.
The stale file cannot remain staged after a successful install/update repair.

## User-facing diagnostics

When a local candidate is skipped:

```text
⚠️  Skipping incompatible framework bundle at /home/alice/.amplihack:
    recipes/smart-orchestrator.yaml is stale: expected composable sub-recipes
    smart-classify-route, smart-execute-routing, smart-reflect-loop,
    smart-validate-summarize
```

When the staged destination fails validation:

```text
install failed: staged framework bundle is incompatible:
  /home/alice/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
  contains stale monolithic smart-orchestrator behavior
```

When a companion recipe is missing:

```text
install failed: framework bundle is incompatible:
  missing required smart-orchestrator companion recipe:
  amplifier-bundle/recipes/smart-reflect-loop.yaml
```

Diagnostics include the path and the failed contract. They do not dump full
recipe contents or environment variables.

## Security model

Framework bundle candidates are treated as untrusted until validation succeeds,
whether they come from `AMPLIHACK_HOME`, a local checkout, the executable path,
the compile-time workspace root, `~/.amplihack`, `--local`, or a download.

The compatibility validator only reads files. It does not execute YAML, shell,
Python, helper scripts, or bundle assets while deciding whether a source can be
used. Unreadable files, missing companion recipes, and incomplete bundle roots
fail closed with an explicit error instead of falling back to success-shaped
defaults.

## Manifest and checksum policy

Framework compatibility validation complements install completeness verification;
it does not replace it.

| Check | Purpose | Failure mode |
| --- | --- | --- |
| Source-derived completeness manifest | Confirms required source directories, child directories, skills, and staged bundle paths were copied. | Missing or partial files/directories fail install. |
| Smart-orchestrator compatibility validator | Confirms the copied recipes satisfy the current composable contract. | Stale monolithic or incomplete smart-orchestrator assets fail install. |
| Release archive checksum | Confirms downloaded binaries or archives match the expected remote artifact when that download path provides checksums. | Download/update fails before install staging. |

Do not use a hard-coded SHA-256 of `smart-orchestrator.yaml` as the primary
compatibility gate. Exact hashes are too strict for harmless comments or
metadata changes and too weak as the only staged-state check because a matching
parent file does not prove the required companion recipes were staged. Use the
structural contract and companion-recipe presence checks instead.

## Configuration

No new configuration is required.

| Input | Effect |
| --- | --- |
| `AMPLIHACK_HOME` | Selects the install/staging root. A bundle under this root is validated before it can be reused. |
| `--local <PATH>` | Uses a caller-provided source path. The path is still validated and fails if incompatible. |
| `--force-refresh` | Hidden install flag used by post-update install. Bypasses local source candidates, downloads a fresh bundle, then validates source and staged destination. |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | Suppresses startup self-heal. It does not disable explicit install/update compatibility validation. |

## Contributor API

The install compatibility validator lives in
`crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `validate_framework_bundle_compatibility(root: &Path) -> Result<()>`

Validates a candidate framework source before install accepts it.

Use this before returning a path from source discovery or before copying from a
user-provided `--local` path.

The `root` may be either:

| Accepted root | Example |
| --- | --- |
| Repository root containing `amplifier-bundle/` | `/src/amplihack-rs` |
| Bundle root itself | `/src/amplihack-rs/amplifier-bundle` |

The function resolves the bundle root internally and returns an error when the
smart-orchestrator contract is not satisfied.

### `validate_staged_framework_bundle(root: &Path) -> Result<()>`

Validates the staged bundle after copy and atomic replacement.

Use this against the installed Amplihack home, normally:

```text
~/.amplihack/amplifier-bundle/
```

This check is a hard install failure. It prevents copy bugs, stale destination
files, or partial staging from being reported as success.

### `is_compatible_framework_bundle(root: &Path) -> bool`

Boolean helper for source discovery.

Use this when scanning optional local candidates such as `AMPLIHACK_HOME` or an
executable walk-up path. A `false` result means "do not select this candidate";
the caller may continue to the next candidate or fall back to network download.

Do not use this helper for final staging verification. Final verification should
call `validate_staged_framework_bundle()` so the user receives the actionable
error.

### `BundleCompatibilityError`

Structured error used by the validator. Error messages are stable enough for
diagnostics and tests, but callers should branch on variants rather than parsing
human text.

Expected variants:

| Variant | Meaning |
| --- | --- |
| `MissingBundleRoot` | No `amplifier-bundle/` directory was found from the provided root. |
| `MissingSmartOrchestrator` | `recipes/smart-orchestrator.yaml` is absent. |
| `MissingCompanionRecipe` | One of the four required `smart-*` companion recipes is absent. |
| `IncompatibleSmartOrchestrator` | The parent recipe does not delegate to the required companion recipes. |
| `StaleSmartOrchestrator` | Known stale monolithic, Python/importlib, `orch_helper.py`, or old `helper-path` orchestration behavior was detected. |
| `UnreadableRecipe` | A required recipe file exists but cannot be read. |

## Regression coverage

Tests cover:

1. The repository's canonical `amplifier-bundle/recipes/smart-orchestrator.yaml`
   is accepted.
2. A stale monolithic smart-orchestrator using old Python/importlib behavior is
   rejected.
3. A stale smart-orchestrator that references `resolve-bundle-asset helper-path`
   as an orchestration helper is rejected.
4. A bundle missing any required companion recipe is rejected.
5. Source discovery skips a stale `AMPLIHACK_HOME` bundle and continues to a
   compatible source.
6. Install/update repair replaces a staged stale monolithic
   `smart-orchestrator.yaml`; it cannot remain staged after success.
7. `helper-path` continues to resolve to
   `amplifier-bundle/bin/multitask-orchestrator.sh`.

Focused validation:

```sh
cargo test -p amplihack-cli bundle_compat
cargo test -p amplihack-cli install_flow
```

Workspace validation:

```sh
cargo test -p amplihack-cli
cargo check --workspace
```

## See also

- [amplihack install / uninstall command reference](install-command.md)
- [Install completeness verification](install-completeness.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md)
- [resolve-bundle-asset command reference](resolve-bundle-asset-command.md)
