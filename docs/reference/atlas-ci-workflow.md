# Atlas CI Workflow

Reference for `.github/workflows/atlas.yml` — the GitHub Actions workflow that rebuilds the code atlas on every push to `main`.

## Trigger

```yaml
on:
  push:
    branches: [main]
```

Runs on every push to the `main` branch. Does not run on pull requests or tags.

## Permissions

```yaml
permissions:
  contents: read
  actions: write
```

Read-only repository access. No write-back to the repository — the workflow produces artifacts, not commits. The `actions: write` permission is required by `actions/upload-artifact@v4` to create workflow artifacts.

## Job: `atlas`

| Setting | Value |
|---------|-------|
| Runner | `ubuntu-latest` |
| Timeout | 30 minutes |
| Node memory | `NODE_OPTIONS=--max-old-space-size=32768` |

### Steps

| # | Step | Details |
|---|------|---------|
| 1 | Checkout | `actions/checkout@v4` |
| 2 | Rust toolchain | `dtolnay/rust-toolchain@stable` |
| 3 | Rust cache | `Swatinem/rust-cache@v2` |
| 4 | Install amplihack | `cargo install --path bins/amplihack --locked` |
| 5 | Node.js setup | `actions/setup-node@v4` (node 22, enables npm cache) |
| 6 | Install diagram tools | graphviz (apt) and mermaid-cli (npm) installed in parallel |
| 7 | Run atlas recipe | `amplihack recipe run amplifier-bundle/recipes/code-atlas.yaml` with `continue-on-error: true` |
| 8 | Upload artifact | `actions/upload-artifact@v4` — uploads `docs/atlas/` as artifact named `code-atlas` |

### Recipe Step Behavior

The recipe step runs with `continue-on-error: true`. This means:

- Partial output is still uploaded as an artifact
- Agent steps that require an LLM backend may fail in CI without API keys
- The workflow succeeds even if the recipe produces incomplete output
- Diagram rendering works if graphviz and mermaid-cli installed successfully

### Artifact

| Property | Value |
|----------|-------|
| Name | `code-atlas` |
| Path | `docs/atlas/` |
| Retention | 90 days (GitHub default) |
| Upload condition | `if: always()` — uploads even if recipe step fails |

Download artifacts from the Actions tab in GitHub or via CLI:

```sh
gh run download --name code-atlas
```

## Parameters

The workflow passes no custom context to the recipe, using defaults:

| Parameter | Default |
|-----------|---------|
| `codebase_path` | `.` (repository root) |
| `output_dir` | `docs/atlas` |
| `layers` | `[1, 2, 3, 4, 5, 6, 7, 8]` (all) |
| `bug_hunt` | `true` |
| `publish` | `false` |

## Environment Variables

| Variable | Value | Purpose |
|----------|-------|---------|
| `CARGO_TERM_COLOR` | `always` | Colored Cargo output in logs |
| `NODE_OPTIONS` | `--max-old-space-size=32768` | Prevent OOM during mermaid rendering |

## Relationship to Other Workflows

The atlas workflow is independent of `ci.yml`. It does not block releases or PRs. It shares the same Rust toolchain and cache patterns (`dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`) as the CI workflow for cache efficiency.
