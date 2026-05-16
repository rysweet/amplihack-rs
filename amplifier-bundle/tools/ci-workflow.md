# CI Workflow Tool

A comprehensive CI workflow management tool that provides higher-level CI/CD functionality.

## Features

- **Parallel CI Diagnostics**: Run multiple checks simultaneously
- **Iterative Fix Application**: Automatically apply and test fixes
- **Status Polling**: Monitor CI status with exponential backoff
- **Local and Remote Checks**: Combines local validation with GitHub CI status

## Installation

The tool is available as the `amplihack ci-workflow` CLI subcommand.

## Command-Line Usage

### Diagnose CI Issues

Run comprehensive diagnostics to identify CI problems:

```bash
# Diagnose current branch
amplihack ci-workflow diagnose

# Diagnose specific PR
amplihack ci-workflow diagnose --pr 123

# Diagnose specific branch
amplihack ci-workflow diagnose --branch feature/new-feature

# Get JSON output
amplihack ci-workflow diagnose --json
```

### Iterate CI Fixes

Automatically attempt to fix CI issues with configurable retry logic:

```bash
# Default 5 attempts
amplihack ci-workflow iterate-fixes --pr 123

# Custom attempt limit
amplihack ci-workflow iterate-fixes --max-attempts 3 --pr 123

# JSON output for scripting
amplihack ci-workflow iterate-fixes --pr 123 --json
```

### Poll CI Status

Monitor CI status with intelligent polling and backoff:

```bash
# Poll current branch (5 minute default timeout)
amplihack ci-workflow poll-status

# Poll specific PR with custom timeout
amplihack ci-workflow poll-status 123 --timeout 600

# Disable exponential backoff
amplihack ci-workflow poll-status --no-backoff --interval 10

# JSON output for automation
amplihack ci-workflow poll-status --json
```

## Rust API Usage

```rust
use amplihack::ci_workflow::{diagnose_ci, iterate_fixes, poll_status};

// Run diagnostics
let result = diagnose_ci(Some(123), None)?;
println!("Overall status: {}", result.overall_status);

// Iterate fixes
let fixes_result = iterate_fixes(3, Some(123))?;
if fixes_result.success {
    println!("CI issues resolved!");
}

// Poll status
let poll_result = poll_status(
    Some("123"),  // PR number or branch
    300,          // 5 minutes
    true,         // exponential_backoff
)?;
println!("Final status: {}", poll_result.final_status);
```

## Diagnostic Components

The `diagnose` command checks:

1. **CI Status**: GitHub Actions/CI status from PR or branch
2. **Lint Check**: Runs available linters (pre-commit, clippy, etc.)
3. **Test Check**: Executes test suites (cargo test, npm test, etc.)
4. **Build Check**: Verifies build process (make, npm, cargo, etc.)

## Fix Iteration Strategy

The `iterate-fixes` command:

1. Runs diagnostics to identify issues
2. Applies automatic fixes based on issue type:
   - **Lint issues**: Runs auto-formatters (cargo fmt, cargo clippy --fix, pre-commit)
   - **Dependency issues**: Updates dependencies (cargo update, npm install)
   - **Test failures**: Logs for manual intervention
3. Commits and pushes fixes
4. Re-runs diagnostics
5. Repeats until success or max attempts reached

## Polling Behavior

The `poll-status` command:

- Starts with specified interval (default 10s)
- Optionally applies exponential backoff (1.5x multiplier)
- Caps maximum interval at 60 seconds
- Exits when CI completes or timeout reached
- Returns appropriate exit codes for scripting

## Exit Codes

- `0`: Success (CI passing or command succeeded)
- `1`: Failure (CI failing or command failed)

## Integration with CI/CD Agents

This tool is designed to work with the CI/CD specialist agent (`~/.amplihack/.claude/agents/amplihack/cicd.md`) to provide automated CI management capabilities.

## Dependencies

- Rust toolchain
- GitHub CLI (`gh`) installed and authenticated
- Git configured with appropriate permissions
- Project-specific tools (linters, test runners, build tools)
