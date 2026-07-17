---
name: code-atlas
version: 2.0.0
description: |
  Builds comprehensive, living code-atlases as multi-layer architecture documents derived from
  code-first truth. Defaults to both Graphviz DOT and Mermaid. User can override to single format.
  Language-agnostic (Go, TypeScript, Python, .NET, Rust, Java). Files issues with
  'code-atlas-bughunt' label. Treats atlas-building as a multi-agent bug-hunting journey:
  graph-form reasoning exposes structural bugs, route/DTO mismatches, orphaned env vars,
  dead code paths, and stale documentation that linear review misses. Three-pass bug hunt
  with per-journey PASS/FAIL/NEEDS_ATTENTION verdicts.
  Use when: creating architecture documentation, investigating unfamiliar codebases,
  hunting structural bugs, setting up CI/CD diagram refresh, or publishing to GitHub Pages/mkdocs.
invokes:
  skills:
    - code-visualizer
    - mermaid-diagram-generator
    - lsp-setup
  agents:
    - visualization-architect
    - analyzer
    - reviewer
---

# Code Atlas Skill

## Purpose

Build exhaustive, regeneratable architecture atlases directly from code truth. A code-atlas
is a living document set: diagrams, graphs, and inventory tables that form a navigable map
of any codebase. Atlas-building is investigation: structured reasoning about code in graph
form reveals structural bugs, API contract mismatches, and architectural drift that linear
code review misses.

An atlas is complete when any engineer, given only the atlas and a bug report, can trace the
full execution path without opening the source code.

## Layer Overview

Layer definitions are the single source of truth in [LAYERS.yaml](./LAYERS.yaml).
All references below use slugs from that file.

| Slug                 | Name                           | Description                                                   | Recommended Diagram Type                 |
| -------------------- | ------------------------------ | ------------------------------------------------------------- | ---------------------------------------- |
| `repo-surface`       | Repository Surface             | All source files, project structure, build systems            | Mermaid `flowchart TD`                   |
| `ast-lsp-bindings`   | AST+LSP Symbol Bindings        | Cross-file symbol references, dead code, interface mismatches | Mermaid `flowchart LR` or DOT digraph    |
| `compile-deps`       | Compile-time Dependencies      | Package imports, dependency trees, circular deps              | DOT digraph (handles large trees better) |
| `runtime-topology`   | Runtime Topology               | Services, containers, ports, inter-service connections        | DOT digraph with subgraph clusters       |
| `api-contracts`      | API Contracts                  | HTTP routes, gRPC, GraphQL, middleware chains                 | Mermaid `flowchart TD`                   |
| `data-flow`          | Data Flow                      | DTO-to-storage chains, transformation steps                   | Mermaid `flowchart LR`                   |
| `service-components` | Service Component Architecture | Per-service internal module/package structure                 | Mermaid `graph TD` (one per service)     |
| `user-journeys`      | User Journey Scenarios         | End-to-end paths from entry to outcome                        | Mermaid `sequenceDiagram`                |

### Per-Layer Scope Guidance

| Slug                 | Scope Target                                                                       |
| -------------------- | ---------------------------------------------------------------------------------- |
| `repo-surface`       | Top-level directories and build entry points. Do not enumerate every file.         |
| `ast-lsp-bindings`   | Exported symbols and their cross-file references. Focus on public API surface.     |
| `compile-deps`       | Direct dependencies and one level of transitive. Include version constraints.      |
| `runtime-topology`   | All deployed services, their ports, and inter-service protocols.                   |
| `api-contracts`      | Every HTTP/gRPC/GraphQL endpoint with auth, DTOs, and middleware.                  |
| `data-flow`          | Primary read/write paths per service. Skip internal caching flows unless relevant. |
| `service-components` | Top-level packages/modules within each service. Show coupling edges.               |
| `user-journeys`      | Derive from api-contracts routes + pages/CLI entries. Trace 3-8 key journeys.      |

## Skill Delegation Architecture

