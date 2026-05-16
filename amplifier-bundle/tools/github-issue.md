# GitHub Issue Creation Tool

## Purpose

Programmatic GitHub issue creation with validation and structured output.

## Module Location

Available as the `amplihack github-issue` CLI subcommand.

## Public Interface

### Main Function

```rust
use amplihack::github_issue::create_issue;

let result = create_issue(CreateIssueOptions {
    title: "Bug: Authentication fails".into(),  // Required
    body: Some("Details here".into()),           // Optional
    labels: vec!["bug".into(), "auth".into()],   // Optional
    assignees: vec!["username".into()],           // Optional
    milestone: Some("v1.0".into()),              // Optional
    project: Some("Sprint 1".into()),            // Optional
    repo: Some("owner/repo".into()),             // Optional (uses current if omitted)
})?;
```

### Return Structure

```json
{
    "success": true,
    "issue_url": "https://github.com/...",
    "issue_number": 42,
    "error": null
}
```

## Requirements

- GitHub CLI (`gh`) must be installed
- Must be authenticated (`gh auth login`)
- Repository must exist and user must have write access

## Error Handling

The tool handles:

- Missing GitHub CLI
- Authentication failures
- Invalid inputs
- Network timeouts (30 second limit)
- Malformed responses

## Command Line Usage

```bash
# Simple issue
amplihack github-issue "Title here"

# With options
amplihack github-issue "Bug report" \
    --body "Description" \
    --label bug \
    --label high-priority \
    --assignee username \
    --milestone "v2.0"
```

## Implementation Details

- Wraps `gh issue create` command
- Validates inputs before execution
- Parses output to extract issue URL and number
- Returns structured JSON for programmatic use
- Zero dependencies beyond standard library

## Testing

Run test suite:

```bash
cargo test
```

## Philosophy Compliance

✓ **Ruthless Simplicity**: Single-purpose wrapper around gh CLI
✓ **Zero-BS**: No stubs, every function works
✓ **Self-contained**: No external dependencies
✓ **Clear Contract**: Well-defined inputs and outputs
✓ **Regeneratable**: Can be rebuilt from this specification
