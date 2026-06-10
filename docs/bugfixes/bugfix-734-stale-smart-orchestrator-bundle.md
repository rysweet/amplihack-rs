# Bug Fix #734 — Stale smart-orchestrator bundle after v0.11.0

> **Issue:** [#734](https://github.com/rysweet/amplihack-rs/issues/734)

---

## Summary

After updating to v0.11.0, some installs kept an old
`~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml`. The installed
binary was current, but the staged framework bundle still contained the old
monolithic smart-orchestrator recipe. That recipe tried to run stale
Python/importlib orchestration behavior during `parse-decomposition`, so
`smart-orchestrator` failed even though the source tree and release tag carried
the correct composable recipe.

The fix makes install/update validate framework bundle compatibility before
accepting a source and after staging the destination. Stale monolithic
smart-orchestrator assets are rejected and repaired instead of being silently
reused.

## Root cause

The v0.11.0 source bundle contains the correct composable
`smart-orchestrator.yaml`, which delegates to four sub-recipes:

1. `smart-classify-route`
2. `smart-execute-routing`
3. `smart-reflect-loop`
4. `smart-validate-summarize`

The failing user install had a stale `AMPLIHACK_HOME` bundle where
`recipes/smart-orchestrator.yaml` was an older monolithic file. Because local
bundle discovery checks local candidates before downloading fresh assets, the
stale local bundle could be selected again and re-staged when no compatibility
gate rejected it.

This was not a missing `helper-path` or missing `orch_helper.py` problem.
`helper-path` intentionally resolves to:

```text
amplifier-bundle/bin/multitask-orchestrator.sh
```

`orch_helper.py` no longer exists and is not restored by this fix.

## Fix

Install now has a framework bundle compatibility validator that checks the
smart-orchestrator contract before a bundle can be used or considered staged
successfully.

The validator requires:

| Asset | Requirement |
| --- | --- |
| `recipes/smart-orchestrator.yaml` | Exists and is the composable parent recipe |
| `recipes/smart-classify-route.yaml` | Exists |
| `recipes/smart-execute-routing.yaml` | Exists |
| `recipes/smart-reflect-loop.yaml` | Exists |
| `recipes/smart-validate-summarize.yaml` | Exists |
| Parent recipe steps | Reference all four companion recipes with recipe steps |
| `recipes/_recipe_manifest.json` | Contains non-empty hash entries for `smart-orchestrator` and all four companion recipes |

The check is structural. It does not use line count as a validator and does not
pin the parent recipe to one exact content hash. Source-derived completeness
verification still proves files and directories were copied; the compatibility
validator proves the staged recipe set is semantically current.

The validator rejects:

| Stale behavior | Result |
| --- | --- |
| Monolithic smart-orchestrator without the four companion recipe references | Candidate skipped or install fails |
| Python/importlib orchestration inside `smart-orchestrator.yaml` | Candidate skipped or install fails |
| `orch_helper.py` as a current dependency | Candidate skipped or install fails |
| Old `resolve-bundle-asset helper-path` orchestration-helper flow inside the monolithic recipe | Candidate skipped or install fails |

These stale-marker checks apply to the current
`recipes/smart-orchestrator.yaml` behavior. Historical documentation, tests, and
bugfix notes may still mention `orch_helper.py`, `importlib`, or old
`helper-path` usage when describing past failures.

The validator treats every source as untrusted. It only reads files and never
executes YAML, shell, Python, helper scripts, or bundle assets while deciding
whether a bundle can be accepted. Missing, unreadable, or incomplete assets fail
closed.

## Install and update behavior

`amplihack install` validates every local source candidate before accepting it.
An incompatible `AMPLIHACK_HOME` bundle is skipped, not reused:

```text
⚠️  Skipping incompatible framework bundle at /home/alice/.amplihack:
    recipes/smart-orchestrator.yaml is stale
```

After copying assets, install validates the staged destination. A successful
install means the stale monolithic `smart-orchestrator.yaml` did not remain in
`~/.amplihack/amplifier-bundle/recipes/`.

`amplihack update` continues to spawn the new binary for post-update install and
uses the hidden `--force-refresh` flag. The downloaded bundle is still validated
before staging and after staging, so update cannot report success while leaving
an incompatible smart-orchestrator installed.

## User repair

Run:

```sh
amplihack update
```

or:

```sh
amplihack install
```

Then verify the staged smart-orchestrator:

```sh
grep -E 'recipe: "(smart-classify-route|smart-execute-routing|smart-reflect-loop|smart-validate-summarize)"' \
  ~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
```

All four companion recipes should appear.

## Contributor API

The validator is implemented in:

```text
crates/amplihack-cli/src/commands/install/bundle_compat.rs
```

Internal install-module functions:

| Function | Purpose |
| --- | --- |
| `validate_framework_bundle_compatibility(root: &Path) -> Result<()>` | Validate a candidate source bundle before source discovery or copy accepts it. |
| `validate_staged_framework_bundle(root: &Path) -> Result<()>` | Hard-fail install when the staged destination is missing or incompatible. |
| `is_compatible_framework_bundle(root: &Path) -> bool` | Boolean helper for skipping optional local candidates during source discovery. |

See the
[framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
for the full contract.

## Regression coverage

Tests prove:

1. The canonical repository bundle is accepted.
2. A stale monolithic smart-orchestrator is rejected.
3. A stale smart-orchestrator using Python/importlib behavior is rejected.
4. A stale smart-orchestrator using the old `resolve-bundle-asset helper-path`
   orchestration-helper path is rejected.
5. Missing companion recipes are rejected.
6. Source discovery skips stale `AMPLIHACK_HOME` bundles.
7. A stale monolithic smart-orchestrator cannot remain staged after
   install/update repair.
8. `helper-path` still resolves to
   `amplifier-bundle/bin/multitask-orchestrator.sh`.

Focused checks:

```sh
cargo test -p amplihack-cli bundle_compat
cargo test -p amplihack-cli install_flow
```

Broader checks:

```sh
cargo test -p amplihack-cli
cargo check --workspace
```

## Related

- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
- [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md)
- [Bug fix #675 — update does not refresh amplifier-bundle](bugfix-675-update-stale-bundle.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