```
code-atlas (this skill)
  Responsibilities:
    - Atlas layer orchestration (all 8 layers)
    - Language-agnostic code exploration
    - Three-pass bug-hunting workflow
    - Staleness detection triggers
    - Density management (split, not prompt)
    - Publication workflow (GitHub Pages, mkdocs, SVG)

  Delegates to:
    compile-deps / service-components analyzer (language-pluggable; mode label always visible):
      code-visualizer skill     Python repos ONLY: Python AST module analysis (optional adapter)
      cargo metadata + rust-analyzer/ripgrep   Rust repos: dependency + module static approximation
      analyzer agent            Any other language: static-approximation dep/module mapping
    mermaid-diagram-generator   Mermaid syntax generation and formatting
    lsp-setup skill             Layer ast-lsp-bindings: LSP-assisted symbol queries (optional)
    visualization-architect     Complex DOT graph rendering and cross-layer layouts
    analyzer agent              Deep codebase investigation and dependency mapping
    reviewer agent              Contradiction hunting (Passes 1, 2, 3)
```

The compile-deps / service-components analyzer is **language-pluggable**: no adapter is
hard-required. The Python `code-visualizer` is used only for Python repositories; Rust repos
use `cargo metadata` + rust-analyzer/ripgrep static approximation; all other languages fall
back to the `analyzer` agent. The chosen analyzer is stated via a visible mode label
(`analyzer: python-ast | rust-cargo-metadata | static-approximation`), mirroring how
ast-lsp-bindings labels its mode. **Python is never required for non-Python repositories.**

## When to Use This Skill

| Trigger                                 | Use Case                                         |
| --------------------------------------- | ------------------------------------------------ |
| Starting work on an unfamiliar codebase | Full atlas build before coding                   |
| Onboarding a new engineer               | Share atlas as navigation guide                  |
| Before a major refactor                 | Map current state; plan changes against topology |
| Bug hunt stalled                        | Pass 1 + Pass 2 bug-hunting through graphs       |
| Docs feel stale                         | Staleness check + targeted rebuild               |
| Adding CI/CD quality gate               | Register atlas freshness checks                  |
| Publishing documentation site           | GitHub Pages / mkdocs publication workflow       |
| Reviewing an unfamiliar PR              | PR impact view using diff against current atlas  |

## Quick Start

```
# Build a full atlas
User: Build a complete code atlas for this repository

# Run bug hunting
User: Run code atlas bug hunting passes on this service

# Check freshness
User: Are our architecture diagrams still accurate?

# Publish
User: Publish our code atlas to GitHub Pages
```

## Why Both Mermaid and Graphviz

The skill defaults to building atlas diagrams in both formats because they find different
bugs. A controlled experiment across 7 repos showed only ~15% overlap in bugs found --
running both finds ~1.7x the bugs of either alone. The different syntax forces different
reasoning paths through the same code. Evidence is documented in PR #3221.

The user can override to a single format:

```
User: Build a code atlas using only Mermaid
User: Build a code atlas in DOT format only
```

## Diagram Density Policy

There are no hard node/edge limits. Instead:

**If a diagram would be unreadably dense, split into sub-diagrams by package or service
boundary.** In batch mode, auto-group without prompting the user. Each sub-diagram should
target 15-40 nodes for readability.

For example, a runtime-topology diagram with 80 services should be split into sub-diagrams
by domain (e.g., `runtime-topology-payments.mmd`, `runtime-topology-auth.mmd`) plus one
high-level overview diagram showing inter-domain connections.

A table is only produced as a companion to a diagram, never as a replacement.

## Recipe: 12-Phase Atlas Build

The atlas build follows these phases in order:

1. **Validate Prerequisites** -- Check tools (mmdc, dot), select the graph backend
   (auto-detect order: `kuzu` -> `lbug` -> `neo4j` -> `portable-cypher-only`), select the
   compile-deps analyzer (`python-ast` | `rust-cargo-metadata` | `static-approximation`),
   detect LSP mode
