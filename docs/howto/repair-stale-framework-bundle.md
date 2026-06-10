---
title: Repair a stale framework bundle
description: Diagnose and repair an install where the binary is current but smart-orchestrator recipes are stale.
last_updated: 2026-06-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Repair a stale framework bundle

Use this guide when `amplihack --version` shows a current release but
`amplihack recipe run smart-orchestrator` fails with old recipe behavior,
especially Python/importlib errors or `orch_helper.py`-style failures.

Current installs validate and repair this automatically. A successful
`amplihack install` or post-update install replaces stale
`smart-orchestrator.yaml` assets with the canonical composable recipe and its
four companion recipes.

## Symptoms

Common symptoms of a stale bundle:

```text
parse-decomposition failed
```

```text
orch_helper.py not found
```

```text
ModuleNotFoundError: No module named ...
```

```text
resolve-bundle-asset helper-path
```

The last string is only a stale smart-orchestrator symptom when it appears
inside the old monolithic recipe execution path. `helper-path` remains a valid
asset name and correctly resolves to
`amplifier-bundle/bin/multitask-orchestrator.sh`.

## Repair with install

Run install from the current binary:

```sh
amplihack install
```

The installer validates local bundle candidates before using them. If
`AMPLIHACK_HOME` contains a stale `amplifier-bundle/`, install skips it and uses
the next compatible local source or a fresh download.

Expected behavior when a stale local bundle is found:

```text
⚠️  Skipping incompatible framework bundle at /home/alice/.amplihack:
    recipes/smart-orchestrator.yaml is stale
✓ Staged framework assets
✓ Verified staged framework assets
amplihack installed successfully.
```

## Repair after update

Normally, no manual step is required:

```sh
amplihack update
```

After replacing the binary, update runs:

```sh
amplihack install --force-refresh
```

The hidden `--force-refresh` flag downloads fresh framework assets, validates
the source bundle, stages it, then validates the staged destination. If the
post-update install fails, the binary update has still completed; run
`amplihack install` manually to retry the asset repair.

## Repair from a local checkout

Use `--local` when you intentionally want to install from a checkout:

```sh
cd /path/to/amplihack-rs
cargo build --release
./target/release/amplihack install --local .
```

The local checkout must contain a compatible `amplifier-bundle/`. The installer
does not trust `--local` blindly; stale or incomplete smart-orchestrator assets
cause install to fail with an actionable compatibility error.

## Verify the repaired smart-orchestrator

Confirm the staged parent recipe delegates to the four companion recipes:

```sh
grep -E 'recipe: "(smart-classify-route|smart-execute-routing|smart-reflect-loop|smart-validate-summarize)"' \
  ~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
```

Expected output includes all four recipes:

```text
recipe: "smart-classify-route"
recipe: "smart-execute-routing"
recipe: "smart-reflect-loop"
recipe: "smart-validate-summarize"
```

Confirm the companion recipes are present:

```sh
for recipe in smart-classify-route smart-execute-routing smart-reflect-loop smart-validate-summarize; do
  test -f "$HOME/.amplihack/amplifier-bundle/recipes/${recipe}.yaml" \
    && echo "ok: ${recipe}"
done
```

Confirm `helper-path` still resolves to the multitask wrapper:

```sh
amplihack resolve-bundle-asset helper-path
```

Expected suffix:

```text
amplifier-bundle/bin/multitask-orchestrator.sh
```

## Verify by running smart-orchestrator

Run a minimal Q&A task:

```sh
amplihack recipe run smart-orchestrator \
  -c task_description="What is 2+2?" \
  -c repo_path=.
```

The recipe should enter the composable flow:

```text
smart-classify-route
smart-execute-routing
smart-reflect-loop
smart-validate-summarize
```

It should not attempt to import Python orchestration helpers or reference
`orch_helper.py`.

## If install still fails

Capture the compatibility error and the resolved paths:

```sh
amplihack --version
which -a amplihack
echo "AMPLIHACK_HOME=${AMPLIHACK_HOME:-$HOME/.amplihack}"
amplihack install 2>&1 | tee /tmp/amplihack-install.log
```

Do not repair this by copying `orch_helper.py` into the bundle or remapping
`helper-path` to a Python file. Those actions recreate the stale behavior that
the installer is designed to reject.

## See also

- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
- [Install completeness verification](../reference/install-completeness.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [resolve-bundle-asset command reference](../reference/resolve-bundle-asset-command.md)
