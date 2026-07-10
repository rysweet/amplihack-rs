---
title: Services / Executables
---
# Executables (inventory)

amplihack-rs is a CLI toolchain, not a networked service mesh. It ships 3 binary targets:

| Binary | Crate | Role |
|---|---|---|
| `amplihack` | `bins/amplihack` | Main CLI: install, `recipe run`, orchestration, hooks staging |
| `amplihack-hooks-bin` | `bins/amplihack-hooks` | Hook executable (PreToolUse/PostToolUse/session hooks) |
| `amplihack-asset-resolver-bin` | `bins/amplihack-asset-resolver` | Asset resolver helper |

See [runtime-topology](../runtime-topology/README.md).