2. **Build Layers 1-4** (structural) -- repo-surface, ast-lsp-bindings, compile-deps, runtime-topology
3. **Build Layers 5-8** (behavioral) -- api-contracts, data-flow, service-components, user-journeys
4. **Verify All 8 Layers** -- Hard gate: every slug must have .mmd + .dot + rendered .svg + README with embedded images
5. **Bug Hunt (Mermaid arm)** -- 3-pass hunt using only .mmd diagrams
6. **Bug Hunt (Graphviz arm)** -- 3-pass hunt using only .dot diagrams (parallel with step 5)
7. **Merge Findings** -- Deduplicate across both arms
8. **Multi-Agent Validation** -- 3 specialists vote independently; threshold >= 2/3 to confirm
9. **File Issues** -- Validated bugs filed as GitHub issues (never stored in atlas)
10. **Emit Portable Graph + Live Ingest (`ingest-to-graph`)** -- ALWAYS emit the portable
    OpenCypher artifact set (schema + per-layer node/rel data + queries) encoding all 8 layers
    AND their inter-layer link relationships; then load those same artifacts into the selected
    live backend (`kuzu` | `lbug` | `neo4j`), or record `portable-cypher-only` when no engine is
    available. The selected backend is ALWAYS recorded in `index.md` and `staleness-map.yaml`;
    never a silent skip.
11. **Publish Atlas** -- Render SVGs, write index, update mkdocs nav
12. **Final Checklist Review** -- Independent reviewer verifies completeness of all deliverables

After each build phase, diagrams are written to `docs/atlas/{slug}/` with `.mmd` source,
`.dot` source, rendered `*-mermaid.svg` and `*-dot.svg`, and a README.md that embeds the
SVGs inline using `![alt](file.svg)` syntax.

## Bug-Hunting Workflow Overview

The atlas is an active investigation tool. Three passes transform it from a map into a
high-confidence bug-detection engine. Each pass runs in a fresh context window to prevent
anchoring bias.

- **Pass 1 (Comprehensive Build + Hunt)**: Build all layers, then systematically hunt
  contradictions between them. Route/DTO mismatches, orphaned env vars, dead runtime paths,
  stale doc references.
- **Pass 2 (Fresh-Eyes Cross-Check)**: A new context window re-examines the atlas
  independently. Confirms, overturns, or escalates Pass 1 findings.
- **Pass 3 (Scenario Deep-Dive)**: Every user-journeys journey is traced end-to-end through
  api-contracts, data-flow, runtime-topology, service-components, and ast-lsp-bindings.
  Each journey receives a verdict: PASS, FAIL, or NEEDS_ATTENTION.

Full checklists, templates, and output formats: [bug-hunt-guide.md](./bug-hunt-guide.md)

## Layer 8: ast-lsp-bindings Operating Modes

Layer ast-lsp-bindings operates in one of two modes, always labelled on line 1 of its README:

| Mode                   | Trigger                             | Mechanism                       |
| ---------------------- | ----------------------------------- | ------------------------------- |
| `lsp-assisted`         | lsp-setup reports active LSP server | Delegates symbol queries to LSP |
| `static-approximation` | LSP unavailable                     | ripgrep + code-visualizer AST   |

The mode label is never absent, never defaulted silently.

## Graph Representation & Backend Selection

The atlas produces a **queryable graph representation of the code across all 8 layers, with
first-class links between the layers**. This goal is engine-neutral. It is satisfied by two
things working together:

1. **Portable graph artifacts (mandatory, always emitted).** The artifact set under
   `docs/atlas/cypher/` is the required, engine-neutral deliverable. It encodes every layer's
   nodes and relationships PLUS the inter-layer link relationships in portable OpenCypher.
   This is generated on every build regardless of which live database (if any) is present.
2. **Live-DB ingestion (pluggable, selectable backend).** Loading the artifacts into a running
   graph engine is an optional convenience, never a hard dependency. Supported backends:

   | `graph_backend`         | Engine                                                      | Notes                                                              |
   | ----------------------- | ----------------------------------------------------------- | ------------------------------------------------------------------ |
   | `kuzu`                  | Kuzu embedded graph DB (Python/CLI)                         | `CREATE NODE/REL TABLE ... PRIMARY KEY` schema adapter             |
   | `lbug`                  | Embedded lbug/ladybug Rust graph store                      | Native to Simard and other native-Rust/amplihack projects         |
   | `neo4j`                 | Neo4j or any OpenCypher-compatible server                   | Standard OpenCypher schema adapter                                 |
   | `portable-cypher-only`  | None — emit portable artifacts only                         | First-class recorded outcome when no engine is available          |

