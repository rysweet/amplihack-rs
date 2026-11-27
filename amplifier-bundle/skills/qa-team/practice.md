# qa-team: practice

## Framework Integration [LEVEL 2]

### Running Tests

**Single test**:

```bash
gadugi-test run test-scenario.yaml
```

**Multiple tests**:

```bash
gadugi-test run tests/*.yaml
```

**With options**:

```bash
gadugi-test run test.yaml \
  --verbose \
  --evidence-dir ./test-evidence \
  --retry 2 \
  --timeout 60s
```

### CI/CD Integration

**GitHub Actions** (`.github/workflows/agentic-tests.yml`):

```yaml
name: Agentic Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3

      - name: Install gadugi-agentic-test
        run: npm install -g @gadugi/agentic-test

      - name: Run tests
        run: gadugi-test run tests/agentic/*.yaml

      - name: Upload evidence
        if: always()
        uses: actions/upload-artifact@v3
        with:
          name: test-evidence
          path: ./evidence/
```

### Evidence Collection

The framework automatically collects evidence for debugging:

```
evidence/
  scenario-name-20250116-093045/
    ├── scenario.yaml          # Original test scenario
    ├── execution-log.json     # Detailed execution log
    ├── screenshots/           # All captured screenshots
    │   ├── step-1.png
    │   ├── step-3.png
    │   └── step-5.png
    ├── output-captures/       # CLI/TUI output
    │   ├── stdout.txt
    │   └── stderr.txt
    ├── timing.json            # Performance metrics
    └── report.html            # Human-readable report
```

## Best Practices [LEVEL 2]

### 1. Start Simple, Add Complexity

Begin with basic smoke tests, then add detail:

```yaml
# Level 1: Basic smoke test
steps:
  - action: launch
    target: "./app"
  - action: verify_output
    contains: "Ready"

# Level 2: Add interaction
steps:
  - action: launch
    target: "./app"
  - action: send_input
    value: "command\n"
  - action: verify_output
    contains: "Success"

# Level 3: Add error handling and edge cases
steps:
  - action: launch
    target: "./app"
  - action: send_input
    value: "invalid\n"
  - action: verify_output
    contains: "Error"
  - action: send_input
    value: "command\n"
  - action: verify_output
    contains: "Success"
```

### 2. Use Descriptive Names and Descriptions

```yaml
# Bad
scenario:
  name: "Test 1"
  steps:
    - action: click
      selector: "button"

# Good
scenario:
  name: "User Login Flow - Valid Credentials"
  description: "Verifies user can log in with valid email and password"
  steps:
    - action: click
      selector: "button[type=submit]"
      description: "Submit login form"
```

### 3. Verify Critical Paths Only

Don't test every tiny detail. Focus on user-facing behavior:

```yaml
# Bad - Tests implementation details
- action: verify_element
  selector: ".internal-cache-status"
  contains: "initialized"

# Good - Tests user-visible behavior
- action: verify_element
  selector: ".welcome-message"
  contains: "Welcome back"
```

### 4. Use Prerequisites for Test Dependencies

```yaml
scenario:
  name: "User Profile Edit"

  prerequisites:
    - "User testuser@example.com exists"
    - "User is logged in"
    - "Database is seeded with test data"

  steps:
    # Test assumes prerequisites are met
    - action: navigate
      url: "/profile"
```

### 5. Keep Tests Independent

Each test should set up its own state and clean up:

```yaml
scenario:
  name: "Create Document"

  steps:
    # Create test user (don't assume exists)
    - action: api_call
      endpoint: "/api/users"
      method: POST
      data: { email: "test@example.com" }

    # Run test
    - action: navigate
      url: "/documents/new"
    # ... test steps ...

  cleanup:
    # Remove test user
    - action: api_call
      endpoint: "/api/users/test@example.com"
      method: DELETE
```

### 6. Use Tags for Organization

```yaml
scenario:
  name: "Critical Payment Flow"
  tags: [smoke, critical, payment, e2e]
  # Run with: gadugi-test run --tags critical
```

### 7. Add Timeouts Strategically

```yaml
steps:
  # Quick operations - short timeout
  - action: click
    selector: "button"
    timeout: 2s

  # Network operations - longer timeout
  - action: wait_for_element
    selector: ".data-loaded"
    timeout: 10s

  # Complex operations - generous timeout
  - action: verify_element
    selector: ".report-generated"
    timeout: 60s
```

