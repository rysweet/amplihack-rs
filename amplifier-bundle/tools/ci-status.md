# CI Status Tool

Check GitHub CI/CD status for branches, pull requests, and commits.

## Purpose

Provides a simple, structured way to check CI status without complex bash scripting. Returns consistent JSON output for programmatic use and human-readable summaries for display.

## Usage

### As a Rust Library

```rust
use amplihack::ci_status::check_ci_status;

// Check current branch
let result = check_ci_status(None)?;

// Check specific PR
let result = check_ci_status(Some("123"))?;  // or Some("#123")

// Check specific branch
let result = check_ci_status(Some("main"))?;

// Access structured data
if result.success {
    println!("Status: {}", result.status);
    println!("Total checks: {}", result.summary.total);
}
```

### As a CLI Tool

```bash
# Check current branch
amplihack ci-status

# Check specific PR
amplihack ci-status 123

# Check specific branch
amplihack ci-status main

# Get JSON output
amplihack ci-status --json

# Make executable and use directly
amplihack ci-status
```

## Output Structure

### Successful Response

```json
{
  "success": true,
  "status": "PASSING",  // PASSING, FAILING, PENDING, RUNNING, MIXED, NO_CHECKS, NO_RUNS
  "reference_type": "pr",  // "pr" or "branch"
  "pr_number": 123,  // if checking a PR
  "branch": "feature/xyz",  // branch name
  "checks": [...],  // for PRs: list of check details
  "runs": [...],  // for branches: list of workflow runs
  "summary": {
    "total": 5,
    "passed": 5,  // for PRs
    "failed": 0,
    "pending": 0,  // for PRs
    "in_progress": 0,  // for branches
    "successful": 5,  // for branches
    "completed": 5  // for branches
  }
}
```

### Error Response

```json
{
  "success": false,
  "error": "Error message describing what went wrong"
}
```

## Status Values

- **PASSING**: All checks/runs successful
- **FAILING**: One or more checks/runs failed
- **PENDING**: Checks are queued but not started (PR only)
- **RUNNING**: Workflows are currently running
- **MIXED**: Mixed results (some passed, some other status)
- **NO_CHECKS**: No CI checks configured (PR only)
- **NO_RUNS**: No workflow runs found

## Requirements

- Rust toolchain
- GitHub CLI (`gh`) installed and authenticated
- Git repository with GitHub remote

## Examples

### Check if CI is passing before merge

```rust
use amplihack::ci_status::check_ci_status;

let result = check_ci_status(None)?;
if result.success && result.status == "PASSING" {
    println!("Safe to merge!");
} else {
    println!("CI status: {}", result.status);
}
```

### Monitor CI progress

```rust
use std::{thread, time::Duration};
use amplihack::ci_status::check_ci_status;

loop {
    let result = check_ci_status(Some("123"))?;  // PR number
    println!("Status: {}", result.status);

    if result.status == "PASSING" || result.status == "FAILING" {
        break;
    }

    thread::sleep(Duration::from_secs(30));  // Check every 30 seconds
}
```

### Get failed checks details

```rust
use amplihack::ci_status::check_ci_status;

let result = check_ci_status(None)?;
if result.status == "FAILING" {
    if let Some(checks) = &result.checks {
        let failed: Vec<_> = checks.iter()
            .filter(|c| c.conclusion == "FAILURE")
            .collect();
        for check in failed {
            println!("Failed: {}", check.name);
            println!("  URL: {}", check.details_url.as_deref().unwrap_or("N/A"));
        }
    }
}
```

## Error Handling

The tool handles common errors gracefully:

- Missing `gh` CLI returns clear error message
- Network timeouts (30-second default)
- Invalid PR numbers or branches
- Malformed JSON responses
- Repository without GitHub remote

## Integration with Agents

This tool can be used by agents to:

1. Wait for CI to complete before proceeding
2. Verify tests pass before creating PRs
3. Monitor deployment status
4. Generate CI status reports
5. Trigger actions based on CI results

## Performance Notes

- Commands have a 30-second timeout
- Results are not cached (always fresh)
- Minimal dependencies (Rust standard library)
- Efficient JSON parsing for large check lists