**Selection.** Pass an explicit `graph_backend` option to force a backend; otherwise the skill
auto-detects in this order: `kuzu` -> `lbug` -> `neo4j` -> `portable-cypher-only`. Absence of
kuzu (or Python) MUST NOT hard-fail the build.

**Fail-visible, never silent.** `portable-cypher-only` is a **visible, recorded outcome**, never
a silent skip. Every build records `graph_backend: <kuzu|lbug|neo4j|portable-cypher-only>` in
both `docs/atlas/index.md` (generation metadata) and `docs/atlas/staleness-map.yaml` (top-level
field). The portable cypher graph is always emitted.

**Native-Rust / Simard guidance.** For Simard and other native-Rust/amplihack projects the graph
backend is `lbug` — the embedded graph store already linked into the Rust binary — loaded from the
same portable cypher artifacts. Under those projects' hard NO-kuzu / NO-Python policy, kuzu and
Python are NOT used; the analyzer runs in `rust-cargo-metadata` or `static-approximation` mode and
the backend is `lbug` (or `portable-cypher-only` if the store is not being populated).

The canonical engine-neutral graph model and the per-backend schema emission adapters (kuzu,
lbug/ladybug, neo4j) are defined in [reference.md](./reference.md). The inter-layer link
relationships are mandatory in every adapter.

### Compile-deps / Service-components Analyzer Modes

Compile-deps and service-components analysis is **language-pluggable** and its mode is always
labelled (mirroring ast-lsp-bindings):

| Mode                   | Trigger                       | Mechanism                                            |
| ---------------------- | ----------------------------- | ---------------------------------------------------- |
| `python-ast`           | Python repo detected          | Delegates to `code-visualizer` (optional adapter)    |
| `rust-cargo-metadata`  | Rust repo detected            | `cargo metadata` + rust-analyzer/ripgrep approximation |
| `static-approximation` | Any other language / fallback | ripgrep + `analyzer` agent static mapping            |

The analyzer mode label is never absent and never defaulted silently. Python is never required
for non-Python repositories.

## Output Structure

```
docs/atlas/
  index.md             (records graph_backend: <kuzu|lbug|neo4j|portable-cypher-only>)
  staleness-map.yaml   (top-level graph_backend: <...> field)
  {slug}/
    README.md          (embeds SVG diagrams inline with ![alt](file.svg))
    *-mermaid.svg      (rendered Mermaid diagrams)
    *-dot.svg          (rendered Graphviz diagrams)
    *.mmd              (Mermaid source)
    *.dot              (Graphviz source)
    inventory.md       (where applicable)
  cypher/              (ALWAYS emitted — the mandatory, engine-neutral graph deliverable)
    schema.cypher      (canonical graph model; backend-specific schema adapter selected)
    atlas-layers.cypher       (per-layer node/rel data for all 8 layers)
    atlas-services.cypher
    atlas-bugs.cypher
    atlas-relationships.cypher (inter-layer link relationships — DEPENDS_ON, CALLS, EXPOSES,
                                USES_DTO, REFERENCES, READS_FROM, WRITES_TO, USES_ENV, TRAVERSES)
    queries.cypher     (ready-to-run example queries)
```

**Three non-negotiable output rules:**

1. Bug hunt results are **never stored in the atlas**. All findings are filed as
   GitHub issues with the `code-atlas-bughunt` label.
2. A graph representation is **required**; the **live backend is pluggable** and the selected
   backend is **always recorded** (`graph_backend: <kuzu|lbug|neo4j|portable-cypher-only>` in
   `index.md` and `staleness-map.yaml`). Absence of kuzu (or Python) never hard-fails the build.
   Never silently drop graph output.
