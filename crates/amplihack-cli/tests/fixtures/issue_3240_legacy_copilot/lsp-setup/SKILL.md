---
name: lsp-setup
description: Auto-discovers and configures Language Server Protocol (LSP) servers for project languages
type: skill
activationStrategy: lazy-aggressive
activationKeywords:
  - LSP
  - Language Server Protocol
  - LSP setup
  - LSP configuration
  - configure LSP
  - enable LSP
  - lsp-setup
  - language server
  - code intelligence
  - rust-analyzer
  - gopls
activationContextWindow: 3
persistenceThreshold: 20
---

# LSP Setup Skill

## Purpose

Help users enable or troubleshoot language-server-backed code intelligence. This skill uses installed language servers, editor/agent integration, repository conventions, and a bundled native status helper.

## Workflow

1. Detect project languages from manifests and source files.
2. Check whether a suitable language server is already available on `PATH`.
3. Prefer existing project setup instructions and package-manager scripts.
4. For missing servers, recommend the ecosystem-standard install command for the detected language.
5. Verify by running the language server's status/version command or by exercising available code-intelligence tools.

## Native Helper

```bash
amplifier-bundle/skills/lsp-setup/scripts/lsp-setup.sh status
amplifier-bundle/skills/lsp-setup/scripts/lsp-setup.sh recommend
```

The helper detects common project languages, checks language-server availability on `PATH`, and prints ecosystem-standard install hints.

## Common Servers

| Language | Server |
| --- | --- |
| Rust | `rust-analyzer` |
| Go | `gopls` |
| TypeScript/JavaScript | TypeScript language server |
| C#/.NET | Roslyn / C# Dev Kit server |
| Java | Eclipse JDT LS |

Python projects may use Pyright or basedpyright when the repository already uses that ecosystem; this is language support, not an amplihack runtime dependency.

## Failure Handling

If LSP setup is unavailable, state that explicitly and fall back to static analysis with `rg`, build metadata, and existing tests. Do not claim LSP-assisted results when only static analysis was used.
