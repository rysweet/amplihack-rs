# Code Atlas

Generated architecture documentation for the amplihack codebase.

## Layers

The code atlas produces 8 documentation layers:

| Layer | Name | Description |
|-------|------|-------------|
| 1 | repo-surface | Source files, project structure, build systems |
| 2 | ast-lsp-bindings | Cross-file refs, dead code, interface mismatches |
| 3 | compile-deps | Packages, modules, circular dependency detection |
| 4 | runtime-topology | Services, containers, ports, connections |
| 5 | api-contracts | HTTP routes, gRPC, GraphQL, OpenAPI, DTOs |
| 6 | data-flow | DTO-to-storage chain, transformation steps |
| 7 | service-components | Per-service module/package diagrams |
| 8 | user-journeys | Entry-point to outcome sequence diagrams |

## Regeneration

Run the code-atlas recipe to regenerate this directory:

```bash
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml
```

See [How to Run Code Atlas](../howto/run-code-atlas.md) for detailed instructions.

## CI

The atlas is rebuilt on every push to `main` via the
[atlas workflow](../../.github/workflows/atlas.yml) and uploaded as a
`code-atlas` artifact.
