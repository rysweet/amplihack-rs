# Code Atlas Architecture

The code atlas is an 8-layer architecture document generated from source code analysis. Each layer captures a different aspect of the codebase, from file structure to end-to-end user journeys.

## Why Eight Layers

A single architecture diagram either shows too little (one box per service) or too much (every function call). The atlas solves this by separating concerns into layers that can be read independently or cross-referenced.

The layers progress from static structure to dynamic behavior:

| Layer | Slug | What It Maps |
|-------|------|-------------|
| 1 | `repo-surface` | Source files, project structure, build systems |
| 2 | `ast-lsp-bindings` | Cross-file symbol references, dead code, interface mismatches |
| 3 | `compile-deps` | Package imports, dependency trees, circular dependency detection |
| 4 | `runtime-topology` | Services, containers, ports, connections |
| 5 | `api-contracts` | HTTP routes, CLI commands, gRPC, GraphQL, OpenAPI, DTOs |
| 6 | `data-flow` | DTO-to-storage chains, transformation steps |
| 7 | `service-components` | Per-service internal module/package structure |
| 8 | `user-journeys` | Entry-point to outcome sequence diagrams |

Layers 1-4 are structural (what exists). Layers 5-8 are behavioral (what happens).

## Dual-Format Diagrams

Every layer produces both Mermaid (`.mmd`) and Graphviz DOT (`.dot`) diagrams. This is not redundant — the two formats force different representations of the same architecture:

- **Mermaid** uses abbreviated syntax that emphasizes flow and grouping
- **Graphviz DOT** uses explicit `A -> B [label="..."]` syntax that exposes every edge

An experiment showed the two formats find approximately 85% different bugs when used in the bug-hunt passes. Running both in parallel finds roughly 1.7x the bugs of either alone.

## Density Guard

Diagrams with more than 50 nodes or 100 edges trigger an interactive prompt rather than silently degrading to a table. The three options are: (a) split into subdiagrams, (b) filter by relevance, (c) proceed with the dense diagram. Tables are never substituted for diagrams without explicit user consent.

## Bug Hunt: Three-Pass Dual-Format

When `bug_hunt` is enabled (the default), the recipe runs a structured hunt:

1. **Pass 1 — Comprehensive Hunt**: Read all 8 layers, cross-reference for contradictions (route-DTO mismatches, orphaned env vars, dead runtime paths, stale docs)
2. **Pass 2 — Fresh Eyes**: Re-read from scratch in a new context window, ignoring Pass 1 conclusions. Cross-check each finding as CONFIRMED, OVERTURNED, or NEEDS_ATTENTION
3. **Pass 3 — Scenario Deep-Dive**: Trace each user journey through the structural layers. Produce per-journey PASS/FAIL/NEEDS_ATTENTION verdicts

The Mermaid arm and Graphviz arm run in parallel, reading only their respective format. Findings are then merged and deduplicated by `file:line`.

## Multi-Agent Validation

Merged findings pass through three independent specialist reviewers:

- **Security specialist**: Error handling, input validation, credential exposure
- **Architecture specialist**: Module boundaries, API contracts, dependency direction
- **Testing specialist**: Edge cases, race conditions, resource leaks

Each reviewer votes CONFIRMED (1 point), UNCERTAIN (0.5 points), or FALSE_POSITIVE (0 points). A finding needs a score of 2.0 or higher to be filed as a GitHub issue. Rejected findings are logged but not filed individually — they get a single summary issue with the `code-atlas-needs-review` label.

## Graph Ingestion

Atlas data is ingested into LadybugDB (Kuzu) with three node types (`AtlasLayer`, `AtlasService`, `AtlasBug`) and four relationship types (`ATLAS_MAPS`, `SERVICE_CONTAINS`, `BUG_IN`, `LAYER_FOUND_BUG`). Standalone OpenCypher `.cypher` files are also generated in `docs/atlas/cypher/` for portability to any OpenCypher-compatible graph database.

## CI Lifecycle

The atlas rebuilds on every push to `main` via `.github/workflows/atlas.yml`. The workflow installs diagram rendering tools, runs the recipe, and uploads `docs/atlas/` as a GitHub Actions artifact. The initial committed atlas provides a baseline; CI produces fresh snapshots without committing back to the repository.

Bugs are never stored in the atlas documentation — they exist only as GitHub issues. The atlas is a living architecture reference, not a bug tracker.