## Testing Strategies [LEVEL 2]

### Smoke Tests

Minimal tests that verify critical functionality works:

```yaml
scenario:
  name: "Smoke Test - Application Starts"
  tags: [smoke]

  steps:
    - action: launch
      target: "./app"
    - action: verify_output
      contains: "Ready"
      timeout: 5s
```

Run before every commit: `gadugi-test run --tags smoke`

### Happy Path Tests

Test the ideal user journey:

```yaml
scenario:
  name: "Happy Path - User Registration"

  steps:
    - action: navigate
      url: "/register"
    - action: type
      selector: "#email"
      value: "newuser@example.com"
    - action: type
      selector: "#password"
      value: "SecurePass123!"
    - action: click
      selector: "button[type=submit]"
    - action: wait_for_url
      contains: "/welcome"
```

### Error Path Tests

Verify error handling:

```yaml
scenario:
  name: "Error Path - Invalid Login"

  steps:
    - action: navigate
      url: "/login"
    - action: type
      selector: "#email"
      value: "invalid@example.com"
    - action: type
      selector: "#password"
      value: "wrongpassword"
    - action: click
      selector: "button[type=submit]"
    - action: verify_element
      selector: ".error-message"
      contains: "Invalid credentials"
```

### Regression Tests

Prevent bugs from reappearing:

```yaml
scenario:
  name: "Regression - Issue #123 Password Reset"
  tags: [regression, bug-123]
  description: "Verifies password reset email is sent (was broken in v1.2)"

  steps:
    - action: navigate
      url: "/forgot-password"
    - action: type
      selector: "#email"
      value: "user@example.com"
    - action: click
      selector: "button[type=submit]"
    - action: verify_element
      selector: ".success-message"
      contains: "Reset email sent"
```

## Philosophy Alignment [LEVEL 2]

This skill follows amplihack's core principles:

### Ruthless Simplicity

- **YAML over code**: Declarative tests are simpler than programmatic tests
- **No implementation details**: Tests describe WHAT, not HOW
- **Minimal boilerplate**: Each test is focused and concise

### Modular Design (Bricks & Studs)

- **Self-contained scenarios**: Each YAML file is independent
- **Clear contracts**: Steps have well-defined inputs/outputs
- **Composable actions**: Reuse actions across different test types

### Zero-BS Implementation

- **No stubs**: Every example in this skill is a complete, runnable test
- **Working defaults**: Tests run with minimal configuration
- **Clear errors**: Framework provides actionable error messages

### Outside-In Thinking

- **User perspective**: Tests verify behavior users care about
- **Implementation agnostic**: Refactoring doesn't break tests
- **Behavior-driven**: Focus on outcomes, not internals

## Common Pitfalls and Solutions [LEVEL 2]

### Pitfall 1: Over-Specifying

**Problem**: Test breaks when UI changes slightly

```yaml
# Bad - Too specific
- action: verify_element
  selector: "div.container > div.row > div.col-md-6 > span.text-primary.font-bold"
  contains: "Welcome"
```

**Solution**: Use flexible selectors

```yaml
# Good - Focused on behavior
- action: verify_element
  selector: ".welcome-message"
  contains: "Welcome"
```

### Pitfall 2: Missing Waits

**Problem**: Test fails intermittently due to timing

```yaml
# Bad - No wait for async operation
- action: click
  selector: ".load-data-button"
- action: verify_element
  selector: ".data-table" # May not exist yet!
```

**Solution**: Always wait for dynamic content

```yaml
# Good - Wait for element to appear
- action: click
  selector: ".load-data-button"
- action: wait_for_element
  selector: ".data-table"
  timeout: 10s
- action: verify_element
  selector: ".data-table"
```

### Pitfall 3: Testing Implementation Details

**Problem**: Test coupled to internal state

```yaml
# Bad - Tests internal cache state
- action: verify_output
  contains: "Cache hit ratio: 85%"
```

**Solution**: Test user-visible behavior

```yaml
# Good - Tests response time
- action: verify_response_time
  less_than: 100ms
  description: "Fast response indicates caching works"
```

### Pitfall 4: Flaky Assertions

**Problem**: Assertions depend on exact timing or formatting

