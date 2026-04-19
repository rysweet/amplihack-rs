# Run the Code Atlas Recipe

Generate a multi-layer architecture atlas of your codebase with dual-format diagrams and automated bug hunting.

## Prerequisites

- `amplihack` CLI installed (`amplihack --version`)
- `graphviz` for DOT rendering (`dot -V`)
- `mermaid-cli` for Mermaid rendering (`mmdc --version`)

Optional tools are detected automatically. Missing renderers skip SVG generation but still produce source files.

## Run the Full Atlas

From the repository root:

```sh
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml
```

This builds all 8 layers, runs the 3-pass dual-format bug hunt, validates findings with three specialist agents, files confirmed bugs as GitHub issues, ingests results into LadybugDB, and publishes the atlas to `docs/atlas/`.

> **Note:** The bug hunt, issue filing, and multi-agent validation steps require an LLM backend. Set `ANTHROPIC_API_KEY` (or your configured backend credentials) before running, or use `"bug_hunt": false` to skip agent-dependent steps.

Output lands in `docs/atlas/` by default.

## Run Specific Layers Only

Build only the layers you need:

```sh
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml \
  --context '{"layers": [3, 4, 7, 8], "bug_hunt": false}'
```

This builds compile-deps, runtime-topology, service-components, and user-journeys without running the bug hunt.

## Target a Subdirectory

Scope the atlas to a single service:

```sh
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml \
  --context '{"codebase_path": "crates/amplihack-memory", "output_dir": "docs/atlas-memory"}'
```

## Change Output Location

```sh
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml \
  --context '{"output_dir": "tmp/atlas-draft"}'
```

## Skip Bug Hunting

Generate architecture diagrams without the 3-pass bug hunt:

```sh
amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml \
  --context '{"bug_hunt": false}'
```

## Inspect the Recipe Before Running

Preview the recipe steps:

```sh
amplihack recipe show amplifier-bundle/recipes/code-atlas.yaml
```

Dry-run without executing:

```sh
amplihack recipe validate amplifier-bundle/recipes/code-atlas.yaml
```

## Output Structure

After a successful run, `docs/atlas/` contains:

```
docs/atlas/
  index.md                          # Links to all 8 layers
  staleness-map.yaml                # Glob-to-layer mappings for CI
  repo-surface/
    repo-surface.mmd                # Mermaid source
    repo-surface.dot                # Graphviz DOT source
    repo-surface-mermaid.svg        # Rendered Mermaid diagram
    repo-surface-dot.svg            # Rendered Graphviz diagram
    README.md                       # Layer description with embedded SVGs
  ast-lsp-bindings/
    ...
  compile-deps/
    ...
  runtime-topology/
    ...
  api-contracts/
    ...
  data-flow/
    ...
  service-components/
    ...
  user-journeys/
    ...
  cypher/
    schema.cypher                   # Graph schema definitions
    atlas-layers.cypher             # Layer node data
    atlas-services.cypher           # Service node data
    atlas-bugs.cypher               # Bug node data
    atlas-relationships.cypher      # Relationship data
    queries.cypher                  # Example queries
  bug-reports/
    merged-findings.md              # Combined findings from both arms
    validated-bugs.md               # Bugs confirmed by multi-agent review
    rejected-bugs.md                # Findings that failed validation
    filed-issues.md                 # Links to created GitHub issues
    mermaid-arm/                    # Raw Mermaid-based findings
    graphviz-arm/                   # Raw Graphviz-based findings
    validation/                     # Per-specialist verdicts
```

## CI Integration

The atlas rebuilds automatically on every push to `main` via `.github/workflows/atlas.yml`. The workflow uploads the `docs/atlas/` tree as a `code-atlas` artifact. See [Atlas CI Workflow](../reference/atlas-ci-workflow.md) for configuration details.

## Troubleshooting

**Recipe hangs:** The bug-hunt steps use agent calls that require an LLM backend. If no backend is configured, the recipe may stall. Run with `"bug_hunt": false` to skip agent steps.

**Missing SVGs:** Install `graphviz` and `@mermaid-js/mermaid-cli`. The recipe produces `.mmd` and `.dot` source files regardless, but skips rendering without these tools.

**LadybugDB errors:** The graph ingestion step requires the `kuzu` Python package. Install with `pip install kuzu` if missing. The recipe fails loudly rather than skipping this step.
