---
title: Framework bundle compatibility reference
description: Compatibility contract for staged amplifier-bundle activation, smart-orchestrator recipes, and active orch_helper.py rejection.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: reference
---

# Framework bundle compatibility reference

`amplihack install` and the post-update repair path refresh
`~/.amplihack/amplifier-bundle` from the current Rust distribution and validate
the result before reporting success.

The validator prevents mixed-version installs where the Rust binary is current
but installed recipes are stale. In particular, active smart-orchestrator paths
must not reference or require `orch_helper.py`.

## Compatibility contract

A compatible installed bundle contains the current composable
smart-orchestrator recipe and all required companion recipes.

| Path | Required role |
| --- | --- |
| `amplifier-bundle/recipes/smart-orchestrator.yaml` | Composable parent recipe. |
| `amplifier-bundle/recipes/smart-classify-route.yaml` | Classification, preflight, decomposition, activation, and session setup. |
| `amplifier-bundle/recipes/smart-execute-routing.yaml` | Q&A, operations, development, investigation, parallel, fallback, and adaptive routing. |
| `amplifier-bundle/recipes/smart-reflect-loop.yaml` | Goal-seeking reflection loop. |
| `amplifier-bundle/recipes/smart-validate-summarize.yaml` | Outside-in validation, summary, and completion. |
| `amplifier-bundle/recipes/_recipe_manifest.json` | Manifest entries for the parent and companion recipes. |

The parent recipe delegates to the companion recipes:

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

The validator checks structure and active execution references. It does not pin
one exact YAML hash, so harmless comments or metadata changes do not break
compatibility.

## Active `orch_helper.py` rejection

No active smart-orchestrator execution path may depend on `orch_helper.py`.

Rejected active patterns include:

| Pattern | Why it is rejected |
| --- | --- |
| `orch_helper.py` in `smart-orchestrator.yaml` command, script, env, arg, helper, or asset-resolution fields | The active recipe would require a removed Python helper. |
| Python `importlib` orchestration in `smart-orchestrator.yaml` | The recipe predates the Rust/composable orchestration path. |
| `resolve-bundle-asset helper-path` used by the stale monolithic smart-orchestrator helper flow | It reintroduces old helper dispatch behavior. |
| A monolithic smart-orchestrator that does not delegate to the companion recipes | It bypasses the composable contract. |

Allowed references:

| Location | Allowed use |
| --- | --- |
| Tests | Regression fixtures proving stale recipes are rejected. |
| Documentation | Historical explanation or troubleshooting. |
| Compatibility rejection logic | Marker strings used to identify stale active recipes. |

The named asset `helper-path` remains valid outside stale smart-orchestrator
helper dispatch. It resolves to:

```text
amplifier-bundle/bin/multitask-orchestrator.sh
```

Do not restore `orch_helper.py` or remap `helper-path` to a Python file.

## Staged bundle activation

Install/update replaces the installed bundle; it does not merge over the
existing directory.

Refresh order:

1. Resolve the current Rust distribution's authoritative `amplifier-bundle`.
2. Validate the source bundle.
3. Copy the source bundle into a temporary staging directory under the install
   root.
4. Validate the staged bundle, including smart-orchestrator compatibility.
5. Activate the staged bundle with a same-install-root swap/rename strategy so
   the selected installed bundle changes as one final activation step.
6. Re-run post-activation compatibility checks against
   `~/.amplihack/amplifier-bundle`.
7. Verify install completeness and write install metadata only after the
   replacement is valid.

The atomic guarantee is scoped to the final activation step after staging and
validation. The installer must not delete the active bundle and then copy files
into place, and must not merge new files over old files. If the platform cannot
perform the activation without a merge/delete window, install/update fails
closed instead of reporting success.

If validation fails before activation, the previous installed bundle remains
selected. If staging or post-activation validation fails, install/update exits
non-zero and does not write success metadata.

Because activation is whole-directory, stale files that existed only in the old
installed bundle cannot survive a successful refresh.

## Source selection

`amplihack install` uses compatible assets from the current Rust distribution.
For a development checkout, that is the checkout's `amplifier-bundle/`. For a
packaged release, that is the bundle shipped with the release distribution.

Source behavior:

| Invocation | Bundle source behavior |
| --- | --- |
| `amplihack install` | Uses compatible current-distribution assets and validates before staging. |
| `amplihack install --local <PATH>` | Uses the explicit local source only if it passes compatibility validation. |
| `amplihack install --force-refresh` | Bypasses the installed bundle as a source and refreshes from the current Rust distribution. |
| `amplihack update` | Installs the new Rust binary, then runs that binary's `install --force-refresh` repair path. |

The installed `~/.amplihack/amplifier-bundle` is never trusted as an
authoritative source for `--force-refresh`.

## User-facing diagnostics

When a stale active recipe is rejected:

