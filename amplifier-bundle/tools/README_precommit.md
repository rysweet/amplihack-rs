# Pre-commit Workflow

Use the repository's native format, lint, and test commands to manage
pre-commit failures.

## Features

- **Analyze failures**: Identify which hooks are failing and categorize them
- **Auto-fix**: Automatically fix formatting issues with supported tools
- **Environment verification**: Check pre-commit setup and dependencies
- **Success verification**: Run all pre-commit checks and report status

## Usage

### Command Line

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test --workspace --locked
scripts/check-no-python-assets.sh
scripts/check-recipes-no-python.sh
```

## Supported Auto-fix Tools

- **JavaScript/TypeScript**: prettier, eslint
- **Rust**: rustfmt

## Example Workflow

1. When pre-commit fails, run `analyze` to understand issues
2. Run `auto-fix` to fix formatting issues automatically
3. Run `verify-success` to confirm all checks pass
4. If issues persist, check `verify-env` to ensure tools are installed
