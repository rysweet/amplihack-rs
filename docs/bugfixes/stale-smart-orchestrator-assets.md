---
title: Stale smart-orchestrator installed assets
description: Root cause and durable fix for stale orch_helper.py smart-orchestrator regressions.
last_updated: 2026-07-10
review_schedule: as-needed
owner: amplihack-maintainers
doc_type: bugfix
---

# Stale smart-orchestrator installed assets

## Root cause

The `orch_helper.py` regression came from stale installed assets shadowing fresh
assets. The current `amplihack-rs` source bundle is clean, but an installed
top-level bundle can remain stale at:

```text
~/.amplihack/amplifier-bundle/recipes/smart-orchestrator.yaml
```

The stale v1.5.1 recipe references legacy markers such as `orch_helper.py`,
`importlib`, `parse-decomposition`, and
`resolve-bundle-asset helper-path`. Before the fix, startup self-heal skipped
asset refresh whenever `.installed-version` matched the binary, and recipe
resolution could select `AMPLIHACK_HOME/amplifier-bundle/recipes` before a
fresh compatible `.claude/recipes` fallback.

## Fix

Current builds centralize smart-orchestrator compatibility checks and apply
them in three places:

1. Install/update validates source and staged `amplifier-bundle` assets and
   atomically replaces the top-level installed bundle.
2. Startup self-heal requires both a matching `.installed-version` and a
   compatible `~/.amplihack/amplifier-bundle`; stale installed assets trigger
   the normal install repair path even when the stamp matches.
3. Recipe resolution skips stale smart-orchestrator candidates when a compatible
   fallback exists, and fails loudly with repair guidance when every candidate is
   stale.

## Validation

After install or update, the resolved smart-orchestrator recipe must not contain
any legacy marker:

```sh
AMPGREP='orch_helper\.py|importlib|parse-decomposition|resolve-bundle-asset helper-path'
amplihack recipe show smart-orchestrator | grep -E "$AMPGREP" && exit 1 || echo "ok: no stale markers"
```

The canonical parent recipe should delegate to these companion recipes:

```text
smart-classify-route
smart-execute-routing
smart-reflect-loop
smart-validate-summarize
```

See also: [Repair a stale framework bundle](../howto/repair-stale-framework-bundle.md).
