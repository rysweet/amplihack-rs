# Platform Bridge Test Suite

Comprehensive TDD test suite for the platform bridge module following the testing pyramid principle (60% unit, 30% integration, 10% E2E).

## Test Files

### 1. `common/mod.rs` - Shared Test Fixtures

**Purpose**: Provides common test data, mocks, and utilities for all test modules.

**Key Fixtures**:

- Platform URLs (GitHub, Azure DevOps, unknown platforms)
- Success/error response examples
- Mock subprocess results
- Security test data (injection attempts, oversized inputs)
- Git remote output examples
- CLI argument examples
- Azure DevOps configuration examples
- Expected command structures

**Usage**: All fixtures are automatically available to all test files via Rust's test module system.

### 2. `detector_test.rs` - Platform Detection Tests

**Coverage**: 100+ test cases

**Test Categories**:

- Platform enum values (GitHub, AzDO, Unknown)
- PlatformDetector initialization
- GitHub URL detection (HTTPS, SSH, git://, shorthand)
- Azure DevOps URL detection (dev.azure.com, visualstudio.com, SSH)
- Unknown platform detection (GitLab, Bitbucket, custom servers)
- Error handling (missing repo, no remotes, timeout, git not found)
- Multiple remotes (origin priority, upstream fallback, first remote)
- Remote URL extraction
- Caching behavior
- Subprocess call format validation

**Key Test Patterns**:

```rust
#[test]
fn test_detect_github_https_url() {
    // Arrange: set up mock command runner
    let mock_output = CommandOutput {
        status: 0,
        stdout: git_remote_output_github().into(),
        stderr: String::new(),
    };
    let runner = MockCommandRunner::new(mock_output);

    // Act
    let detector = PlatformDetector::new(Box::new(runner));
    let platform = detector.detect();

    // Assert
    assert_eq!(platform, Platform::GitHub);
}
```

### 3. `github_bridge_test.rs` - GitHub Bridge Tests

**Coverage**: 70+ test cases

**Test Categories**:

- Bridge initialization
- `create_issue` operation (success, failure, command construction, labels support)
- `create_draft_pr` operation (draft flag, base branch support)
- `mark_pr_ready` operation (nonexistent PRs, already ready PRs)
- `add_pr_comment` operation (multiline comments, invalid PRs)
- `check_ci_status` operation (passing, failing, in-progress, no checks)
- Response format consistency
- JSON parsing (valid, invalid, empty responses)
- Timeout configuration
- gh CLI not installed handling

**Key Test Patterns**:

```rust
#[test]
fn test_create_issue_success() {
    // Arrange: mock gh CLI returning a created issue
    let mock_output = CommandOutput {
        status: 0,
        stdout: r#"{"number": 123, "url": "https://github.com/owner/repo/issues/123"}"#.into(),
        stderr: String::new(),
    };
    let runner = MockCommandRunner::new(mock_output);

    // Act
    let bridge = GitHubBridge::new(Box::new(runner));
    let result = bridge.create_issue("Test Issue", "Issue description").unwrap();

    // Assert
    assert!(result.success);
    assert_eq!(result.issue_number, 123);
}
```

### 4. `azdo_bridge_test.rs` - Azure DevOps Bridge Tests

**Coverage**: 75+ test cases

**Test Categories**:

- Bridge initialization with config
- Environment variable loading
- `create_issue` operation (work item terminology, org/project config)
- `create_draft_pr` operation (repository config, target branch)
- `mark_pr_ready` operation (draft flag update)
- `add_pr_comment` operation (multiline support)
- `check_ci_status` operation (build status parsing, no builds)
- Configuration management (validation, URL formatting, overrides)
- Response format consistency
- JSON parsing
- az CLI not installed handling

**Key Test Patterns**:

```rust
#[test]
fn test_create_issue_uses_config_org_and_project() {
    // Arrange: mock az CLI with org/project config
    let mock_output = CommandOutput {
        status: 0,
        stdout: r#"{"id": 789}"#.into(),
        stderr: String::new(),
    };
    let runner = MockCommandRunner::new(mock_output);
    let config = azdo_config_complete();

    // Act
    let bridge = AzureDevOpsBridge::new(Box::new(runner), config);
    bridge.create_issue("Test", "Test").unwrap();

    // Assert
    let args = runner.last_call_args();
    let org_index = args.iter().position(|a| a == "--org").unwrap();
    assert!(args[org_index + 1].contains("myorg"));
}
```

### 5. `cli_test.rs` - CLI Interface Tests

**Coverage**: 85+ test cases

**Test Categories**:

- CLI initialization
- `create-issue` command (argument parsing, JSON output, labels, missing args)
- `create-pr` command (draft default, base branch, JSON output)
- `mark-pr-ready` command (PR number parsing, invalid numbers)
- `add-pr-comment` command (stdin reading, multiline comments)
- `check-ci-status` command (ref parsing, check details output)
- Platform auto-detection integration
- Platform override with --platform flag
- Error handling (bridge errors, invalid commands, exceptions)
- Help text generation (usage, command-specific help, command list)
- main() entry point
- JSON output consistency (valid JSON, pretty-printing)

**Key Test Patterns**:

```rust
#[test]
fn test_create_issue_parses_arguments() {
    // Arrange: set up mock bridge
    let mock_bridge = MockGitHubBridge::new();
    mock_bridge.on_create_issue(BridgeResult {
        success: true,
        issue_number: Some(123),
        ..Default::default()
    });

    // Act
    let cli = Cli::new(Platform::GitHub, Box::new(mock_bridge));
    let args = vec!["create-issue", "--title", "Test Issue", "--body", "Issue description"];
    let result = cli.run(&args);

    // Assert
    assert_eq!(result, 0);
    assert_eq!(mock_bridge.create_issue_calls(), 1);
}
```

### 6. `security_test.rs` - Security Tests

**Coverage**: 65+ test cases

**Test Categories**:

- Command injection prevention (PR titles, branch names, comments, issue bodies)
- Input validation (PR numbers, empty inputs, type checking)
- Length limits (titles, bodies, comments, branch names)
- Special character handling (Unicode, newlines, null bytes, quotes)
- Error message sanitization (token leakage, password leakage, path leakage)
- Subprocess argument escaping (list args, shell=False, command chaining prevention)
- Branch name validation (special chars, path traversal)
- Azure DevOps security parity (injection prevention, input validation)
- CLI security integration (PR number validation, dangerous platform names, stdin sanitization)
- Timeout configuration (DoS prevention, reasonable limits)

**Key Test Patterns**:

```rust
#[test]
fn test_pr_title_injection_prevented() {
    // Arrange: use malicious PR title
    let mock_output = CommandOutput {
        status: 0,
        stdout: r#"{"number": 123}"#.into(),
        stderr: String::new(),
    };
    let runner = MockCommandRunner::new(mock_output);
    let malicious_title = malicious_pr_title();

    // Act
    let bridge = GitHubBridge::new(Box::new(runner));
    let _result = bridge.create_draft_pr(&malicious_title, "Normal body", "feature/test");

    // Assert: command was called with list args, not shell interpolation
    let args = runner.last_call_args();
    assert!(args.len() > 1, "should use arg list, not shell string");
    assert!(!runner.used_shell(), "must not use shell=true");
}
```

## Running Tests

The legacy platform-bridge test implementation has been retired with the final
Python winddown. Use the native workspace checks for amplihack-rs:

```bash
cargo test --workspace --locked
scripts/check-no-python-assets.sh
scripts/probe-no-python.sh
```

## Test Philosophy

### TDD Approach

All tests are written **before** implementation. Tests will fail until corresponding implementation is complete.

### Testing Pyramid

- **60% Unit tests**: Fast, isolated, heavily mocked
- **30% Integration tests**: Multiple components working together
- **10% E2E tests**: Complete workflows

### Arrange-Act-Assert Pattern

All tests follow AAA pattern:

```rust
#[test]
fn test_example() {
    // Arrange - Set up test data and mocks
    let mock_output = CommandOutput {
        status: 0,
        stdout: r#"{"number": 123}"#.into(),
        stderr: String::new(),
    };
    let runner = MockCommandRunner::new(mock_output);

    // Act - Execute the functionality
    let bridge = GitHubBridge::new(Box::new(runner));
    let result = bridge.create_issue("Test", "Test").unwrap();

    // Assert - Verify the outcome
    assert!(result.success);
}
```

### Clear Test Names

Test names describe behavior being tested:

- ✅ `test_create_issue_success` - Clear what's being tested
- ✅ `test_pr_title_injection_prevented` - Describes security behavior
- ❌ `test_case_1` - Unclear purpose

### Mock External Dependencies

All external dependencies are mocked:

- `Command` execution - Mock CLI tool calls via trait objects
- Platform detection - Mock git commands
- File system operations - Use `tempdir` crate fixtures

## Expected Test Failures (TDD)

Until implementation is complete, expect these compile errors:

```rust
// Modules not implemented yet
use crate::detector::{PlatformDetector, Platform};  // unresolved import
use crate::github_bridge::GitHubBridge;              // unresolved import
use crate::azdo_bridge::AzureDevOpsBridge;           // unresolved import
use crate::cli::{Cli, main};                         // unresolved import
```

After implementation, expect these test failures:

1. **detector.rs**: Missing Platform enum, PlatformDetector struct, detection logic
2. **github_bridge.rs**: Missing GitHubBridge struct, 5 operation methods
3. **azdo_bridge.rs**: Missing AzureDevOpsBridge struct, config loading, 5 operations
4. **cli.rs**: Missing Cli struct, argument parsing, command routing
5. **Security**: Missing input validation, sanitization, escaping logic

## Test Coverage Goals

- **Line coverage**: 90%+
- **Branch coverage**: 85%+
- **Function coverage**: 95%+

## Security Test Importance

Security tests are **critical** and must pass before any production use:

- Command injection prevention (highest priority)
- Input validation (required)
- Error message sanitization (required)
- Subprocess escaping (required)

## Contributing

When adding new functionality:

1. Write tests first (TDD)
2. Ensure tests fail without implementation
3. Implement functionality
4. Verify tests pass
5. Check coverage remains >90%

## Test Data

All test data is centralized in `common/mod.rs`:

- Realistic URLs for both platforms
- Example API responses
- Security attack vectors
- Configuration examples

This ensures consistency across all test files and makes it easy to update test data.
