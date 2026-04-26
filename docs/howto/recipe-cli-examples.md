# Recipe CLI Examples

Real-world examples showing how to use the `amplihack recipe` CLI commands in
different development scenarios.

## Contents

- [Development Scenarios](#development-scenarios)
- [Testing Scenarios](#testing-scenarios)
- [CI/CD Integration](#cicd-integration)
- [Custom Recipe Development](#custom-recipe-development)

## Development Scenarios

### Implement a New Feature

**Scenario**: Add JWT authentication to an API using the full 22-step workflow.

```bash
amplihack recipe run default-workflow \
  --context '{
    "task_description": "Add JWT authentication to REST API with token refresh",
    "repo_path": "/home/user/api-server",
    "branch_name": "feat/jwt-auth"
  }'
```

**What happens**:

1. Creates branch `feat/jwt-auth`
2. Clarifies requirements (prompt-writer agent)
3. Designs architecture (architect agent)
4. Writes failing tests (tester agent)
5. Implements code (builder agent)
6. Reviews code (reviewer agent)
7. Runs tests and CI
8. Creates pull request
9. Merges to main (after CI passes)

**Expected duration**: 15–25 minutes

### Quick Bug Fix

**Scenario**: Fix a null pointer exception in production code.

```bash
amplihack recipe run quick-fix \
  --context '{
    "task_description": "Fix NullPointerException in UserProfileHandler.getAvatar() when user has no avatar set",
    "repo_path": ".",
    "branch_name": "fix/avatar-null-check"
  }'
```

**What happens**:

1. Analyzes the bug location and root cause
2. Generates and applies fix
3. Runs existing tests
4. Commits changes with descriptive message

**Expected duration**: 2–5 minutes

### Investigate Unknown Codebase

**Scenario**: Understand how authentication works in a legacy system.

```bash
amplihack recipe run investigation \
  --context '{
    "task_description": "How does the authentication and session management system work?",
    "repo_path": "/home/user/legacy-app",
    "focus_area": "src/auth/"
  }'
```

**What happens**:

1. Maps code structure in `src/auth/`
2. Traces data flow through authentication
3. Identifies session storage mechanism
4. Documents findings in markdown
5. Generates architecture diagram

**Output files**:

- `docs/INVESTIGATION_auth-system-<timestamp>.md`
- `docs/diagrams/auth-flow.mermaid`

**Expected duration**: 10–15 minutes

### Refactor Legacy Code

**Scenario**: Refactor a monolithic function into smaller, testable units.

```bash
amplihack recipe run refactor \
  --context '{
    "task_description": "Refactor process_order() function — split into smaller functions with single responsibilities",
    "repo_path": ".",
    "target_file": "src/service/order_service.rs",
    "branch_name": "refactor/order-service"
  }'
```

**What happens**:

1. Analyzes current code structure
2. Proposes refactoring plan (shows before/after)
3. Writes tests for existing behavior
4. Applies refactoring incrementally
5. Verifies tests still pass after each step
6. Reviews for regressions

**Expected duration**: 8–12 minutes

## Testing Scenarios

### Run Only Verification Steps

**Scenario**: Code is already implemented, just need to verify tests and CI.

```bash
amplihack recipe run verification-workflow \
  --context '{"repo_path": "."}'
```

**What happens**:

1. Runs test suite
2. Checks code coverage
3. Runs linting/formatting
4. Builds project
5. Generates verification report

**Expected duration**: 3–6 minutes

### Validate Recipe Before Running

**Scenario**: Created a custom recipe, want to verify it is correct.

```bash
# First validate the YAML
amplihack recipe validate ~/.amplihack/.claude/recipes/my-api-workflow.yaml
```

**Expected output**:

```
Validating my-api-workflow.yaml...
  [OK] Valid YAML syntax
  [OK] Required fields present (name, steps)
  [OK] All step IDs unique
  [OK] Template variables resolve against context
  [OK] Agent references valid (api-designer, builder, tester)
  [OK] No circular dependencies

Recipe "my-api-workflow" is valid (7 steps)
```

**Then dry-run**:

```bash
amplihack recipe run my-api-workflow --dry-run \
  --context '{"endpoint": "/api/users", "repo_path": "."}'
```

**Expected output**:

```
[DRY RUN] my-api-workflow (7 steps)
[Step 1/7] design-api (agent: api-designer)
  Would run agent: amplihack:api-designer
[Step 2/7] generate-openapi (type: bash)
  Would execute: generate_openapi
...
No changes made (dry run mode)
```

## CI/CD Integration

### GitHub Actions Workflow

```yaml
name: Recipe Verification
on: [push, pull_request]

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install amplihack
        run: cargo install amplihack
      - name: Run verification
        run: |
          amplihack recipe run verification-workflow \
            --context repo_path="." \
            --context branch_name="${{ github.ref_name }}"
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```

### Exit Code Handling

```bash
amplihack recipe run verification-workflow \
  --context repo_path="." \
  --context branch_name="main"

case $? in
  0) echo "✓ All checks passed" ;;
  1) echo "✗ Validation failed" ;;
  2) echo "✗ Missing context" ;;
  3) echo "✗ Step failed" ;;
  *) echo "✗ Unknown error" ;;
esac
```

### Save Execution Log

```bash
amplihack recipe run default-workflow \
  --context task_description="Deploy to staging" \
  --output execution-log.json
```

## Custom Recipe Development

### Create a Custom Recipe

```yaml
# ~/.amplihack/.claude/recipes/my-workflow.yaml
name: my-workflow
description: Custom workflow for my team
steps:
  - id: analyze
    type: agent
    agent: analyzer
    prompt: "Analyze {{focus_area}} for issues"
  - id: fix
    type: agent
    agent: builder
    prompt: "Fix issues found in analysis"
    depends_on: [analyze]
  - id: test
    type: bash
    command: "cargo test"
    depends_on: [fix]
```

### Test the Custom Recipe

```bash
# Validate
amplihack recipe validate my-workflow.yaml

# Dry run
amplihack recipe run my-workflow --dry-run --context focus_area="src/"

# Real run
amplihack recipe run my-workflow --context focus_area="src/"
```

## Related Documentation

- [Recipe Quick Reference](../reference/recipe-quick-reference.md) — CLI cheat sheet
- [Recipe Execution Flow](../concepts/recipe-execution-flow.md) — execution internals
- [Recipe Resilience](../concepts/recipe-resilience.md) — error handling
- [Recipe Discovery Troubleshooting](recipe-discovery-troubleshooting.md) — fixing discovery
