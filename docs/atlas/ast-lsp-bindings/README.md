mode: static-approximation (ripgrep, no live LSP server)

---
title: AST + LSP Symbol Bindings
---
# Layer: ast-lsp-bindings

**Mode: static-approximation** — no live rust-analyzer LSP session was available, so
public-symbol counts are approximated by ripgrep over `pub fn|struct|enum|trait|type|const|mod`.
Approx **4867 public items** across 29 crates.

![ast-lsp-bindings (mermaid)](ast-lsp-bindings-mermaid.svg)

![ast-lsp-bindings (dot)](ast-lsp-bindings-dot.svg)
