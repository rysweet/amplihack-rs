---
title: Repair a stale framework bundle
description: Use install/update to atomically replace stale amplifier-bundle assets from the current Rust distribution.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: howto
---

# Repair a stale framework bundle

Use this guide when `amplihack --version` shows the current Rust binary but
`amplihack recipe run smart-orchestrator` behaves like an older install.
Common stale-bundle symptoms include Python/importlib errors,
`orch_helper.py` failures, or old monolithic smart-orchestrator execution.

Current install/update repairs this by replacing
`~/.amplihack/amplifier-bundle` from the current Rust distribution. The
replacement is atomic and fail-closed: install does not merge over old files and
does not report success while stale active recipe paths remain.

## Repair with install

Run:

```bash
amplihack install --force-refresh
```

`--force-refresh` is normally used by `amplihack update` after the new binary is
installed. Running it directly is useful when you know the binary is already
current and only the installed bundle needs repair.

Expected behavior:

```text
Refreshed amplifier-bundle from current distribution
Verified smart-orchestrator compatibility
Verified no active orch_helper.py dependency
```

## Repair after update

Normally, no manual action is required:

```bash
amplihack update
```

Update replaces the binary, then spawns the new Rust binary as:

```bash
amplihack install --force-refresh
```

The post-update install performs the same repair as a direct install: binary
precedence, shadowing stale wrapper quarantine, managed PATH persistence,
staged bundle activation, compatibility validation, and final Rust-first
verification.

## Verify smart-orchestrator assets

Confirm the staged parent recipe delegates to the companion recipes:

```bash
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

```bash
for recipe in smart-classify-route smart-execute-routing smart-reflect-loop smart-validate-summarize; do
  test -f "$HOME/.amplihack/amplifier-bundle/recipes/${recipe}.yaml" \
    && echo "ok: ${recipe}"
done
```

Confirm active recipes do not depend on `orch_helper.py`:

```bash
grep -R "orch_helper.py" ~/.amplihack/amplifier-bundle/recipes || true
```

Expected output: no matches in active recipes. Mentions in tests,
documentation, or compatibility rejection logic are acceptable; executable
recipe paths are not.

## Verify by running smart-orchestrator

Run a minimal task:

```bash
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

It should not attempt to import Python orchestration helpers or execute
`orch_helper.py`.

## If install still fails

Capture the paths and the compatibility error:

```bash
amplihack --version
which -a amplihack
echo "AMPLIHACK_HOME=${AMPLIHACK_HOME:-$HOME/.amplihack}"
amplihack install --force-refresh 2>&1 | tee ./amplihack-install.log
```

Do not repair this by copying `orch_helper.py` into the bundle or remapping
`helper-path` to a Python file. Those actions recreate stale behavior that the
installer rejects.

## See also

- [Framework bundle compatibility reference](../reference/framework-bundle-compatibility.md)
- [Install/update PATH conflict reference](../reference/install-update-path-conflicts.md)
- [Post-update install re-exec](../features/update-reexec-new-binary.md)
- [Install completeness verification](../reference/install-completeness.md)
