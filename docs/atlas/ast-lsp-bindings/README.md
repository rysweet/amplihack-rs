# Layer 2: AST / LSP Bindings

Cross-file symbol references, dead code detection, and interface mismatches.

**Mode**: static-approximation (no LSP server running during atlas build)

## Overview

This layer maps how modules reference each other at the symbol level. Key
cross-file binding paths include the CLI entry point dispatching through
`amplihack-cli`, the hook system deserialising `HookInput` from `amplihack-types`,
and the agent system storing/retrieving from `amplihack-memory`.

| Binding Path | From | To |
|-------------|------|-----|
| CLI dispatch | `bins/amplihack/main.rs` | `cli::commands::dispatch` |
| Hook protocol | `hooks::pre_tool_use` | `types::HookInput` |
| Agent memory | `agent_core::agent` | `memory::backend` |
| State paths | `state::AtomicJsonFile` | `types::ProjectDirs` |

## Diagram (Graphviz)

![AST/LSP Bindings — Graphviz](ast-lsp-bindings-dot.svg)

## Diagram source

- [ast-lsp-bindings.dot](ast-lsp-bindings.dot) (Graphviz DOT)
- [ast-lsp-bindings.mmd](ast-lsp-bindings.mmd) (Mermaid)