3. The **portable OpenCypher graph is always emitted** under `docs/atlas/cypher/` — schema,
   per-layer node/rel data for all 8 layers, and the inter-layer link relationships — regardless
   of which live backend is selected. This is what satisfies "a graph representation of the code
   across layers with links between them."

## Staleness Detection

Staleness triggers are defined per-layer in [LAYERS.yaml](./LAYERS.yaml) as glob patterns.
When `git diff` matches a trigger pattern, the corresponding layer is marked stale.

Full trigger table, rebuild commands, and incremental rebuild strategy:
[reference.md](./reference.md)

## CI Integration

Three GitHub Actions patterns are available:

1. Post-merge staleness gate with auto-commit
2. PR impact check with layer annotations
3. Scheduled weekly full rebuild with issue creation on failure

Full workflow YAML and configuration: [publication-guide.md](./publication-guide.md)

## Publication

Outputs GitHub Pages-ready `docs/atlas/` structure. Compatible with mkdocs-material and
plain GitHub Pages. SVGs are committed so no render step is needed at read time.

SVG generation commands, mkdocs integration, and deployment workflows:
[publication-guide.md](./publication-guide.md)

## Diagram Examples

Per-layer Mermaid and DOT examples with recommended diagram types:
[examples.md](./examples.md)

## Security Controls

All security controls (SEC-01 through SEC-19) are defined in [SECURITY.md](./SECURITY.md).
Key controls:

- Secret values never emitted (env files parsed for key names only)
- Path traversal prevented via realpath() boundary validation
- Mermaid/DOT/SVG labels sanitized (XSS prevention)
- Bug report code quotes redacted of credential patterns
- All file:line references use relative paths (SEC-16)

## API Contracts

Typed contracts for all skill delegations and filesystem layout:
[API-CONTRACTS.md](./API-CONTRACTS.md)

## Reference

Error codes, canonical engine-neutral graph model + per-backend schema adapters, staleness trigger table:
[reference.md](./reference.md)

## Success Criteria

A complete atlas satisfies:

- All 8 layers produced with diagrams in `docs/atlas/{slug}/`
- Both DOT and Mermaid source files present (unless user requested single format)
- SVG renders alongside source files
- Bug hunt findings filed as GitHub issues (never stored in atlas docs)
- Every filed bug includes: layer reference, file path, line number, code evidence
- No orphaned nodes in diagrams
- ast-lsp-bindings README states mode on line 1

## Limitations

- **Not a static analysis tool**: Uses grep, AST, config parsing -- not a compiler
- **Staleness is heuristic**: Git diff pattern matching, not semantic analysis
- **Language-pluggable analyzer**: compile-deps/service-components analysis uses a per-language
  adapter (`python-ast` via code-visualizer, `rust-cargo-metadata`, or `static-approximation`).
  Python is not required for non-Python repos; the active mode is always labelled.
- **SVG rendering requires Graphviz/Mermaid CLI**: CI environments need these installed
- **Bug hunting is probabilistic**: Human review required before filing
- **Single-repository focus**: Cross-repo deps require manual configuration
- **No runtime instrumentation**: Call frequencies and latency require APM tools

## Remember

> **Diagramming is investigation, not just documentation.**

The most valuable output of a code atlas is the bugs and contradictions discovered while
reasoning about the system in graph form.

Five rules that are never negotiable:

1. **No silent diagram-to-table substitution.** If density is high, split into sub-diagrams.
2. **Mode is always visible.** ast-lsp-bindings README always states its mode on line 1.
3. **Three-pass bug hunting.** Pass 1 hunts. Pass 2 validates. Pass 3 verdicts per journey.
4. **Bugs go to issues, never the atlas.** The atlas is a living architecture doc, not a bug report.
5. **A graph representation is required; the live backend is pluggable.** The portable OpenCypher
   graph (all 8 layers + inter-layer links) is always emitted; the selected live backend
   (`kuzu` | `lbug` | `neo4j` | `portable-cypher-only`) is always recorded. Never silently drop
   graph output; absence of kuzu never hard-fails the build.

**Rebuild from code truth. Hunt contradictions. File evidence-backed bugs. Repeat.**