```text
install failed: framework bundle is incompatible:
  amplifier-bundle/recipes/smart-orchestrator.yaml
  contains active orch_helper.py dependency
```

When a companion recipe is missing:

```text
install failed: framework bundle is incompatible:
  missing required smart-orchestrator companion recipe:
  amplifier-bundle/recipes/smart-reflect-loop.yaml
```

When an installed stale bundle is replaced:

```text
Refreshed amplifier-bundle from current distribution
Verified smart-orchestrator compatibility
Verified no active orch_helper.py dependency
```

Diagnostics include paths and failed contracts. They do not dump full recipe
contents or environment variables.

## Configuration

No new user configuration is required.

| Input | Effect |
| --- | --- |
| `AMPLIHACK_HOME` | Selects the install root. Defaults to `~/.amplihack`. |
| `--local <PATH>` | Uses a caller-provided source path after compatibility validation. |
| `--force-refresh` | Hidden install flag used by update repair and direct stale-bundle repair; bypasses the installed bundle as a source. |
| `AMPLIHACK_SKIP_AUTO_INSTALL` | Suppresses startup self-heal. It does not disable explicit install/update compatibility validation. |

## Contributor API

The install compatibility validator lives in
`crates/amplihack-cli/src/commands/install/bundle_compat.rs`.

### `validate_framework_bundle_compatibility(root: &Path) -> Result<()>`

Validates a candidate framework source before install accepts it.

The `root` may be either:

| Accepted root | Example |
| --- | --- |
| Repository root containing `amplifier-bundle/` | `/src/amplihack-rs` |
| Bundle root itself | `/src/amplihack-rs/amplifier-bundle` |

The function resolves the bundle root internally and returns an error when the
smart-orchestrator contract is not satisfied.

### `validate_staged_framework_bundle(root: &Path) -> Result<()>`

Validates the staged installed bundle after copy and before final replacement.
Use this against:

```text
~/.amplihack/amplifier-bundle/
```

This check is a hard install failure. It prevents copy bugs, stale destination
files, or partial staging from being reported as success.

### `replace_installed_bundle_atomically(source: &Path, install_root: &Path) -> Result<()>`

Stages a validated source bundle in a temporary directory, validates the staged
copy, and atomically activates it as `install_root/amplifier-bundle`.

Callers must not copy files directly into the installed bundle or merge source
files over the destination.

### `reject_active_orch_helper_dependency(bundle_root: &Path) -> Result<()>`

Scans active recipe/runtime dispatch paths for stale `orch_helper.py`
dependencies. The scan is scoped to executable recipe behavior and compatibility
rules; it does not reject documentation, tests, or rejection fixtures that
mention the string.

### `is_compatible_framework_bundle(root: &Path) -> bool`

Boolean helper for optional source discovery. A `false` result means "do not
select this candidate"; the caller may continue to another source. Final
staging verification should call `validate_staged_framework_bundle()` so the
user receives an actionable error.

### `BundleCompatibilityError`

Structured error used by the validator. Callers should branch on variants
rather than parsing human text.

| Variant | Meaning |
| --- | --- |
| `MissingBundleRoot` | No `amplifier-bundle/` directory was found from the provided root. |
| `MissingSmartOrchestrator` | `recipes/smart-orchestrator.yaml` is absent. |
| `MissingCompanionRecipe` | One of the four required companion recipes is absent. |
| `IncompatibleSmartOrchestrator` | The parent recipe does not delegate to the required companion recipes. |
| `ActiveOrchHelperDependency` | An executable smart-orchestrator path references or requires `orch_helper.py`. |
| `StaleSmartOrchestrator` | Known stale monolithic, Python/importlib, or old helper-path orchestration behavior was detected. |
| `UnreadableRecipe` | A required recipe file exists but cannot be read. |

## Regression coverage

Tests cover:

1. The repository's canonical `smart-orchestrator.yaml` is accepted.
2. A stale monolithic smart-orchestrator is rejected.
3. Active `orch_helper.py` dependencies are rejected.
4. Documentation, tests, and compatibility rejection fixtures may mention
   `orch_helper.py`.
5. A bundle missing any companion recipe is rejected.
6. `--force-refresh` replaces the installed bundle instead of merging over it.
7. Stale installed smart-orchestrator files cannot remain after successful
   install/update repair.
8. `helper-path` continues to resolve to
   `amplifier-bundle/bin/multitask-orchestrator.sh`.

Focused validation:

```bash
cargo test -p amplihack-cli bundle_compat
cargo test -p amplihack-cli active_recipe_guard
cargo test -p amplihack-cli install_flow
```

## See also

- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md)
- [Install/update PATH conflict reference](install-update-path-conflicts.md)
- [Install completeness verification](install-completeness.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [resolve-bundle-asset command reference](resolve-bundle-asset-command.md)
