# Developing amplihack

**Type**: How-To (Task-Oriented)

How to set up a local development environment for contributing to amplihack-rs.

## Prerequisites

- Rust toolchain (1.70+) via [rustup.rs](https://rustup.rs/)
- Node.js 18+ and npm (for Claude Code CLI)
- git 2.0+

```bash
# Verify prerequisites
rustc --version && cargo --version && node --version && git --version
```

## Clone and Build

```bash
git clone https://github.com/rysweet/amplihack-rs.git
cd amplihack-rs
cargo build
```

## Run Tests

```bash
cargo test
```

## Project Structure

| Directory            | Purpose                                      |
| -------------------- | -------------------------------------------- |
| `crates/`            | Rust crate workspace                         |
| `crates/amplihack-cli/` | Main CLI binary                           |
| `amplifier-bundle/`  | Framework assets (agents, recipes, commands)  |
| `docs/`              | Documentation (mkdocs site)                  |
| `tests/`             | Integration and end-to-end tests             |

## Key Crates

| Crate                  | Purpose                          |
| ---------------------- | -------------------------------- |
| `amplihack-cli`        | CLI argument parsing and routing |
| `amplihack-core`       | Core types and utilities         |
| `amplihack-agent-core` | Agent runtime and lifecycle      |
| `amplihack-agent-eval` | Evaluation harness               |
| `amplihack-memory`     | Memory backend                   |
| `amplihack-recipes`    | Recipe runner                    |

## Development Workflow

### 1. Create a Feature Branch

```bash
git checkout -b feat/my-feature
```

### 2. Make Changes

Edit Rust code in `crates/`, framework assets in `amplifier-bundle/`,
or documentation in `docs/`.

### 3. Format and Lint

```bash
cargo fmt --all
cargo clippy -- -D warnings
```

### 4. Test

```bash
cargo test
```

### 5. Build Documentation (Optional)

```bash
docker run --rm -v "$PWD:/docs" squidfunk/mkdocs-material build --strict
```

### 6. Commit and Push

```bash
git add <files>
git commit -m "feat: description of change"
git push -u origin feat/my-feature
```

### 7. Create Pull Request

PRs require:

- All CI checks passing (lint, test, build across 4 targets)
- Documentation updates for user-facing changes
- No regressions in existing tests

## Common Tasks

### Adding a CLI Subcommand

1. Create a new module in `crates/amplihack-cli/src/commands/`
2. Register it in `crates/amplihack-cli/src/commands/mod.rs`
3. Add tests in the module's `tests/` subdirectory

### Adding a Recipe

1. Create a YAML file in `amplifier-bundle/recipes/`
2. Define steps with `type: agent`, `type: shell`, or `type: sub_recipe`
3. Test with `amplihack recipe run your-recipe.yaml`

### Adding an Agent

1. Create a markdown file in `amplifier-bundle/agents/`
2. Define the agent's role, tools, and instructions
3. Register in the appropriate directory (core, specialized, or workflows)

## Troubleshooting

### Build Failures

```bash
# Clean build artifacts and retry
cargo clean && cargo build
```

### Test Failures

```bash
# Run a specific test with output
cargo test test_name -- --nocapture
```

### Pre-commit Hook Issues

The project uses pre-commit hooks. If they fail:

```bash
# Run hooks manually to see errors
pre-commit run --all-files
```

## Related

- [Prerequisites](../reference/prerequisites.md) — detailed tool installation
- [Create Your Own Tools](../howto/create-your-own-tools.md) — building custom tools
- [Code Review](../reference/code-review.md) — code review practices