```yaml
# Bad - Exact timestamp match will fail
- action: verify_output
  contains: "Created at: 2025-11-16 09:30:45"
```

**Solution**: Use flexible patterns

```yaml
# Good - Match pattern, not exact value
- action: verify_output
  matches: "Created at: \\d{4}-\\d{2}-\\d{2} \\d{2}:\\d{2}:\\d{2}"
```

### Pitfall 5: Not Cleaning Up

**Problem**: Tests leave artifacts that affect future runs

```yaml
# Bad - No cleanup
steps:
  - action: create_file
    path: "./test-data.json"
  - action: launch
    target: "./app"
```

**Solution**: Always use cleanup section

```yaml
# Good - Cleanup ensures clean slate
steps:
  - action: create_file
    path: "./test-data.json"
  - action: launch
    target: "./app"

cleanup:
  - action: delete_file
    path: "./test-data.json"
```

## Integration with Other Skills [LEVEL 2]

### Works Well With

**test-gap-analyzer**:

- Use test-gap-analyzer to find untested functions
- Write outside-in tests for critical user-facing paths
- Use unit tests (from test-gap-analyzer) for internal functions

**philosophy-guardian**:

- Ensure test YAML follows ruthless simplicity
- Verify tests focus on behavior, not implementation

**reviewing-code**:

- Include outside-in tests in PR reviews
- Verify tests cover changed functionality
- Check test readability and clarity

**module-spec-generator**:

- Generate module specs that include outside-in test scenarios
- Use specs as templates for test YAML

### Example Combined Workflow

```bash
# 1. Analyze coverage gaps
claude "Use test-gap-analyzer on ./src"

# 2. Write outside-in tests for critical paths
claude "Use qa-team to create web tests for authentication"

# 3. Verify philosophy compliance
claude "Use philosophy-guardian to review new test files"

# 4. Include in PR
git add tests/agentic/
git commit -m "Add outside-in tests for auth flow"
```

## Troubleshooting [LEVEL 2]

### Test Times Out

**Symptom**: Test exceeds timeout and fails

**Causes**:

- Application takes longer to start than expected
- Network requests are slow
- Element never appears (incorrect selector)

**Solutions**:

```yaml
# Increase timeout
- action: wait_for_element
  selector: ".slow-loading-element"
  timeout: 30s # Increase from default

# Add intermediate verification
- action: launch
  target: "./app"
- action: wait_for_output
  contains: "Initializing..."
  timeout: 5s
- action: wait_for_output
  contains: "Ready"
  timeout: 20s
```

### Element Not Found

**Symptom**: `verify_element` or `click` fails with "element not found"

**Causes**:

- Incorrect CSS selector
- Element not yet rendered (timing issue)
- Element in iframe or shadow DOM

**Solutions**:

```yaml
# Add wait before interaction
- action: wait_for_element
  selector: ".target-element"
  timeout: 10s
- action: click
  selector: ".target-element"

# Use more specific selector
- action: click
  selector: "button[data-testid='submit-button']"

# Handle iframe
- action: switch_to_iframe
  selector: "iframe#payment-frame"
- action: click
  selector: ".pay-now-button"
```

### Test Passes Locally, Fails in CI

**Symptom**: Test works on dev machine but fails in CI environment

**Causes**:

- Different screen size (web/Electron)
- Missing dependencies
- Timing differences (slower CI machines)
- Environment variable differences

**Solutions**:

```yaml
# Set explicit viewport size (web/Electron)
scenario:
  environment:
    viewport:
      width: 1920
      height: 1080

# Add longer timeouts in CI
- action: wait_for_element
  selector: ".element"
  timeout: 30s  # Generous for CI

# Verify prerequisites
prerequisites:
  - "Chrome browser installed"
  - "Environment variable API_KEY is set"
```

### Output Doesn't Match Expected

**Symptom**: `verify_output` fails even though output looks correct

**Causes**:

- Extra whitespace or newlines
- ANSI color codes in output
- Case sensitivity

**Solutions**:

```yaml
# Use flexible matching
- action: verify_output
  matches: "Result:\\s+Success" # Allow flexible whitespace

# Strip ANSI codes
- action: verify_output
  contains: "Success"
  strip_ansi: true

# Case-insensitive match
- action: verify_output
  contains: "success"
  case_sensitive: false
```
